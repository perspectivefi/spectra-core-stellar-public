// SPDX-License-Identifier: BUSL-1.1
//! Mainnet-fork e2e for the LimitOrderEngine over a **real wrapper that
//! delegates to the live Upshift "August" 4626 vault** — the faithful analog of
//! the Blend wrapper (`blend-fungible-vault-wrapper`), which delegates every
//! op into the live Blend pool.
//!
//! Unlike a token-custodying mock, `UpshiftWrapperContract` holds no balances of
//! its own: `deposit`/`redeem`/`convert`/`preview`/`total_assets` all call
//! straight into the Upshift vault. So a single Underlying-funded fill performs
//! deposit + pricing + settlement **in one transaction**, instantiating the
//! Upshift vault WASM *inside the metered call* — exactly the "everything at
//! once" cost we need to compare against Blend's 40 MiB wall.
//!
//! Run (first run needs mainnet RPC to populate the cache):
//! ```text
//! cargo test -p limit-order-engine fork_upshift_wrapper -- --ignored --nocapture
//! ```

extern crate std;

use soroban_sdk::testutils::Address as _;
use soroban_sdk::{Address, Env, Vec, token};
use spectra_common::limit_order_engine::{FillOrderParams, MakerTokenType, Order, OrderType};

use crate::LimitOrderEngineContractClient;
use soroban_env_host::InvocationResourceLimits;

const YT_WASM: &[u8] = include_bytes!("../../../target/wasm32v1-none/release/yield_token.wasm");
const LOE_WASM: &[u8] =
    include_bytes!("../../../target/wasm32v1-none/release/limit_order_engine.wasm");
const PT_WASM: &[u8] = include_bytes!("../../../target/wasm32v1-none/release/principal_token.wasm");
const REGISTRY_WASM: &[u8] = include_bytes!("../../../target/wasm32v1-none/release/registry.wasm");

/// Upshift "August" ERC-4626 vault over XLM (the wrapper's backend).
const UPSHIFT_VAULT: &str = "CC6TRAPQD3NK7THUKWPV5SL2JHKQGNXZVB6S6MVYFSLRWAKEFUWZKZ7J";
/// Native XLM Stellar Asset Contract — the wrapper's (and vault's) underlying.
const XLM: &str = "CAS3J7GYLGXMF6TDJBBYYSE3HQ6BBSMLNUQ34T6TZMYMW2EVH34XOWMA";

const FORK_LEDGER: u32 = 62_980_314;
const XLM_UNIT: i128 = 10_000_000;
const PT_DURATION_SECS: u64 = 31_536_000;

/// Mainnet per-tx memory cap; the fill's modelled memory is compared to it.
const MAINNET_MEM_CAP: u64 = 40 * 1024 * 1024;

/// All cost-estimate limits maxed so invocation metering runs without tripping
/// on the forked vault's footprint; we read only the fill's modelled memory.
fn enforce_unbounded(env: &Env) {
    env.cost_estimate()
        .enforce_resource_limits(InvocationResourceLimits {
            instructions: i64::MAX,
            mem_bytes: i64::MAX,
            disk_read_entries: u32::MAX,
            write_entries: u32::MAX,
            ledger_entries: u32::MAX,
            disk_read_bytes: u32::MAX,
            write_bytes: u32::MAX,
            contract_events_size_bytes: u32::MAX,
            max_contract_data_key_size_bytes: u32::MAX,
            max_contract_data_entry_size_bytes: u32::MAX,
            max_contract_code_entry_size_bytes: u32::MAX,
        });
    env.cost_estimate().budget().reset_unlimited();
}

// ── A real Spectra-style wrapper that DELEGATES to the live Upshift vault ────
//
// Holds no token balances of its own; every economic op is forwarded to the
// Upshift 4626 vault, so calling it instantiates the vault WASM (the analog of
// the Blend wrapper forwarding into the Blend pool). Wrapper shares track the
// vault shares the wrapper holds 1:1.
mod upshift_wrapper {
    use soroban_sdk::{
        Address, Env, MuxedAddress, String, contract, contractimpl, contracttype, token,
    };
    use stellar_tokens::fungible::{Base, FungibleToken, FungibleTokenClient};
    use stellar_tokens::vault::FungibleVaultClient;

    #[contracttype]
    enum Key {
        Vault,
        Underlying,
    }

    #[contract]
    pub struct UpshiftWrapperContract;

    #[contractimpl]
    impl UpshiftWrapperContract {
        pub fn initialize(env: &Env, vault: Address, underlying: Address) {
            let dec = FungibleTokenClient::new(env, &vault).decimals();
            Base::set_metadata(
                env,
                dec,
                String::from_str(env, "Wrapped Upshift XLM"),
                String::from_str(env, "wupsXLM"),
            );
            env.storage().instance().set(&Key::Vault, &vault);
            env.storage().instance().set(&Key::Underlying, &underlying);
        }

        fn vault(env: &Env) -> Address {
            env.storage().instance().get(&Key::Vault).unwrap()
        }

        pub fn query_asset(env: &Env) -> Address {
            env.storage().instance().get(&Key::Underlying).unwrap()
        }

        pub fn vault_share(env: &Env) -> Address {
            Self::vault(env)
        }

        pub fn total_assets(env: &Env) -> i128 {
            FungibleVaultClient::new(env, &Self::vault(env))
                .convert_to_assets(&Base::total_supply(env))
        }

        pub fn preview_redeem(env: &Env, shares: i128) -> i128 {
            FungibleVaultClient::new(env, &Self::vault(env)).preview_redeem(&shares)
        }
        pub fn preview_deposit(env: &Env, assets: i128) -> i128 {
            FungibleVaultClient::new(env, &Self::vault(env)).preview_deposit(&assets)
        }
        pub fn preview_withdraw(env: &Env, assets: i128) -> i128 {
            FungibleVaultClient::new(env, &Self::vault(env)).preview_withdraw(&assets)
        }
        pub fn convert_to_shares(env: &Env, assets: i128) -> i128 {
            FungibleVaultClient::new(env, &Self::vault(env)).convert_to_shares(&assets)
        }
        pub fn convert_to_assets(env: &Env, shares: i128) -> i128 {
            FungibleVaultClient::new(env, &Self::vault(env)).convert_to_assets(&shares)
        }

        /// Pull underlying from `from`, supply it into the Upshift vault (vault
        /// shares to the wrapper), mint wrapper shares 1:1 to `receiver`.
        pub fn deposit(
            env: &Env,
            assets: i128,
            receiver: Address,
            from: Address,
            operator: Address,
        ) -> i128 {
            operator.require_auth();
            let underlying = Self::query_asset(env);
            let wrapper = env.current_contract_address();
            let und = token::Client::new(env, &underlying);
            if operator == from {
                und.transfer(&from, &wrapper, &assets);
            } else {
                und.transfer_from(&operator, &from, &wrapper, &assets);
            }
            let vault = Self::vault(env);
            // The vault pulls the underlying from the wrapper during `deposit`.
            und.approve(&wrapper, &vault, &assets, &(env.ledger().sequence() + 1));
            let minted = FungibleVaultClient::new(env, &vault)
                .deposit(&assets, &wrapper, &wrapper, &wrapper);
            Base::mint(env, &receiver, minted);
            minted
        }

        /// Burn `shares`, redeem the matching vault shares for underlying
        /// straight to `receiver`.
        pub fn redeem(
            env: &Env,
            shares: i128,
            receiver: Address,
            owner: Address,
            operator: Address,
        ) -> i128 {
            operator.require_auth();
            if operator != owner {
                Base::spend_allowance(env, &owner, &operator, shares);
            }
            Base::update(env, Some(&owner), None, shares);
            let vault = Self::vault(env);
            let wrapper = env.current_contract_address();
            FungibleVaultClient::new(env, &vault).redeem(&shares, &receiver, &wrapper, &wrapper)
        }

        pub fn withdraw(
            env: &Env,
            assets: i128,
            receiver: Address,
            owner: Address,
            operator: Address,
        ) -> i128 {
            operator.require_auth();
            let shares = Self::preview_withdraw(env, assets);
            if operator != owner {
                Base::spend_allowance(env, &owner, &operator, shares);
            }
            Base::update(env, Some(&owner), None, shares);
            let vault = Self::vault(env);
            let wrapper = env.current_contract_address();
            FungibleVaultClient::new(env, &vault).withdraw(&assets, &receiver, &wrapper, &wrapper)
        }

        /// Pull vault shares (Upshift shares) from `caller`, mint wrapper shares
        /// 1:1 — the VaultShare maker path.
        pub fn wrap_with_min_shares(
            env: &Env,
            vault_shares: i128,
            receiver: Address,
            caller: Address,
            _min_shares: i128,
        ) -> i128 {
            caller.require_auth();
            let vault = Self::vault(env);
            let wrapper = env.current_contract_address();
            token::Client::new(env, &vault).transfer_from(
                &wrapper,
                &caller,
                &wrapper,
                &vault_shares,
            );
            Base::mint(env, &receiver, vault_shares);
            vault_shares
        }

        pub fn unwrap_with_min_vault_shares(
            env: &Env,
            shares: i128,
            receiver: Address,
            owner: Address,
            operator: Address,
            _min_vs: i128,
        ) -> i128 {
            operator.require_auth();
            if operator != owner {
                Base::spend_allowance(env, &owner, &operator, shares);
            }
            Base::update(env, Some(&owner), None, shares);
            let vault = Self::vault(env);
            let wrapper = env.current_contract_address();
            token::Client::new(env, &vault).transfer(&wrapper, &receiver, &shares);
            shares
        }
    }

    #[contractimpl]
    impl FungibleToken for UpshiftWrapperContract {
        type ContractType = Base;
        fn balance(e: &Env, a: Address) -> i128 {
            Base::balance(e, &a)
        }
        fn total_supply(e: &Env) -> i128 {
            Base::total_supply(e)
        }
        fn allowance(e: &Env, o: Address, s: Address) -> i128 {
            Base::allowance(e, &o, &s)
        }
        fn transfer(e: &Env, f: Address, t: MuxedAddress, a: i128) {
            Base::transfer(e, &f, &t, a);
        }
        fn transfer_from(e: &Env, s: Address, f: Address, t: Address, a: i128) {
            Base::transfer_from(e, &s, &f, &t, a);
        }
        fn approve(e: &Env, o: Address, s: Address, a: i128, l: u32) {
            Base::approve(e, &o, &s, a, l);
        }
        fn decimals(e: &Env) -> u32 {
            Base::decimals(e)
        }
        fn name(e: &Env) -> String {
            Base::name(e)
        }
        fn symbol(e: &Env) -> String {
            Base::symbol(e)
        }
    }
}

use upshift_wrapper::{UpshiftWrapperContract, UpshiftWrapperContractClient};

fn fork_mainnet() -> soroban_fork::ForkedEnv {
    let rpc = std::env::var("MAINNET_RPC_URL")
        .unwrap_or_else(|_| "https://soroban-rpc.mainnet.stellar.gateway.fm".into());
    let ledger: u32 = std::env::var("LOE_UPSHIFT_FORK_LEDGER")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(FORK_LEDGER);
    let cache = std::env::var("LOE_UPSHIFT_WRAP_FORK_CACHE").unwrap_or_else(|_| {
        concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/fork_cache_upshift_wrap.json"
        )
        .into()
    });
    soroban_fork::ForkConfig::new(&rpc)
        .at_ledger(ledger)
        .cache_file(&cache)
        .build()
        .expect("fork build")
}

struct Market<'a> {
    reserve: Address,
    ibt: Address,
    pt: Address,
    yt: Address,
    maturity: u64,
    engine: LimitOrderEngineContractClient<'a>,
}

fn fund(env: &Env, reserve: &Address, whale: &Address, to: &Address, amount: i128) {
    token::Client::new(env, reserve).transfer(whale, to, &amount);
}

/// Deposit `xlm` held by `who` through the wrapper (which supplies it into the
/// live Upshift vault), minting wrapper IBT to `who`. Returns shares minted.
fn give_ibt(env: &Env, m: &Market, who: &Address, xlm: i128) -> i128 {
    let live = env.ledger().sequence() + 100;
    token::Client::new(env, &m.reserve).approve(who, &m.ibt, &xlm, &live);
    UpshiftWrapperContractClient::new(env, &m.ibt).deposit(&xlm, who, who, who)
}

fn approve_engine(env: &Env, m: &Market, tkn: &Address, owner: &Address, amount: i128) {
    let live = env.ledger().sequence() + 100;
    token::Client::new(env, tkn).approve(owner, &m.engine.address, &amount, &live);
}

fn mk_order(
    m: &Market,
    salt: u64,
    ot: OrderType,
    maker: &Address,
    mtt: MakerTokenType,
    making: i128,
) -> Order {
    Order {
        salt,
        expiry: m.maturity - 1,
        order_type: ot,
        pt: m.pt.clone(),
        maker: maker.clone(),
        receiver: maker.clone(),
        making_amount: making,
        implied_rate: 100_000_000_000_000_000,
        maker_token_type: mtt,
        fail_safe_rate: 0,
    }
}

/// Deposit `xlm` for `who` through the wrapper, then tokenize the minted IBT
/// into PT + YT (both to `who`).
fn give_pt_yt(env: &Env, m: &Market, who: &Address, xlm: i128) {
    let ibts = give_ibt(env, m, who, xlm);
    let live = env.ledger().sequence() + 100;
    token::Client::new(env, &m.ibt).approve(who, &m.pt, &ibts, &live);
    principal_token::PrincipalTokenContractClient::new(env, &m.pt)
        .deposit_ibt(who, &ibts, who, who);
}

/// Register `order`, fill it with `taker` paying `taker_in` wrapper IBT, and
/// print the fill's modelled memory vs the mainnet cap. Returns `actual_taking`.
fn register_and_fill(
    env: &Env,
    m: &Market,
    order: &Order,
    taker: &Address,
    receiver: &Address,
    making: i128,
    taker_in: i128,
) -> i128 {
    m.engine.register_order(order);
    let mut params = Vec::new(env);
    params.push_back(FillOrderParams {
        order: order.clone(),
        making_amount: making,
        max_taking: i128::MAX,
    });
    env.cost_estimate().budget().reset_tracker();
    let results = m
        .engine
        .fill_orders(taker, receiver, &params, &m.ibt, &taker_in);
    let mem = env.cost_estimate().budget().memory_bytes_cost();
    let cpu = env.cost_estimate().budget().cpu_instruction_cost();
    let fits = if mem <= MAINNET_MEM_CAP {
        "FITS"
    } else {
        "OVER"
    };
    std::println!(
        "upshift-WRAPPER fill salt={} ({:?}/{:?}): mem_bytes={mem} (cap {MAINNET_MEM_CAP}) {fits} cpu_insns={cpu}",
        order.salt,
        order.order_type,
        order.maker_token_type,
    );
    env.cost_estimate().budget().reset_unlimited();
    let r0 = results.get(0).unwrap();
    assert_eq!(r0.actual_making, making, "order should fully fill");
    assert!(r0.actual_taking > 0, "actual_taking must be positive");
    let bal = |t: &Address| token::Client::new(env, t).balance(&m.engine.address);
    assert_eq!(bal(&m.ibt), 0, "engine retained IBT");
    assert_eq!(bal(&m.pt), 0, "engine retained PT");
    assert_eq!(bal(&m.yt), 0, "engine retained YT");
    r0.actual_taking
}

/// Full lifecycle over the delegating Upshift wrapper, with our contracts
/// deployed as **WASM** (mainnet-accurate metering). Runs all four order types
/// and prints each fill's modelled memory vs the 40 MiB cap. The IbtForPt fill
/// does deposit + price + settle in one transaction, instantiating the live
/// Upshift vault through the wrapper. NOTE: the wrapper itself still runs native
/// here (inline test contract), so these figures slightly UNDER-count a fully
/// on-chain wrapper; treat the delegating-wrapper numbers as a lower bound.
#[test]
#[ignore = "requires mainnet RPC access (first run populates the cache)"]
fn fork_upshift_wrapper_full_lifecycle() {
    let env = fork_mainnet();
    env.mock_all_auths_allowing_non_root_auth();
    // Setup unmetered; the fill's modelled memory is measured (cost-estimate).
    enforce_unbounded(&env);
    let whale = Address::from_str(&env, &env.test_accounts()[0].account_strkey());
    let e: &Env = &env;

    let admin = Address::generate(e);
    let reserve = Address::from_str(e, XLM);
    let vault = Address::from_str(e, UPSHIFT_VAULT);

    let making = 50 * XLM_UNIT;

    // ── Wrapper IBT delegating to the live Upshift vault ─────────────────
    let ibt = env.register(UpshiftWrapperContract, ());
    UpshiftWrapperContractClient::new(e, &ibt).initialize(&vault, &reserve);

    // Seed so PT.initialize's `total_assets != 0` guard passes.
    fund(e, &reserve, &whale, &admin, making);
    give_ibt(
        e,
        &Market {
            reserve: reserve.clone(),
            ibt: ibt.clone(),
            pt: admin.clone(),
            yt: admin.clone(),
            maturity: 0,
            engine: LimitOrderEngineContractClient::new(e, &ibt),
        },
        &admin,
        making,
    );

    // ── Registry ─────────────────────────────────────────────────────────
    let registry = env.register(REGISTRY_WASM, (admin.clone(),));
    let registry_c = registry::RegistryContractClient::new(e, &registry);
    registry_c.set_fee_collector(&admin, &admin);
    let yt_hash = env.deployer().upload_contract_wasm(YT_WASM);
    registry_c.set_yt_wasm_hash(&admin, &yt_hash);

    // ── PT (+ YT) over the wrapper IBT ───────────────────────────────────
    let pt = env.register(PT_WASM, ());
    let pt_c = principal_token::PrincipalTokenContractClient::new(e, &pt);
    pt_c.initialize(&ibt, &PT_DURATION_SECS, &admin, &registry);
    let maturity = pt_c.maturity();
    let yt = pt_c.get_yt();
    registry_c.add_pt(&admin, &pt);

    // ── Engine ───────────────────────────────────────────────────────────
    let engine_addr = env.register(
        LOE_WASM,
        (admin.clone(), admin.clone(), 0i128, registry.clone()),
    );
    let engine = LimitOrderEngineContractClient::new(e, &engine_addr);

    let m = Market {
        reserve: reserve.clone(),
        ibt: ibt.clone(),
        pt: pt.clone(),
        yt: yt.clone(),
        maturity,
        engine,
    };

    // ── Users — one maker per order type ─────────────────────────────────
    let m_ibt_for_pt = Address::generate(e);
    let m_ibt_for_yt = Address::generate(e);
    let m_pt_for_ibt = Address::generate(e);
    let m_yt_for_ibt = Address::generate(e);
    let taker = Address::generate(e);
    let receiver = Address::generate(e);

    // Wrapper IBT/PT/YT are 13-dec while XLM is 7-dec; scale Ibt/PT/YT-
    // denominated making amounts to IBT units.
    let making_ibt = UpshiftWrapperContractClient::new(e, &m.ibt).preview_deposit(&making);

    for u in [&m_ibt_for_pt, &m_ibt_for_yt, &m_pt_for_ibt, &m_yt_for_ibt] {
        fund(e, &reserve, &whale, u, 2 * making);
    }
    fund(e, &reserve, &whale, &taker, 200 * making);
    let taker_ibt = give_ibt(e, &m, &taker, 200 * making);
    assert!(taker_ibt > 0);

    // ── IbtForPt: maker funds Underlying (XLM). The wrapper supplies it into
    //    the live Upshift vault during the fill (deposit + price + settle). ──
    approve_engine(e, &m, &m.reserve, &m_ibt_for_pt, making);
    let o1 = mk_order(
        &m,
        1,
        OrderType::IbtForPt,
        &m_ibt_for_pt,
        MakerTokenType::Underlying,
        making,
    );
    let t1 = register_and_fill(e, &m, &o1, &taker, &receiver, making, 4 * making_ibt);
    assert_eq!(
        token::Client::new(e, &m.pt).balance(&m_ibt_for_pt),
        t1,
        "IbtForPt maker should receive PT"
    );

    // ── IbtForYt: maker funds Ibt (wrapper shares) directly. ──
    give_ibt(e, &m, &m_ibt_for_yt, 2 * making);
    approve_engine(e, &m, &m.ibt, &m_ibt_for_yt, making_ibt);
    let o2 = mk_order(
        &m,
        2,
        OrderType::IbtForYt,
        &m_ibt_for_yt,
        MakerTokenType::Ibt,
        making_ibt,
    );
    let t2 = register_and_fill(e, &m, &o2, &taker, &receiver, making_ibt, 20 * making_ibt);
    assert_eq!(
        token::Client::new(e, &m.yt).balance(&m_ibt_for_yt),
        t2,
        "IbtForYt maker should receive YT"
    );

    // ── PtForIbt: maker sells PT, paid out in Underlying (XLM). ──
    give_pt_yt(e, &m, &m_pt_for_ibt, 2 * making);
    approve_engine(e, &m, &m.pt, &m_pt_for_ibt, making_ibt);
    let o3 = mk_order(
        &m,
        3,
        OrderType::PtForIbt,
        &m_pt_for_ibt,
        MakerTokenType::Underlying,
        making_ibt,
    );
    let before = token::Client::new(e, &m.reserve).balance(&m_pt_for_ibt);
    register_and_fill(e, &m, &o3, &taker, &receiver, making_ibt, 4 * making_ibt);
    let after = token::Client::new(e, &m.reserve).balance(&m_pt_for_ibt);
    assert!(after > before, "PtForIbt maker should receive XLM");

    // ── YtForIbt: maker sells YT, paid out in Ibt. ──
    give_pt_yt(e, &m, &m_yt_for_ibt, 2 * making);
    approve_engine(e, &m, &m.yt, &m_yt_for_ibt, making_ibt);
    let o4 = mk_order(
        &m,
        4,
        OrderType::YtForIbt,
        &m_yt_for_ibt,
        MakerTokenType::Ibt,
        making_ibt,
    );
    let before_ibt = token::Client::new(e, &m.ibt).balance(&m_yt_for_ibt);
    register_and_fill(e, &m, &o4, &taker, &receiver, making_ibt, 4 * making_ibt);
    let after_ibt = token::Client::new(e, &m.ibt).balance(&m_yt_for_ibt);
    assert!(after_ibt > before_ibt, "YtForIbt maker should receive IBT");

    // ── Warp to maturity, redeem back through the wrapper → Upshift ───────
    env.warp(
        u32::try_from(PT_DURATION_SECS / 5 + 100).unwrap(),
        PT_DURATION_SECS + 100,
    );
    for holder in [&m_ibt_for_pt, &receiver] {
        let pt_bal = token::Client::new(e, &m.pt).balance(holder);
        if pt_bal > 0 {
            let got = pt_c.redeem(holder, &pt_bal, holder, holder);
            assert!(got > 0, "redeem at expiry must return underlying");
        }
    }

    std::println!("upshift delegating-wrapper lifecycle complete: matured at {maturity}");
}

// ===========================================================================
// Full cube: order_type × maker_denom × taker_token on the delegating wrapper.
// ===========================================================================

#[derive(Clone, Copy, Debug)]
enum TakerPay {
    Ibt,
    Pt,
    Yt,
}

#[allow(clippy::too_many_arguments)]
fn measure_cell(
    e: &Env,
    env: &soroban_fork::ForkedEnv,
    m: &Market,
    whale: &Address,
    bank: &Address,
    salt: u64,
    ot: OrderType,
    mtt: MakerTokenType,
    taker_pay: TakerPay,
    making_xlm: i128,
    making_ibt: i128,
) {
    let maker = Address::generate(e);
    let taker = Address::generate(e);
    let receiver = Address::generate(e);

    let funding_side = matches!(ot, OrderType::IbtForPt | OrderType::IbtForYt);
    let making = if funding_side && mtt == MakerTokenType::Underlying {
        making_xlm
    } else {
        making_ibt
    };

    let making_token = match ot {
        OrderType::IbtForPt | OrderType::IbtForYt => match mtt {
            MakerTokenType::Underlying => m.reserve.clone(),
            MakerTokenType::Ibt => m.ibt.clone(),
            MakerTokenType::VaultShare => unreachable!("VaultShare unsupported here"),
        },
        OrderType::PtForIbt => m.pt.clone(),
        OrderType::YtForIbt => m.yt.clone(),
    };
    let maker_need = making * 2;
    if making_token == m.reserve {
        fund(e, &m.reserve, whale, &maker, maker_need);
    } else {
        token::Client::new(e, &making_token).transfer(bank, &maker, &maker_need);
    }
    approve_engine(e, m, &making_token, &maker, making);

    let taker_token = match taker_pay {
        TakerPay::Ibt => m.ibt.clone(),
        TakerPay::Pt => m.pt.clone(),
        TakerPay::Yt => m.yt.clone(),
    };
    let taker_in = if matches!(ot, OrderType::IbtForYt) {
        25 * making_ibt
    } else {
        6 * making_ibt
    };
    token::Client::new(e, &taker_token).transfer(bank, &taker, &taker_in);

    let order = mk_order(m, salt, ot, &maker, mtt, making);
    m.engine.register_order(&order);
    let mut params = Vec::new(e);
    params.push_back(FillOrderParams {
        order: order.clone(),
        making_amount: making,
        max_taking: i128::MAX,
    });
    env.cost_estimate().budget().reset_tracker();
    let results = m
        .engine
        .fill_orders(&taker, &receiver, &params, &taker_token, &taker_in);
    let mem = env.cost_estimate().budget().memory_bytes_cost();
    let cpu = env.cost_estimate().budget().cpu_instruction_cost();
    env.cost_estimate().budget().reset_unlimited();
    let fits = if mem <= MAINNET_MEM_CAP {
        "FITS"
    } else {
        "OVER"
    };
    let r0 = results.get(0).unwrap();
    assert_eq!(r0.actual_making, making, "cell salt={salt} must fully fill");
    std::println!(
        "MATRIX-WRAPPER salt={salt} {ot:?}/{mtt:?}/taker_{taker_pay:?}: mem_bytes={mem} {fits} cpu={cpu}"
    );
}

#[test]
#[ignore = "requires mainnet RPC access (first run populates the cache)"]
fn fork_upshift_wrapper_memory_matrix() {
    let env = fork_mainnet();
    env.mock_all_auths_allowing_non_root_auth();
    enforce_unbounded(&env);
    let whale = Address::from_str(&env, &env.test_accounts()[0].account_strkey());
    let e: &Env = &env;
    let admin = Address::generate(e);
    let reserve = Address::from_str(e, XLM);
    let vault = Address::from_str(e, UPSHIFT_VAULT);
    let making = 10 * XLM_UNIT;

    let ibt = env.register(UpshiftWrapperContract, ());
    UpshiftWrapperContractClient::new(e, &ibt).initialize(&vault, &reserve);

    let seed_market = Market {
        reserve: reserve.clone(),
        ibt: ibt.clone(),
        pt: admin.clone(),
        yt: admin.clone(),
        maturity: 0,
        engine: LimitOrderEngineContractClient::new(e, &ibt),
    };
    fund(e, &reserve, &whale, &admin, making);
    give_ibt(e, &seed_market, &admin, making);

    let registry = env.register(REGISTRY_WASM, (admin.clone(),));
    let registry_c = registry::RegistryContractClient::new(e, &registry);
    registry_c.set_fee_collector(&admin, &admin);
    let yt_hash = env.deployer().upload_contract_wasm(YT_WASM);
    registry_c.set_yt_wasm_hash(&admin, &yt_hash);

    let pt = env.register(PT_WASM, ());
    let pt_c = principal_token::PrincipalTokenContractClient::new(e, &pt);
    pt_c.initialize(&ibt, &PT_DURATION_SECS, &admin, &registry);
    let maturity = pt_c.maturity();
    let yt = pt_c.get_yt();
    registry_c.add_pt(&admin, &pt);

    let engine_addr = env.register(
        LOE_WASM,
        (admin.clone(), admin.clone(), 0i128, registry.clone()),
    );
    let engine = LimitOrderEngineContractClient::new(e, &engine_addr);

    let m = Market {
        reserve: reserve.clone(),
        ibt: ibt.clone(),
        pt: pt.clone(),
        yt: yt.clone(),
        maturity,
        engine,
    };

    let making_ibt =
        stellar_tokens::vault::FungibleVaultClient::new(e, &m.ibt).preview_deposit(&making);

    let bank = Address::generate(e);
    fund(e, &reserve, &whale, &bank, 300 * making);
    give_ibt(e, &m, &bank, 300 * making);
    let live = e.ledger().sequence() + 100;
    token::Client::new(e, &m.ibt).approve(&bank, &m.pt, &(120 * making_ibt), &live);
    principal_token::PrincipalTokenContractClient::new(e, &m.pt).deposit_ibt(
        &bank,
        &(120 * making_ibt),
        &bank,
        &bank,
    );

    let cells = [
        (
            OrderType::IbtForPt,
            MakerTokenType::Underlying,
            TakerPay::Ibt,
        ),
        (
            OrderType::IbtForPt,
            MakerTokenType::Underlying,
            TakerPay::Pt,
        ),
        (OrderType::IbtForPt, MakerTokenType::Ibt, TakerPay::Ibt),
        (OrderType::IbtForPt, MakerTokenType::Ibt, TakerPay::Pt),
        (
            OrderType::IbtForYt,
            MakerTokenType::Underlying,
            TakerPay::Ibt,
        ),
        (
            OrderType::IbtForYt,
            MakerTokenType::Underlying,
            TakerPay::Yt,
        ),
        (OrderType::IbtForYt, MakerTokenType::Ibt, TakerPay::Ibt),
        (OrderType::IbtForYt, MakerTokenType::Ibt, TakerPay::Yt),
        (
            OrderType::PtForIbt,
            MakerTokenType::Underlying,
            TakerPay::Ibt,
        ),
        (
            OrderType::PtForIbt,
            MakerTokenType::Underlying,
            TakerPay::Yt,
        ),
        (OrderType::PtForIbt, MakerTokenType::Ibt, TakerPay::Ibt),
        (OrderType::PtForIbt, MakerTokenType::Ibt, TakerPay::Yt),
        (
            OrderType::YtForIbt,
            MakerTokenType::Underlying,
            TakerPay::Ibt,
        ),
        (
            OrderType::YtForIbt,
            MakerTokenType::Underlying,
            TakerPay::Pt,
        ),
        (OrderType::YtForIbt, MakerTokenType::Ibt, TakerPay::Ibt),
        (OrderType::YtForIbt, MakerTokenType::Ibt, TakerPay::Pt),
    ];
    let mut salt = 200u64;
    for (ot, mtt, tp) in cells {
        salt += 1;
        measure_cell(
            e, &env, &m, &whale, &bank, salt, ot, mtt, tp, making, making_ibt,
        );
    }
    std::println!("upshift wrapper cube complete ({} cells)", cells.len());
}
