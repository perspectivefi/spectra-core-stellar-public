// SPDX-License-Identifier: BUSL-1.1
//! Mainnet-fork e2e for the multi-denomination LimitOrderEngine.
//!
//! Forks Stellar mainnet with `soroban-fork` against the **live Blend v2
//! "Fixed" pool**, deploys the `BlendFungibleVaultWrapper` fresh as the IBT
//! over the pool's XLM reserve, tokenizes it into PT + YT, stands up a
//! LimitOrderEngine market, runs every order type with several users,
//! warps to PT maturity (Blend accrues yield over the elapsed time), and
//! redeems the PT holders back to XLM.
//!
//! Funding: `soroban-fork` pre-mints deterministic test accounts with ~100K
//! XLM each. XLM is native, so account-source transfers authorize cleanly
//! under `mock_all_auths` (classic-asset SAC mint and deployed-contract auth
//! cannot be mocked — hence XLM-from-a-test-account rather than USDC).
//!
//! The Blend adapter supports the Ibt and Underlying maker denominations only
//! (`vault_share`/`wrap`/`unwrap` panic `NotSupported`); VaultShare is covered
//! end-to-end by `e2e_mock_wrapper_full_lifecycle` in `test.rs`.
//!
//! Addresses are the real mainnet deployments (Blend `blend-utils`
//! `mainnet.contracts.json`). The fork is pinned to a fixed ledger; once the
//! cache file is populated the run is deterministic and offline.
//!
//! Run (first run needs mainnet RPC to populate the cache):
//! ```text
//! cargo test -p limit-order-engine fork_ -- --ignored --nocapture
//! ```

extern crate std;

use soroban_env_host::InvocationResourceLimits;
use soroban_sdk::testutils::Address as _;
use soroban_sdk::testutils::cost_estimate::NetworkInvocationResourceLimits;
use soroban_sdk::{Address, Env, Vec};
use spectra_common::limit_order_engine::{FillOrderParams, MakerTokenType, Order, OrderType};

use crate::LimitOrderEngineContractClient;

const YT_WASM: &[u8] = include_bytes!("../../../target/wasm32v1-none/release/yield_token.wasm");
// Deploy our own contracts as WASM (not native `env.register(Type, ())`) so the
// fill is metered exactly as on mainnet — native Rust execution under-counts
// memory/CPU vs the WASM VM. The forked Blend pool is already real WASM.
const LOE_WASM: &[u8] =
    include_bytes!("../../../target/wasm32v1-none/release/limit_order_engine.wasm");
const PT_WASM: &[u8] = include_bytes!("../../../target/wasm32v1-none/release/principal_token.wasm");
const REGISTRY_WASM: &[u8] = include_bytes!("../../../target/wasm32v1-none/release/registry.wasm");
const WRAPPER_WASM: &[u8] =
    include_bytes!("../../../target/wasm32v1-none/release/blend_fungible_vault_wrapper.wasm");

// ── Real Stellar mainnet addresses (Blend blend-utils mainnet.contracts.json) ──

/// Blend v2 "Fixed" lending pool.
const BLEND_POOL: &str = "CAJJZSGMMM3PD7N33TAPHGBUGTB43OC73HVIK2L2G6BNGGGYOSSYBXBD";
/// Native XLM Stellar Asset Contract — a reserve in the Fixed pool and the
/// IBT's underlying asset.
const XLM: &str = "CAS3J7GYLGXMF6TDJBBYYSE3HQ6BBSMLNUQ34T6TZMYMW2EVH34XOWMA";

/// Ledger to pin the fork at (locks the block so state never changes). Override
/// with `LOE_FORK_LEDGER` to recapture against a fresher ledger.
const FORK_LEDGER: u32 = 62_980_314;

/// XLM has 7 decimals: 1 XLM = 10^7 stroops.
const XLM_UNIT: i128 = 10_000_000;

const PT_DURATION_SECS: u64 = 31_536_000; // 1 year

fn fork_mainnet() -> soroban_fork::ForkedEnv {
    let rpc = std::env::var("MAINNET_RPC_URL")
        .unwrap_or_else(|_| "https://soroban-rpc.mainnet.stellar.gateway.fm".into());
    let ledger: u32 = std::env::var("LOE_FORK_LEDGER")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(FORK_LEDGER);
    let cache = std::env::var("LOE_FORK_CACHE").unwrap_or_else(|_| {
        concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fork_cache_loe.json").into()
    });
    soroban_fork::ForkConfig::new(&rpc)
        .at_ledger(ledger)
        .cache_file(&cache)
        .build()
        .expect("fork build")
}

struct Market<'a> {
    reserve: Address,
    wrapper: Address,
    pt: Address,
    yt: Address,
    maturity: u64,
    engine: LimitOrderEngineContractClient<'a>,
}

/// Move `amount` XLM from the `whale` (a fork test account pre-funded with
/// ~100K XLM) to `to`.
fn fund(env: &Env, reserve: &Address, whale: &Address, to: &Address, amount: i128) {
    soroban_sdk::token::Client::new(env, reserve).transfer(whale, to, &amount);
}

/// Deposit `assets` XLM held by `who` into the wrapper, minting IBT to `who`.
/// Returns shares minted.
fn give_ibt(env: &Env, m: &Market, who: &Address, assets: i128) -> i128 {
    blend_fungible_vault_wrapper::BlendFungibleVaultWrapperContractClient::new(env, &m.wrapper)
        .deposit(&assets, who, who, who)
}

/// Deposit `assets` XLM for `who`, then tokenize the *actual* IBT shares minted
/// (Blend's b_rate makes shares != assets) into PT + YT (both to `who`).
fn give_pt_yt(env: &Env, m: &Market, who: &Address, assets: i128) {
    let ibts = give_ibt(env, m, who, assets);
    let live = env.ledger().sequence() + 100;
    soroban_sdk::token::Client::new(env, &m.wrapper).approve(who, &m.pt, &ibts, &live);
    principal_token::PrincipalTokenContractClient::new(env, &m.pt)
        .deposit_ibt(who, &ibts, who, who);
}

fn approve_engine(env: &Env, m: &Market, token: &Address, owner: &Address, amount: i128) {
    let live = env.ledger().sequence() + 100;
    soroban_sdk::token::Client::new(env, token).approve(owner, &m.engine.address, &amount, &live);
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
        implied_rate: 100_000_000_000_000_000, // 10% APY, 18-dec WAD
        maker_token_type: mtt,
        fail_safe_rate: 0,
    }
}

/// Register `order` and fill it with `taker` paying `taker_in` IBT; assert it
/// fully fills and the engine retains no IBT/PT/YT. Returns `actual_taking`.
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
        .fill_orders(taker, receiver, &params, &m.wrapper, &taker_in);
    std::println!(
        "BLEND fill salt={}: mem_bytes={} cpu_insns={}",
        order.salt,
        env.cost_estimate().budget().memory_bytes_cost(),
        env.cost_estimate().budget().cpu_instruction_cost()
    );
    let r0 = results.get(0).unwrap();
    assert_eq!(r0.actual_making, making, "order should fully fill");
    assert!(r0.actual_taking > 0, "actual_taking must be positive");
    let bal = |t: &Address| soroban_sdk::token::Client::new(env, t).balance(&m.engine.address);
    assert_eq!(bal(&m.wrapper), 0, "engine retained IBT");
    assert_eq!(bal(&m.pt), 0, "engine retained PT");
    assert_eq!(bal(&m.yt), 0, "engine retained YT");
    r0.actual_taking
}

/// Full lifecycle across all four order types on the live Blend Fixed pool.
#[test]
#[ignore = "requires mainnet RPC access (first run populates the cache)"]
fn fork_blend_wrapper_full_lifecycle() {
    let env = fork_mainnet();
    // Non-root variant: the Blend wrapper's `authorize_pull` and PT tokenisation
    // perform contract sub-auth (`authorize_as_current_contract`), which plain
    // `mock_all_auths` rejects with Auth/InvalidAction.
    env.mock_all_auths_allowing_non_root_auth();
    // Our contracts are deployed as WASM (see *_WASM consts) so the fill is
    // metered as on mainnet — native execution under-counts memory by ~1.6-2x.
    // A fill against a Blend-backed PT instantiates the Blend pool (during
    // pricing's `pt.preview_redeem` and on `deposit_ibt`); WASM-accurate per-fill
    // memory is ≈76-85 MiB, well over the 40 MiB mainnet cap — matching the live
    // testnet revert (Budget/ExceededLimit). The b_rate cache cuts this by
    // ~15-19 MiB but does not bring it under the cap. The high enforce limit +
    // `reset_unlimited` keep cost-estimate accounting on while letting the
    // lifecycle run; each fill's modelled memory is printed, not asserted.
    env.cost_estimate()
        .enforce_resource_limits(InvocationResourceLimits {
            mem_bytes: 512 * 1024 * 1024,
            instructions: 100_000_000_000,
            ..InvocationResourceLimits::mainnet()
        });
    env.cost_estimate().budget().reset_unlimited();
    let whale = Address::from_str(&env, &env.test_accounts()[0].account_strkey());
    let e: &Env = &env;

    let admin = Address::generate(e);
    let reserve = Address::from_str(e, XLM);
    let pool = Address::from_str(e, BLEND_POOL);

    let making = 50 * XLM_UNIT; // 50 XLM working size

    // ── Deploy the Blend wrapper as the IBT over XLM ─────────────────────
    let wrapper = env.register(WRAPPER_WASM, (pool.clone(), reserve.clone(), admin.clone()));

    // ── Registry ─────────────────────────────────────────────────────────
    let registry = env.register(REGISTRY_WASM, (admin.clone(),));
    let registry_c = registry::RegistryContractClient::new(e, &registry);
    registry_c.set_fee_collector(&admin, &admin);
    let yt_hash = env.deployer().upload_contract_wasm(YT_WASM);
    registry_c.set_yt_wasm_hash(&admin, &yt_hash);

    // ── PT (+ YT). Seed the wrapper first so total_assets != 0 at init. ──
    let pt = env.register(PT_WASM, ());
    let pt_c = principal_token::PrincipalTokenContractClient::new(e, &pt);

    fund(e, &reserve, &whale, &admin, making);
    blend_fungible_vault_wrapper::BlendFungibleVaultWrapperContractClient::new(e, &wrapper)
        .deposit(&making, &admin, &admin, &admin);

    pt_c.initialize(&wrapper, &PT_DURATION_SECS, &admin, &registry);
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
        wrapper: wrapper.clone(),
        pt: pt.clone(),
        yt: yt.clone(),
        maturity,
        engine,
    };

    // ── Users ────────────────────────────────────────────────────────────
    let m_ibt_for_pt = Address::generate(e);
    let m_ibt_for_yt = Address::generate(e);
    let m_pt_for_ibt = Address::generate(e);
    let m_yt_for_ibt = Address::generate(e);
    let taker = Address::generate(e);
    let receiver = Address::generate(e);

    // 2x headroom: Blend's b_rate makes minted IBT slightly less than the XLM
    // deposited, so order making_amount must sit below the maker's holdings.
    for u in [&m_ibt_for_pt, &m_ibt_for_yt, &m_pt_for_ibt, &m_yt_for_ibt] {
        fund(e, &reserve, &whale, u, 2 * making);
    }
    // Taker pays IBT on every fill (IbtForYt needs a lot of YT → a lot of IBT).
    fund(e, &reserve, &whale, &taker, 50 * making);
    let taker_ibt = give_ibt(e, &m, &taker, 50 * making);
    assert!(taker_ibt > 0);

    // ── IbtForPt: maker funds Underlying (XLM) ───────────────────────────
    approve_engine(e, &m, &m.reserve, &m_ibt_for_pt, making);
    let o1 = mk_order(
        &m,
        1,
        OrderType::IbtForPt,
        &m_ibt_for_pt,
        MakerTokenType::Underlying,
        making,
    );
    let t1 = register_and_fill(e, &m, &o1, &taker, &receiver, making, 4 * making);
    assert_eq!(
        soroban_sdk::token::Client::new(e, &m.pt).balance(&m_ibt_for_pt),
        t1,
        "IbtForPt maker should receive PT"
    );

    // ── IbtForYt: maker funds Ibt. YT is cheap → needs a large taker pay. ─
    give_ibt(e, &m, &m_ibt_for_yt, 2 * making);
    approve_engine(e, &m, &m.wrapper, &m_ibt_for_yt, making);
    let o2 = mk_order(
        &m,
        2,
        OrderType::IbtForYt,
        &m_ibt_for_yt,
        MakerTokenType::Ibt,
        making,
    );
    let t2 = register_and_fill(e, &m, &o2, &taker, &receiver, making, 20 * making);
    assert_eq!(
        soroban_sdk::token::Client::new(e, &m.yt).balance(&m_ibt_for_yt),
        t2,
        "IbtForYt maker should receive YT"
    );

    // ── PtForIbt: maker sells PT, paid out in Underlying (XLM) ────────────
    give_pt_yt(e, &m, &m_pt_for_ibt, 2 * making);
    approve_engine(e, &m, &m.pt, &m_pt_for_ibt, making);
    let o3 = mk_order(
        &m,
        3,
        OrderType::PtForIbt,
        &m_pt_for_ibt,
        MakerTokenType::Underlying,
        making,
    );
    let before = soroban_sdk::token::Client::new(e, &m.reserve).balance(&m_pt_for_ibt);
    register_and_fill(e, &m, &o3, &taker, &receiver, making, 4 * making);
    let after = soroban_sdk::token::Client::new(e, &m.reserve).balance(&m_pt_for_ibt);
    assert!(after > before, "PtForIbt maker should receive XLM");

    // ── YtForIbt: maker sells YT, paid out in Ibt ────────────────────────
    give_pt_yt(e, &m, &m_yt_for_ibt, 2 * making);
    approve_engine(e, &m, &m.yt, &m_yt_for_ibt, making);
    let o4 = mk_order(
        &m,
        4,
        OrderType::YtForIbt,
        &m_yt_for_ibt,
        MakerTokenType::Ibt,
        making,
    );
    let before_ibt = soroban_sdk::token::Client::new(e, &m.wrapper).balance(&m_yt_for_ibt);
    register_and_fill(e, &m, &o4, &taker, &receiver, making, 4 * making);
    let after_ibt = soroban_sdk::token::Client::new(e, &m.wrapper).balance(&m_yt_for_ibt);
    assert!(after_ibt > before_ibt, "YtForIbt maker should receive IBT");

    // ── Warp to maturity (Blend accrues yield over the elapsed time) ─────
    env.warp(
        u32::try_from(PT_DURATION_SECS / 5 + 100).unwrap(),
        PT_DURATION_SECS + 100,
    );

    // ── At expiry, every PT holder redeems back to XLM ───────────────────
    for holder in [&m_ibt_for_pt, &receiver] {
        let pt_bal = soroban_sdk::token::Client::new(e, &m.pt).balance(holder);
        if pt_bal > 0 {
            let got = pt_c.redeem(holder, &pt_bal, holder, holder);
            assert!(got > 0, "redeem at expiry must return XLM");
            assert_eq!(
                soroban_sdk::token::Client::new(e, &m.pt).balance(holder),
                0,
                "PT fully redeemed"
            );
        }
    }

    std::println!("fork lifecycle complete: PT matured and redeemed at {maturity}");
}

// ===========================================================================
// Full cube: order_type × maker_denom × taker_token on the live Blend pool.
// ===========================================================================

const MEM_CAP: u64 = 41_943_040; // 40 MiB

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
            MakerTokenType::Ibt => m.wrapper.clone(),
            MakerTokenType::VaultShare => unreachable!("VaultShare unsupported on Blend"),
        },
        OrderType::PtForIbt => m.pt.clone(),
        OrderType::YtForIbt => m.yt.clone(),
    };
    let maker_need = making * 2;
    if making_token == m.reserve {
        fund(e, &m.reserve, whale, &maker, maker_need);
    } else {
        soroban_sdk::token::Client::new(e, &making_token).transfer(bank, &maker, &maker_need);
    }
    approve_engine(e, m, &making_token, &maker, making);

    let taker_token = match taker_pay {
        TakerPay::Ibt => m.wrapper.clone(),
        TakerPay::Pt => m.pt.clone(),
        TakerPay::Yt => m.yt.clone(),
    };
    let taker_in = if matches!(ot, OrderType::IbtForYt) {
        25 * making_ibt
    } else {
        6 * making_ibt
    };
    soroban_sdk::token::Client::new(e, &taker_token).transfer(bank, &taker, &taker_in);

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
    let fits = if mem <= MEM_CAP { "FITS" } else { "OVER" };
    let r0 = results.get(0).unwrap();
    assert_eq!(r0.actual_making, making, "cell salt={salt} must fully fill");
    std::println!(
        "MATRIX-BLEND salt={salt} {ot:?}/{mtt:?}/taker_{taker_pay:?}: mem_bytes={mem} {fits} cpu={cpu}"
    );
}

#[test]
#[ignore = "requires mainnet RPC access (first run populates the cache)"]
fn fork_blend_memory_matrix() {
    let env = fork_mainnet();
    env.mock_all_auths_allowing_non_root_auth();
    env.cost_estimate()
        .enforce_resource_limits(InvocationResourceLimits {
            mem_bytes: 512 * 1024 * 1024,
            instructions: 100_000_000_000,
            ..InvocationResourceLimits::mainnet()
        });
    env.cost_estimate().budget().reset_unlimited();
    let whale = Address::from_str(&env, &env.test_accounts()[0].account_strkey());
    let e: &Env = &env;

    let admin = Address::generate(e);
    let reserve = Address::from_str(e, XLM);
    let pool = Address::from_str(e, BLEND_POOL);
    let making = 10 * XLM_UNIT;

    let wrapper = env.register(WRAPPER_WASM, (pool.clone(), reserve.clone(), admin.clone()));

    let registry = env.register(REGISTRY_WASM, (admin.clone(),));
    let registry_c = registry::RegistryContractClient::new(e, &registry);
    registry_c.set_fee_collector(&admin, &admin);
    let yt_hash = env.deployer().upload_contract_wasm(YT_WASM);
    registry_c.set_yt_wasm_hash(&admin, &yt_hash);

    // Seed the wrapper so total_assets != 0 at PT init.
    fund(e, &reserve, &whale, &admin, making);
    blend_fungible_vault_wrapper::BlendFungibleVaultWrapperContractClient::new(e, &wrapper)
        .deposit(&making, &admin, &admin, &admin);

    let pt = env.register(PT_WASM, ());
    let pt_c = principal_token::PrincipalTokenContractClient::new(e, &pt);
    pt_c.initialize(&wrapper, &PT_DURATION_SECS, &admin, &registry);
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
        wrapper: wrapper.clone(),
        pt: pt.clone(),
        yt: yt.clone(),
        maturity,
        engine,
    };

    // Token bank: one big deposit (→ shares), tokenize a chunk into PT+YT.
    let bank = Address::generate(e);
    fund(e, &reserve, &whale, &bank, 300 * making);
    let bank_shares = give_ibt(e, &m, &bank, 300 * making);
    let making_ibt = bank_shares / 300;
    let live = e.ledger().sequence() + 100;
    soroban_sdk::token::Client::new(e, &m.wrapper).approve(
        &bank,
        &m.pt,
        &(120 * making_ibt),
        &live,
    );
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
    std::println!("blend cube complete ({} cells)", cells.len());
}

// ===========================================================================
// Copy of the Blend cube that also exercises the wrapper's liquidity-aware
// max_deposit / max_withdraw views on top of each full fill route. Kept as a
// separate test so the baseline matrix stays untouched.
//
// NOTE: `memory_bytes_cost()` reports the LAST invocation's memory, not a
// running total, so the fill and each view are measured with their own
// `reset_tracker`. The 40 MiB limit is enforced per invocation, so the cell
// "fits" iff every invocation (the heavy fill and the two cheap views) fits;
// we report all three plus the peak.
// ===========================================================================

#[allow(clippy::too_many_arguments)]
fn measure_cell_with_max(
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
            MakerTokenType::Ibt => m.wrapper.clone(),
            MakerTokenType::VaultShare => unreachable!("VaultShare unsupported on Blend"),
        },
        OrderType::PtForIbt => m.pt.clone(),
        OrderType::YtForIbt => m.yt.clone(),
    };
    let maker_need = making * 2;
    if making_token == m.reserve {
        fund(e, &m.reserve, whale, &maker, maker_need);
    } else {
        soroban_sdk::token::Client::new(e, &making_token).transfer(bank, &maker, &maker_need);
    }
    approve_engine(e, m, &making_token, &maker, making);

    let taker_token = match taker_pay {
        TakerPay::Ibt => m.wrapper.clone(),
        TakerPay::Pt => m.pt.clone(),
        TakerPay::Yt => m.yt.clone(),
    };
    let taker_in = if matches!(ot, OrderType::IbtForYt) {
        25 * making_ibt
    } else {
        6 * making_ibt
    };
    soroban_sdk::token::Client::new(e, &taker_token).transfer(bank, &taker, &taker_in);

    let order = mk_order(m, salt, ot, &maker, mtt, making);
    m.engine.register_order(&order);
    let mut params = Vec::new(e);
    params.push_back(FillOrderParams {
        order: order.clone(),
        making_amount: making,
        max_taking: i128::MAX,
    });

    // 1) The full fill route. For maker=Underlying funding cells this already
    //    runs the max_deposit guard inside convert_to_ibt.
    env.cost_estimate().budget().reset_tracker();
    let results = m
        .engine
        .fill_orders(&taker, &receiver, &params, &taker_token, &taker_in);
    let mem_fill = env.cost_estimate().budget().memory_bytes_cost();

    // 2) The two liquidity-aware views, each its own metered invocation.
    let wc =
        blend_fungible_vault_wrapper::BlendFungibleVaultWrapperContractClient::new(e, &m.wrapper);
    env.cost_estimate().budget().reset_tracker();
    let _ = wc.max_deposit(&maker);
    let mem_md = env.cost_estimate().budget().memory_bytes_cost();
    env.cost_estimate().budget().reset_tracker();
    let _ = wc.max_withdraw(&maker);
    let mem_mw = env.cost_estimate().budget().memory_bytes_cost();
    env.cost_estimate().budget().reset_unlimited();

    let peak = mem_fill.max(mem_md).max(mem_mw);
    let fits = if peak <= MEM_CAP { "FITS" } else { "OVER" };
    let r0 = results.get(0).unwrap();
    assert_eq!(r0.actual_making, making, "cell salt={salt} must fully fill");
    assert!(
        peak <= MEM_CAP,
        "cell salt={salt} peak {peak} exceeds the {MEM_CAP}-byte cap"
    );
    std::println!(
        "MATRIX+MAX salt={salt} {ot:?}/{mtt:?}/taker_{taker_pay:?}: fill={mem_fill} max_deposit={mem_md} max_withdraw={mem_mw} peak={peak} {fits}"
    );
}

#[test]
#[ignore = "requires mainnet RPC access (first run populates the cache)"]
fn fork_blend_memory_matrix_with_max() {
    let env = fork_mainnet();
    env.mock_all_auths_allowing_non_root_auth();
    env.cost_estimate()
        .enforce_resource_limits(InvocationResourceLimits {
            mem_bytes: 512 * 1024 * 1024,
            instructions: 100_000_000_000,
            ..InvocationResourceLimits::mainnet()
        });
    env.cost_estimate().budget().reset_unlimited();
    let whale = Address::from_str(&env, &env.test_accounts()[0].account_strkey());
    let e: &Env = &env;

    let admin = Address::generate(e);
    let reserve = Address::from_str(e, XLM);
    let pool = Address::from_str(e, BLEND_POOL);
    let making = 10 * XLM_UNIT;

    let wrapper = env.register(WRAPPER_WASM, (pool.clone(), reserve.clone(), admin.clone()));

    let registry = env.register(REGISTRY_WASM, (admin.clone(),));
    let registry_c = registry::RegistryContractClient::new(e, &registry);
    registry_c.set_fee_collector(&admin, &admin);
    let yt_hash = env.deployer().upload_contract_wasm(YT_WASM);
    registry_c.set_yt_wasm_hash(&admin, &yt_hash);

    fund(e, &reserve, &whale, &admin, making);
    blend_fungible_vault_wrapper::BlendFungibleVaultWrapperContractClient::new(e, &wrapper)
        .deposit(&making, &admin, &admin, &admin);

    let pt = env.register(PT_WASM, ());
    let pt_c = principal_token::PrincipalTokenContractClient::new(e, &pt);
    pt_c.initialize(&wrapper, &PT_DURATION_SECS, &admin, &registry);
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
        wrapper: wrapper.clone(),
        pt: pt.clone(),
        yt: yt.clone(),
        maturity,
        engine,
    };

    let bank = Address::generate(e);
    fund(e, &reserve, &whale, &bank, 300 * making);
    let bank_shares = give_ibt(e, &m, &bank, 300 * making);
    let making_ibt = bank_shares / 300;
    let live = e.ledger().sequence() + 100;
    soroban_sdk::token::Client::new(e, &m.wrapper).approve(
        &bank,
        &m.pt,
        &(120 * making_ibt),
        &live,
    );
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
    let mut salt = 300u64;
    for (ot, mtt, tp) in cells {
        salt += 1;
        measure_cell_with_max(
            e, &env, &m, &whale, &bank, salt, ot, mtt, tp, making, making_ibt,
        );
    }
    std::println!("blend cube + max views complete ({} cells)", cells.len());
}
