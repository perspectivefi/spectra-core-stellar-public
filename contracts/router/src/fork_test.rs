// SPDX-License-Identifier: BUSL-1.1
//! Mainnet-fork integration test for `BrokerSwap`.
//!
//! Forks Stellar mainnet with `soroban-fork`. The Router is the only
//! contract deployed fresh; the StellarBroker and Aquarius XLM/USDC pool
//! are already live on the fork and are used as-is.
//!
//! Route data captured from StellarBroker on 2026-06-11 for XLM → USDC:
//!   Protocol: AquaConstant, Pool: CA6PU...JBE, si=0, bi=1
//!
//! Note: USDC is sent to a generated contract-style address, not to CALLER
//! directly. CALLER is a classic G… account with no USDC trustline; the
//! XLM SAC returns Error(Contract, #13) "trustline missing" when crediting
//! a G… account that hasn't run ChangeTrust. Contract-style addresses carry
//! their balance in Soroban storage and need no trustline.
//!
//! ```text
//! # Run (requires network — excluded from default `make test`):
//! cargo test -p router fork_broker_swap_real -- --ignored --nocapture
//! ```

extern crate std;

use soroban_sdk::testutils::Address as _;
use soroban_sdk::{Address, BytesN, Env, String, contractclient, vec};
use spectra_common::router_types::{
    Amount, BridgeNativePtToEvmArgs, BrokerSwapArgs, Command, PathStep, Protocol, Recipient, Route,
    TransferArgs, TransferFromArgs,
};

use crate::MainRouterContract;

/// Yield-token wasm, uploaded by PT during `initialize`.
const YT_WASM: &[u8] = include_bytes!("../../../target/wasm32v1-none/release/yield_token.wasm");

/// Operator-only bridge entry points not on the shared `PtBridgeClient`, used
/// to configure the live bridge on the fork (auth satisfied by mock_all_auths).
#[allow(dead_code)] // only the generated `BridgeAdminClient` is used
#[contractclient(name = "BridgeAdminClient")]
pub trait BridgeAdmin {
    fn set_chain_name(env: Env, chain_id: u32, chain_name: String);
    fn set_trusted_remote(env: Env, chain: String, address: String);
    fn get_native_pt_locked(env: Env, pt_address: Address) -> i128;
}

// ─── Real mainnet addresses (captured 2026-06-11) ─────────────────────────

/// XLM (native) Stellar Asset Contract on Soroban.
const XLM_SAC: &str = "CAS3J7GYLGXMF6TDJBBYYSE3HQ6BBSMLNUQ34T6TZMYMW2EVH34XOWMA";

/// USDC on Stellar mainnet (Soroban token contract).
const USDC_SOROBAN: &str = "CCW67TSZV3SSS2HXMBQ5JFGCKJNXKZM7UQUWUZPUTHXSTZLEO7SJMI75";

/// Aquarius constant-product pool: XLM/USDC on mainnet.
const AQUA_POOL: &str = "CA6PUJLBYKZKUEKLZJMKBZLEKP2OTHANDEOWSFF44FTSYLKQPIICCJBE";

/// Live StellarBroker contract on mainnet — already initialized, protocols
/// enabled, fee token set. Used directly from the fork; not redeployed.
const STELLAR_BROKER: &str = "CBWP275BNGLHWFTQVB6QHA67MLX7WTX6YZ5LNSUVOK7W2TMKWX7OPYOJ";

/// Real mainnet account used as the XLM source.
/// Balance at fork time: 73,423,086 stroops (≈7.34 XLM).
const CALLER: &str = "GCNC7GXVI2LYUNS7VMJXRDC57HV2LPX2PXXTSVQIJ6NFH5QQWNNIUUC7";

// ─── Helpers ───────────────────────────────────────────────────────────────

/// Ledger sequence pinned at capture time (2026-06-11).
const FORK_LEDGER: u32 = 62_980_314;

/// Cache file persists the RPC ledger-entry fetches for ledger 62_980_314.
/// On first run it is populated from the RPC; subsequent runs replay from it,
/// making USDC output values 100% deterministic without hitting the network.
const FORK_CACHE: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/tests/fork_cache_62980314.json"
);

fn fork_mainnet() -> soroban_fork::ForkedEnv {
    let rpc = std::env::var("MAINNET_RPC_URL")
        .unwrap_or_else(|_| "https://soroban-rpc.mainnet.stellar.gateway.fm".into());
    soroban_fork::ForkConfig::new(&rpc)
        .at_ledger(FORK_LEDGER)
        .cache_file(FORK_CACHE)
        .build()
        .expect("fork build")
}

// ─── Tests ─────────────────────────────────────────────────────────────────

/// Full BrokerSwap happy-path on a mainnet fork.
/// Swaps 5 XLM from CALLER's real balance through the live StellarBroker
/// and Aquarius XLM/USDC pool. Asserts USDC arrives at a contract-style
/// recipient and the Router ends with zero balances of both tokens.
#[test]
#[ignore = "requires mainnet RPC access"]
fn fork_broker_swap_real_xlm_to_usdc() {
    let env = fork_mainnet();
    env.mock_all_auths();
    let e: &soroban_sdk::Env = &env;

    let admin = Address::generate(e);

    // ── Deploy Router ────────────────────────────────────────────────────
    let registry_addr = Address::generate(e);
    let router_addr = env.register(MainRouterContract, (admin.clone(), registry_addr.clone()));
    let router = crate::MainRouterContractClient::new(e, &router_addr);

    // ── Point Router at the live mainnet StellarBroker ───────────────────
    let broker_addr = Address::from_str(e, STELLAR_BROKER);
    router.set_broker(&admin, &broker_addr);

    // ── Addresses ────────────────────────────────────────────────────────
    let xlm = Address::from_str(e, XLM_SAC);
    let usdc = Address::from_str(e, USDC_SOROBAN);
    let caller = Address::from_str(e, CALLER);

    // Contract-style address: carries Soroban-storage balance, no trustline
    // required — unlike CALLER (a G… account) which has no USDC trustline.
    let usdc_recipient = Address::generate(e);

    // ── Route: 5 XLM → USDC via live Aquarius pool ──────────────────────
    // 5 XLM = 50_000_000 stroops (< CALLER's 73,423,086 balance).
    // Mainnet quote (2026-06-11): 10 XLM → ≈1.912 USDC (7 decimals =
    // 19_120_000 units). Conservative mins absorb pool-state drift
    // between capture time and the fork's RPC snapshot.
    let mut path = soroban_sdk::Vec::new(e);
    path.push_back(PathStep {
        protocol: Protocol::AquaConstant,
        asset: usdc.clone(),
        pool: Address::from_str(e, AQUA_POOL),
        si: 0,
        bi: 1,
    });
    let mut routes = soroban_sdk::Vec::new(e);
    routes.push_back(Route {
        path,
        amount: 50_000_000,
        min: 5_000_000,
        estimated: 9_560_000,
    });

    // ── Approve Router to pull caller's XLM ─────────────────────────────
    soroban_sdk::token::Client::new(e, &xlm).approve(
        &caller,
        &router_addr,
        &i128::MAX,
        &(e.ledger().sequence() + 100_000),
    );

    // ── Execute: TransferFrom → BrokerSwap → Transfer ────────────────────
    let mut cmds = soroban_sdk::Vec::new(e);
    cmds.push_back(Command::TransferFrom(TransferFromArgs {
        token: xlm.clone(),
        amount: Amount::Exact(50_000_000),
    }));
    cmds.push_back(Command::BrokerSwap(BrokerSwapArgs {
        token_in: xlm.clone(),
        token_out: usdc.clone(),
        routes,
        min_amount_out: 5_000_000,
        fixed_fee_permille: 0,
        fee_path: soroban_sdk::Vec::new(e),
    }));
    cmds.push_back(Command::Transfer(TransferArgs {
        token: usdc.clone(),
        recipient: Recipient::Address(usdc_recipient.clone()),
        amount: Amount::Balance,
    }));

    router.execute(&caller, &cmds, &(e.ledger().timestamp() + 100));

    // ── Assert results ───────────────────────────────────────────────────
    let usdc_client = soroban_sdk::token::Client::new(e, &usdc);
    let xlm_client = soroban_sdk::token::Client::new(e, &xlm);

    let recipient_usdc = usdc_client.balance(&usdc_recipient);
    let router_usdc = usdc_client.balance(&router_addr);
    let caller_xlm = xlm_client.balance(&caller);

    std::println!("Recipient USDC: {recipient_usdc}");
    std::println!("Router USDC:    {router_usdc}");
    std::println!("Caller XLM:     {caller_xlm}");

    assert_eq!(router_usdc, 0, "Router should have 0 USDC after swap");
    assert_eq!(
        recipient_usdc, 9_477_987,
        "5 XLM → USDC at ledger {FORK_LEDGER}"
    );
    assert_eq!(
        xlm_client.balance(&router_addr),
        0,
        "Router should have 0 XLM after swap"
    );
}

/// BrokerSwap at ledger+500 (≈42 minutes forward): 1 XLM → USDC.
/// Exercises the swap with a minimal viable amount at a later simulated block.
#[test]
#[ignore = "requires mainnet RPC access"]
fn fork_broker_swap_real_1_xlm_500_ledgers() {
    let env = fork_mainnet();
    env.warp_ledger(500);
    env.mock_all_auths();
    let e: &soroban_sdk::Env = &env;

    let admin = Address::generate(e);

    let router_addr = env.register(MainRouterContract, (admin.clone(), Address::generate(e)));
    let router = crate::MainRouterContractClient::new(e, &router_addr);
    router.set_broker(&admin, &Address::from_str(e, STELLAR_BROKER));

    let xlm = Address::from_str(e, XLM_SAC);
    let usdc = Address::from_str(e, USDC_SOROBAN);
    let caller = Address::from_str(e, CALLER);
    let usdc_recipient = Address::generate(e);

    // 1 XLM = 10_000_000 stroops. Quote: 10 XLM → ≈1.912 USDC → 1 XLM → ≈1_912_000 units.
    let mut path = soroban_sdk::Vec::new(e);
    path.push_back(PathStep {
        protocol: Protocol::AquaConstant,
        asset: usdc.clone(),
        pool: Address::from_str(e, AQUA_POOL),
        si: 0,
        bi: 1,
    });
    let mut routes = soroban_sdk::Vec::new(e);
    routes.push_back(Route {
        path,
        amount: 10_000_000,
        min: 1_000_000,
        estimated: 1_912_000,
    });

    soroban_sdk::token::Client::new(e, &xlm).approve(
        &caller,
        &router_addr,
        &i128::MAX,
        &(e.ledger().sequence() + 100_000),
    );

    let mut cmds = soroban_sdk::Vec::new(e);
    cmds.push_back(Command::TransferFrom(TransferFromArgs {
        token: xlm.clone(),
        amount: Amount::Exact(10_000_000),
    }));
    cmds.push_back(Command::BrokerSwap(BrokerSwapArgs {
        token_in: xlm.clone(),
        token_out: usdc.clone(),
        routes,
        min_amount_out: 1_000_000,
        fixed_fee_permille: 0,
        fee_path: soroban_sdk::Vec::new(e),
    }));
    cmds.push_back(Command::Transfer(TransferArgs {
        token: usdc.clone(),
        recipient: Recipient::Address(usdc_recipient.clone()),
        amount: Amount::Balance,
    }));

    router.execute(&caller, &cmds, &(e.ledger().timestamp() + 100));

    let usdc_client = soroban_sdk::token::Client::new(e, &usdc);
    let xlm_client = soroban_sdk::token::Client::new(e, &xlm);
    let recipient_usdc = usdc_client.balance(&usdc_recipient);

    std::println!("Recipient USDC (1 XLM, +500 ledgers): {recipient_usdc}");

    assert_eq!(usdc_client.balance(&router_addr), 0);
    assert_eq!(
        recipient_usdc, 1_895_598,
        "1 XLM → USDC at ledger {FORK_LEDGER} +500"
    );
    assert_eq!(xlm_client.balance(&router_addr), 0);
}

/// BrokerSwap at ledger+2000 (≈2.8 hours forward): 6 XLM → USDC.
/// Exercises the swap with a near-maximum amount at a later simulated block.
#[test]
#[ignore = "requires mainnet RPC access"]
fn fork_broker_swap_real_6_xlm_2000_ledgers() {
    let env = fork_mainnet();
    env.warp_ledger(2000);
    env.mock_all_auths();
    let e: &soroban_sdk::Env = &env;

    let admin = Address::generate(e);

    let router_addr = env.register(MainRouterContract, (admin.clone(), Address::generate(e)));
    let router = crate::MainRouterContractClient::new(e, &router_addr);
    router.set_broker(&admin, &Address::from_str(e, STELLAR_BROKER));

    let xlm = Address::from_str(e, XLM_SAC);
    let usdc = Address::from_str(e, USDC_SOROBAN);
    let caller = Address::from_str(e, CALLER);
    let usdc_recipient = Address::generate(e);

    // 6 XLM = 60_000_000 stroops (< CALLER's 73,423,086). Quote: ≈11_472_000 units.
    let mut path = soroban_sdk::Vec::new(e);
    path.push_back(PathStep {
        protocol: Protocol::AquaConstant,
        asset: usdc.clone(),
        pool: Address::from_str(e, AQUA_POOL),
        si: 0,
        bi: 1,
    });
    let mut routes = soroban_sdk::Vec::new(e);
    routes.push_back(Route {
        path,
        amount: 60_000_000,
        min: 5_000_000,
        estimated: 11_472_000,
    });

    soroban_sdk::token::Client::new(e, &xlm).approve(
        &caller,
        &router_addr,
        &i128::MAX,
        &(e.ledger().sequence() + 100_000),
    );

    let mut cmds = soroban_sdk::Vec::new(e);
    cmds.push_back(Command::TransferFrom(TransferFromArgs {
        token: xlm.clone(),
        amount: Amount::Exact(60_000_000),
    }));
    cmds.push_back(Command::BrokerSwap(BrokerSwapArgs {
        token_in: xlm.clone(),
        token_out: usdc.clone(),
        routes,
        min_amount_out: 5_000_000,
        fixed_fee_permille: 0,
        fee_path: soroban_sdk::Vec::new(e),
    }));
    cmds.push_back(Command::Transfer(TransferArgs {
        token: usdc.clone(),
        recipient: Recipient::Address(usdc_recipient.clone()),
        amount: Amount::Balance,
    }));

    router.execute(&caller, &cmds, &(e.ledger().timestamp() + 100));

    let usdc_client = soroban_sdk::token::Client::new(e, &usdc);
    let xlm_client = soroban_sdk::token::Client::new(e, &xlm);
    let recipient_usdc = usdc_client.balance(&usdc_recipient);

    std::println!("Recipient USDC (6 XLM, +2000 ledgers): {recipient_usdc}");

    assert_eq!(usdc_client.balance(&router_addr), 0);
    assert_eq!(
        recipient_usdc, 11_373_584,
        "6 XLM → USDC at ledger {FORK_LEDGER} +2000"
    );
    assert_eq!(xlm_client.balance(&router_addr), 0);
}

// ─── Bridge: real Stellar→EVM PT bridge-out on a mainnet fork ───────

/// Live `PtBridge` on Stellar mainnet.
const PT_BRIDGE: &str = "CACMUSAUMYFVDLWMGDQE7C5M3L4DVZLXY3U3FSVW3KQ4N747RI6I67YW";

/// the Router calls the live `PtBridge` through the shared
/// `bridge_traits::PtBridgeClient`, whose `bridge_native_pt_to_evm` returns
/// `BytesN<32>`. This forks mainnet and drives a **real** native-PT bridge-out:
/// deploy a PT, fund the Router with it, configure the bridge's chain +
/// trusted-remote (operator ops, satisfied by `mock_all_auths`), then `execute`
/// a `BridgeNativePtToEvm` command with `gas_amount = 0` (the cross-chain
/// message is emitted but unpaid). Decoding the 32-byte message id is the
/// point: before the fix, the router trapped on the non-void bridge return
/// and the whole batch reverted.
///
/// The EVM-delivery leg cannot be forked; this exercises the full Stellar side —
/// PT lock, messenger call, and `BytesN<32>` return decode through our client.
#[test]
fn fork_bridge_native_pt_to_evm_real() {
    let env = fork_mainnet();
    env.mock_all_auths();
    env.cost_estimate().budget().reset_unlimited();
    let e: &Env = &env;
    let admin = Address::generate(e);

    // ── Deploy a real PT stack on the fork ───────────────────────────────
    let underlying = env.register(spectra_mocks::MockUnderlyingContract, ());
    let und_c = spectra_mocks::MockUnderlyingContractClient::new(e, &underlying);
    und_c.initialize(
        &String::from_str(e, "USD"),
        &String::from_str(e, "USD"),
        &6u32,
    );

    let ibt = env.register(spectra_mocks::MockIbtOzContract, ());
    spectra_mocks::MockIbtOzContractClient::new(e, &ibt).initialize(
        &underlying,
        &String::from_str(e, "IBT"),
        &String::from_str(e, "IBT"),
        &0u32,
    );

    let registry = env.register(registry::RegistryContract, (admin.clone(),));
    let reg_c = registry::RegistryContractClient::new(e, &registry);
    reg_c.set_fee_collector(&admin, &admin);
    reg_c.set_yt_wasm_hash(&admin, &env.deployer().upload_contract_wasm(YT_WASM));

    und_c.mint(&ibt, &1i128); // seed so PT init's total_assets != 0

    let pt = env.register(principal_token::PrincipalTokenContract, ());
    let pt_c = principal_token::PrincipalTokenContractClient::new(e, &pt);
    pt_c.initialize(&ibt, &(86_400_u64 * 30), &admin, &registry);

    // ── Deploy Router, point it at the live bridge ───────────────────────
    let router_addr = env.register(MainRouterContract, (admin.clone(), registry.clone()));
    let router = crate::MainRouterContractClient::new(e, &router_addr);
    let bridge = Address::from_str(e, PT_BRIDGE);
    router.set_bridge(&admin, &bridge);

    // ── Fund the Router with PT (deposit underlying, mint PT to the router) ─
    let depositor = Address::generate(e);
    let assets = 100_000_000i128; // 100 underlying (6 dec)
    und_c.mint(&depositor, &assets);
    und_c.approve(&depositor, &pt, &assets, &(e.ledger().sequence() + 1000));
    let pt_minted = pt_c.deposit(&depositor, &assets, &router_addr, &depositor);
    assert!(pt_minted > 0);

    // ── Configure the live bridge for target chain 1 (operator ops) ──────
    let admin_c = BridgeAdminClient::new(e, &bridge);
    let chain = String::from_str(e, "ethereum");
    admin_c.set_chain_name(&1u32, &chain);
    admin_c.set_trusted_remote(
        &chain,
        &String::from_str(e, "0x00000000000000000000000000000000000000aa"),
    );

    // ── Bridge the PT to EVM (gas 0: message emitted, unpaid) ─────
    let cmds = vec![
        e,
        Command::BridgeNativePtToEvm(BridgeNativePtToEvmArgs {
            pt: pt.clone(),
            amount: Amount::Exact(pt_minted),
            target_chain_id: 1,
            evm_recipient: BytesN::from_array(e, &[0xAB; 20]),
            gas_token: underlying.clone(), // unused at gas_amount = 0
            gas_amount: Amount::Exact(0),
            refund_address: Recipient::Caller,
            min_amount_out: 0,
        }),
    ];
    router.execute(&depositor, &cmds, &(e.ledger().timestamp() + 100));

    // ── The bridge locked the PT: proves the full call + BytesN<32> return
    //    decode succeeded end-to-end (previously the router trapped here). ──
    let locked = admin_c.get_native_pt_locked(&pt);
    assert!(locked > 0, "bridge must record the locked PT amount");
    assert_eq!(
        soroban_sdk::token::Client::new(e, &pt).balance(&router_addr),
        0,
        "router's PT must be locked in the bridge"
    );
    std::println!("bridged {locked} PT to EVM via the live bridge");
}
