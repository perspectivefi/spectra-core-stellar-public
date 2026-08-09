// SPDX-License-Identifier: BUSL-1.1
//! Mainnet-fork memory benchmark for the liquidity-aware `max_*` views.
//!
//! The liquidity-aware `max_deposit` / `max_redeem` / `max_withdraw` overrides
//! each read the live Blend reserve (`load_reserve` → one `get_reserve`
//! cross-contract call). This benchmarks whether that extra pool read is
//! too memory-costly on Soroban. It forks Stellar mainnet against the
//! **live Blend v2 "Fixed" pool**, deploys the wrapper as real WASM over the
//! pool's XLM reserve, and measures each `max_*` call's metered `mem_bytes`
//! against the 40 MiB mainnet per-invocation cap — the same method the LOE
//! `fork_blend_memory_matrix` uses.
//!
//! The wrapper is deployed as WASM (not native `env.register(Type, ())`) so
//! memory is metered exactly as on mainnet; native Rust execution under-counts
//! memory vs the WASM VM. The forked Blend pool is already real WASM.
//!
//! Run (first run needs mainnet RPC to populate the cache):
//! ```text
//! cargo test -p blend-fungible-vault-wrapper fork_ -- --ignored --nocapture
//! ```

extern crate std;

use soroban_env_host::InvocationResourceLimits;
use soroban_sdk::testutils::Address as _;
use soroban_sdk::testutils::cost_estimate::NetworkInvocationResourceLimits;
use soroban_sdk::{Address, Env};

use crate::BlendFungibleVaultWrapperContractClient;

/// Wrapper WASM built with the workspace's 64 KiB `-zstack-size` flag, so the
/// metered memory matches the production build (the stack reservation is
/// counted as linear memory on every cross-contract frame).
const WRAPPER_WASM: &[u8] =
    include_bytes!("../../../target/wasm32v1-none/release/blend_fungible_vault_wrapper.wasm");

// ── Real Stellar mainnet addresses (Blend blend-utils mainnet.contracts.json) ──

/// Blend v2 "Fixed" lending pool.
const BLEND_POOL: &str = "CAJJZSGMMM3PD7N33TAPHGBUGTB43OC73HVIK2L2G6BNGGGYOSSYBXBD";
/// Native XLM Stellar Asset Contract — a reserve in the Fixed pool and the
/// IBT's underlying asset.
const XLM: &str = "CAS3J7GYLGXMF6TDJBBYYSE3HQ6BBSMLNUQ34T6TZMYMW2EVH34XOWMA";

/// Ledger to pin the fork at. Override with `WRAPPER_FORK_LEDGER`.
const FORK_LEDGER: u32 = 62_980_314;

/// XLM has 7 decimals: 1 XLM = 10^7 stroops.
const XLM_UNIT: i128 = 10_000_000;

/// Mainnet per-invocation memory cap (see LOE MEMORY_LIMITS.md).
const MEM_CAP: u64 = 41_943_040; // 40 MiB

fn fork_mainnet() -> soroban_fork::ForkedEnv {
    let rpc = std::env::var("MAINNET_RPC_URL")
        .unwrap_or_else(|_| "https://soroban-rpc.mainnet.stellar.gateway.fm".into());
    let ledger: u32 = std::env::var("WRAPPER_FORK_LEDGER")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(FORK_LEDGER);
    let cache = std::env::var("WRAPPER_FORK_CACHE").unwrap_or_else(|_| {
        concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/fork_cache_wrapper_max.json"
        )
        .into()
    });
    soroban_fork::ForkConfig::new(&rpc)
        .at_ledger(ledger)
        .cache_file(&cache)
        .build()
        .expect("fork build")
}

/// Measure one `max_*` view: reset the tracker, invoke, read metered memory,
/// print FITS/OVER vs the 40 MiB cap, and assert it fits (a liquidity-blind
/// view read must stay well under the settlement-path budget).
fn measure(env: &soroban_fork::ForkedEnv, label: &str, call: impl FnOnce() -> i128) {
    env.cost_estimate().budget().reset_tracker();
    let value = call();
    let mem = env.cost_estimate().budget().memory_bytes_cost();
    let cpu = env.cost_estimate().budget().cpu_instruction_cost();
    env.cost_estimate().budget().reset_unlimited();
    let fits = if mem <= MEM_CAP { "FITS" } else { "OVER" };
    std::println!("MAX-VIEW {label}: value={value} mem_bytes={mem} {fits} cpu={cpu}");
    assert!(
        mem <= MEM_CAP,
        "{label} modelled memory {mem} exceeds the {MEM_CAP}-byte mainnet cap — \
         the live get_reserve read is too costly for a view"
    );
}

/// Deploy the wrapper over the live pool, give an owner a real position, and
/// benchmark every `max_*` view against the mainnet memory cap.
#[test]
#[ignore = "requires mainnet RPC access (first run populates the cache)"]
fn fork_max_functions_memory() {
    let env = fork_mainnet();
    env.mock_all_auths_allowing_non_root_auth();
    // Raise the enforce cap and keep accounting on via reset_unlimited so the
    // deposit setup runs; each max_* call's modelled memory is measured against
    // MEM_CAP explicitly (matches the LOE matrix method).
    env.cost_estimate()
        .enforce_resource_limits(InvocationResourceLimits {
            mem_bytes: 512 * 1024 * 1024,
            instructions: 100_000_000_000,
            ..InvocationResourceLimits::mainnet()
        });
    env.cost_estimate().budget().reset_unlimited();

    let e: &Env = &env;
    let admin = Address::generate(e);
    let pool = Address::from_str(e, BLEND_POOL);
    let reserve = Address::from_str(e, XLM);
    let whale = Address::from_str(e, &env.test_accounts()[0].account_strkey());

    // Deploy the wrapper (real WASM) as the IBT over the pool's XLM reserve.
    let wrapper = env.register(WRAPPER_WASM, (pool.clone(), reserve.clone(), admin.clone()));
    let w = BlendFungibleVaultWrapperContractClient::new(e, &wrapper);

    // Give an owner a real bToken position: fund XLM from the whale, supply it
    // to the pool through the wrapper (a live Blend SUPPLY submit on the fork).
    let owner = Address::generate(e);
    let deposit = 50 * XLM_UNIT;
    soroban_sdk::token::Client::new(e, &reserve).transfer(&whale, &owner, &(deposit * 2));
    let shares = w.deposit(&deposit, &owner, &owner, &owner);
    assert!(
        shares > 0,
        "owner must hold shares for the redeem/withdraw views"
    );

    // Benchmark each liquidity-aware view (one live get_reserve read each).
    measure(&env, "max_deposit", || w.max_deposit(&owner));
    measure(&env, "max_redeem", || w.max_redeem(&owner));
    measure(&env, "max_withdraw", || w.max_withdraw(&owner));
}

/// End-to-end proof that the liquidity-aware quotes are *honest* on the live
/// pool: a withdraw/redeem/deposit taken exactly at the quoted ceiling settles
/// without reverting. This is the point of the overrides — the abstract base's
/// liquidity-blind `max_*` would over-report and the settlement would trap. The
/// quotes are also the settlement-path guards (`withdraw`/`redeem`/`deposit`
/// each check `amount > max_*`), so this exercises them exactly as production.
#[test]
#[ignore = "requires mainnet RPC access (first run populates the cache)"]
fn fork_max_quotes_are_fillable_e2e() {
    let env = fork_mainnet();
    env.mock_all_auths_allowing_non_root_auth();
    env.cost_estimate()
        .enforce_resource_limits(InvocationResourceLimits {
            mem_bytes: 512 * 1024 * 1024,
            instructions: 100_000_000_000,
            ..InvocationResourceLimits::mainnet()
        });
    env.cost_estimate().budget().reset_unlimited();

    let e: &Env = &env;
    let admin = Address::generate(e);
    let pool = Address::from_str(e, BLEND_POOL);
    let reserve = Address::from_str(e, XLM);
    let whale = Address::from_str(e, &env.test_accounts()[0].account_strkey());
    let xlm = soroban_sdk::token::Client::new(e, &reserve);

    let wrapper = env.register(WRAPPER_WASM, (pool.clone(), reserve.clone(), admin.clone()));
    let w = BlendFungibleVaultWrapperContractClient::new(e, &wrapper);

    // ── max_withdraw quote is fillable ───────────────────────────────────
    // Alice supplies 50 XLM, then withdraws exactly the quoted ceiling.
    let alice = Address::generate(e);
    xlm.transfer(&whale, &alice, &(100 * XLM_UNIT));
    w.deposit(&(50 * XLM_UNIT), &alice, &alice, &alice);
    let mw = w.max_withdraw(&alice);
    assert!(mw > 0, "max_withdraw must be positive for a funded owner");
    let before = xlm.balance(&alice);
    // Must NOT revert — the guard is `assets > max_withdraw`.
    w.withdraw(&mw, &alice, &alice, &alice);
    assert_eq!(
        xlm.balance(&alice) - before,
        mw,
        "withdraw at the quote must deliver exactly the quoted underlying"
    );
    assert_eq!(
        w.max_withdraw(&alice),
        0,
        "position must be drained after withdrawing the full quote"
    );

    // ── max_redeem quote is fillable ─────────────────────────────────────
    // Bob supplies 50 XLM, then redeems exactly the quoted share ceiling.
    let bob = Address::generate(e);
    xlm.transfer(&whale, &bob, &(100 * XLM_UNIT));
    let bob_shares = w.deposit(&(50 * XLM_UNIT), &bob, &bob, &bob);
    let mr = w.max_redeem(&bob);
    assert_eq!(
        mr, bob_shares,
        "a liquid pool must quote the owner's full share balance as redeemable"
    );
    let got = w.redeem(&mr, &bob, &bob, &bob); // must NOT revert
    assert!(got > 0, "redeem at the quote must return underlying");
    assert_eq!(w.balance(&bob), 0, "all shares burned after full redeem");

    // ── max_deposit headroom accepts a deposit ───────────────────────────
    let carol = Address::generate(e);
    let headroom = w.max_deposit(&carol);
    let amount = 10 * XLM_UNIT;
    assert!(
        headroom > amount,
        "supply-cap headroom must exceed a modest deposit"
    );
    xlm.transfer(&whale, &carol, &amount);
    let minted = w.deposit(&amount, &carol, &carol, &carol); // within headroom, succeeds
    assert!(
        minted > 0,
        "deposit within the quoted headroom must mint shares"
    );

    std::println!("e2e: max_withdraw/max_redeem/max_deposit quotes all fillable on the live pool");
}

/// Which settlement operation a matrix cell measures. Each op invokes its
/// `max_*` guard (`deposit`→`max_deposit`, `withdraw`→`max_withdraw`,
/// `redeem`→`max_redeem`) before touching the pool.
#[derive(Clone, Copy, Debug)]
enum Op {
    Deposit,
    Withdraw,
    Redeem,
}

/// A copy of the LOE `fork_blend_memory_matrix` method, applied to the wrapper
/// settlement operations that now embed a liquidity-aware `max_*` guard. For
/// each op it funds a fresh owner, then measures ONLY the guarded operation's
/// metered `mem_bytes` against the 40 MiB mainnet cap. This is the production-
/// relevant number: the guard's `get_reserve` read runs inside the real
/// deposit/withdraw/redeem path, not as an isolated view.
#[test]
#[ignore = "requires mainnet RPC access (first run populates the cache)"]
fn fork_operations_memory_matrix() {
    let env = fork_mainnet();
    env.mock_all_auths_allowing_non_root_auth();
    env.cost_estimate()
        .enforce_resource_limits(InvocationResourceLimits {
            mem_bytes: 512 * 1024 * 1024,
            instructions: 100_000_000_000,
            ..InvocationResourceLimits::mainnet()
        });
    env.cost_estimate().budget().reset_unlimited();

    let e: &Env = &env;
    let admin = Address::generate(e);
    let pool = Address::from_str(e, BLEND_POOL);
    let reserve = Address::from_str(e, XLM);
    let whale = Address::from_str(e, &env.test_accounts()[0].account_strkey());
    let xlm = soroban_sdk::token::Client::new(e, &reserve);

    let wrapper = env.register(WRAPPER_WASM, (pool.clone(), reserve.clone(), admin.clone()));
    let w = BlendFungibleVaultWrapperContractClient::new(e, &wrapper);

    let stake = 20 * XLM_UNIT;
    for op in [Op::Deposit, Op::Withdraw, Op::Redeem] {
        let owner = Address::generate(e);
        xlm.transfer(&whale, &owner, &(stake * 2));

        // Setup (under reset_unlimited): fund the position the op consumes, so
        // only the guarded operation itself is inside the measured window.
        let amount = match op {
            Op::Deposit => stake,
            Op::Withdraw => {
                w.deposit(&stake, &owner, &owner, &owner);
                w.max_withdraw(&owner)
            }
            Op::Redeem => {
                w.deposit(&stake, &owner, &owner, &owner);
                w.max_redeem(&owner)
            }
        };

        env.cost_estimate().budget().reset_tracker();
        match op {
            Op::Deposit => {
                w.deposit(&amount, &owner, &owner, &owner);
            }
            Op::Withdraw => {
                w.withdraw(&amount, &owner, &owner, &owner);
            }
            Op::Redeem => {
                w.redeem(&amount, &owner, &owner, &owner);
            }
        }
        let mem = env.cost_estimate().budget().memory_bytes_cost();
        let cpu = env.cost_estimate().budget().cpu_instruction_cost();
        env.cost_estimate().budget().reset_unlimited();

        let fits = if mem <= MEM_CAP { "FITS" } else { "OVER" };
        std::println!("OPS-MATRIX {op:?} (guarded by max_*): mem_bytes={mem} {fits} cpu={cpu}");
        assert!(
            mem <= MEM_CAP,
            "{op:?} modelled memory {mem} exceeds the {MEM_CAP}-byte mainnet cap"
        );
    }
}
