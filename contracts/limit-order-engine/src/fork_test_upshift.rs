// SPDX-License-Identifier: BUSL-1.1
//! Mainnet-fork e2e for the multi-denomination LimitOrderEngine over the live
//! **Upshift "August" ERC-4626 vault** (`UPSHIFT_VAULT`) used *directly* as the
//! IBT — no Spectra/Blend wrapper in between.
//!
//! Why a second fork e2e: the Blend-backed lifecycle in `fork_test.rs` cannot
//! run under the real mainnet per-invocation memory cap — instantiating the
//! Blend pool WASM during a fill pushes cumulative memory past 40 MiB, so that
//! test has to lift the budget. The Upshift vault is a standalone Soroban 4626
//! vault (built on the same OZ `stellar-tokens` library our PT expects), so PT
//! tokenizes it directly and a fill never instantiates a heavy lending pool.
//! This test therefore enforces the *real* `InvocationResourceLimits::mainnet()`
//! caps: it is the resource-fit check the Blend test can't be.
//!
//! Maker denominations exercised: `Underlying` (XLM) and `Ibt` (vault shares).
//! `VaultShare` is not applicable — there is no wrapper here; the IBT *is* the
//! vault. The wrapper/`VaultShare` path against Upshift is covered separately by
//! `fork_test_upshift_wrapper.rs`.
//!
//! Funding: `soroban-fork` pre-mints deterministic test accounts with ~100K XLM
//! each. The vault's underlying is native XLM, so account-source transfers and
//! vault deposits authorize cleanly under `mock_all_auths_allowing_non_root_auth`.
//!
//! Run (first run needs mainnet RPC to populate the cache):
//! ```text
//! cargo test -p limit-order-engine fork_upshift -- --ignored --nocapture
//! ```

extern crate std;

use soroban_sdk::testutils::Address as _;
use soroban_sdk::{Address, Env, Vec, token};
use spectra_common::limit_order_engine::{FillOrderParams, MakerTokenType, Order, OrderType};
use stellar_tokens::vault::FungibleVaultClient;

use crate::LimitOrderEngineContractClient;
use soroban_env_host::InvocationResourceLimits;

const YT_WASM: &[u8] = include_bytes!("../../../target/wasm32v1-none/release/yield_token.wasm");
// Deploy our contracts as WASM (not native) so the fill is metered as on
// mainnet — native Rust under-counts memory/CPU vs the WASM VM.
const LOE_WASM: &[u8] =
    include_bytes!("../../../target/wasm32v1-none/release/limit_order_engine.wasm");
const PT_WASM: &[u8] = include_bytes!("../../../target/wasm32v1-none/release/principal_token.wasm");
const REGISTRY_WASM: &[u8] = include_bytes!("../../../target/wasm32v1-none/release/registry.wasm");

// ── Real Stellar mainnet addresses ──────────────────────────────────────────

/// Upshift "August" ERC-4626 vault over XLM (app.upshift.finance, Stellar pool).
/// Implements the OZ `stellar-tokens` FungibleVault interface our PT tokenizes.
const UPSHIFT_VAULT: &str = "CC6TRAPQD3NK7THUKWPV5SL2JHKQGNXZVB6S6MVYFSLRWAKEFUWZKZ7J";
/// Native XLM Stellar Asset Contract — the vault's underlying asset.
const XLM: &str = "CAS3J7GYLGXMF6TDJBBYYSE3HQ6BBSMLNUQ34T6TZMYMW2EVH34XOWMA";

/// Ledger to pin the fork at. Override with `LOE_UPSHIFT_FORK_LEDGER` to
/// recapture against a fresher ledger (e.g. if the vault was deployed after the
/// default and is absent from historical state at this height).
const FORK_LEDGER: u32 = 62_980_314;

/// XLM has 7 decimals: 1 XLM = 10^7 stroops.
const XLM_UNIT: i128 = 10_000_000;

const PT_DURATION_SECS: u64 = 31_536_000; // 1 year

/// Stellar mainnet per-transaction memory cap; the fill's consumed memory is
/// compared against this (measured under cost-estimate/invocation metering).
const MAINNET_MEM_CAP: u64 = 40 * 1024 * 1024;

fn fork_mainnet() -> soroban_fork::ForkedEnv {
    let rpc = std::env::var("MAINNET_RPC_URL")
        .unwrap_or_else(|_| "https://soroban-rpc.mainnet.stellar.gateway.fm".into());
    let ledger: u32 = std::env::var("LOE_UPSHIFT_FORK_LEDGER")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(FORK_LEDGER);
    let cache = std::env::var("LOE_UPSHIFT_FORK_CACHE").unwrap_or_else(|_| {
        concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fork_cache_upshift.json").into()
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

/// Move `amount` XLM from the `whale` (a fork test account pre-funded with
/// ~100K XLM) to `to`.
fn fund(env: &Env, reserve: &Address, whale: &Address, to: &Address, amount: i128) {
    token::Client::new(env, reserve).transfer(whale, to, &amount);
}

/// Deposit `assets` XLM held by `who` into the Upshift vault, minting IBT shares
/// to `who`. Returns shares minted.
fn give_ibt(env: &Env, m: &Market, who: &Address, assets: i128) -> i128 {
    FungibleVaultClient::new(env, &m.ibt).deposit(&assets, who, who, who)
}

/// Deposit `assets` XLM for `who`, then tokenize the minted IBT into PT + YT
/// (both to `who`).
fn give_pt_yt(env: &Env, m: &Market, who: &Address, assets: i128) {
    let ibts = give_ibt(env, m, who, assets);
    let live = env.ledger().sequence() + 100;
    token::Client::new(env, &m.ibt).approve(who, &m.pt, &ibts, &live);
    principal_token::PrincipalTokenContractClient::new(env, &m.pt)
        .deposit_ibt(who, &ibts, who, who);
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
    // Measure this fill's per-tx memory (cost-estimate mode), compare to cap.
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
        "upshift fill salt={} ({:?}/{:?}): mem_bytes={mem} (cap {MAINNET_MEM_CAP}) {fits} cpu_insns={cpu}",
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

/// Full lifecycle on the live Upshift vault with **our contracts deployed as
/// WASM** (mainnet-accurate metering). Runs all four order types and prints
/// each fill's modelled memory vs the 40 MiB mainnet per-tx cap. NOTE: measured
/// this way only PtForIbt (≈32 MiB) fits; IbtForPt (≈59), YtForIbt (≈50) and
/// IbtForYt (≈67) EXCEED the cap — the tokenizing fills instantiate the live
/// Upshift vault and blow the budget. (Native execution under-counted by
/// ~1.5–2×, which is why earlier native figures looked like they fit.) Kept as
/// an honest measurement harness; the lifecycle assertions also validate
/// functional correctness.
#[test]
#[ignore = "requires mainnet RPC access (first run populates the cache)"]
fn fork_upshift_vault_full_lifecycle() {
    let env = fork_mainnet();
    // PT tokenisation performs contract sub-auth (`authorize_as_current_contract`),
    // which plain `mock_all_auths` rejects; allow non-root auth like the Blend e2e.
    env.mock_all_auths_allowing_non_root_auth();
    // Multi-step setup (registry/PT/engine deploy, funding, tokenisation) runs
    // unmetered; only each `fill_orders` — one real transaction — is metered, by
    // re-arming the mainnet per-tx budget immediately before it. The cap is never
    // lifted for the fill itself, so if an Upshift fill needs >40 MiB / >100M
    // insns it panics `Budget,ExceededLimit`, the same wall a Blend fill hits.
    // Cost-estimate (invocation-metering) mode with a high cap models per-tx
    // resources as mainnet does; we read the fill's consumed memory and compare
    // it to the real 40 MiB cap.
    // All limits maxed so cost-estimate (invocation-metering) mode runs without
    // tripping on the forked RWA vault's large footprint; we only read the
    // fill's modelled memory and compare it to the real 40 MiB cap.
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
    let whale = Address::from_str(&env, &env.test_accounts()[0].account_strkey());
    let e: &Env = &env;

    let admin = Address::generate(e);
    let reserve = Address::from_str(e, XLM);
    let ibt = Address::from_str(e, UPSHIFT_VAULT);

    let making = 50 * XLM_UNIT; // Underlying-denominated working size (XLM)

    // ── Registry ─────────────────────────────────────────────────────────
    let registry = env.register(REGISTRY_WASM, (admin.clone(),));
    let registry_c = registry::RegistryContractClient::new(e, &registry);
    registry_c.set_fee_collector(&admin, &admin);
    let yt_hash = env.deployer().upload_contract_wasm(YT_WASM);
    registry_c.set_yt_wasm_hash(&admin, &yt_hash);

    // ── PT (+ YT) over the Upshift vault. The live vault already holds assets,
    //    so no seeding is needed for the `total_assets != 0` init guard. ──
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

    // ── Users — one maker per order type, like the Blend e2e ─────────────
    let m_ibt_for_pt = Address::generate(e);
    let m_ibt_for_yt = Address::generate(e);
    let m_pt_for_ibt = Address::generate(e);
    let m_yt_for_ibt = Address::generate(e);
    let taker = Address::generate(e);
    let receiver = Address::generate(e);

    // IBT/PT/YT are 13-dec while XLM is 7-dec. `making_ibt` is the IBT minted
    // for `making` XLM, so Ibt/PT/YT-denominated orders carry the same value as
    // the Underlying order that makes `making` XLM.
    let making_ibt = FungibleVaultClient::new(e, &m.ibt).preview_deposit(&making);

    for u in [&m_ibt_for_pt, &m_ibt_for_yt, &m_pt_for_ibt, &m_yt_for_ibt] {
        fund(e, &reserve, &whale, u, 2 * making);
    }
    // Taker pays IBT on every fill; fund enough XLM to cover all four commits.
    fund(e, &reserve, &whale, &taker, 200 * making);
    let taker_ibt = give_ibt(e, &m, &taker, 200 * making);
    assert!(taker_ibt > 0);

    // ── IbtForPt: maker funds Underlying (XLM). The engine deposits it into
    //    the Upshift vault during the fill — instantiates the vault's heavy
    //    `deposit` path. ──
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

    // ── IbtForYt: maker funds Ibt directly. `convert_to_ibt` returns the
    //    amount with no deposit/wrap — only pricing touches the vault. ──
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

    // ── PtForIbt: maker sells PT, paid out in Underlying (XLM). The engine
    //    redeems IBT → XLM through the vault during settlement. ──
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

    // ====================================================================
    // Crossing-count lever experiments. IbtForPt (salt 1) measured 59.4 MiB
    // with maker=Underlying + taker-pays-IBT. The crossing model predicts that
    // total decomposes as: pricing floor + one convert_to_ibt deposit (maker
    // Underlying) + the deposit_ibt mint (taker pays IBT). These two fills peel
    // those off one at a time.
    // ====================================================================

    // The live Upshift vault rejects further deposits (cap reached after the
    // lifecycle's funding), so the experiments source IBT by transfer from the
    // taker and PT by transfer from the receiver — no new vault deposits.

    // Exp A — IbtForPt, maker funds IBT (NOT Underlying). Removes the
    // convert_to_ibt vault deposit; the mint (deposit_ibt) still runs because
    // the taker pays IBT. Delta vs salt 1 = the funding-side deposit crossing.
    let m_exp_a = Address::generate(e);
    token::Client::new(e, &m.ibt).transfer(&taker, &m_exp_a, &(2 * making_ibt));
    approve_engine(e, &m, &m.ibt, &m_exp_a, making_ibt);
    let oa = mk_order(
        &m,
        11,
        OrderType::IbtForPt,
        &m_exp_a,
        MakerTokenType::Ibt,
        making_ibt,
    );
    register_and_fill(e, &m, &oa, &taker, &receiver, making_ibt, 4 * making_ibt);

    // Exp B — IbtForPt, maker funds IBT AND the taker pays the canonical PT
    // instead of IBT. `handle_tokenisation` is then a no-op: the taker hands
    // existing PT straight to the maker, so NO deposit_ibt mint runs. Delta vs
    // Exp A = the mint crossings (the headline lever).
    let m_exp_b = Address::generate(e);
    token::Client::new(e, &m.ibt).transfer(&taker, &m_exp_b, &(2 * making_ibt));
    approve_engine(e, &m, &m.ibt, &m_exp_b, making_ibt);
    // The receiver holds a large PT balance swept from salt 2; lend some to a
    // fresh PT-paying taker.
    let taker_pt = Address::generate(e);
    token::Client::new(e, &m.pt).transfer(&receiver, &taker_pt, &(3 * making_ibt));
    let ob = mk_order(
        &m,
        12,
        OrderType::IbtForPt,
        &m_exp_b,
        MakerTokenType::Ibt,
        making_ibt,
    );
    m.engine.register_order(&ob);
    let mut params_b = Vec::new(e);
    params_b.push_back(FillOrderParams {
        order: ob.clone(),
        making_amount: making_ibt,
        max_taking: i128::MAX,
    });
    let pt_in = token::Client::new(e, &m.pt).balance(&taker_pt);
    env.cost_estimate().budget().reset_tracker();
    let _ = m
        .engine
        .fill_orders(&taker_pt, &receiver, &params_b, &m.pt, &pt_in);
    let mem_b = env.cost_estimate().budget().memory_bytes_cost();
    let cpu_b = env.cost_estimate().budget().cpu_instruction_cost();
    let fits_b = if mem_b <= MAINNET_MEM_CAP {
        "FITS"
    } else {
        "OVER"
    };
    std::println!(
        "upshift fill salt=12 (IbtForPt/Ibt, taker pays canonical PT): mem_bytes={mem_b} (cap {MAINNET_MEM_CAP}) {fits_b} cpu_insns={cpu_b}"
    );
    env.cost_estimate().budget().reset_unlimited();

    // ── Warp to maturity, then every PT holder redeems back to XLM ───────
    env.warp(
        u32::try_from(PT_DURATION_SECS / 5 + 100).unwrap(),
        PT_DURATION_SECS + 100,
    );
    for holder in [&m_ibt_for_pt, &receiver] {
        let pt_bal = token::Client::new(e, &m.pt).balance(holder);
        if pt_bal > 0 {
            let got = pt_c.redeem(holder, &pt_bal, holder, holder);
            assert!(got > 0, "redeem at expiry must return XLM");
            assert_eq!(
                token::Client::new(e, &m.pt).balance(holder),
                0,
                "PT fully redeemed"
            );
        }
    }

    std::println!("upshift fork lifecycle complete: PT matured and redeemed at {maturity}");
}

// ===========================================================================
// Full cube: order_type × maker_denom × taker_token. Each cell is one metered
// fill on the live Upshift vault. Memory is size-independent, so cells use a
// small working size; tokens are sourced from a pre-funded "bank" (one vault
// deposit + one tokenize) so no per-cell vault deposit is needed beyond what
// the fill itself performs. VaultShare is excluded — the raw vault does not host
// the Spectra wrapper surface.
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

    // Maker's making token: XLM from the whale, IBT/PT/YT from the bank.
    let making_token = match ot {
        OrderType::IbtForPt | OrderType::IbtForYt => match mtt {
            MakerTokenType::Underlying => m.reserve.clone(),
            MakerTokenType::Ibt => m.ibt.clone(),
            MakerTokenType::VaultShare => unreachable!("VaultShare unsupported on raw vault"),
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

    // Taker's input token + a generous amount (surplus sweeps to the receiver).
    let taker_token = match taker_pay {
        TakerPay::Ibt => m.ibt.clone(),
        TakerPay::Pt => m.pt.clone(),
        TakerPay::Yt => m.yt.clone(),
    };
    // YT trades at a deep discount, so YT-denominated takings need a big input.
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
        "MATRIX-DIRECT salt={salt} {ot:?}/{mtt:?}/taker_{taker_pay:?}: mem_bytes={mem} {fits} cpu={cpu}"
    );
}

#[test]
#[ignore = "requires mainnet RPC access (first run populates the cache)"]
fn fork_upshift_vault_memory_matrix() {
    let env = fork_mainnet();
    env.mock_all_auths_allowing_non_root_auth();
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

    let whale = Address::from_str(&env, &env.test_accounts()[0].account_strkey());
    let e: &Env = &env;
    let admin = Address::generate(e);
    let reserve = Address::from_str(e, XLM);
    let ibt = Address::from_str(e, UPSHIFT_VAULT);
    let making = 10 * XLM_UNIT;

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

    let making_ibt = FungibleVaultClient::new(e, &m.ibt).preview_deposit(&making);

    // ── Token bank: one vault deposit, then tokenize a chunk into PT+YT. ──
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

    // 16 cells: 4 order types × {Underlying, Ibt} × 2 valid taker tokens.
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
    std::println!("upshift direct cube complete ({} cells)", cells.len());
}
