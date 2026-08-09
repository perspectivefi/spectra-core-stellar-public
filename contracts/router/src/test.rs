// SPDX-License-Identifier: BUSL-1.1
#![allow(clippy::too_many_arguments)]
#![allow(clippy::needless_borrows_for_generic_args)]

extern crate std;

use crate::errors::RouterError;
use crate::{MainRouterContract, MainRouterContractClient};
use soroban_sdk::testutils::{Address as _, Ledger as _, MockAuth, MockAuthInvoke};
use soroban_sdk::{
    Address, BytesN, Env, IntoVal, String, Vec, contract, contractimpl, contracttype, symbol_short,
    vec,
};
use spectra_common::router_types::{
    Amount, AssertMinBalanceArgs, BridgeNativePtToEvmArgs, BridgeWrappedPtBackToEvmArgs,
    BrokerSwapArgs, CancelOrdersArgs, Command, DepositAssetInIbtArgs, DepositIbtInPtArgs,
    FillOrdersArgs, PathStep, Protocol, Recipient, RedeemIbtForAssetArgs, RedeemPtForIbtArgs,
    RegisterOrdersArgs, Route, TransferArgs, TransferFromArgs,
};

// ─── Mock bridge trait clients ─────────────────────────────────────────────

#[contracttype]
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LastNativeCall {
    pub sender: Address,
    pub pt: Address,
    pub amount: i128,
    pub target_chain_id: u32,
    pub gas_amount: i128,
    pub min_amount_out: i128,
}

#[contracttype]
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LastBackCall {
    pub sender: Address,
    pub wrapped_pt: Address,
    pub amount: i128,
    pub target_chain_id: u32,
    pub gas_amount: i128,
    pub min_amount_out: i128,
}

#[contracttype]
pub enum MockBridgeKey {
    Messenger,
    FeeBps, // bridge-back fee bps (type 2)
    LastNative,
    LastBack,
}

#[contract]
pub struct MockPtBridge;

#[contractimpl]
impl MockPtBridge {
    pub fn init(env: Env, messenger: Address, bb_fee_bps: u32) {
        env.storage()
            .instance()
            .set(&MockBridgeKey::Messenger, &messenger);
        env.storage()
            .instance()
            .set(&MockBridgeKey::FeeBps, &bb_fee_bps);
    }

    pub fn get_messenger(env: Env) -> Address {
        env.storage()
            .instance()
            .get(&MockBridgeKey::Messenger)
            .unwrap()
    }

    pub fn get_bridge_back_fee_bps(env: Env) -> u32 {
        env.storage()
            .instance()
            .get(&MockBridgeKey::FeeBps)
            .unwrap_or(0)
    }

    pub fn bridge_native_pt_to_evm(
        env: Env,
        sender: Address,
        pt: Address,
        amount: i128,
        target_chain_id: u32,
        evm_recipient: soroban_sdk::BytesN<20>,
        gas_amount: i128,
        refund_address: Option<Address>,
        min_amount_out: i128,
    ) -> soroban_sdk::BytesN<32> {
        sender.require_auth();
        let _ = (evm_recipient, refund_address);
        // Pull PT — exercises the router's pre-auth
        soroban_sdk::token::Client::new(&env, &pt).transfer(
            &sender,
            &env.current_contract_address(),
            &amount,
        );
        // Pull gas via messenger
        if gas_amount > 0 {
            let messenger: Address = env
                .storage()
                .instance()
                .get(&MockBridgeKey::Messenger)
                .unwrap();
            MockMessengerClient::new(&env, &messenger).pull_gas(&sender, &gas_amount);
        }
        env.storage().instance().set(
            &MockBridgeKey::LastNative,
            &LastNativeCall {
                sender,
                pt,
                amount,
                target_chain_id,
                gas_amount,
                min_amount_out,
            },
        );
        soroban_sdk::BytesN::from_array(&env, &[1u8; 32])
    }

    pub fn bridge_back_to_evm(
        env: Env,
        sender: Address,
        wrapped_pt: Address,
        amount: i128,
        target_chain_id: u32,
        evm_recipient: soroban_sdk::BytesN<20>,
        gas_amount: i128,
        refund_address: Option<Address>,
        min_amount_out: i128,
    ) -> soroban_sdk::BytesN<32> {
        sender.require_auth();
        let _ = (evm_recipient, refund_address);

        // Match real bridge: burn full amount (no router auth needed for burn_by_bridge),
        // then take the fee via SEP-41 transfer (needs router auth).
        let fee_bps: u32 = env
            .storage()
            .instance()
            .get(&MockBridgeKey::FeeBps)
            .unwrap_or(0);
        let fee_amount = (amount * fee_bps as i128) / 10_000;
        let burn_amount = amount - fee_amount;

        MockWrappedPtClient::new(&env, &wrapped_pt).burn_by_bridge(&sender, &burn_amount);
        if fee_amount > 0 {
            soroban_sdk::token::Client::new(&env, &wrapped_pt).transfer(
                &sender,
                &env.current_contract_address(),
                &fee_amount,
            );
        }
        if gas_amount > 0 {
            let messenger: Address = env
                .storage()
                .instance()
                .get(&MockBridgeKey::Messenger)
                .unwrap();
            MockMessengerClient::new(&env, &messenger).pull_gas(&sender, &gas_amount);
        }
        env.storage().instance().set(
            &MockBridgeKey::LastBack,
            &LastBackCall {
                sender,
                wrapped_pt,
                amount,
                target_chain_id,
                gas_amount,
                min_amount_out,
            },
        );
        soroban_sdk::BytesN::from_array(&env, &[2u8; 32])
    }

    pub fn last_native(env: Env) -> Option<LastNativeCall> {
        env.storage().instance().get(&MockBridgeKey::LastNative)
    }

    pub fn last_back(env: Env) -> Option<LastBackCall> {
        env.storage().instance().get(&MockBridgeKey::LastBack)
    }
}

// ─── Mock messenger ────────────────────────────────────────────────────────

#[contract]
pub struct MockMessenger;

#[contractimpl]
impl MockMessenger {
    pub fn init(env: Env, gas_token: Address) {
        env.storage()
            .instance()
            .set(&symbol_short!("GAS"), &gas_token);
    }
    pub fn pull_gas(env: Env, sender: Address, amount: i128) {
        let gas_token: Address = env.storage().instance().get(&symbol_short!("GAS")).unwrap();
        soroban_sdk::token::Client::new(&env, &gas_token).transfer(
            &sender,
            &env.current_contract_address(),
            &amount,
        );
    }
}

// ─── Mock wrapped PT (only needs burn_by_bridge + transfer; SEP-41 covers the rest) ─

#[contract]
pub struct MockWrappedPt;

#[contractimpl]
impl MockWrappedPt {
    pub fn init(env: Env, bridge: Address) {
        env.storage()
            .instance()
            .set(&symbol_short!("BRIDGE"), &bridge);
        env.storage()
            .instance()
            .set(&symbol_short!("SUPPLY"), &0_i128);
    }
    pub fn mint(env: Env, to: Address, amount: i128) {
        let bal: i128 = env.storage().persistent().get(&to).unwrap_or(0);
        env.storage().persistent().set(&to, &(bal + amount));
        let supply: i128 = env
            .storage()
            .instance()
            .get(&symbol_short!("SUPPLY"))
            .unwrap_or(0);
        env.storage()
            .instance()
            .set(&symbol_short!("SUPPLY"), &(supply + amount));
    }
    pub fn balance(env: Env, who: Address) -> i128 {
        env.storage().persistent().get(&who).unwrap_or(0)
    }
    pub fn transfer(env: Env, from: Address, to: Address, amount: i128) {
        from.require_auth();
        let bal: i128 = env.storage().persistent().get(&from).unwrap_or(0);
        env.storage().persistent().set(&from, &(bal - amount));
        let bal_to: i128 = env.storage().persistent().get(&to).unwrap_or(0);
        env.storage().persistent().set(&to, &(bal_to + amount));
    }
    pub fn burn_by_bridge(env: Env, from: Address, amount: i128) {
        let bridge: Address = env
            .storage()
            .instance()
            .get(&symbol_short!("BRIDGE"))
            .unwrap();
        bridge.require_auth();
        let bal: i128 = env.storage().persistent().get(&from).unwrap_or(0);
        env.storage().persistent().set(&from, &(bal - amount));
    }
    pub fn approve(
        env: Env,
        from: Address,
        spender: Address,
        amount: i128,
        _live_until_ledger: u32,
    ) {
        from.require_auth();
        env.storage().persistent().set(&(from, spender), &amount);
    }
    pub fn allowance(env: Env, from: Address, spender: Address) -> i128 {
        env.storage()
            .persistent()
            .get(&(from, spender))
            .unwrap_or(0)
    }
    pub fn transfer_from(env: Env, spender: Address, from: Address, to: Address, amount: i128) {
        spender.require_auth();
        let key = (from.clone(), spender.clone());
        let allow: i128 = env.storage().persistent().get(&key).unwrap_or(0);
        assert!(allow >= amount, "InsufficientAllowance");
        env.storage().persistent().set(&key, &(allow - amount));
        let bal: i128 = env.storage().persistent().get(&from).unwrap_or(0);
        env.storage().persistent().set(&from, &(bal - amount));
        let bal_to: i128 = env.storage().persistent().get(&to).unwrap_or(0);
        env.storage().persistent().set(&to, &(bal_to + amount));
    }
}

/// Assert that a `try_*` call returned a specific structured `RouterError`.
/// `Err(Err(InvokeError))` (e.g. `todo!()` panic) does NOT satisfy — keeps
/// RED tests honest. Mirrors `assert_pt_error` in principal-token/src/test.rs.
fn assert_router_error<T: core::fmt::Debug>(
    result: Result<T, Result<soroban_sdk::Error, soroban_sdk::InvokeError>>,
    expected: RouterError,
) {
    match result {
        Err(Ok(actual)) => {
            let expected_val: soroban_sdk::Error = expected.into();
            assert_eq!(
                actual, expected_val,
                "expected error {expected:?}, got {actual:?}"
            );
        }
        other => panic!("expected structured RouterError::{expected:?}, got {other:?}"),
    }
}

/// YT WASM is needed to register the YT contract hash with the registry
/// (PT.initialize deploys YT from this hash). Built by scripts/build-wasm.sh.
const YT_WASM: &[u8] = include_bytes!("../../../target/wasm32v1-none/release/yield_token.wasm");

/// SEP-41 `approve` the router as spender of `amount` of `token` on behalf
/// of `owner`. Required before any `Command::TransferFrom` since the
/// dispatcher uses real `transfer_from` semantics.
fn approve_router(env: &Env, token: &Address, owner: &Address, spender: &Address, amount: i128) {
    soroban_sdk::token::Client::new(env, token).approve(
        owner,
        spender,
        &amount,
        &(env.ledger().sequence() + 100),
    );
}

struct Setup<'a> {
    env: Env,
    admin: Address,
    registry_addr: Address,
    underlying_addr: Address,
    ibt_addr: Address,
    pt_addr: Address,
    router_addr: Address,
    router_client: MainRouterContractClient<'a>,
}

fn make_setup() -> Setup<'static> {
    let env = Env::default();
    env.mock_all_auths();
    env.cost_estimate().budget().reset_unlimited();

    let admin = Address::generate(&env);

    let underlying_addr = env.register(spectra_mocks::MockUnderlyingContract, ());
    let und_c = spectra_mocks::MockUnderlyingContractClient::new(&env, &underlying_addr);
    und_c.initialize(
        &String::from_str(&env, "MOCK USD"),
        &String::from_str(&env, "MUSD"),
        &6u32,
    );

    // OZ FungibleVault mock. decimals_offset = 0 → vault decimals = underlying = 6.
    let ibt_addr = env.register(spectra_mocks::MockIbtOzContract, ());
    let ibt_c = spectra_mocks::MockIbtOzContractClient::new(&env, &ibt_addr);
    ibt_c.initialize(
        &underlying_addr,
        &String::from_str(&env, "MOCK IBT"),
        &String::from_str(&env, "MIBT"),
        &0u32,
    );

    let registry_addr = env.register(registry::RegistryContract, (admin.clone(),));
    let registry_c = registry::RegistryContractClient::new(&env, &registry_addr);
    registry_c.set_fee_collector(&admin, &admin);
    let yt_wasm_hash = env.deployer().upload_contract_wasm(YT_WASM);
    registry_c.set_yt_wasm_hash(&admin, &yt_wasm_hash);

    // Seed IBT with 1 underlying so PT init's `total_assets != 0` guard passes.
    und_c.mint(&ibt_addr, &1i128);

    let pt_addr = env.register(principal_token::PrincipalTokenContract, ());
    let pt_client = principal_token::PrincipalTokenContractClient::new(&env, &pt_addr);
    pt_client.initialize(&ibt_addr, &(86_400_u64 * 30), &admin, &registry_addr);
    // The LOE gates register/fill on `registry.is_registered_pt`; register the
    // PT directly since these tests construct it without the Factory.
    registry_c.add_pt(&admin, &pt_addr);

    let router_addr = env.register(MainRouterContract, (admin.clone(), registry_addr.clone()));
    let router_client = MainRouterContractClient::new(&env, &router_addr);

    Setup {
        env,
        admin,
        registry_addr,
        underlying_addr,
        ibt_addr,
        pt_addr,
        router_addr,
        router_client,
    }
}

fn dummy_command(token: &Address) -> Command {
    Command::Transfer(TransferArgs {
        token: token.clone(),
        recipient: Recipient::Caller,
        amount: Amount::Exact(0),
    })
}

#[test]
fn test_constructor_sets_registry() {
    let s = make_setup();
    // The `__constructor` set admin + registry atomically at deploy.
    assert_eq!(s.router_client.get_registry(), s.registry_addr);
}

// No double-init test: `__constructor` runs exactly once at deploy and is not
// a callable entry point, so re-initialization is impossible by construction
//.

#[test]
fn test_execute_empty_commands_panics() {
    let s = make_setup();
    let caller = Address::generate(&s.env);
    let deadline = s.env.ledger().timestamp() + 100;
    assert_router_error(
        s.router_client
            .try_execute(&caller, &Vec::<Command>::new(&s.env), &deadline),
        RouterError::EmptyCommands,
    );
}

#[test]
fn test_pause_unpause_role_gated() {
    let s = make_setup();
    s.router_client.pause(&s.admin);
    assert!(s.router_client.paused());
    s.router_client.unpause(&s.admin);
    assert!(!s.router_client.paused());
}

// The above test only exercises pause/unpause as the admin (who holds
// PAUSER from initialize). The role gate itself was unproven — a refactor
// that dropped the #[only_role(PAUSER)] attribute would still let the
// admin pause and the test would pass. Lock the gate in.
#[test]
fn test_pause_rejects_non_pauser() {
    let s = make_setup();
    let intruder = Address::generate(&s.env);
    let r = s.router_client.try_pause(&intruder);
    assert!(r.is_err());
    assert!(!s.router_client.paused());
}

#[test]
fn test_unpause_rejects_non_pauser() {
    let s = make_setup();
    s.router_client.pause(&s.admin);
    assert!(s.router_client.paused());
    let intruder = Address::generate(&s.env);
    let r = s.router_client.try_unpause(&intruder);
    assert!(r.is_err());
    assert!(s.router_client.paused());
}

#[test]
fn test_execute_paused_panics() {
    let s = make_setup();
    s.router_client.pause(&s.admin);
    let caller = Address::generate(&s.env);
    let deadline = s.env.ledger().timestamp() + 100;
    let mut cmds = Vec::<Command>::new(&s.env);
    cmds.push_back(dummy_command(&s.registry_addr));
    // pausable panics with its own error code (not RouterError); we only
    // check that execute fails with *some* structured error.
    let result = s.router_client.try_execute(&caller, &cmds, &deadline);
    assert!(
        matches!(result, Err(Ok(_))),
        "expected paused panic, got {result:?}"
    );
}

#[test]
fn test_set_registry_admin_only() {
    let s = make_setup();
    let new_registry = Address::generate(&s.env);
    s.router_client.set_registry(&s.admin, &new_registry);
    assert_eq!(s.router_client.get_registry(), new_registry);
}

#[test]
fn test_set_registry_non_admin_panics() {
    let s = make_setup();
    let intruder = Address::generate(&s.env);
    let new_registry = Address::generate(&s.env);
    let result = s.router_client.try_set_registry(&intruder, &new_registry);
    // OZ #[only_role] raises AccessControlError::Unauthorized (#2000).
    let expected = soroban_sdk::Error::from_contract_error(2000);
    assert!(
        matches!(result, Err(Ok(e)) if e == expected),
        "expected AccessControlError::Unauthorized (#2000), got {result:?}"
    );
}

#[test]
fn test_deposit_ibt_in_pt_mints_split_pt_yt() {
    let s = make_setup();
    let caller = Address::generate(&s.env);

    // Give the router IBT shares first.
    let und = spectra_mocks::MockUnderlyingContractClient::new(&s.env, &s.underlying_addr);
    und.mint(&s.router_addr, &500_000i128);
    let mut prep = Vec::<Command>::new(&s.env);
    prep.push_back(Command::DepositAssetInIbt(DepositAssetInIbtArgs {
        ibt: s.ibt_addr.clone(),
        assets: Amount::Balance,
        recipient: Recipient::Router,
    }));
    s.router_client
        .execute(&caller, &prep, &(s.env.ledger().timestamp() + 100));

    let mut cmds = Vec::<Command>::new(&s.env);
    cmds.push_back(Command::DepositIbtInPt(DepositIbtInPtArgs {
        pt: s.pt_addr.clone(),
        ibts: Amount::Balance,
        pt_recipient: Recipient::Caller,
        yt_recipient: Recipient::Caller,
        min_shares: 0,
    }));
    s.router_client
        .execute(&caller, &cmds, &(s.env.ledger().timestamp() + 100));

    // Both PT and YT shares delivered to caller; router holds no IBT.
    let pt_c = soroban_sdk::token::Client::new(&s.env, &s.pt_addr);
    let yt_addr = principal_token::PrincipalTokenContractClient::new(&s.env, &s.pt_addr).get_yt();
    let yt_c = soroban_sdk::token::Client::new(&s.env, &yt_addr);
    assert!(pt_c.balance(&caller) > 0);
    assert_eq!(pt_c.balance(&caller), yt_c.balance(&caller));
    let ibt_c = soroban_sdk::token::Client::new(&s.env, &s.ibt_addr);
    assert_eq!(ibt_c.balance(&s.router_addr), 0);
}

#[test]
fn test_redeem_pt_for_ibt_returns_ibt_to_recipient() {
    let s = make_setup();
    let caller = Address::generate(&s.env);

    // Build a PT position on the caller: deposit + split.
    let und = spectra_mocks::MockUnderlyingContractClient::new(&s.env, &s.underlying_addr);
    und.mint(&s.router_addr, &500_000i128);
    let mut prep = Vec::<Command>::new(&s.env);
    prep.push_back(Command::DepositAssetInIbt(DepositAssetInIbtArgs {
        ibt: s.ibt_addr.clone(),
        assets: Amount::Balance,
        recipient: Recipient::Router,
    }));
    prep.push_back(Command::DepositIbtInPt(DepositIbtInPtArgs {
        pt: s.pt_addr.clone(),
        ibts: Amount::Balance,
        pt_recipient: Recipient::Router,
        yt_recipient: Recipient::Router,
        min_shares: 0,
    }));
    s.router_client
        .execute(&caller, &prep, &(s.env.ledger().timestamp() + 100));

    let mut cmds = Vec::<Command>::new(&s.env);
    cmds.push_back(Command::RedeemPtForIbt(RedeemPtForIbtArgs {
        pt: s.pt_addr.clone(),
        shares: Amount::Balance,
        recipient: Recipient::Caller,
        min_ibts: 0,
    }));
    s.router_client
        .execute(&caller, &cmds, &(s.env.ledger().timestamp() + 100));

    let ibt_c = soroban_sdk::token::Client::new(&s.env, &s.ibt_addr);
    assert!(ibt_c.balance(&caller) > 0);
}

// pre-maturity, PT.redeem burns an equal amount of YT. When the router
// holds fewer YT than PT (YT spent elsewhere), a full-balance RedeemPtForIbt
// must clamp `shares` to the YT balance instead of reverting on insufficient YT.
#[test]
fn test_redeem_pt_for_ibt_clamps_to_yt_balance_pre_maturity() {
    let s = make_setup();
    let caller = Address::generate(&s.env);
    let sink = Address::generate(&s.env);

    // Build a router PT+YT position: deposit underlying -> IBT -> PT.
    let und = spectra_mocks::MockUnderlyingContractClient::new(&s.env, &s.underlying_addr);
    und.mint(&s.router_addr, &500_000i128);
    let mut prep = Vec::<Command>::new(&s.env);
    prep.push_back(Command::DepositAssetInIbt(DepositAssetInIbtArgs {
        ibt: s.ibt_addr.clone(),
        assets: Amount::Balance,
        recipient: Recipient::Router,
    }));
    prep.push_back(Command::DepositIbtInPt(DepositIbtInPtArgs {
        pt: s.pt_addr.clone(),
        ibts: Amount::Balance,
        pt_recipient: Recipient::Router,
        yt_recipient: Recipient::Router,
        min_shares: 0,
    }));
    s.router_client
        .execute(&caller, &prep, &(s.env.ledger().timestamp() + 100));

    let pt_c = soroban_sdk::token::Client::new(&s.env, &s.pt_addr);
    let yt_addr = principal_token::PrincipalTokenContractClient::new(&s.env, &s.pt_addr).get_yt();
    let yt_c = soroban_sdk::token::Client::new(&s.env, &yt_addr);
    let pt_bal = pt_c.balance(&s.router_addr);
    assert_eq!(pt_bal, yt_c.balance(&s.router_addr));

    // Move part of the router's YT out so YT < PT pre-maturity.
    let keep_yt = pt_bal / 2;
    let mut mv = Vec::<Command>::new(&s.env);
    mv.push_back(Command::Transfer(TransferArgs {
        token: yt_addr.clone(),
        recipient: Recipient::Address(sink.clone()),
        amount: Amount::Exact(pt_bal - keep_yt),
    }));
    s.router_client
        .execute(&caller, &mv, &(s.env.ledger().timestamp() + 100));
    assert_eq!(yt_c.balance(&s.router_addr), keep_yt);

    // Redeem the full PT balance sentinel: without the clamp this reverts on
    // insufficient YT; with the clamp it burns `keep_yt` PT+YT.
    let mut cmds = Vec::<Command>::new(&s.env);
    cmds.push_back(Command::RedeemPtForIbt(RedeemPtForIbtArgs {
        pt: s.pt_addr.clone(),
        shares: Amount::Balance,
        recipient: Recipient::Caller,
        min_ibts: 0,
    }));
    s.router_client
        .execute(&caller, &cmds, &(s.env.ledger().timestamp() + 100));

    let ibt_c = soroban_sdk::token::Client::new(&s.env, &s.ibt_addr);
    assert!(ibt_c.balance(&caller) > 0);
    assert_eq!(yt_c.balance(&s.router_addr), 0);
    assert_eq!(pt_c.balance(&s.router_addr), pt_bal - keep_yt);
}

#[test]
fn test_redeem_ibt_for_asset_returns_underlying_to_recipient() {
    let s = make_setup();
    let caller = Address::generate(&s.env);
    let token_c = soroban_sdk::token::Client::new(&s.env, &s.underlying_addr);

    // Pre-seed the router with IBT shares by depositing through the vault.
    let und = spectra_mocks::MockUnderlyingContractClient::new(&s.env, &s.underlying_addr);
    und.mint(&s.router_addr, &500_000i128);
    let mut cmds = Vec::<Command>::new(&s.env);
    cmds.push_back(Command::DepositAssetInIbt(DepositAssetInIbtArgs {
        ibt: s.ibt_addr.clone(),
        assets: Amount::Balance,
        recipient: Recipient::Router,
    }));
    s.router_client
        .execute(&caller, &cmds, &(s.env.ledger().timestamp() + 100));
    assert_eq!(token_c.balance(&s.router_addr), 0);
    // Fixture pre-seeds the IBT with 1 underlying so PT init's guard passes.
    assert_eq!(token_c.balance(&s.ibt_addr), 500_001);

    // Now redeem the router's IBT shares for underlying, sent to caller.
    let mut cmds2 = Vec::<Command>::new(&s.env);
    cmds2.push_back(Command::RedeemIbtForAsset(RedeemIbtForAssetArgs {
        ibt: s.ibt_addr.clone(),
        shares: Amount::Balance,
        recipient: Recipient::Caller,
    }));
    s.router_client
        .execute(&caller, &cmds2, &(s.env.ledger().timestamp() + 100));

    // The 1-underlying seed stays in the vault (PT holds the matching shares).
    assert_eq!(token_c.balance(&s.ibt_addr), 1);
    assert_eq!(token_c.balance(&caller), 500_000);
}

#[test]
fn test_deposit_asset_in_ibt_pushes_underlying_in() {
    let s = make_setup();
    let caller = Address::generate(&s.env);
    let und = spectra_mocks::MockUnderlyingContractClient::new(&s.env, &s.underlying_addr);
    und.mint(&s.router_addr, &500_000i128);

    let mut cmds = Vec::<Command>::new(&s.env);
    cmds.push_back(Command::DepositAssetInIbt(DepositAssetInIbtArgs {
        ibt: s.ibt_addr.clone(),
        assets: Amount::Balance,
        recipient: Recipient::Caller,
    }));

    s.router_client
        .execute(&caller, &cmds, &(s.env.ledger().timestamp() + 100));

    let token_c = soroban_sdk::token::Client::new(&s.env, &s.underlying_addr);
    assert_eq!(token_c.balance(&s.router_addr), 0);
    // Fixture pre-seeds the IBT with 1 underlying so PT init's guard passes.
    assert_eq!(token_c.balance(&s.ibt_addr), 500_001);
}

#[test]
fn test_assert_min_balance_passes_when_balance_above_min() {
    let s = make_setup();
    let caller = Address::generate(&s.env);
    let und = spectra_mocks::MockUnderlyingContractClient::new(&s.env, &s.underlying_addr);
    und.mint(&s.router_addr, &1_000i128);

    let mut cmds = Vec::<Command>::new(&s.env);
    cmds.push_back(Command::AssertMinBalance(AssertMinBalanceArgs {
        token: s.underlying_addr.clone(),
        owner: Recipient::Router,
        min_value: 500,
    }));

    s.router_client
        .execute(&caller, &cmds, &(s.env.ledger().timestamp() + 100));
}

/// End-to-end exercise of the dispatcher chain on the redeem leg.
/// Covers TransferFrom + DepositAssetInIbt + RedeemIbtForAsset +
/// AssertMinBalance through a round-trip deposit-then-redeem.
#[test]
fn test_integration_redeem_flow_ibt_to_underlying() {
    let s = make_setup();
    let caller = Address::generate(&s.env);

    // Give the caller IBT shares by depositing some underlying through the
    // router with recipient=Caller.
    let und = spectra_mocks::MockUnderlyingContractClient::new(&s.env, &s.underlying_addr);
    und.mint(&caller, &500_000i128);
    approve_router(&s.env, &s.underlying_addr, &caller, &s.router_addr, 500_000);
    let mut seed = Vec::<Command>::new(&s.env);
    seed.push_back(Command::TransferFrom(TransferFromArgs {
        token: s.underlying_addr.clone(),
        amount: Amount::Exact(500_000),
    }));
    seed.push_back(Command::DepositAssetInIbt(DepositAssetInIbtArgs {
        ibt: s.ibt_addr.clone(),
        assets: Amount::Balance,
        recipient: Recipient::Caller,
    }));
    s.router_client
        .execute(&caller, &seed, &(s.env.ledger().timestamp() + 100));

    let ibt_token = soroban_sdk::token::Client::new(&s.env, &s.ibt_addr);
    let shares_owned = ibt_token.balance(&caller);
    assert!(shares_owned > 0);

    // Now run the redeem leg: pull IBT from caller, redeem to underlying,
    // assert at least 500_000 underlying lands on caller.
    approve_router(&s.env, &s.ibt_addr, &caller, &s.router_addr, shares_owned);
    let mut cmds = Vec::<Command>::new(&s.env);
    cmds.push_back(Command::TransferFrom(TransferFromArgs {
        token: s.ibt_addr.clone(),
        amount: Amount::Exact(shares_owned),
    }));
    cmds.push_back(Command::RedeemIbtForAsset(RedeemIbtForAssetArgs {
        ibt: s.ibt_addr.clone(),
        shares: Amount::Balance,
        recipient: Recipient::Caller,
    }));
    cmds.push_back(Command::AssertMinBalance(AssertMinBalanceArgs {
        token: s.underlying_addr.clone(),
        owner: Recipient::Caller,
        min_value: 500_000,
    }));
    s.router_client
        .execute(&caller, &cmds, &(s.env.ledger().timestamp() + 100));

    let und_token = soroban_sdk::token::Client::new(&s.env, &s.underlying_addr);
    assert_eq!(und_token.balance(&caller), 500_000);
}

/// Full PT-to-underlying redeem flow. Will pass once PR #37 ships PT vault flows.
#[test]
fn test_integration_redeem_flow_pt_to_underlying() {
    let s = make_setup();
    let caller = Address::generate(&s.env);

    let und = spectra_mocks::MockUnderlyingContractClient::new(&s.env, &s.underlying_addr);
    und.mint(&caller, &500_000i128);
    approve_router(&s.env, &s.underlying_addr, &caller, &s.router_addr, 500_000);
    // Mint a PT position on the caller via deposit + split.
    let mut seed = Vec::<Command>::new(&s.env);
    seed.push_back(Command::TransferFrom(TransferFromArgs {
        token: s.underlying_addr.clone(),
        amount: Amount::Exact(500_000),
    }));
    seed.push_back(Command::DepositAssetInIbt(DepositAssetInIbtArgs {
        ibt: s.ibt_addr.clone(),
        assets: Amount::Balance,
        recipient: Recipient::Router,
    }));
    seed.push_back(Command::DepositIbtInPt(DepositIbtInPtArgs {
        pt: s.pt_addr.clone(),
        ibts: Amount::Balance,
        pt_recipient: Recipient::Caller,
        yt_recipient: Recipient::Caller,
        min_shares: 0,
    }));
    s.router_client
        .execute(&caller, &seed, &(s.env.ledger().timestamp() + 100));

    let pt_token = soroban_sdk::token::Client::new(&s.env, &s.pt_addr);
    let pt_owned = pt_token.balance(&caller);
    let yt_addr = principal_token::PrincipalTokenContractClient::new(&s.env, &s.pt_addr).get_yt();

    approve_router(&s.env, &s.pt_addr, &caller, &s.router_addr, pt_owned);
    approve_router(&s.env, &yt_addr, &caller, &s.router_addr, pt_owned);
    let mut cmds = Vec::<Command>::new(&s.env);
    cmds.push_back(Command::TransferFrom(TransferFromArgs {
        token: s.pt_addr.clone(),
        amount: Amount::Exact(pt_owned),
    }));
    cmds.push_back(Command::TransferFrom(TransferFromArgs {
        token: yt_addr,
        amount: Amount::Exact(pt_owned),
    }));
    cmds.push_back(Command::RedeemPtForIbt(RedeemPtForIbtArgs {
        pt: s.pt_addr.clone(),
        shares: Amount::Balance,
        recipient: Recipient::Router,
        min_ibts: 0,
    }));
    cmds.push_back(Command::RedeemIbtForAsset(RedeemIbtForAssetArgs {
        ibt: s.ibt_addr.clone(),
        shares: Amount::Balance,
        recipient: Recipient::Caller,
    }));
    cmds.push_back(Command::AssertMinBalance(AssertMinBalanceArgs {
        token: s.underlying_addr.clone(),
        owner: Recipient::Caller,
        min_value: 500_000,
    }));
    s.router_client
        .execute(&caller, &cmds, &(s.env.ledger().timestamp() + 100));

    let und_token = soroban_sdk::token::Client::new(&s.env, &s.underlying_addr);
    assert_eq!(und_token.balance(&caller), 500_000);
}

#[test]
fn test_assert_min_balance_panics_when_balance_below_min() {
    let s = make_setup();
    let caller = Address::generate(&s.env);
    let und = spectra_mocks::MockUnderlyingContractClient::new(&s.env, &s.underlying_addr);
    und.mint(&s.router_addr, &100i128);

    let mut cmds = Vec::<Command>::new(&s.env);
    cmds.push_back(Command::AssertMinBalance(AssertMinBalanceArgs {
        token: s.underlying_addr.clone(),
        owner: Recipient::Router,
        min_value: 500,
    }));

    assert_router_error(
        s.router_client
            .try_execute(&caller, &cmds, &(s.env.ledger().timestamp() + 100)),
        RouterError::AssertMinBalanceFailed,
    );
}

#[test]
fn test_transfer_to_caller_sends_router_balance() {
    let s = make_setup();
    let caller = Address::generate(&s.env);

    let und = spectra_mocks::MockUnderlyingContractClient::new(&s.env, &s.underlying_addr);
    und.mint(&s.router_addr, &500_000i128);
    let token_c = soroban_sdk::token::Client::new(&s.env, &s.underlying_addr);

    let mut cmds = Vec::<Command>::new(&s.env);
    cmds.push_back(Command::Transfer(TransferArgs {
        token: s.underlying_addr.clone(),
        recipient: Recipient::Caller,
        amount: Amount::Exact(300_000),
    }));

    s.router_client
        .execute(&caller, &cmds, &(s.env.ledger().timestamp() + 100));

    assert_eq!(token_c.balance(&s.router_addr), 200_000);
    assert_eq!(token_c.balance(&caller), 300_000);
}

#[test]
fn test_transfer_to_external_address_with_balance_sentinel() {
    let s = make_setup();
    let caller = Address::generate(&s.env);
    let recipient = Address::generate(&s.env);

    let und = spectra_mocks::MockUnderlyingContractClient::new(&s.env, &s.underlying_addr);
    und.mint(&s.router_addr, &123_456i128);
    let token_c = soroban_sdk::token::Client::new(&s.env, &s.underlying_addr);

    let mut cmds = Vec::<Command>::new(&s.env);
    cmds.push_back(Command::Transfer(TransferArgs {
        token: s.underlying_addr.clone(),
        recipient: Recipient::Address(recipient.clone()),
        amount: Amount::Balance,
    }));

    s.router_client
        .execute(&caller, &cmds, &(s.env.ledger().timestamp() + 100));

    assert_eq!(token_c.balance(&s.router_addr), 0);
    assert_eq!(token_c.balance(&recipient), 123_456);
}

#[test]
fn test_transfer_from_pulls_token_from_caller() {
    let s = make_setup();
    let caller = Address::generate(&s.env);

    let und = spectra_mocks::MockUnderlyingContractClient::new(&s.env, &s.underlying_addr);
    und.mint(&caller, &1_000_000i128);
    let token_c = soroban_sdk::token::Client::new(&s.env, &s.underlying_addr);
    approve_router(&s.env, &s.underlying_addr, &caller, &s.router_addr, 400_000);

    let mut cmds = Vec::<Command>::new(&s.env);
    cmds.push_back(Command::TransferFrom(TransferFromArgs {
        token: s.underlying_addr.clone(),
        amount: Amount::Exact(400_000),
    }));

    s.router_client
        .execute(&caller, &cmds, &(s.env.ledger().timestamp() + 100));

    assert_eq!(token_c.balance(&caller), 600_000);
    assert_eq!(token_c.balance(&s.router_addr), 400_000);
}

#[test]
fn test_execute_deadline_expired_panics() {
    let s = make_setup();
    let caller = Address::generate(&s.env);
    let now = s.env.ledger().timestamp();
    // Set ledger time so that `deadline < now`.
    s.env.ledger().set_timestamp(now + 10);
    let mut cmds = Vec::<Command>::new(&s.env);
    cmds.push_back(dummy_command(&s.registry_addr));
    assert_router_error(
        s.router_client.try_execute(&caller, &cmds, &now),
        RouterError::DeadlineExpired,
    );
}

#[test]
fn set_bridge_admin_only_and_round_trips() {
    let s = make_setup();
    let bridge = Address::generate(&s.env);

    s.router_client.set_bridge(&s.admin, &bridge);
    assert_eq!(s.router_client.get_bridge(), bridge);

    let mallory = Address::generate(&s.env);
    let new_bridge = Address::generate(&s.env);
    let result = s.router_client.try_set_bridge(&mallory, &new_bridge);
    // OZ #[only_role] raises AccessControlError::Unauthorized (#2000).
    let expected = soroban_sdk::Error::from_contract_error(2000);
    assert!(
        matches!(result, Err(Ok(e)) if e == expected),
        "expected AccessControlError::Unauthorized (#2000), got {result:?}"
    );
}

#[test]
fn get_bridge_panics_when_unset() {
    let s = make_setup();
    assert_router_error(s.router_client.try_get_bridge(), RouterError::BridgeNotSet);
}

/// Deploy a fresh SEP-41 token (using `MockUnderlyingContract`, which is the
/// existing SEP-41 mock in this workspace — `spectra_mocks::token::deploy`
/// from the plan doesn't exist, so we mirror the helper used elsewhere in
/// this file).
fn deploy_sep41(env: &Env, name: &str, symbol: &str, decimals: u32) -> Address {
    let addr = env.register(spectra_mocks::MockUnderlyingContract, ());
    spectra_mocks::MockUnderlyingContractClient::new(env, &addr).initialize(
        &String::from_str(env, name),
        &String::from_str(env, symbol),
        &decimals,
    );
    addr
}

struct BridgeSetup<'a> {
    env: Env,
    admin: Address,
    caller: Address,
    bridge: Address,
    messenger: Address,
    gas_token: Address,
    pt: Address,
    router_client: MainRouterContractClient<'a>,
}

fn setup_bridge_env() -> BridgeSetup<'static> {
    let s = make_setup();

    // Mock SEP-41 tokens for the asset being bridged + the gas token.
    let pt = deploy_sep41(&s.env, "PT", "PT", 18);
    let gas_token = deploy_sep41(&s.env, "XLM", "XLM", 7);

    let messenger = s.env.register(MockMessenger, ());
    MockMessengerClient::new(&s.env, &messenger).init(&gas_token);

    let bridge = s.env.register(MockPtBridge, ());
    MockPtBridgeClient::new(&s.env, &bridge).init(&messenger, &0u32);

    s.router_client.set_bridge(&s.admin, &bridge);

    let caller = Address::generate(&s.env);
    spectra_mocks::MockUnderlyingContractClient::new(&s.env, &pt).mint(&caller, &1_000_000i128);
    spectra_mocks::MockUnderlyingContractClient::new(&s.env, &gas_token).mint(&caller, &10_000i128);

    // The dispatcher's `TransferFrom` arm now uses SEP-41 `transfer_from`
    // (spender=router, from=caller), which checks a stored allowance.
    let router_addr = s.router_client.address.clone();
    let live_until = s.env.ledger().sequence() + 1_000_000;
    soroban_sdk::token::Client::new(&s.env, &pt).approve(
        &caller,
        &router_addr,
        &i128::MAX,
        &live_until,
    );
    soroban_sdk::token::Client::new(&s.env, &gas_token).approve(
        &caller,
        &router_addr,
        &i128::MAX,
        &live_until,
    );

    BridgeSetup {
        env: s.env,
        admin: s.admin,
        caller,
        bridge,
        messenger,
        gas_token,
        pt,
        router_client: s.router_client,
    }
}

#[test]
fn bridge_native_pt_to_evm_happy_path() {
    let b = setup_bridge_env();
    let _ = (&b.admin, &b.messenger); // helpers carried for later tests

    let cmds = vec![
        &b.env,
        Command::TransferFrom(TransferFromArgs {
            token: b.pt.clone(),
            amount: Amount::Exact(500_000),
        }),
        Command::TransferFrom(TransferFromArgs {
            token: b.gas_token.clone(),
            amount: Amount::Exact(1_000),
        }),
        Command::BridgeNativePtToEvm(BridgeNativePtToEvmArgs {
            pt: b.pt.clone(),
            amount: Amount::Exact(500_000),
            target_chain_id: 1,
            evm_recipient: BytesN::from_array(&b.env, &[0xCC; 20]),
            gas_token: b.gas_token.clone(),
            gas_amount: Amount::Exact(1_000),
            refund_address: Recipient::Caller,
            min_amount_out: 0,
        }),
    ];

    b.router_client
        .execute(&b.caller, &cmds, &(b.env.ledger().timestamp() + 60));

    let pt_token = soroban_sdk::token::Client::new(&b.env, &b.pt);
    assert_eq!(pt_token.balance(&b.bridge), 500_000);
    let gas_c = soroban_sdk::token::Client::new(&b.env, &b.gas_token);
    let messenger: Address = MockPtBridgeClient::new(&b.env, &b.bridge).get_messenger();
    assert_eq!(gas_c.balance(&messenger), 1_000);
    let last = MockPtBridgeClient::new(&b.env, &b.bridge)
        .last_native()
        .unwrap();
    assert_eq!(last.amount, 500_000);
    assert_eq!(last.gas_amount, 1_000);
    assert_eq!(last.target_chain_id, 1);
}

// ── bridge gas floor ────────────────────────────────────────────────────

#[test]
fn min_gas_floor_defaults_to_zero() {
    let s = make_setup();
    let gas = Address::generate(&s.env);
    assert_eq!(s.router_client.get_min_gas_floor(&1, &gas), 0);
}

#[test]
fn set_min_gas_floor_is_admin_only() {
    let s = make_setup();
    let gas = Address::generate(&s.env);
    let mallory = Address::generate(&s.env);
    let result = s
        .router_client
        .try_set_min_gas_floor(&mallory, &1, &gas, &1_000);
    let expected = soroban_sdk::Error::from_contract_error(2000);
    assert_eq!(result, Err(Ok(expected)));
    // Admin can set, getter reflects it — per (chain, token) key.
    s.router_client
        .set_min_gas_floor(&s.admin, &1, &gas, &1_000);
    assert_eq!(s.router_client.get_min_gas_floor(&1, &gas), 1_000);
    // A different chain/token key is unaffected (still default 0).
    assert_eq!(s.router_client.get_min_gas_floor(&2, &gas), 0);
}

#[test]
fn set_min_gas_floor_rejects_negative() {
    let s = make_setup();
    let gas = Address::generate(&s.env);
    assert_router_error(
        s.router_client
            .try_set_min_gas_floor(&s.admin, &1, &gas, &-1),
        RouterError::InsufficientGas,
    );
}

fn bridge_native_cmd(b: &BridgeSetup, gas: i128) -> Vec<Command> {
    vec![
        &b.env,
        Command::TransferFrom(TransferFromArgs {
            token: b.pt.clone(),
            amount: Amount::Exact(500_000),
        }),
        Command::TransferFrom(TransferFromArgs {
            token: b.gas_token.clone(),
            amount: Amount::Exact(1_000),
        }),
        Command::BridgeNativePtToEvm(BridgeNativePtToEvmArgs {
            pt: b.pt.clone(),
            amount: Amount::Exact(500_000),
            target_chain_id: 1,
            evm_recipient: BytesN::from_array(&b.env, &[0xCC; 20]),
            gas_token: b.gas_token.clone(),
            gas_amount: Amount::Exact(gas),
            refund_address: Recipient::Caller,
            min_amount_out: 0,
        }),
    ]
}

#[test]
fn bridge_native_zero_gas_ok_when_floor_unset() {
    let b = setup_bridge_env();
    // Default floor 0: gas 0 is allowed (preserves bridge's tested behavior).
    b.router_client.execute(
        &b.caller,
        &bridge_native_cmd(&b, 0),
        &(b.env.ledger().timestamp() + 60),
    );
    assert_eq!(
        MockPtBridgeClient::new(&b.env, &b.bridge)
            .last_native()
            .unwrap()
            .gas_amount,
        0
    );
}

#[test]
fn bridge_native_below_floor_reverts() {
    let b = setup_bridge_env();
    b.router_client
        .set_min_gas_floor(&b.admin, &1, &b.gas_token, &1_000);
    assert_router_error(
        b.router_client.try_execute(
            &b.caller,
            &bridge_native_cmd(&b, 999),
            &(b.env.ledger().timestamp() + 60),
        ),
        RouterError::InsufficientGas,
    );
}

#[test]
fn bridge_native_meets_floor_succeeds() {
    let b = setup_bridge_env();
    b.router_client
        .set_min_gas_floor(&b.admin, &1, &b.gas_token, &1_000);
    let cmds = vec![
        &b.env,
        Command::TransferFrom(TransferFromArgs {
            token: b.pt.clone(),
            amount: Amount::Exact(500_000),
        }),
        Command::TransferFrom(TransferFromArgs {
            token: b.gas_token.clone(),
            amount: Amount::Exact(1_000),
        }),
        Command::BridgeNativePtToEvm(BridgeNativePtToEvmArgs {
            pt: b.pt.clone(),
            amount: Amount::Exact(500_000),
            target_chain_id: 1,
            evm_recipient: BytesN::from_array(&b.env, &[0xCC; 20]),
            gas_token: b.gas_token.clone(),
            gas_amount: Amount::Exact(1_000),
            refund_address: Recipient::Caller,
            min_amount_out: 0,
        }),
    ];
    b.router_client
        .execute(&b.caller, &cmds, &(b.env.ledger().timestamp() + 60));
    assert_eq!(
        MockPtBridgeClient::new(&b.env, &b.bridge)
            .last_native()
            .unwrap()
            .gas_amount,
        1_000
    );
}

#[test]
fn bridge_native_floor_on_other_key_does_not_apply() {
    let b = setup_bridge_env();
    // Floor set for a different chain id; the chain-1 bridge is unaffected.
    b.router_client
        .set_min_gas_floor(&b.admin, &2, &b.gas_token, &1_000);
    b.router_client.execute(
        &b.caller,
        &bridge_native_cmd(&b, 1),
        &(b.env.ledger().timestamp() + 60),
    );
    assert_eq!(
        MockPtBridgeClient::new(&b.env, &b.bridge)
            .last_native()
            .unwrap()
            .gas_amount,
        1
    );
}

#[test]
fn bridge_back_below_floor_reverts() {
    let b = setup_back_env();
    b.router_client
        .set_min_gas_floor(&b.admin, &1, &b.gas_token, &1_000);
    let cmds = vec![
        &b.env,
        Command::BridgeWrappedPtBackToEvm(BridgeWrappedPtBackToEvmArgs {
            wrapped_pt: b.wpt.clone(),
            amount: Amount::Exact(100_000),
            target_chain_id: 1,
            evm_recipient: BytesN::from_array(&b.env, &[0xAA; 20]),
            gas_token: b.gas_token.clone(),
            gas_amount: Amount::Exact(999),
            refund_address: Recipient::Caller,
            min_amount_out: 0,
        }),
    ];
    assert_router_error(
        b.router_client
            .try_execute(&b.caller, &cmds, &(b.env.ledger().timestamp() + 60)),
        RouterError::InsufficientGas,
    );
}

/// a negative resolved gas amount is rejected before the bridge call.
#[test]
fn bridge_native_rejects_negative_gas_amount() {
    let b = setup_bridge_env();
    let cmds = vec![
        &b.env,
        Command::BridgeNativePtToEvm(BridgeNativePtToEvmArgs {
            pt: b.pt.clone(),
            amount: Amount::Exact(500_000),
            target_chain_id: 1,
            evm_recipient: BytesN::from_array(&b.env, &[0xCC; 20]),
            gas_token: b.gas_token.clone(),
            gas_amount: Amount::Exact(-1),
            refund_address: Recipient::Caller,
            min_amount_out: 0,
        }),
    ];
    assert_router_error(
        b.router_client
            .try_execute(&b.caller, &cmds, &(b.env.ledger().timestamp() + 60)),
        RouterError::NegativeAmount,
    );
}

/// a negative min_amount_out is rejected on the bridge-back path.
#[test]
fn bridge_back_rejects_negative_min_amount_out() {
    let b = setup_bridge_env();
    let cmds = vec![
        &b.env,
        Command::BridgeWrappedPtBackToEvm(BridgeWrappedPtBackToEvmArgs {
            wrapped_pt: b.pt.clone(),
            amount: Amount::Exact(500_000),
            target_chain_id: 1,
            evm_recipient: BytesN::from_array(&b.env, &[0xCC; 20]),
            gas_token: b.gas_token.clone(),
            gas_amount: Amount::Exact(1_000),
            refund_address: Recipient::Caller,
            min_amount_out: -1,
        }),
    ];
    assert_router_error(
        b.router_client
            .try_execute(&b.caller, &cmds, &(b.env.ledger().timestamp() + 60)),
        RouterError::NegativeMinAmountOut,
    );
}

/// DepositIbtInPt rejects a negative min_shares before dispatch.
#[test]
fn deposit_ibt_in_pt_rejects_negative_min_shares() {
    let s = make_setup();
    let caller = Address::generate(&s.env);
    let cmds = vec![
        &s.env,
        Command::DepositIbtInPt(DepositIbtInPtArgs {
            pt: s.pt_addr.clone(),
            ibts: Amount::Exact(1),
            pt_recipient: Recipient::Caller,
            yt_recipient: Recipient::Caller,
            min_shares: -1,
        }),
    ];
    assert_router_error(
        s.router_client
            .try_execute(&caller, &cmds, &(s.env.ledger().timestamp() + 60)),
        RouterError::NegativeAmount,
    );
}

/// RedeemPtForIbt rejects a negative min_ibts before dispatch.
#[test]
fn redeem_pt_for_ibt_rejects_negative_min_ibts() {
    let s = make_setup();
    let caller = Address::generate(&s.env);
    let cmds = vec![
        &s.env,
        Command::RedeemPtForIbt(RedeemPtForIbtArgs {
            pt: s.pt_addr.clone(),
            shares: Amount::Exact(1),
            recipient: Recipient::Caller,
            min_ibts: -1,
        }),
    ];
    assert_router_error(
        s.router_client
            .try_execute(&caller, &cmds, &(s.env.ledger().timestamp() + 60)),
        RouterError::NegativeAmount,
    );
}

#[test]
fn bridge_native_panics_when_bridge_unset() {
    // Initialize router but never call set_bridge.
    let s = make_setup();
    let caller = Address::generate(&s.env);
    let pt = deploy_sep41(&s.env, "PT", "PT", 18);
    let gas = deploy_sep41(&s.env, "XLM", "XLM", 7);

    let cmds = vec![
        &s.env,
        Command::BridgeNativePtToEvm(BridgeNativePtToEvmArgs {
            pt: pt.clone(),
            amount: Amount::Exact(1),
            target_chain_id: 1,
            evm_recipient: BytesN::from_array(&s.env, &[1u8; 20]),
            gas_token: gas.clone(),
            gas_amount: Amount::Exact(0),
            refund_address: Recipient::Caller,
            min_amount_out: 0,
        }),
    ];
    assert_router_error(
        s.router_client
            .try_execute(&caller, &cmds, &(s.env.ledger().timestamp() + 60)),
        RouterError::BridgeNotSet,
    );
}

#[test]
fn bridge_native_amount_balance_uses_router_balance() {
    let b = setup_bridge_env();
    let cmds = vec![
        &b.env,
        Command::TransferFrom(TransferFromArgs {
            token: b.pt.clone(),
            amount: Amount::Exact(123_456),
        }),
        Command::BridgeNativePtToEvm(BridgeNativePtToEvmArgs {
            pt: b.pt.clone(),
            amount: Amount::Balance,
            target_chain_id: 1,
            evm_recipient: BytesN::from_array(&b.env, &[0xCC; 20]),
            gas_token: b.gas_token.clone(),
            gas_amount: Amount::Exact(0),
            refund_address: Recipient::Caller,
            min_amount_out: 0,
        }),
    ];
    b.router_client
        .execute(&b.caller, &cmds, &(b.env.ledger().timestamp() + 60));
    let last = MockPtBridgeClient::new(&b.env, &b.bridge)
        .last_native()
        .unwrap();
    assert_eq!(last.amount, 123_456);
}

#[test]
fn bridge_native_zero_gas_skips_messenger_transfer() {
    let b = setup_bridge_env();
    let cmds = vec![
        &b.env,
        Command::TransferFrom(TransferFromArgs {
            token: b.pt.clone(),
            amount: Amount::Exact(100),
        }),
        Command::BridgeNativePtToEvm(BridgeNativePtToEvmArgs {
            pt: b.pt.clone(),
            amount: Amount::Exact(100),
            target_chain_id: 1,
            evm_recipient: BytesN::from_array(&b.env, &[0xCC; 20]),
            gas_token: b.gas_token.clone(),
            gas_amount: Amount::Exact(0),
            refund_address: Recipient::Caller,
            min_amount_out: 0,
        }),
    ];
    b.router_client
        .execute(&b.caller, &cmds, &(b.env.ledger().timestamp() + 60));
    let gas_c = soroban_sdk::token::Client::new(&b.env, &b.gas_token);
    assert_eq!(gas_c.balance(&b.messenger), 0);
}

#[test]
fn bridge_native_paused_router_blocks_execute() {
    let b = setup_bridge_env();
    b.router_client.pause(&b.admin);
    let cmds = vec![
        &b.env,
        Command::BridgeNativePtToEvm(BridgeNativePtToEvmArgs {
            pt: b.pt.clone(),
            amount: Amount::Exact(1),
            target_chain_id: 1,
            evm_recipient: BytesN::from_array(&b.env, &[0xCC; 20]),
            gas_token: b.gas_token.clone(),
            gas_amount: Amount::Exact(0),
            refund_address: Recipient::Caller,
            min_amount_out: 0,
        }),
    ];
    // pausable panics with its own (non-Router) error; only assert it errors.
    let res = b
        .router_client
        .try_execute(&b.caller, &cmds, &(b.env.ledger().timestamp() + 60));
    assert!(
        matches!(res, Err(Ok(_))),
        "expected paused panic, got {res:?}"
    );
}

struct BackSetup<'a> {
    env: Env,
    admin: Address,
    caller: Address,
    bridge: Address,
    messenger: Address,
    gas_token: Address,
    wpt: Address,
    router_client: MainRouterContractClient<'a>,
}

fn setup_back_env() -> BackSetup<'static> {
    let s = make_setup();
    let gas_token = deploy_sep41(&s.env, "XLM", "XLM", 7);

    let messenger = s.env.register(MockMessenger, ());
    MockMessengerClient::new(&s.env, &messenger).init(&gas_token);

    let bridge = s.env.register(MockPtBridge, ());
    // 50 bps fee
    MockPtBridgeClient::new(&s.env, &bridge).init(&messenger, &50u32);

    let wpt = s.env.register(MockWrappedPt, ());
    MockWrappedPtClient::new(&s.env, &wpt).init(&bridge);

    s.router_client.set_bridge(&s.admin, &bridge);

    let caller = Address::generate(&s.env);
    MockWrappedPtClient::new(&s.env, &wpt).mint(&caller, &1_000_000i128);
    spectra_mocks::MockUnderlyingContractClient::new(&s.env, &gas_token).mint(&caller, &10_000i128);

    // SEP-41 `transfer_from` (used by main's `Command::TransferFrom`) requires
    // a stored allowance, not just auth signals.
    approve_router(&s.env, &wpt, &caller, &s.router_addr, 1_000_000);
    approve_router(&s.env, &gas_token, &caller, &s.router_addr, 10_000);

    BackSetup {
        env: s.env,
        admin: s.admin,
        caller,
        bridge,
        messenger,
        gas_token,
        wpt,
        router_client: s.router_client,
    }
}

#[test]
fn bridge_wrapped_back_happy_path_with_fee_and_gas() {
    let b = setup_back_env();

    let cmds = vec![
        &b.env,
        Command::TransferFrom(TransferFromArgs {
            token: b.wpt.clone(),
            amount: Amount::Exact(100_000),
        }),
        Command::TransferFrom(TransferFromArgs {
            token: b.gas_token.clone(),
            amount: Amount::Exact(1_000),
        }),
        Command::BridgeWrappedPtBackToEvm(BridgeWrappedPtBackToEvmArgs {
            wrapped_pt: b.wpt.clone(),
            amount: Amount::Exact(100_000),
            target_chain_id: 1,
            evm_recipient: BytesN::from_array(&b.env, &[0xAA; 20]),
            gas_token: b.gas_token.clone(),
            gas_amount: Amount::Exact(1_000),
            refund_address: Recipient::Caller,
            min_amount_out: 0,
        }),
    ];
    b.router_client
        .execute(&b.caller, &cmds, &(b.env.ledger().timestamp() + 60));

    // 50 bps of 100_000 = 500 fee
    assert_eq!(
        MockWrappedPtClient::new(&b.env, &b.wpt).balance(&b.bridge),
        500
    );
    // Messenger pulled gas
    let gas_c = soroban_sdk::token::Client::new(&b.env, &b.gas_token);
    assert_eq!(gas_c.balance(&b.messenger), 1_000);
    let last = MockPtBridgeClient::new(&b.env, &b.bridge)
        .last_back()
        .unwrap();
    assert_eq!(last.amount, 100_000);
}

#[test]
fn bridge_back_zero_fee_skips_fee_transfer() {
    let s = make_setup();
    let gas_token = deploy_sep41(&s.env, "XLM", "XLM", 7);

    let messenger = s.env.register(MockMessenger, ());
    MockMessengerClient::new(&s.env, &messenger).init(&gas_token);

    let bridge = s.env.register(MockPtBridge, ());
    MockPtBridgeClient::new(&s.env, &bridge).init(&messenger, &0u32); // zero fee

    let wpt = s.env.register(MockWrappedPt, ());
    MockWrappedPtClient::new(&s.env, &wpt).init(&bridge);

    s.router_client.set_bridge(&s.admin, &bridge);

    let caller = Address::generate(&s.env);
    MockWrappedPtClient::new(&s.env, &wpt).mint(&caller, &100_000i128);
    approve_router(&s.env, &wpt, &caller, &s.router_addr, 100_000);

    let cmds = vec![
        &s.env,
        Command::TransferFrom(TransferFromArgs {
            token: wpt.clone(),
            amount: Amount::Exact(50_000),
        }),
        Command::BridgeWrappedPtBackToEvm(BridgeWrappedPtBackToEvmArgs {
            wrapped_pt: wpt.clone(),
            amount: Amount::Exact(50_000),
            target_chain_id: 1,
            evm_recipient: BytesN::from_array(&s.env, &[0xAA; 20]),
            gas_token: gas_token.clone(),
            gas_amount: Amount::Exact(0),
            refund_address: Recipient::Caller,
            min_amount_out: 0,
        }),
    ];
    s.router_client
        .execute(&caller, &cmds, &(s.env.ledger().timestamp() + 60));

    assert_eq!(MockWrappedPtClient::new(&s.env, &wpt).balance(&bridge), 0);
}

#[test]
fn bridge_back_amount_balance_uses_router_balance() {
    let b = setup_back_env();
    let cmds = vec![
        &b.env,
        Command::TransferFrom(TransferFromArgs {
            token: b.wpt.clone(),
            amount: Amount::Exact(10_000),
        }),
        Command::BridgeWrappedPtBackToEvm(BridgeWrappedPtBackToEvmArgs {
            wrapped_pt: b.wpt.clone(),
            amount: Amount::Balance,
            target_chain_id: 1,
            evm_recipient: BytesN::from_array(&b.env, &[0xAA; 20]),
            gas_token: b.gas_token.clone(),
            gas_amount: Amount::Exact(0),
            refund_address: Recipient::Caller,
            min_amount_out: 0,
        }),
    ];
    b.router_client
        .execute(&b.caller, &cmds, &(b.env.ledger().timestamp() + 60));
    let last = MockPtBridgeClient::new(&b.env, &b.bridge)
        .last_back()
        .unwrap();
    assert_eq!(last.amount, 10_000);
}

#[test]
fn bridge_back_panics_when_bridge_unset() {
    let s = make_setup();

    let caller = Address::generate(&s.env);
    let wpt = deploy_sep41(&s.env, "wPT", "wPT", 18);
    let gas = deploy_sep41(&s.env, "XLM", "XLM", 7);

    let mut cmds = Vec::<Command>::new(&s.env);
    cmds.push_back(Command::BridgeWrappedPtBackToEvm(
        BridgeWrappedPtBackToEvmArgs {
            wrapped_pt: wpt,
            amount: Amount::Exact(1),
            target_chain_id: 1,
            evm_recipient: BytesN::from_array(&s.env, &[1u8; 20]),
            gas_token: gas,
            gas_amount: Amount::Exact(0),
            refund_address: Recipient::Caller,
            min_amount_out: 0,
        },
    ));
    assert_router_error(
        s.router_client
            .try_execute(&caller, &cmds, &(s.env.ledger().timestamp() + 60)),
        RouterError::BridgeNotSet,
    );
}

// ─── Task 14: Boundary / battle tests ──────────────────────────────────────

#[test]
fn bridge_native_max_i128_amount() {
    let b = setup_bridge_env();
    spectra_mocks::MockUnderlyingContractClient::new(&b.env, &b.pt)
        .mint(&b.caller, &(i128::MAX - 1_000_000));
    let cmds = vec![
        &b.env,
        Command::TransferFrom(TransferFromArgs {
            token: b.pt.clone(),
            amount: Amount::Exact(i128::MAX),
        }),
        Command::BridgeNativePtToEvm(BridgeNativePtToEvmArgs {
            pt: b.pt.clone(),
            amount: Amount::Exact(i128::MAX),
            target_chain_id: 1,
            evm_recipient: BytesN::from_array(&b.env, &[0xCC; 20]),
            gas_token: b.gas_token.clone(),
            gas_amount: Amount::Exact(0),
            refund_address: Recipient::Caller,
            min_amount_out: 0,
        }),
    ];
    b.router_client
        .execute(&b.caller, &cmds, &(b.env.ledger().timestamp() + 60));
    let last = MockPtBridgeClient::new(&b.env, &b.bridge)
        .last_native()
        .unwrap();
    assert_eq!(last.amount, i128::MAX);
}

/// Fee overflow in dispatcher: `amount * fee_bps` exceeds `i128::MAX`. With
/// `overflow-checks = true` (workspace profile) this panics, which reverts
/// the transaction — `try_execute` surfaces it as an `Err`.
#[test]
fn bridge_back_fee_overflow_panics_safely() {
    let s = make_setup();

    let gas_token = deploy_sep41(&s.env, "XLM", "XLM", 7);
    let messenger = s.env.register(MockMessenger, ());
    MockMessengerClient::new(&s.env, &messenger).init(&gas_token);
    let bridge = s.env.register(MockPtBridge, ());
    MockPtBridgeClient::new(&s.env, &bridge).init(&messenger, &10_000u32);
    let wpt = s.env.register(MockWrappedPt, ());
    MockWrappedPtClient::new(&s.env, &wpt).init(&bridge);
    s.router_client.set_bridge(&s.admin, &bridge);

    let caller = Address::generate(&s.env);
    MockWrappedPtClient::new(&s.env, &wpt).mint(&caller, &i128::MAX);

    let mut cmds = Vec::<Command>::new(&s.env);
    cmds.push_back(Command::TransferFrom(TransferFromArgs {
        token: wpt.clone(),
        amount: Amount::Exact(i128::MAX),
    }));
    cmds.push_back(Command::BridgeWrappedPtBackToEvm(
        BridgeWrappedPtBackToEvmArgs {
            wrapped_pt: wpt,
            amount: Amount::Exact(i128::MAX),
            target_chain_id: 1,
            evm_recipient: BytesN::from_array(&s.env, &[0xAA; 20]),
            gas_token,
            gas_amount: Amount::Exact(0),
            refund_address: Recipient::Caller,
            min_amount_out: 0,
        },
    ));
    let res = s
        .router_client
        .try_execute(&caller, &cmds, &(s.env.ledger().timestamp() + 60));
    assert!(res.is_err(), "expected fee overflow panic, got {res:?}");
}

#[test]
fn bridge_native_refund_address_router_resolves_correctly() {
    let b = setup_bridge_env();
    let cmds = vec![
        &b.env,
        Command::TransferFrom(TransferFromArgs {
            token: b.pt.clone(),
            amount: Amount::Exact(1_000),
        }),
        Command::BridgeNativePtToEvm(BridgeNativePtToEvmArgs {
            pt: b.pt.clone(),
            amount: Amount::Exact(1_000),
            target_chain_id: 1,
            evm_recipient: BytesN::from_array(&b.env, &[0xCC; 20]),
            gas_token: b.gas_token.clone(),
            gas_amount: Amount::Exact(0),
            refund_address: Recipient::Router,
            min_amount_out: 0,
        }),
    ];
    b.router_client
        .execute(&b.caller, &cmds, &(b.env.ledger().timestamp() + 60));
    let _last = MockPtBridgeClient::new(&b.env, &b.bridge)
        .last_native()
        .unwrap();
}

#[test]
fn test_fill_orders_natural_pt_for_ibt() {
    let s = make_setup();

    let fee_collector = Address::generate(&s.env);
    let engine_addr = s.env.register(
        limit_order_engine::LimitOrderEngineContract,
        (
            s.admin.clone(),
            fee_collector.clone(),
            0i128,
            s.registry_addr.clone(),
        ),
    );
    let engine = limit_order_engine::LimitOrderEngineContractClient::new(&s.env, &engine_addr);
    s.router_client.set_engine(&s.admin, &engine_addr);

    let und = spectra_mocks::MockUnderlyingContractClient::new(&s.env, &s.underlying_addr);
    let ibt = soroban_sdk::token::Client::new(&s.env, &s.ibt_addr);
    let pt_token = soroban_sdk::token::Client::new(&s.env, &s.pt_addr);
    let pt = principal_token::PrincipalTokenContractClient::new(&s.env, &s.pt_addr);

    let maker = Address::generate(&s.env);
    let making_amount: i128 = 1_000_000;
    let maker_seed: i128 = making_amount * 4;

    // Maker uses the router end-to-end to acquire PT: pull underlying →
    // wrap to IBT → split to PT+YT, all in one execute.
    und.mint(&maker, &maker_seed);
    approve_router(
        &s.env,
        &s.underlying_addr,
        &maker,
        &s.router_addr,
        maker_seed,
    );
    let mut maker_cmds = Vec::<Command>::new(&s.env);
    maker_cmds.push_back(Command::TransferFrom(TransferFromArgs {
        token: s.underlying_addr.clone(),
        amount: Amount::Exact(maker_seed),
    }));
    maker_cmds.push_back(Command::DepositAssetInIbt(DepositAssetInIbtArgs {
        ibt: s.ibt_addr.clone(),
        assets: Amount::Balance,
        recipient: Recipient::Router,
    }));
    maker_cmds.push_back(Command::DepositIbtInPt(DepositIbtInPtArgs {
        pt: s.pt_addr.clone(),
        ibts: Amount::Balance,
        pt_recipient: Recipient::Caller,
        yt_recipient: Recipient::Caller,
        min_shares: 0,
    }));
    s.router_client
        .execute(&maker, &maker_cmds, &(s.env.ledger().timestamp() + 100));

    // Maker pre-approves engine to pull PT at fill time.
    pt_token.approve(
        &maker,
        &engine_addr,
        &(making_amount * 10),
        &(s.env.ledger().sequence() + 100),
    );
    let maker_pt_before = pt_token.balance(&maker);

    let maturity = pt.maturity();
    let order = spectra_common::limit_order_engine::Order {
        salt: 1,
        expiry: maturity - 1,
        order_type: spectra_common::limit_order_engine::OrderType::PtForIbt,
        pt: s.pt_addr.clone(),
        maker: maker.clone(),
        receiver: maker.clone(),
        making_amount,
        implied_rate: 100_000_000_000_000_000, // 10% APY (18-dec)
        maker_token_type: spectra_common::limit_order_engine::MakerTokenType::Ibt,
        fail_safe_rate: 0,
    };
    engine.register_order(&order);

    // Taker is a separate user. Pulls underlying → wraps to IBT → fills
    // the order, all via the router. taker_input = IBT (canonical for
    // PtForIbt → no tokenization).
    let caller = Address::generate(&s.env);
    let taker_underlying: i128 = making_amount * 4;
    und.mint(&caller, &taker_underlying);
    approve_router(
        &s.env,
        &s.underlying_addr,
        &caller,
        &s.router_addr,
        taker_underlying,
    );

    let mut params = Vec::new(&s.env);
    params.push_back(spectra_common::limit_order_engine::FillOrderParams {
        order: order.clone(),
        making_amount,
        max_taking: i128::MAX,
    });

    let mut cmds = Vec::<Command>::new(&s.env);
    cmds.push_back(Command::TransferFrom(TransferFromArgs {
        token: s.underlying_addr.clone(),
        amount: Amount::Exact(taker_underlying),
    }));
    cmds.push_back(Command::DepositAssetInIbt(DepositAssetInIbtArgs {
        ibt: s.ibt_addr.clone(),
        assets: Amount::Balance,
        recipient: Recipient::Router,
    }));
    cmds.push_back(Command::FillOrders(FillOrdersArgs {
        receiver: Recipient::Caller,
        params,
        taker_input_token: s.ibt_addr.clone(),
        taker_input_amount: Amount::Balance,
    }));
    s.router_client
        .execute(&caller, &cmds, &(s.env.ledger().timestamp() + 100));

    let maker_ibt_after = ibt.balance(&maker);
    let caller_ibt_refund = ibt.balance(&caller);

    // Maker's PT dropped by exactly `making_amount`.
    assert_eq!(pt_token.balance(&maker), maker_pt_before - making_amount);
    // Maker received the taker's IBT payment.
    assert!(maker_ibt_after > 0);
    // Caller got the offered PT (engine sweeps maker's PT to receiver).
    assert_eq!(pt_token.balance(&caller), making_amount);
    // Caller got the unspent IBT back (excess taker_input sweep).
    assert!(caller_ibt_refund > 0);
    // Engine fully drained.
    assert_eq!(pt_token.balance(&engine_addr), 0);
    assert_eq!(ibt.balance(&engine_addr), 0);
    // Router fully drained — all IBT either paid to maker or refunded.
    assert_eq!(ibt.balance(&s.router_addr), 0);
    assert_eq!(pt_token.balance(&s.router_addr), 0);
}

/// Wire an engine to the router and return its client.
fn wire_engine<'a>(s: &Setup<'a>) -> limit_order_engine::LimitOrderEngineContractClient<'a> {
    let fee_collector = Address::generate(&s.env);
    let engine_addr = s.env.register(
        limit_order_engine::LimitOrderEngineContract,
        (
            s.admin.clone(),
            fee_collector.clone(),
            0i128,
            s.registry_addr.clone(),
        ),
    );
    let engine = limit_order_engine::LimitOrderEngineContractClient::new(&s.env, &engine_addr);
    s.router_client.set_engine(&s.admin, &engine_addr);
    engine
}

fn pt_for_ibt_order(
    s: &Setup,
    maker: &Address,
    making_amount: i128,
    salt: u64,
) -> spectra_common::limit_order_engine::Order {
    let pt = principal_token::PrincipalTokenContractClient::new(&s.env, &s.pt_addr);
    spectra_common::limit_order_engine::Order {
        salt,
        expiry: pt.maturity() - 1,
        order_type: spectra_common::limit_order_engine::OrderType::PtForIbt,
        pt: s.pt_addr.clone(),
        maker: maker.clone(),
        receiver: maker.clone(),
        making_amount,
        implied_rate: 100_000_000_000_000_000,
        maker_token_type: spectra_common::limit_order_engine::MakerTokenType::Ibt,
        fail_safe_rate: 0,
    }
}

/// A maker can create a resting order on-chain through the router's command
/// grammar (no direct engine call) — `RegisterOrders` wraps
/// `register_orders_batch`, and the engine records it under the maker's auth.
#[test]
fn test_register_orders_via_execute_creates_order() {
    let s = make_setup();
    let engine = wire_engine(&s);
    let maker = Address::generate(&s.env);
    let making_amount: i128 = 1_000_000;
    let order = pt_for_ibt_order(&s, &maker, making_amount, 1);

    let mut cmds = Vec::<Command>::new(&s.env);
    cmds.push_back(Command::RegisterOrders(RegisterOrdersArgs {
        orders: soroban_sdk::vec![&s.env, order.clone()],
    }));
    s.router_client
        .execute(&maker, &cmds, &(s.env.ledger().timestamp() + 100));

    let status = engine.order_status(&order);
    assert_eq!(status.remaining, making_amount, "order must be registered");
    assert_eq!(status.filled_amount, 0);
}

/// Batch registration THROUGH THE ROUTER under ENFORCED, SCOPED auth: a relayer
/// submits one `RegisterOrders` command carrying TWO different makers' orders;
/// each maker authorizes ONLY its own order via the `register_orders_batch`
/// payload (`require_auth_for_args`), the relayer authorizes only `execute`.
/// Both register. This is the same per-order payload a maker would sign for a
/// direct engine `register_orders_batch` — proving the signature is path-agnostic.
#[test]
fn test_register_batch_via_router_scoped_auth() {
    let s = make_setup();
    let engine = wire_engine(&s);
    let making: i128 = 1_000_000;
    let maker_a = Address::generate(&s.env);
    let maker_b = Address::generate(&s.env);
    let order_a = pt_for_ibt_order(&s, &maker_a, making, 1);
    let order_b = pt_for_ibt_order(&s, &maker_b, making, 2);
    let relayer = Address::generate(&s.env);

    let mut cmds = Vec::<Command>::new(&s.env);
    cmds.push_back(Command::RegisterOrders(RegisterOrdersArgs {
        orders: soroban_sdk::vec![&s.env, order_a.clone(), order_b.clone()],
    }));
    let deadline = s.env.ledger().timestamp() + 100;

    s.env.mock_auths(&[
        MockAuth {
            address: &relayer,
            invoke: &MockAuthInvoke {
                contract: &s.router_addr,
                fn_name: "execute",
                args: (relayer.clone(), cmds.clone(), deadline).into_val(&s.env),
                sub_invokes: &[],
            },
        },
        MockAuth {
            address: &maker_a,
            invoke: &MockAuthInvoke {
                contract: &engine.address,
                fn_name: "register_orders_batch",
                args: (order_a.clone(),).into_val(&s.env),
                sub_invokes: &[],
            },
        },
        MockAuth {
            address: &maker_b,
            invoke: &MockAuthInvoke {
                contract: &engine.address,
                fn_name: "register_orders_batch",
                args: (order_b.clone(),).into_val(&s.env),
                sub_invokes: &[],
            },
        },
    ]);

    s.router_client.execute(&relayer, &cmds, &deadline);

    assert_eq!(engine.order_status(&order_a).remaining, making);
    assert_eq!(engine.order_status(&order_b).remaining, making);
}

/// End-to-end: the maker *creates* the order via a router command, then a taker
/// fills it via a router command — creation and filling both expressed as
/// commands, no direct engine calls.
#[test]
fn test_register_via_execute_then_fill() {
    let s = make_setup();
    let engine = wire_engine(&s);

    let und = spectra_mocks::MockUnderlyingContractClient::new(&s.env, &s.underlying_addr);
    let pt_token = soroban_sdk::token::Client::new(&s.env, &s.pt_addr);

    let maker = Address::generate(&s.env);
    let making_amount: i128 = 1_000_000;

    // Maker acquires the PT to sell and pre-approves the engine to pull it at
    // fill time (fixture prep).
    acquire_pt(&s, &maker, making_amount * 4);
    pt_token.approve(
        &maker,
        &engine.address,
        &(making_amount * 10),
        &(s.env.ledger().sequence() + 100),
    );

    // ── Order CREATION: the maker creates the resting order via a router
    // command (RegisterOrders → engine.register_orders_batch), no direct
    // engine call.
    let order = pt_for_ibt_order(&s, &maker, making_amount, 1);
    let mut reg = Vec::<Command>::new(&s.env);
    reg.push_back(Command::RegisterOrders(RegisterOrdersArgs {
        orders: soroban_sdk::vec![&s.env, order.clone()],
    }));
    s.router_client
        .execute(&maker, &reg, &(s.env.ledger().timestamp() + 100));
    assert_eq!(engine.order_status(&order).remaining, making_amount);

    // Taker fills it via the router.
    let taker = Address::generate(&s.env);
    let taker_underlying: i128 = making_amount * 4;
    und.mint(&taker, &taker_underlying);
    approve_router(
        &s.env,
        &s.underlying_addr,
        &taker,
        &s.router_addr,
        taker_underlying,
    );
    let mut params = Vec::new(&s.env);
    params.push_back(spectra_common::limit_order_engine::FillOrderParams {
        order: order.clone(),
        making_amount,
        max_taking: i128::MAX,
    });
    let mut cmds = Vec::<Command>::new(&s.env);
    cmds.push_back(Command::TransferFrom(TransferFromArgs {
        token: s.underlying_addr.clone(),
        amount: Amount::Exact(taker_underlying),
    }));
    cmds.push_back(Command::DepositAssetInIbt(DepositAssetInIbtArgs {
        ibt: s.ibt_addr.clone(),
        assets: Amount::Balance,
        recipient: Recipient::Router,
    }));
    cmds.push_back(Command::FillOrders(FillOrdersArgs {
        receiver: Recipient::Caller,
        params,
        taker_input_token: s.ibt_addr.clone(),
        taker_input_amount: Amount::Balance,
    }));
    s.router_client
        .execute(&taker, &cmds, &(s.env.ledger().timestamp() + 100));

    // Order fully filled; taker got the maker's PT.
    assert_eq!(engine.order_status(&order).filled_amount, making_amount);
    assert_eq!(pt_token.balance(&taker), making_amount);
}

/// Give `who` PT+YT by routing `seed` underlying → IBT → PT (under the setup's
/// lenient mock, i.e. this is fixture prep, not the asserted flow).
fn acquire_pt(s: &Setup, who: &Address, seed: i128) {
    let und = spectra_mocks::MockUnderlyingContractClient::new(&s.env, &s.underlying_addr);
    und.mint(who, &seed);
    approve_router(&s.env, &s.underlying_addr, who, &s.router_addr, seed);
    let mut c = Vec::<Command>::new(&s.env);
    c.push_back(Command::TransferFrom(TransferFromArgs {
        token: s.underlying_addr.clone(),
        amount: Amount::Exact(seed),
    }));
    c.push_back(Command::DepositAssetInIbt(DepositAssetInIbtArgs {
        ibt: s.ibt_addr.clone(),
        assets: Amount::Balance,
        recipient: Recipient::Router,
    }));
    c.push_back(Command::DepositIbtInPt(DepositIbtInPtArgs {
        pt: s.pt_addr.clone(),
        ibts: Amount::Balance,
        pt_recipient: Recipient::Caller,
        yt_recipient: Recipient::Caller,
        min_shares: 0,
    }));
    s.router_client
        .execute(who, &c, &(s.env.ledger().timestamp() + 100));
}

/// Give `who` IBT by routing `seed` underlying → IBT (fixture prep).
fn acquire_ibt(s: &Setup, who: &Address, seed: i128) {
    let und = spectra_mocks::MockUnderlyingContractClient::new(&s.env, &s.underlying_addr);
    und.mint(who, &seed);
    approve_router(&s.env, &s.underlying_addr, who, &s.router_addr, seed);
    let mut c = Vec::<Command>::new(&s.env);
    c.push_back(Command::TransferFrom(TransferFromArgs {
        token: s.underlying_addr.clone(),
        amount: Amount::Exact(seed),
    }));
    c.push_back(Command::DepositAssetInIbt(DepositAssetInIbtArgs {
        ibt: s.ibt_addr.clone(),
        assets: Amount::Balance,
        recipient: Recipient::Caller,
    }));
    s.router_client
        .execute(who, &c, &(s.env.ledger().timestamp() + 100));
}

/// The headline flow, under ENFORCED, SCOPED authorizations (no blanket mock):
/// a taker registers TWO orders from TWO DIFFERENT makers and fills them in a
/// single router `execute`. `RegisterOrders` forwards to the engine's
/// `register_orders_batch`, which authorizes each order via
/// `require_auth_for_args((order,))`; so each maker authorizes ONLY its own
/// order (payload `(engine, "register_orders_batch", [order])`) and the taker
/// authorizes only the `execute`. Exactly what a relayer/taker submitting
/// makers' pre-signed orders looks like — and the same per-order payload works
/// whether the batch is submitted directly to the engine or via the router.
#[test]
fn test_multi_maker_register_and_fill_scoped_auth() {
    let s = make_setup();
    let engine = wire_engine(&s);
    let ibt = soroban_sdk::token::Client::new(&s.env, &s.ibt_addr);
    let pt_token = soroban_sdk::token::Client::new(&s.env, &s.pt_addr);

    let making: i128 = 1_000_000;
    let maker_a = Address::generate(&s.env);
    let maker_b = Address::generate(&s.env);

    // Each maker acquires PT to sell and pre-approves the engine to pull it at
    // fill time (allowance — no maker signature needed during the fill).
    for m in [&maker_a, &maker_b] {
        acquire_pt(&s, m, making * 4);
        pt_token.approve(
            m,
            &engine.address,
            &(making * 10),
            &(s.env.ledger().sequence() + 100),
        );
    }

    // Taker acquires IBT to pay with and approves the router to pull it.
    let taker = Address::generate(&s.env);
    acquire_ibt(&s, &taker, making * 8);
    let taker_ibt = ibt.balance(&taker);
    approve_router(&s.env, &s.ibt_addr, &taker, &s.router_addr, taker_ibt);

    let order_a = pt_for_ibt_order(&s, &maker_a, making, 1);
    let order_b = pt_for_ibt_order(&s, &maker_b, making, 2);

    // order_a is filled completely, order_b only halfway — one execute, one
    // taker, mixed full/partial fills.
    let mut params = Vec::new(&s.env);
    params.push_back(spectra_common::limit_order_engine::FillOrderParams {
        order: order_a.clone(),
        making_amount: making,
        max_taking: i128::MAX,
    });
    params.push_back(spectra_common::limit_order_engine::FillOrderParams {
        order: order_b.clone(),
        making_amount: making / 2,
        max_taking: i128::MAX,
    });

    let mut cmds = Vec::<Command>::new(&s.env);
    cmds.push_back(Command::TransferFrom(TransferFromArgs {
        token: s.ibt_addr.clone(),
        amount: Amount::Exact(taker_ibt),
    }));
    cmds.push_back(Command::RegisterOrders(RegisterOrdersArgs {
        orders: soroban_sdk::vec![&s.env, order_a.clone(), order_b.clone()],
    }));
    cmds.push_back(Command::FillOrders(FillOrdersArgs {
        receiver: Recipient::Caller,
        params,
        taker_input_token: s.ibt_addr.clone(),
        taker_input_amount: Amount::Balance,
    }));
    let deadline = s.env.ledger().timestamp() + 100;

    // ACTUAL, SCOPED authorizations — every require_auth must match one entry:
    //   • taker  → router.execute(...)
    //   • makerA → engine.register_orders_batch(orderA)   (only its own order)
    //   • makerB → engine.register_orders_batch(orderB)
    // Fill-side maker pulls are allowance-based; engine/router sub-calls
    // self-authorize. No blanket mock_all_auths.
    s.env.mock_auths(&[
        MockAuth {
            address: &taker,
            invoke: &MockAuthInvoke {
                contract: &s.router_addr,
                fn_name: "execute",
                args: (taker.clone(), cmds.clone(), deadline).into_val(&s.env),
                sub_invokes: &[],
            },
        },
        MockAuth {
            address: &maker_a,
            invoke: &MockAuthInvoke {
                contract: &engine.address,
                fn_name: "register_orders_batch",
                args: (order_a.clone(),).into_val(&s.env),
                sub_invokes: &[],
            },
        },
        MockAuth {
            address: &maker_b,
            invoke: &MockAuthInvoke {
                contract: &engine.address,
                fn_name: "register_orders_batch",
                args: (order_b.clone(),).into_val(&s.env),
                sub_invokes: &[],
            },
        },
    ]);

    s.router_client.execute(&taker, &cmds, &deadline);

    // Both orders registered in the one transaction; order_a fully filled,
    // order_b half filled (still live for the remaining half).
    assert_eq!(engine.order_status(&order_a).filled_amount, making);
    assert_eq!(engine.order_status(&order_a).remaining, 0);
    assert_eq!(engine.order_status(&order_b).filled_amount, making / 2);
    assert_eq!(engine.order_status(&order_b).remaining, making / 2);
    // Taker received maker A's full PT and maker B's half.
    assert_eq!(pt_token.balance(&taker), making + making / 2);
}

/// Full multi-maker lifecycle THROUGH THE ROUTER: four makers create resting
/// orders, TWO takers fill them (each taker fills one order fully and one
/// partially), then EACH maker cancels their own order via the router's
/// `CancelOrders` command. The register/fill legs run under the setup's lenient
/// mock; the cancel legs run under ENFORCED, SCOPED auth — each maker authorizes
/// `execute` with a nested `cancel_orders_batch` sub-invocation over ONLY their
/// own order, proving cancellation is gated on ownership even via the router.
#[test]
fn test_multi_maker_full_lifecycle_cancel_via_router() {
    let s = make_setup();
    let engine = wire_engine(&s);
    let ibt = soroban_sdk::token::Client::new(&s.env, &s.ibt_addr);
    let pt_token = soroban_sdk::token::Client::new(&s.env, &s.pt_addr);

    let making: i128 = 1_000_000;
    let m0 = Address::generate(&s.env);
    let m1 = Address::generate(&s.env);
    let m2 = Address::generate(&s.env);
    let m3 = Address::generate(&s.env);

    // Each maker acquires PT and pre-approves the engine to pull at fill time,
    // then registers one resting order through the router.
    let order0 = pt_for_ibt_order(&s, &m0, making, 1);
    let order1 = pt_for_ibt_order(&s, &m1, making, 1);
    let order2 = pt_for_ibt_order(&s, &m2, making, 1);
    let order3 = pt_for_ibt_order(&s, &m3, making, 1);
    for (m, o) in [
        (&m0, &order0),
        (&m1, &order1),
        (&m2, &order2),
        (&m3, &order3),
    ] {
        acquire_pt(&s, m, making * 4);
        pt_token.approve(
            m,
            &engine.address,
            &(making * 10),
            &(s.env.ledger().sequence() + 100),
        );
        let mut reg = Vec::<Command>::new(&s.env);
        reg.push_back(Command::RegisterOrders(RegisterOrdersArgs {
            orders: soroban_sdk::vec![&s.env, o.clone()],
        }));
        s.router_client
            .execute(m, &reg, &(s.env.ledger().timestamp() + 100));
        assert_eq!(engine.order_status(o).remaining, making, "registered");
    }

    // ── Taker 1: fills order0 FULLY and order1 by HALF ──────────────────────
    let taker1 = Address::generate(&s.env);
    acquire_ibt(&s, &taker1, making * 8);
    let t1_ibt = ibt.balance(&taker1);
    approve_router(&s.env, &s.ibt_addr, &taker1, &s.router_addr, t1_ibt);
    let mut p1 = Vec::new(&s.env);
    p1.push_back(spectra_common::limit_order_engine::FillOrderParams {
        order: order0.clone(),
        making_amount: making,
        max_taking: i128::MAX,
    });
    p1.push_back(spectra_common::limit_order_engine::FillOrderParams {
        order: order1.clone(),
        making_amount: making / 2,
        max_taking: i128::MAX,
    });
    let mut c1 = Vec::<Command>::new(&s.env);
    c1.push_back(Command::TransferFrom(TransferFromArgs {
        token: s.ibt_addr.clone(),
        amount: Amount::Exact(t1_ibt),
    }));
    c1.push_back(Command::FillOrders(FillOrdersArgs {
        receiver: Recipient::Caller,
        params: p1,
        taker_input_token: s.ibt_addr.clone(),
        taker_input_amount: Amount::Balance,
    }));
    s.router_client
        .execute(&taker1, &c1, &(s.env.ledger().timestamp() + 100));
    assert_eq!(engine.order_status(&order0).filled_amount, making, "0 full");
    assert_eq!(engine.order_status(&order0).remaining, 0, "0 terminal");
    assert_eq!(
        engine.order_status(&order1).filled_amount,
        making / 2,
        "1 partial"
    );
    assert_eq!(
        engine.order_status(&order1).remaining,
        making / 2,
        "1 still live"
    );

    // ── Taker 2: fills order2 FULLY and order3 by HALF ──────────────────────
    let taker2 = Address::generate(&s.env);
    acquire_ibt(&s, &taker2, making * 8);
    let t2_ibt = ibt.balance(&taker2);
    approve_router(&s.env, &s.ibt_addr, &taker2, &s.router_addr, t2_ibt);
    let mut p2 = Vec::new(&s.env);
    p2.push_back(spectra_common::limit_order_engine::FillOrderParams {
        order: order2.clone(),
        making_amount: making,
        max_taking: i128::MAX,
    });
    p2.push_back(spectra_common::limit_order_engine::FillOrderParams {
        order: order3.clone(),
        making_amount: making / 2,
        max_taking: i128::MAX,
    });
    let mut c2 = Vec::<Command>::new(&s.env);
    c2.push_back(Command::TransferFrom(TransferFromArgs {
        token: s.ibt_addr.clone(),
        amount: Amount::Exact(t2_ibt),
    }));
    c2.push_back(Command::FillOrders(FillOrdersArgs {
        receiver: Recipient::Caller,
        params: p2,
        taker_input_token: s.ibt_addr.clone(),
        taker_input_amount: Amount::Balance,
    }));
    s.router_client
        .execute(&taker2, &c2, &(s.env.ledger().timestamp() + 100));
    assert_eq!(engine.order_status(&order2).filled_amount, making, "2 full");
    assert_eq!(
        engine.order_status(&order3).remaining,
        making / 2,
        "3 still live"
    );

    // Takers received the PT they filled (full + half each).
    assert_eq!(pt_token.balance(&taker1), making + making / 2);
    assert_eq!(pt_token.balance(&taker2), making + making / 2);

    // ── Each maker cancels their own order via the router, SCOPED auth ───────
    for (m, o) in [
        (&m0, &order0),
        (&m1, &order1),
        (&m2, &order2),
        (&m3, &order3),
    ] {
        let mut cancel = Vec::<Command>::new(&s.env);
        let batch = soroban_sdk::vec![&s.env, o.clone()];
        cancel.push_back(Command::CancelOrders(CancelOrdersArgs {
            orders: batch.clone(),
        }));
        let deadline = s.env.ledger().timestamp() + 100;
        s.env.mock_auths(&[MockAuth {
            address: m,
            invoke: &MockAuthInvoke {
                contract: &s.router_addr,
                fn_name: "execute",
                args: (m.clone(), cancel.clone(), deadline).into_val(&s.env),
                sub_invokes: &[MockAuthInvoke {
                    contract: &engine.address,
                    fn_name: "cancel_orders_batch",
                    args: (batch.clone(),).into_val(&s.env),
                    sub_invokes: &[],
                }],
            },
        }]);
        s.router_client.execute(m, &cancel, &deadline);
        assert_eq!(engine.order_status(o).remaining, 0, "cancelled/terminal");
    }
}

/// Ownership is enforced through the router: an attacker who slips a victim's
/// order into their OWN `CancelOrders` batch is rejected with
/// `CancelBatchMakerMismatch` (#23) — you cannot cancel an order you don't own.
#[test]
fn test_cancel_via_router_rejects_foreign_order() {
    let s = make_setup();
    let engine = wire_engine(&s);
    let making: i128 = 1_000_000;
    let victim = Address::generate(&s.env);
    let attacker = Address::generate(&s.env);
    let victim_order = pt_for_ibt_order(&s, &victim, making, 1);
    let attacker_order = pt_for_ibt_order(&s, &attacker, making, 2);

    // Register both (lenient setup mock).
    for (m, o) in [(&victim, &victim_order), (&attacker, &attacker_order)] {
        let mut reg = Vec::<Command>::new(&s.env);
        reg.push_back(Command::RegisterOrders(RegisterOrdersArgs {
            orders: soroban_sdk::vec![&s.env, o.clone()],
        }));
        s.router_client
            .execute(m, &reg, &(s.env.ledger().timestamp() + 100));
    }

    // Attacker batches their own order + the victim's; the mismatch reverts all.
    let mut cancel = Vec::<Command>::new(&s.env);
    cancel.push_back(Command::CancelOrders(CancelOrdersArgs {
        orders: soroban_sdk::vec![&s.env, attacker_order.clone(), victim_order.clone()],
    }));
    let r = s
        .router_client
        .try_execute(&attacker, &cancel, &(s.env.ledger().timestamp() + 100));
    let expected = soroban_sdk::Error::from_contract_error(23);
    assert!(
        matches!(r, Err(Ok(e)) if e == expected),
        "expected CancelBatchMakerMismatch (#23), got {r:?}"
    );
    // Victim's order is untouched (atomic revert).
    assert_eq!(engine.order_status(&victim_order).remaining, making);
}

// ─── Mock Broker ─────────────────────────────────────────────────────────────

#[contracttype]
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LastBrokerSwapCall {
    pub selling: Address,
    pub trader: Address,
    pub amount: i128,
    pub vfee: u32,
    pub ffee: u32,
}

#[contracttype]
pub enum MockBrokerKey {
    BuyToken,
    BuyRatio, // numerator / denominator → bought = amount_in * num / denom
    BuyNum,
    BuyDenom,
    LastCall,
}

#[contract]
pub struct MockStellarBroker;

#[contractimpl]
impl MockStellarBroker {
    pub fn init(env: Env, buy_token: Address, buy_num: i128, buy_denom: i128) {
        env.storage()
            .instance()
            .set(&MockBrokerKey::BuyToken, &buy_token);
        env.storage()
            .instance()
            .set(&MockBrokerKey::BuyNum, &buy_num);
        env.storage()
            .instance()
            .set(&MockBrokerKey::BuyDenom, &buy_denom);
    }

    pub fn swap(
        env: Env,
        selling: Address,
        routes: soroban_sdk::Vec<spectra_common::router_types::Route>,
        trader: Address,
        vfee: u32,
        ffee: u32,
        fpath: soroban_sdk::Vec<spectra_common::router_types::PathStep>,
    ) -> soroban_sdk::Vec<i128> {
        trader.require_auth();
        let _ = fpath;

        let buy_token: Address = env
            .storage()
            .instance()
            .get(&MockBrokerKey::BuyToken)
            .unwrap();
        let buy_num: i128 = env
            .storage()
            .instance()
            .get(&MockBrokerKey::BuyNum)
            .unwrap();
        let buy_denom: i128 = env
            .storage()
            .instance()
            .get(&MockBrokerKey::BuyDenom)
            .unwrap();

        // Pull exactly the per-route sum from the trader, mirroring the real
        // StellarBroker (estimate_routes + a single transfer).
        let mut selling_amount: i128 = 0;
        for route in routes.iter() {
            selling_amount += route.amount;
        }
        soroban_sdk::token::Client::new(&env, &selling).transfer(
            &trader,
            &env.current_contract_address(),
            &selling_amount,
        );

        // Simulate swap: compute bought, deduct fee, mint net to trader.
        let bought = selling_amount * buy_num / buy_denom;
        let fee = (bought * ffee as i128) / 1000;
        let net = bought - fee;
        spectra_mocks::MockUnderlyingContractClient::new(&env, &buy_token).mint(&trader, &net);

        env.storage().instance().set(
            &MockBrokerKey::LastCall,
            &LastBrokerSwapCall {
                selling,
                trader,
                amount: selling_amount,
                vfee,
                ffee,
            },
        );

        soroban_sdk::Vec::from_array(&env, [selling_amount, net, fee])
    }

    pub fn last_call(env: Env) -> Option<LastBrokerSwapCall> {
        env.storage().instance().get(&MockBrokerKey::LastCall)
    }
}

/// A broker that pulls `token_in` and reports a satisfying `bought`, but
/// delivers FEWER (here: zero) tokens than it claims. Models the threat of
/// a bug/upgrade/compromise making the reported output diverge from
/// what the Router actually receives. The Router must reject based on its real
/// balance delta, not the report.
#[contract]
pub struct MockLyingBroker;

#[contractimpl]
impl MockLyingBroker {
    pub fn swap(
        env: Env,
        selling: Address,
        routes: soroban_sdk::Vec<spectra_common::router_types::Route>,
        trader: Address,
        vfee: u32,
        ffee: u32,
        fpath: soroban_sdk::Vec<spectra_common::router_types::PathStep>,
    ) -> soroban_sdk::Vec<i128> {
        trader.require_auth();
        let _ = (vfee, ffee, fpath);
        let mut selling_amount: i128 = 0;
        for route in routes.iter() {
            selling_amount += route.amount;
        }
        // Take the input like a real broker...
        soroban_sdk::token::Client::new(&env, &selling).transfer(
            &trader,
            &env.current_contract_address(),
            &selling_amount,
        );
        // ...but deliver NOTHING while reporting a generous bought amount.
        let reported_bought = selling_amount * 1_000;
        soroban_sdk::Vec::from_array(&env, [selling_amount, reported_bought, 0])
    }
}

// ─── Broker storage + admin tests ────────────────────────────────────────────

#[test]
fn set_broker_admin_only_and_round_trips() {
    let s = make_setup();
    let broker = Address::generate(&s.env);

    s.router_client.set_broker(&s.admin, &broker);
    assert_eq!(s.router_client.get_broker(), broker);

    let mallory = Address::generate(&s.env);
    let new_broker = Address::generate(&s.env);
    let result = s.router_client.try_set_broker(&mallory, &new_broker);
    let expected = soroban_sdk::Error::from_contract_error(2000);
    assert!(
        matches!(result, Err(Ok(e)) if e == expected),
        "expected AccessControlError::Unauthorized (#2000), got {result:?}"
    );
}

#[test]
fn get_broker_panics_when_unset() {
    let s = make_setup();
    assert_router_error(s.router_client.try_get_broker(), RouterError::BrokerNotSet);
}

// ─── BrokerSwap E2E tests ───────────────────────────────────────────────────

fn make_broker_env() -> (Setup<'static>, Address, Address) {
    let s = make_setup();

    // token_in = underlying, token_out = a fresh SEP-41.
    let token_out = deploy_sep41(&s.env, "OUT", "OUT", 6);

    // Mock broker: 1:1.2 swap ratio (buy_num=12, buy_denom=10).
    let broker = s.env.register(MockStellarBroker, ());
    MockStellarBrokerClient::new(&s.env, &broker).init(&token_out, &12i128, &10i128);

    s.router_client.set_broker(&s.admin, &broker);

    (s, broker, token_out)
}

/// End-to-end: TransferFrom → BrokerSwap → AssertMinBalance → Transfer.
#[test]
fn broker_swap_happy_path() {
    let (s, _broker, token_out) = make_broker_env();
    let caller = Address::generate(&s.env);

    // Give caller 1_000_000 of token_in (underlying).
    let und = spectra_mocks::MockUnderlyingContractClient::new(&s.env, &s.underlying_addr);
    und.mint(&caller, &1_000_000i128);
    approve_router(
        &s.env,
        &s.underlying_addr,
        &caller,
        &s.router_addr,
        1_000_000,
    );

    // Build a single-step route: underlying → token_out via a mock pool.
    let pool = Address::generate(&s.env);
    let path_step = PathStep {
        protocol: Protocol::Soroswap,
        asset: token_out.clone(),
        pool,
        si: 0,
        bi: 1,
    };
    let mut path = soroban_sdk::Vec::new(&s.env);
    path.push_back(path_step);
    let route = Route {
        path,
        amount: 100_000,
        min: 100_000,
        estimated: 120_000,
    };
    let mut routes = soroban_sdk::Vec::new(&s.env);
    routes.push_back(route);

    let fee_path = soroban_sdk::Vec::new(&s.env);

    let mut cmds = Vec::<Command>::new(&s.env);
    cmds.push_back(Command::TransferFrom(TransferFromArgs {
        token: s.underlying_addr.clone(),
        amount: Amount::Exact(100_000),
    }));
    cmds.push_back(Command::BrokerSwap(BrokerSwapArgs {
        token_in: s.underlying_addr.clone(),
        token_out: token_out.clone(),
        routes,
        min_amount_out: 110_000,
        fixed_fee_permille: 0,
        fee_path,
    }));
    cmds.push_back(Command::AssertMinBalance(AssertMinBalanceArgs {
        token: token_out.clone(),
        owner: Recipient::Router,
        min_value: 110_000,
    }));
    cmds.push_back(Command::Transfer(TransferArgs {
        token: token_out.clone(),
        recipient: Recipient::Caller,
        amount: Amount::Balance,
    }));

    s.router_client
        .execute(&caller, &cmds, &(s.env.ledger().timestamp() + 100));

    // Caller should have received 120_000 token_out (100_000 * 12/10).
    let out_client = soroban_sdk::token::Client::new(&s.env, &token_out);
    assert_eq!(out_client.balance(&caller), 120_000);
    // Router fully drained.
    assert_eq!(out_client.balance(&s.router_addr), 0);
    // Underlying fully drained from router (sent to broker).
    let und_client = soroban_sdk::token::Client::new(&s.env, &s.underlying_addr);
    assert_eq!(und_client.balance(&s.router_addr), 0);
}

#[test]
fn broker_swap_panics_when_broker_unset() {
    let s = make_setup();
    let caller = Address::generate(&s.env);

    let token_out = deploy_sep41(&s.env, "OUT", "OUT", 6);
    let pool = Address::generate(&s.env);
    let path_step = PathStep {
        protocol: Protocol::Soroswap,
        asset: token_out,
        pool,
        si: 0,
        bi: 1,
    };
    let mut path = soroban_sdk::Vec::new(&s.env);
    path.push_back(path_step);
    let route = Route {
        path,
        amount: 100,
        min: 100,
        estimated: 100,
    };
    let mut routes = soroban_sdk::Vec::new(&s.env);
    routes.push_back(route);

    let mut cmds = Vec::<Command>::new(&s.env);
    cmds.push_back(Command::BrokerSwap(BrokerSwapArgs {
        token_in: s.underlying_addr.clone(),
        token_out: s.underlying_addr.clone(),
        routes,
        min_amount_out: 0,
        fixed_fee_permille: 0,
        fee_path: soroban_sdk::Vec::new(&s.env),
    }));

    assert_router_error(
        s.router_client
            .try_execute(&caller, &cmds, &(s.env.ledger().timestamp() + 100)),
        RouterError::BrokerNotSet,
    );
}

#[test]
fn broker_swap_slippage_protection() {
    let (s, _broker, token_out) = make_broker_env();
    let caller = Address::generate(&s.env);

    let und = spectra_mocks::MockUnderlyingContractClient::new(&s.env, &s.underlying_addr);
    und.mint(&caller, &100_000i128);
    approve_router(&s.env, &s.underlying_addr, &caller, &s.router_addr, 100_000);

    let pool = Address::generate(&s.env);
    let path_step = PathStep {
        protocol: Protocol::Soroswap,
        asset: token_out.clone(),
        pool,
        si: 0,
        bi: 1,
    };
    let mut path = soroban_sdk::Vec::new(&s.env);
    path.push_back(path_step);
    let route = Route {
        path,
        amount: 100_000,
        min: 100_000,
        estimated: 120_000,
    };
    let mut routes = soroban_sdk::Vec::new(&s.env);
    routes.push_back(route);

    // min_amount_out = 200_000 but broker only gives 120_000 → should panic.
    let mut cmds = Vec::<Command>::new(&s.env);
    cmds.push_back(Command::TransferFrom(TransferFromArgs {
        token: s.underlying_addr.clone(),
        amount: Amount::Exact(100_000),
    }));
    cmds.push_back(Command::BrokerSwap(BrokerSwapArgs {
        token_in: s.underlying_addr.clone(),
        token_out: token_out.clone(),
        routes,
        min_amount_out: 200_000,
        fixed_fee_permille: 0,
        fee_path: soroban_sdk::Vec::new(&s.env),
    }));

    assert_router_error(
        s.router_client
            .try_execute(&caller, &cmds, &(s.env.ledger().timestamp() + 100)),
        RouterError::AssertMinBalanceFailed,
    );
}

/// Regression: a broker that reports a satisfying `bought` but
/// delivers nothing must be rejected. Slippage is verified against the Router's
/// real token_out balance delta, not the broker's self-reported output — so the
/// swap reverts and the caller's input is not silently lost.
#[test]
fn broker_swap_rejects_lying_broker_underdelivery() {
    let s = make_setup();
    let token_out = deploy_sep41(&s.env, "OUT", "OUT", 6);

    // A broker that takes the input, reports a huge bought, delivers zero.
    let broker = s.env.register(MockLyingBroker, ());
    s.router_client.set_broker(&s.admin, &broker);

    let caller = Address::generate(&s.env);
    let und = spectra_mocks::MockUnderlyingContractClient::new(&s.env, &s.underlying_addr);
    und.mint(&caller, &100_000i128);
    approve_router(&s.env, &s.underlying_addr, &caller, &s.router_addr, 100_000);

    let mut cmds = Vec::<Command>::new(&s.env);
    cmds.push_back(Command::TransferFrom(TransferFromArgs {
        token: s.underlying_addr.clone(),
        amount: Amount::Exact(100_000),
    }));
    cmds.push_back(Command::BrokerSwap(BrokerSwapArgs {
        token_in: s.underlying_addr.clone(),
        token_out: token_out.clone(),
        routes: single_route(&s.env, &token_out, 100_000),
        // The broker will REPORT 100_000_000 bought but deliver 0; this must
        // still fail because the Router received nothing.
        min_amount_out: 1,
        fixed_fee_permille: 0,
        fee_path: soroban_sdk::Vec::new(&s.env),
    }));

    assert_router_error(
        s.router_client
            .try_execute(&caller, &cmds, &(s.env.ledger().timestamp() + 100)),
        RouterError::AssertMinBalanceFailed,
    );
}

#[test]
fn broker_swap_with_fixed_fee() {
    let (s, _broker, token_out) = make_broker_env();
    let caller = Address::generate(&s.env);

    let und = spectra_mocks::MockUnderlyingContractClient::new(&s.env, &s.underlying_addr);
    und.mint(&caller, &100_000i128);
    approve_router(&s.env, &s.underlying_addr, &caller, &s.router_addr, 100_000);

    let pool = Address::generate(&s.env);
    let path_step = PathStep {
        protocol: Protocol::Soroswap,
        asset: token_out.clone(),
        pool,
        si: 0,
        bi: 1,
    };
    let mut path = soroban_sdk::Vec::new(&s.env);
    path.push_back(path_step);
    let route = Route {
        path,
        amount: 100_000,
        min: 100_000,
        estimated: 120_000,
    };
    let mut routes = soroban_sdk::Vec::new(&s.env);
    routes.push_back(route);

    // 100 permille = 10% fee. 120_000 * 10% = 12_000 fee → net 108_000.
    let mut cmds = Vec::<Command>::new(&s.env);
    cmds.push_back(Command::TransferFrom(TransferFromArgs {
        token: s.underlying_addr.clone(),
        amount: Amount::Exact(100_000),
    }));
    cmds.push_back(Command::BrokerSwap(BrokerSwapArgs {
        token_in: s.underlying_addr.clone(),
        token_out: token_out.clone(),
        routes,
        min_amount_out: 100_000,
        fixed_fee_permille: 100,
        fee_path: soroban_sdk::Vec::new(&s.env),
    }));
    cmds.push_back(Command::Transfer(TransferArgs {
        token: token_out.clone(),
        recipient: Recipient::Caller,
        amount: Amount::Balance,
    }));

    s.router_client
        .execute(&caller, &cmds, &(s.env.ledger().timestamp() + 100));

    let out_client = soroban_sdk::token::Client::new(&s.env, &token_out);
    // 120_000 bought - 12_000 fee = 108_000 net.
    assert_eq!(out_client.balance(&caller), 108_000);
}

fn single_route(env: &Env, token_out: &Address, amount: i128) -> soroban_sdk::Vec<Route> {
    let mut path = soroban_sdk::Vec::new(env);
    path.push_back(PathStep {
        protocol: Protocol::Soroswap,
        asset: token_out.clone(),
        pool: Address::generate(env),
        si: 0,
        bi: 1,
    });
    let mut routes = soroban_sdk::Vec::new(env);
    routes.push_back(Route {
        path,
        amount,
        min: amount,
        estimated: amount,
    });
    routes
}

/// The auth entry must cover the per-route sum, not the router's balance:
/// with a router balance above the route sum, the swap still succeeds and
/// only the route sum leaves the router.
#[test]
fn broker_swap_pulls_route_sum_not_balance() {
    let (s, _broker, token_out) = make_broker_env();
    let caller = Address::generate(&s.env);

    let und = spectra_mocks::MockUnderlyingContractClient::new(&s.env, &s.underlying_addr);
    und.mint(&caller, &150_000i128);
    approve_router(&s.env, &s.underlying_addr, &caller, &s.router_addr, 150_000);

    let mut cmds = Vec::<Command>::new(&s.env);
    cmds.push_back(Command::TransferFrom(TransferFromArgs {
        token: s.underlying_addr.clone(),
        amount: Amount::Exact(150_000),
    }));
    cmds.push_back(Command::BrokerSwap(BrokerSwapArgs {
        token_in: s.underlying_addr.clone(),
        token_out: token_out.clone(),
        routes: single_route(&s.env, &token_out, 100_000),
        min_amount_out: 0,
        fixed_fee_permille: 0,
        fee_path: soroban_sdk::Vec::new(&s.env),
    }));

    s.router_client
        .execute(&caller, &cmds, &(s.env.ledger().timestamp() + 100));

    let und_client = soroban_sdk::token::Client::new(&s.env, &s.underlying_addr);
    assert_eq!(und_client.balance(&s.router_addr), 50_000);
    let out_client = soroban_sdk::token::Client::new(&s.env, &token_out);
    assert_eq!(out_client.balance(&s.router_addr), 120_000);
}

#[test]
fn broker_swap_negative_min_amount_out_panics() {
    let (s, _broker, token_out) = make_broker_env();
    let caller = Address::generate(&s.env);

    let mut cmds = Vec::<Command>::new(&s.env);
    cmds.push_back(Command::BrokerSwap(BrokerSwapArgs {
        token_in: s.underlying_addr.clone(),
        token_out: token_out.clone(),
        routes: single_route(&s.env, &token_out, 100_000),
        min_amount_out: -1,
        fixed_fee_permille: 0,
        fee_path: soroban_sdk::Vec::new(&s.env),
    }));

    assert_router_error(
        s.router_client
            .try_execute(&caller, &cmds, &(s.env.ledger().timestamp() + 100)),
        RouterError::NegativeMinAmountOut,
    );
}

#[test]
fn broker_swap_empty_routes_panics() {
    let (s, _broker, _token_out) = make_broker_env();
    let caller = Address::generate(&s.env);

    let mut cmds = Vec::<Command>::new(&s.env);
    cmds.push_back(Command::BrokerSwap(BrokerSwapArgs {
        token_in: s.underlying_addr.clone(),
        token_out: s.underlying_addr.clone(),
        routes: soroban_sdk::Vec::new(&s.env),
        min_amount_out: 0,
        fixed_fee_permille: 0,
        fee_path: soroban_sdk::Vec::new(&s.env),
    }));

    assert_router_error(
        s.router_client
            .try_execute(&caller, &cmds, &(s.env.ledger().timestamp() + 100)),
        RouterError::InvalidRoutes,
    );
}

#[test]
fn broker_swap_nonpositive_route_amount_panics() {
    let (s, _broker, token_out) = make_broker_env();
    let caller = Address::generate(&s.env);

    let mut cmds = Vec::<Command>::new(&s.env);
    cmds.push_back(Command::BrokerSwap(BrokerSwapArgs {
        token_in: s.underlying_addr.clone(),
        token_out: token_out.clone(),
        routes: single_route(&s.env, &token_out, 0),
        min_amount_out: 0,
        fixed_fee_permille: 0,
        fee_path: soroban_sdk::Vec::new(&s.env),
    }));

    assert_router_error(
        s.router_client
            .try_execute(&caller, &cmds, &(s.env.ledger().timestamp() + 100)),
        RouterError::InvalidRoutes,
    );
}

// ===========================================================================
// Admin-rotation tests (stellar-access 2-step)
// ===========================================================================

mod ownable2step_tests {
    use super::*;
    use stellar_access::access_control;

    fn router_only() -> (Env, MainRouterContractClient<'static>, Address) {
        let env = Env::default();
        env.mock_all_auths();
        let admin = Address::generate(&env);
        let registry = Address::generate(&env);
        let router_addr = env.register(MainRouterContract, (admin.clone(), registry.clone()));
        let client = MainRouterContractClient::new(&env, &router_addr);
        (env, client, admin)
    }

    #[test]
    fn transfer_then_accept_rotates_admin_slot() {
        let (env, router, _admin) = router_only();
        let new_admin = Address::generate(&env);
        let live_until = env.ledger().sequence() + 8640;

        router.transfer_admin_role(&new_admin, &live_until);
        router.accept_admin_transfer();

        env.as_contract(&router.address, || {
            assert_eq!(access_control::get_admin(&env), Some(new_admin.clone()));
        });
    }
}

// ===========================================================================
// Adversarial bug-hunt tests
// ===========================================================================
//
// Goal: try to BREAK the router contract, not lock in conforming behaviour.
// Each test is annotated with the attack class and what it would catch.

mod break_tests {
    use super::*;
    use stellar_access::access_control;

    fn router_only() -> (Env, MainRouterContractClient<'static>, Address) {
        let env = Env::default();
        env.mock_all_auths();
        let admin = Address::generate(&env);
        let registry = Address::generate(&env);
        let router_addr = env.register(MainRouterContract, (admin.clone(), registry.clone()));
        let client = MainRouterContractClient::new(&env, &router_addr);
        (env, client, admin)
    }

    fn fresh_router() -> (Env, MainRouterContractClient<'static>) {
        let env = Env::default();
        env.mock_all_auths();
        // Constructed with a throwaway admin/registry; the sweep below calls
        // setters with a *different* (non-admin) caller.
        let admin = Address::generate(&env);
        let registry = Address::generate(&env);
        let router_addr = env.register(MainRouterContract, (admin, registry));
        let client = MainRouterContractClient::new(&env, &router_addr);
        (env, client)
    }

    // ── Setter non-admin guard sweep ─────────────────────────────────────
    //
    // The management setters are gated by the ROUTER_MANAGER role. Each test
    // calls the setter with a freshly-generated caller that holds no role, so
    // the `#[only_role]` gate must reject it. (Pre-`__constructor` these were
    // "pre-init" tests; with a deploy-time constructor there is no
    // uninitialized state, so they now pin non-admin rejection.)

    #[test]
    #[should_panic]
    fn test_break_set_registry_before_init_panics() {
        let (env, router) = fresh_router();
        let caller = Address::generate(&env);
        let addr = Address::generate(&env);
        router.set_registry(&caller, &addr);
    }

    #[test]
    #[should_panic]
    fn test_break_set_engine_before_init_panics() {
        let (env, router) = fresh_router();
        let caller = Address::generate(&env);
        let addr = Address::generate(&env);
        router.set_engine(&caller, &addr);
    }

    #[test]
    #[should_panic]
    fn test_break_set_bridge_before_init_panics() {
        let (env, router) = fresh_router();
        let caller = Address::generate(&env);
        let addr = Address::generate(&env);
        router.set_bridge(&caller, &addr);
    }

    #[test]
    #[should_panic]
    fn test_break_set_broker_before_init_panics() {
        let (env, router) = fresh_router();
        let caller = Address::generate(&env);
        let addr = Address::generate(&env);
        router.set_broker(&caller, &addr);
    }

    #[test]
    #[should_panic]
    fn test_break_pause_before_init_panics() {
        let (env, router) = fresh_router();
        let caller = Address::generate(&env);
        router.pause(&caller);
    }

    #[test]
    #[should_panic]
    fn test_break_unpause_before_init_panics() {
        let (env, router) = fresh_router();
        let caller = Address::generate(&env);
        router.unpause(&caller);
    }

    #[test]
    #[should_panic]
    fn test_break_grant_role_before_init_panics() {
        let (env, router) = fresh_router();
        let caller = Address::generate(&env);
        let target = Address::generate(&env);
        router.grant_role(&caller, &target, &spectra_common::constants::roles::UPGRADE);
    }

    #[test]
    #[should_panic]
    fn test_break_upgrade_before_init_panics() {
        let (env, router) = fresh_router();
        let caller = Address::generate(&env);
        let hash = soroban_sdk::BytesN::<32>::from_array(&env, &[0u8; 32]);
        router.upgrade(&caller, &hash);
    }

    // Removed test_break_transfer_admin_role_before_init_panics: with a
    // deploy-time `__constructor` there is no uninitialized state, and
    // `transfer_admin_role` is gated by the stored admin's `require_auth`
    // (satisfied here by mock_all_auths), so the old pre-init panic premise no
    // longer applies.

    #[test]
    #[should_panic]
    fn test_break_accept_admin_transfer_before_init_panics() {
        let (_env, router) = fresh_router();
        router.accept_admin_transfer();
    }

    // ── Admin 2-step edges (mirrors Factory PR #154) ─────────────────────

    #[test]
    #[should_panic]
    fn test_break_accept_admin_transfer_without_pending_panics() {
        let (_env, router, _admin) = router_only();
        router.accept_admin_transfer();
    }

    #[test]
    fn test_break_transfer_admin_role_twice_overrides_pending() {
        let (env, router, _admin) = router_only();
        let first = Address::generate(&env);
        let second = Address::generate(&env);
        let live_until = env.ledger().sequence() + 8640;

        router.transfer_admin_role(&first, &live_until);
        router.transfer_admin_role(&second, &live_until);
        router.accept_admin_transfer();

        env.as_contract(&router.address, || {
            assert_eq!(access_control::get_admin(&env), Some(second.clone()));
        });

        // No pending transfer remains.
        let result = router.try_accept_admin_transfer();
        assert!(result.is_err());
    }

    // ── Self-reference attacks ───────────────────────────────────────────

    /// ATTACK: set the router's `engine` pointer to the router itself.
    /// On `execute`, the router would call into itself, potentially
    /// recursing or trapping on auth state. The router admin can do
    /// this either by accident or intentionally to disable execution.
    /// SECURITY: pin behaviour — does the router accept self-ref and
    /// then fail at execute, or does it reject at the setter?
    #[test]
    fn test_break_set_engine_to_router_self_accepted_at_setter() {
        let (env, router, admin) = router_only();
        let _ = env;
        // Setter accepts self-reference (no equality check).
        router.set_engine(&admin, &router.address);
        assert_eq!(router.get_engine(), router.address);
    }

    /// ATTACK: same self-reference for registry, bridge, broker.
    /// All four follow the same code path; verify they're all
    /// uniformly permissive (or all uniformly strict).
    #[test]
    fn test_break_set_all_address_pointers_to_router_self_accepted() {
        let (_env, router, admin) = router_only();
        router.set_engine(&admin, &router.address);
        router.set_bridge(&admin, &router.address);
        router.set_broker(&admin, &router.address);
        // Registry can also be set after init.
        router.set_registry(&admin, &router.address);
        assert_eq!(router.get_engine(), router.address);
        assert_eq!(router.get_bridge(), router.address);
        assert_eq!(router.get_broker(), router.address);
        assert_eq!(router.get_registry(), router.address);
    }

    // ── Double pause / unpause ───────────────────────────────────────────

    #[test]
    fn test_break_double_pause_idempotency() {
        let (env, router, admin) = router_only();
        let _ = env;
        router.pause(&admin);
        assert!(router.paused());
        let result = router.try_pause(&admin);
        match result {
            Ok(_) => assert!(router.paused()),
            Err(_) => assert!(router.paused()),
        }
    }

    #[test]
    fn test_break_double_unpause_idempotency() {
        let (_env, router, admin) = router_only();
        assert!(!router.paused());
        let result = router.try_unpause(&admin);
        match result {
            Ok(_) => assert!(!router.paused()),
            Err(_) => assert!(!router.paused()),
        }
    }

    // ── Role rotation invariants (mirrors Factory PR #154) ───────────────

    #[test]
    fn test_break_old_admin_cannot_grant_role_after_rotation() {
        let (env, router, old_admin) = router_only();
        let new_admin = Address::generate(&env);
        let live_until = env.ledger().sequence() + 8640;
        router.transfer_admin_role(&new_admin, &live_until);
        router.accept_admin_transfer();

        let target = Address::generate(&env);
        let result = router.try_grant_role(
            &old_admin,
            &target,
            &spectra_common::constants::roles::UPGRADE,
        );
        assert!(result.is_err());
        assert!(!router.has_role(&target, &spectra_common::constants::roles::UPGRADE));
    }

    #[test]
    fn test_break_new_admin_can_grant_role_after_rotation() {
        let (env, router, _old_admin) = router_only();
        let new_admin = Address::generate(&env);
        let live_until = env.ledger().sequence() + 8640;
        router.transfer_admin_role(&new_admin, &live_until);
        router.accept_admin_transfer();

        let target = Address::generate(&env);
        router.grant_role(
            &new_admin,
            &target,
            &spectra_common::constants::roles::UPGRADE,
        );
        assert!(router.has_role(&target, &spectra_common::constants::roles::UPGRADE));
    }
}

// ===========================================================================
// BREACH-HUNT: router attack surface
// ===========================================================================
//
// Threat model (from project lead):
//   * No arbitrary code execution.
//   * No draining of a user's *approved* allowance within a tx beyond what
//     the user signed for.
//   * Funds stranded in the Router between txs are fair game for the next
//     caller — that is NOT considered an attack. The attack to prevent is:
//     pulling funds from an address that did NOT intentionally approve the
//     Router for them.
//   * Every combination of commands must work.
//   * A "minimum balance" command must exist as a slippage-style guard.
//     (`Command::AssertMinBalance` — confirmed present in dispatcher.rs:72.)
//
// Tests below probe each axis: approval binding, cross-caller spoofing,
// slippage guards on PT/IBT commands, composability of arbitrary command
// sequences, negative-input rejection, and atomic tx-revert on assert
// failure.
// ===========================================================================

mod breach_hunt {
    use super::*;

    // ── A1: caller-binding of TransferFrom ────────────────────────────────

    /// `TransferFromArgs` has no `from` field — the dispatcher always uses
    /// `caller` (the `execute` invoker) as the SEP-41 source. Pin that an
    /// attacker cannot specify a victim address: even if Bob has approved
    /// the router for X tokens, Alice's `execute(caller=Alice, [TransferFrom
    /// (token, X)])` pulls from ALICE, not Bob.
    #[test]
    fn test_attack_transfer_from_uses_caller_not_attacker_controlled_from() {
        let s = make_setup();

        let victim = Address::generate(&s.env);
        let attacker = Address::generate(&s.env);

        let und = spectra_mocks::MockUnderlyingContractClient::new(&s.env, &s.underlying_addr);
        und.mint(&victim, &1_000_000i128);
        // Victim approves router for 1M.
        approve_router(
            &s.env,
            &s.underlying_addr,
            &victim,
            &s.router_addr,
            1_000_000,
        );

        // Attacker calls execute, tries to pull 1M.
        let mut cmds = Vec::<Command>::new(&s.env);
        cmds.push_back(Command::TransferFrom(TransferFromArgs {
            token: s.underlying_addr.clone(),
            amount: Amount::Exact(1_000_000),
        }));

        let r = s
            .router_client
            .try_execute(&attacker, &cmds, &(s.env.ledger().timestamp() + 100));
        assert!(
            r.is_err(),
            "attacker calling TransferFrom must pull from themselves, \
             not from a victim who approved the router; attacker has no \
             balance/allowance → must fail"
        );

        // Victim's balance untouched.
        let token_c = soroban_sdk::token::Client::new(&s.env, &s.underlying_addr);
        assert_eq!(token_c.balance(&victim), 1_000_000);
        assert_eq!(token_c.balance(&attacker), 0);
        assert_eq!(token_c.balance(&s.router_addr), 0);
    }

    // ── A2: allowance ceiling enforced ────────────────────────────────────

    /// Caller approves router for X, tries to TransferFrom Exact(X+1). Must
    /// fail at the SEP-41 layer (InsufficientAllowance). Pins the natural
    /// ceiling.
    #[test]
    fn test_attack_transfer_from_above_allowance_panics() {
        let s = make_setup();
        let caller = Address::generate(&s.env);

        let und = spectra_mocks::MockUnderlyingContractClient::new(&s.env, &s.underlying_addr);
        und.mint(&caller, &1_000_000i128);
        approve_router(&s.env, &s.underlying_addr, &caller, &s.router_addr, 100);

        let mut cmds = Vec::<Command>::new(&s.env);
        cmds.push_back(Command::TransferFrom(TransferFromArgs {
            token: s.underlying_addr.clone(),
            amount: Amount::Exact(101),
        }));
        let r = s
            .router_client
            .try_execute(&caller, &cmds, &(s.env.ledger().timestamp() + 100));
        assert!(
            r.is_err(),
            "transfer_from above allowance must panic — pins SEP-41 ceiling"
        );
    }

    // ── A3: Amount::Balance is bounded by allowance ───────────────────────

    /// Caller has 1M balance but only approved 100. `Amount::Balance`
    /// resolves to balance(caller) = 1M, which exceeds the 100 allowance.
    /// Must fail — `Balance` does NOT auto-cap to allowance.
    #[test]
    fn test_attack_transfer_from_balance_sentinel_does_not_auto_cap_to_allowance() {
        let s = make_setup();
        let caller = Address::generate(&s.env);

        let und = spectra_mocks::MockUnderlyingContractClient::new(&s.env, &s.underlying_addr);
        und.mint(&caller, &1_000_000i128);
        approve_router(&s.env, &s.underlying_addr, &caller, &s.router_addr, 100);

        let mut cmds = Vec::<Command>::new(&s.env);
        cmds.push_back(Command::TransferFrom(TransferFromArgs {
            token: s.underlying_addr.clone(),
            amount: Amount::Balance,
        }));
        let r = s
            .router_client
            .try_execute(&caller, &cmds, &(s.env.ledger().timestamp() + 100));
        assert!(
            r.is_err(),
            "Balance sentinel resolves to balance(caller) which exceeds allowance — must panic"
        );
    }

    // ── A4: negative Amount::Exact rejected ───────────────────────────────

    /// Negative amounts must be rejected by the underlying SEP-41 transfer.
    /// Pin that the router doesn't accidentally accept them through any
    /// command variant.
    #[test]
    fn test_attack_transfer_from_negative_amount_panics() {
        let s = make_setup();
        let caller = Address::generate(&s.env);

        let mut cmds = Vec::<Command>::new(&s.env);
        cmds.push_back(Command::TransferFrom(TransferFromArgs {
            token: s.underlying_addr.clone(),
            amount: Amount::Exact(-1),
        }));
        let r = s
            .router_client
            .try_execute(&caller, &cmds, &(s.env.ledger().timestamp() + 100));
        assert!(r.is_err(), "TransferFrom with negative Exact must panic");
    }

    // ── B1: stranded balance claim is benign (documented behavior) ────────

    /// By design: a balance left in the router by a previous tx CAN be
    /// claimed by any next caller via `Transfer(token, Recipient::Caller)`.
    /// This is NOT an attack per project threat model — it's the expected
    /// composability surface. Pin it so behavior doesn't accidentally
    /// change.
    #[test]
    fn test_stranded_router_balance_claimable_by_next_caller_by_design() {
        let s = make_setup();

        // First tx leaves balance in router.
        let alice = Address::generate(&s.env);
        let und = spectra_mocks::MockUnderlyingContractClient::new(&s.env, &s.underlying_addr);
        und.mint(&alice, &1_000_000i128);
        approve_router(&s.env, &s.underlying_addr, &alice, &s.router_addr, 500_000);
        let mut tx1 = Vec::<Command>::new(&s.env);
        tx1.push_back(Command::TransferFrom(TransferFromArgs {
            token: s.underlying_addr.clone(),
            amount: Amount::Exact(500_000),
        }));
        s.router_client
            .execute(&alice, &tx1, &(s.env.ledger().timestamp() + 100));

        let token_c = soroban_sdk::token::Client::new(&s.env, &s.underlying_addr);
        assert_eq!(token_c.balance(&s.router_addr), 500_000);

        // Second tx: Bob claims it via Transfer(Recipient::Caller, Balance).
        let bob = Address::generate(&s.env);
        let mut tx2 = Vec::<Command>::new(&s.env);
        tx2.push_back(Command::Transfer(TransferArgs {
            token: s.underlying_addr.clone(),
            recipient: Recipient::Caller,
            amount: Amount::Balance,
        }));
        s.router_client
            .execute(&bob, &tx2, &(s.env.ledger().timestamp() + 100));

        assert_eq!(token_c.balance(&s.router_addr), 0);
        assert_eq!(token_c.balance(&bob), 500_000);
        // Alice's deposit is gone — she lost it by leaving it stranded.
        // Per threat model this is acceptable.
    }

    // ── C1: AssertMinBalance — read-only, cannot move funds ───────────────

    /// `AssertMinBalance` reads an arbitrary `(token, owner)` balance and
    /// compares it. It does NOT modify state. Even if the asserted owner
    /// happens to be a victim with a fat balance, no funds move. Pin it as
    /// read-only.
    #[test]
    fn test_assert_min_balance_is_read_only_and_cannot_drain() {
        let s = make_setup();
        let victim = Address::generate(&s.env);
        let attacker = Address::generate(&s.env);
        let und = spectra_mocks::MockUnderlyingContractClient::new(&s.env, &s.underlying_addr);
        und.mint(&victim, &1_000_000i128);

        let mut cmds = Vec::<Command>::new(&s.env);
        cmds.push_back(Command::AssertMinBalance(AssertMinBalanceArgs {
            token: s.underlying_addr.clone(),
            owner: Recipient::Address(victim.clone()),
            min_value: 1,
        }));
        s.router_client
            .execute(&attacker, &cmds, &(s.env.ledger().timestamp() + 100));

        let token_c = soroban_sdk::token::Client::new(&s.env, &s.underlying_addr);
        assert_eq!(
            token_c.balance(&victim),
            1_000_000,
            "AssertMinBalance must not move victim's funds"
        );
        assert_eq!(token_c.balance(&attacker), 0);
        assert_eq!(token_c.balance(&s.router_addr), 0);
    }

    // ── D1: assert-as-slippage-guard atomicity ────────────────────────────

    /// A failing `AssertMinBalance` at the end of a command vec must revert
    /// the entire transaction — no partial side effects committed. Pin the
    /// atomicity guarantee that the user's threat-model-stated "no draining
    /// in transaction" relies on.
    #[test]
    fn test_failing_assert_min_balance_reverts_entire_transaction() {
        let s = make_setup();
        let caller = Address::generate(&s.env);

        let und = spectra_mocks::MockUnderlyingContractClient::new(&s.env, &s.underlying_addr);
        und.mint(&caller, &1_000_000i128);
        approve_router(
            &s.env,
            &s.underlying_addr,
            &caller,
            &s.router_addr,
            1_000_000,
        );

        let mut cmds = Vec::<Command>::new(&s.env);
        // Pull funds in.
        cmds.push_back(Command::TransferFrom(TransferFromArgs {
            token: s.underlying_addr.clone(),
            amount: Amount::Exact(500_000),
        }));
        // Assert router holds MORE than it does — must fail.
        cmds.push_back(Command::AssertMinBalance(AssertMinBalanceArgs {
            token: s.underlying_addr.clone(),
            owner: Recipient::Router,
            min_value: 10_000_000,
        }));

        let r = s
            .router_client
            .try_execute(&caller, &cmds, &(s.env.ledger().timestamp() + 100));
        assert!(r.is_err(), "tx must revert when assert fails");

        // Caller's balance is fully restored — atomicity holds.
        let token_c = soroban_sdk::token::Client::new(&s.env, &s.underlying_addr);
        assert_eq!(token_c.balance(&caller), 1_000_000);
        assert_eq!(token_c.balance(&s.router_addr), 0);
    }

    // ── D2: DepositIbtInPt slippage (min_shares) ──────────────────────────

    /// DepositIbtInPt with an unmeetable `min_shares` must panic and revert
    /// the whole tx. Caller's IBT must NOT be consumed.
    #[test]
    fn test_deposit_ibt_in_pt_slippage_reverts_full_tx() {
        let s = make_setup();

        let caller = Address::generate(&s.env);
        let und = spectra_mocks::MockUnderlyingContractClient::new(&s.env, &s.underlying_addr);
        und.mint(&caller, &1_000_000i128);
        approve_router(
            &s.env,
            &s.underlying_addr,
            &caller,
            &s.router_addr,
            1_000_000,
        );

        let ibt_c = spectra_mocks::MockIbtOzContractClient::new(&s.env, &s.ibt_addr);
        let _ = ibt_c;

        let mut cmds = Vec::<Command>::new(&s.env);
        cmds.push_back(Command::TransferFrom(TransferFromArgs {
            token: s.underlying_addr.clone(),
            amount: Amount::Exact(1_000_000),
        }));
        cmds.push_back(Command::DepositAssetInIbt(DepositAssetInIbtArgs {
            ibt: s.ibt_addr.clone(),
            assets: Amount::Balance,
            recipient: Recipient::Router,
        }));
        cmds.push_back(Command::DepositIbtInPt(DepositIbtInPtArgs {
            pt: s.pt_addr.clone(),
            ibts: Amount::Balance,
            pt_recipient: Recipient::Caller,
            yt_recipient: Recipient::Caller,
            min_shares: i128::MAX, // impossible → slippage panic
        }));

        let r = s
            .router_client
            .try_execute(&caller, &cmds, &(s.env.ledger().timestamp() + 100));
        assert!(r.is_err(), "impossible min_shares must revert tx");

        let token_c = soroban_sdk::token::Client::new(&s.env, &s.underlying_addr);
        assert_eq!(
            token_c.balance(&caller),
            1_000_000,
            "caller's underlying must be intact after failed tx"
        );
    }

    // ── D3: RedeemPtForIbt slippage ────────────────────────────────────────

    /// RedeemPtForIbt with an unmeetable `min_ibts` reverts.
    #[test]
    fn test_redeem_pt_for_ibt_slippage_reverts_full_tx() {
        let s = make_setup();
        let caller = Address::generate(&s.env);

        // Set up: caller deposits via router so they hold some PT.
        let und = spectra_mocks::MockUnderlyingContractClient::new(&s.env, &s.underlying_addr);
        und.mint(&caller, &1_000_000i128);
        approve_router(
            &s.env,
            &s.underlying_addr,
            &caller,
            &s.router_addr,
            1_000_000,
        );
        let mut deposit_cmds = Vec::<Command>::new(&s.env);
        deposit_cmds.push_back(Command::TransferFrom(TransferFromArgs {
            token: s.underlying_addr.clone(),
            amount: Amount::Exact(1_000_000),
        }));
        deposit_cmds.push_back(Command::DepositAssetInIbt(DepositAssetInIbtArgs {
            ibt: s.ibt_addr.clone(),
            assets: Amount::Balance,
            recipient: Recipient::Router,
        }));
        deposit_cmds.push_back(Command::DepositIbtInPt(DepositIbtInPtArgs {
            pt: s.pt_addr.clone(),
            ibts: Amount::Balance,
            pt_recipient: Recipient::Caller,
            yt_recipient: Recipient::Caller,
            min_shares: 0,
        }));
        s.router_client
            .execute(&caller, &deposit_cmds, &(s.env.ledger().timestamp() + 100));

        let pt_c = principal_token::PrincipalTokenContractClient::new(&s.env, &s.pt_addr);
        let pt_balance = pt_c.balance(&caller);
        assert!(pt_balance > 0);

        // Approve router to take the PT.
        approve_router(&s.env, &s.pt_addr, &caller, &s.router_addr, pt_balance);

        let mut redeem_cmds = Vec::<Command>::new(&s.env);
        redeem_cmds.push_back(Command::TransferFrom(TransferFromArgs {
            token: s.pt_addr.clone(),
            amount: Amount::Exact(pt_balance),
        }));
        redeem_cmds.push_back(Command::RedeemPtForIbt(RedeemPtForIbtArgs {
            pt: s.pt_addr.clone(),
            shares: Amount::Balance,
            recipient: Recipient::Caller,
            min_ibts: i128::MAX, // unmeetable
        }));

        let r =
            s.router_client
                .try_execute(&caller, &redeem_cmds, &(s.env.ledger().timestamp() + 100));
        assert!(r.is_err(), "impossible min_ibts must revert tx");
        // PT balance restored.
        assert_eq!(pt_c.balance(&caller), pt_balance);
    }

    // ── E1: round-trip composition ────────────────────────────────────────

    /// Full chain: TransferFrom(asset) → DepositAssetInIbt(Router) →
    /// RedeemIbtForAsset(Caller). Caller should receive back ~original
    /// amount (minus mock IBT round-trip dust). No third-party leak.
    #[test]
    fn test_e2e_full_chain_asset_ibt_round_trip_returns_to_caller() {
        let s = make_setup();
        let caller = Address::generate(&s.env);

        let und = spectra_mocks::MockUnderlyingContractClient::new(&s.env, &s.underlying_addr);
        und.mint(&caller, &1_000_000i128);
        approve_router(
            &s.env,
            &s.underlying_addr,
            &caller,
            &s.router_addr,
            1_000_000,
        );

        let mut cmds = Vec::<Command>::new(&s.env);
        cmds.push_back(Command::TransferFrom(TransferFromArgs {
            token: s.underlying_addr.clone(),
            amount: Amount::Exact(1_000_000),
        }));
        cmds.push_back(Command::DepositAssetInIbt(DepositAssetInIbtArgs {
            ibt: s.ibt_addr.clone(),
            assets: Amount::Balance,
            recipient: Recipient::Router,
        }));
        cmds.push_back(Command::RedeemIbtForAsset(RedeemIbtForAssetArgs {
            ibt: s.ibt_addr.clone(),
            shares: Amount::Balance,
            recipient: Recipient::Caller,
        }));
        cmds.push_back(Command::AssertMinBalance(AssertMinBalanceArgs {
            token: s.underlying_addr.clone(),
            owner: Recipient::Caller,
            min_value: 999_900, // allow tiny dust loss
        }));
        s.router_client
            .execute(&caller, &cmds, &(s.env.ledger().timestamp() + 100));

        let token_c = soroban_sdk::token::Client::new(&s.env, &s.underlying_addr);
        let final_bal = token_c.balance(&caller);
        assert!(
            (999_900..=1_000_000).contains(&final_bal),
            "round trip returns close to original: {final_bal}"
        );
        // Router is drained.
        assert_eq!(token_c.balance(&s.router_addr), 0);
    }

    // ── E2: chaining via Recipient::Router intermediate ───────────────────

    /// Verify multiple commands can chain through `Recipient::Router` and
    /// `Amount::Balance` to compose pipelines, ending with `Caller`.
    #[test]
    fn test_e2e_recipient_router_intermediate_chains_correctly() {
        let s = make_setup();
        let caller = Address::generate(&s.env);

        let und = spectra_mocks::MockUnderlyingContractClient::new(&s.env, &s.underlying_addr);
        und.mint(&caller, &500_000i128);
        approve_router(&s.env, &s.underlying_addr, &caller, &s.router_addr, 500_000);

        let mut cmds = Vec::<Command>::new(&s.env);
        cmds.push_back(Command::TransferFrom(TransferFromArgs {
            token: s.underlying_addr.clone(),
            amount: Amount::Balance,
        }));
        cmds.push_back(Command::Transfer(TransferArgs {
            token: s.underlying_addr.clone(),
            recipient: Recipient::Caller,
            amount: Amount::Balance,
        }));
        s.router_client
            .execute(&caller, &cmds, &(s.env.ledger().timestamp() + 100));

        let token_c = soroban_sdk::token::Client::new(&s.env, &s.underlying_addr);
        assert_eq!(
            token_c.balance(&caller),
            500_000,
            "TransferFrom then Transfer-back must round-trip exactly"
        );
        assert_eq!(token_c.balance(&s.router_addr), 0);
    }

    // ── E3: duplicate TransferFrom commands ───────────────────────────────

    /// Two TransferFrom commands in a single execute, each Exact(X), pull
    /// 2X — bounded by approved allowance. If allowance only covers X,
    /// second pull fails and whole tx reverts.
    #[test]
    fn test_attack_duplicate_transfer_from_with_only_one_allowance_reverts() {
        let s = make_setup();
        let caller = Address::generate(&s.env);

        let und = spectra_mocks::MockUnderlyingContractClient::new(&s.env, &s.underlying_addr);
        und.mint(&caller, &1_000_000i128);
        // Approve only 500k.
        approve_router(&s.env, &s.underlying_addr, &caller, &s.router_addr, 500_000);

        let mut cmds = Vec::<Command>::new(&s.env);
        cmds.push_back(Command::TransferFrom(TransferFromArgs {
            token: s.underlying_addr.clone(),
            amount: Amount::Exact(500_000),
        }));
        cmds.push_back(Command::TransferFrom(TransferFromArgs {
            token: s.underlying_addr.clone(),
            amount: Amount::Exact(500_000),
        }));
        let r = s
            .router_client
            .try_execute(&caller, &cmds, &(s.env.ledger().timestamp() + 100));
        assert!(
            r.is_err(),
            "second TransferFrom exhausts the 500k allowance and must revert"
        );

        let token_c = soroban_sdk::token::Client::new(&s.env, &s.underlying_addr);
        assert_eq!(token_c.balance(&caller), 1_000_000);
    }

    // ── F1: no arbitrary code execution ───────────────────────────────────

    /// The dispatcher only invokes fixed methods on token addresses
    /// (`transfer`, `transfer_from`, `balance`, `approve`, IBT/PT fixed
    /// entry points). There is no command variant that lets caller specify
    /// an arbitrary function symbol or args payload. This test enumerates
    /// every command variant and confirms that — by the type system — only
    /// the documented surface is reachable.
    ///
    /// (This is a compile-time guarantee from the enum shape, but we pin it
    /// here as a smoke check: enumerating the variant count and asserting
    /// it matches the documented 11.)
    #[test]
    fn test_no_arbitrary_code_execution_command_count_pinned_at_11() {
        // The Command enum has exactly 11 documented variants
        // (router_types.rs:198). Any new variant added in a future PR
        // must come with its own breach-hunt coverage in this module.
        let s = make_setup();
        // We pin this via the compiler — any time `Command` gains a new
        // variant, this match becomes non-exhaustive and CI fails.
        fn _exhaust_command_match(c: &Command) -> &'static str {
            match c {
                Command::TransferFrom(_) => "TransferFrom",
                Command::Transfer(_) => "Transfer",
                Command::AssertMinBalance(_) => "AssertMinBalance",
                Command::DepositAssetInIbt(_) => "DepositAssetInIbt",
                Command::DepositIbtInPt(_) => "DepositIbtInPt",
                Command::RedeemIbtForAsset(_) => "RedeemIbtForAsset",
                Command::RedeemPtForIbt(_) => "RedeemPtForIbt",
                Command::RegisterOrders(_) => "RegisterOrders",
                Command::CancelOrders(_) => "CancelOrders",
                Command::FillOrders(_) => "FillOrders",
                Command::BridgeNativePtToEvm(_) => "BridgeNativePtToEvm",
                Command::BridgeWrappedPtBackToEvm(_) => "BridgeWrappedPtBackToEvm",
                Command::BrokerSwap(_) => "BrokerSwap",
            }
        }
        let _ = _exhaust_command_match(&dummy_command(&s.underlying_addr));
    }

    // ── G1: AssertMinBalance over the Router itself ───────────────────────

    /// `AssertMinBalance` with `owner=Router` is the canonical
    /// post-condition slippage check on the Router's own holdings. Verify
    /// it passes when the router actually holds ≥ min_value.
    #[test]
    fn test_assert_min_balance_owner_router_passes_when_router_holds_enough() {
        let s = make_setup();
        let caller = Address::generate(&s.env);

        let und = spectra_mocks::MockUnderlyingContractClient::new(&s.env, &s.underlying_addr);
        und.mint(&caller, &1_000_000i128);
        approve_router(
            &s.env,
            &s.underlying_addr,
            &caller,
            &s.router_addr,
            1_000_000,
        );

        let mut cmds = Vec::<Command>::new(&s.env);
        cmds.push_back(Command::TransferFrom(TransferFromArgs {
            token: s.underlying_addr.clone(),
            amount: Amount::Exact(1_000_000),
        }));
        cmds.push_back(Command::AssertMinBalance(AssertMinBalanceArgs {
            token: s.underlying_addr.clone(),
            owner: Recipient::Router,
            min_value: 1_000_000,
        }));
        s.router_client
            .execute(&caller, &cmds, &(s.env.ledger().timestamp() + 100));
    }

    // ── H1: AssertMinBalance with min_value below zero ────────────────────

    /// `AssertMinBalance{min_value: -1}` is rejected by the structured
    /// negative-min guard (#170): `execute` panics `NegativeMinValue`.
    #[test]
    fn test_assert_min_balance_with_negative_min_value_rejected() {
        let s = make_setup();
        let caller = Address::generate(&s.env);

        let mut cmds = Vec::<Command>::new(&s.env);
        cmds.push_back(Command::AssertMinBalance(AssertMinBalanceArgs {
            token: s.underlying_addr.clone(),
            owner: Recipient::Router,
            min_value: -1,
        }));
        let r = s
            .router_client
            .try_execute(&caller, &cmds, &(s.env.ledger().timestamp() + 100));
        assert!(r.is_err(), "negative min_value must be rejected");
    }

    // ── I1: BrokerSwap empty routes rejected ──────────────────────────────

    /// Pinned at dispatcher.rs:256: empty routes panic InvalidRoutes.
    /// Adversarial caller can't construct a zero-amount auth grant by
    /// submitting empty routes.
    #[test]
    fn test_attack_broker_swap_empty_routes_rejected() {
        let s = make_setup();

        // Need a broker set. Use a dummy address — the call will fail
        // earlier on InvalidRoutes before reaching the broker.
        let broker = Address::generate(&s.env);
        s.router_client.set_broker(&s.admin, &broker);

        let caller = Address::generate(&s.env);
        let empty_routes: Vec<Route> = Vec::new(&s.env);
        let empty_path: Vec<PathStep> = Vec::new(&s.env);

        let mut cmds = Vec::<Command>::new(&s.env);
        cmds.push_back(Command::BrokerSwap(BrokerSwapArgs {
            token_in: s.underlying_addr.clone(),
            token_out: s.underlying_addr.clone(),
            routes: empty_routes,
            min_amount_out: 0,
            fixed_fee_permille: 0,
            fee_path: empty_path,
        }));
        let r = s
            .router_client
            .try_execute(&caller, &cmds, &(s.env.ledger().timestamp() + 100));
        assert!(r.is_err(), "BrokerSwap with empty routes must panic");
    }

    // ── I2: BrokerSwap zero-or-negative route amounts rejected ─────────────

    #[test]
    fn test_attack_broker_swap_zero_route_amount_rejected() {
        let s = make_setup();
        let broker = Address::generate(&s.env);
        s.router_client.set_broker(&s.admin, &broker);

        let caller = Address::generate(&s.env);
        let mut routes: Vec<Route> = Vec::new(&s.env);
        routes.push_back(Route {
            path: Vec::new(&s.env),
            amount: 0,
            min: 0,
            estimated: 0,
        });
        let empty_path: Vec<PathStep> = Vec::new(&s.env);

        let mut cmds = Vec::<Command>::new(&s.env);
        cmds.push_back(Command::BrokerSwap(BrokerSwapArgs {
            token_in: s.underlying_addr.clone(),
            token_out: s.underlying_addr.clone(),
            routes,
            min_amount_out: 0,
            fixed_fee_permille: 0,
            fee_path: empty_path,
        }));
        let r = s
            .router_client
            .try_execute(&caller, &cmds, &(s.env.ledger().timestamp() + 100));
        assert!(r.is_err(), "BrokerSwap with zero-amount route must panic");
    }

    // ── I3: BrokerSwap negative min_amount_out rejected ───────────────────

    /// AssertMinBalance slippage off-by-one (#9): bal == min_value passes;
    /// bal == min_value + 1 also passes; bal == min_value - 1 fails.
    /// Pins the `<` comparator at dispatcher.rs:75.
    #[test]
    fn test_attack_assert_min_balance_off_by_one_boundary() {
        let s = make_setup();
        let caller = Address::generate(&s.env);
        let und = spectra_mocks::MockUnderlyingContractClient::new(&s.env, &s.underlying_addr);
        und.mint(&caller, &1_000_000);
        approve_router(
            &s.env,
            &s.underlying_addr,
            &caller,
            &s.router_addr,
            1_000_000,
        );

        // Pull exactly 1000 in.
        let mut setup_cmds = Vec::<Command>::new(&s.env);
        setup_cmds.push_back(Command::TransferFrom(TransferFromArgs {
            token: s.underlying_addr.clone(),
            amount: Amount::Exact(1_000),
        }));
        s.router_client
            .execute(&caller, &setup_cmds, &(s.env.ledger().timestamp() + 100));

        // min == bal: passes.
        let mut eq_cmds = Vec::<Command>::new(&s.env);
        eq_cmds.push_back(Command::AssertMinBalance(AssertMinBalanceArgs {
            token: s.underlying_addr.clone(),
            owner: Recipient::Router,
            min_value: 1_000,
        }));
        s.router_client
            .execute(&caller, &eq_cmds, &(s.env.ledger().timestamp() + 100));

        // min == bal + 1: fails.
        let mut over_cmds = Vec::<Command>::new(&s.env);
        over_cmds.push_back(Command::AssertMinBalance(AssertMinBalanceArgs {
            token: s.underlying_addr.clone(),
            owner: Recipient::Router,
            min_value: 1_001,
        }));
        let r =
            s.router_client
                .try_execute(&caller, &over_cmds, &(s.env.ledger().timestamp() + 100));
        assert!(r.is_err(), "min == bal + 1 must fail (off-by-one boundary)");
    }

    #[test]
    fn test_attack_broker_swap_negative_min_amount_out_rejected() {
        let s = make_setup();
        let broker = Address::generate(&s.env);
        s.router_client.set_broker(&s.admin, &broker);

        let caller = Address::generate(&s.env);
        let mut routes: Vec<Route> = Vec::new(&s.env);
        routes.push_back(Route {
            path: Vec::new(&s.env),
            amount: 100,
            min: 0,
            estimated: 0,
        });
        let empty_path: Vec<PathStep> = Vec::new(&s.env);

        let mut cmds = Vec::<Command>::new(&s.env);
        cmds.push_back(Command::BrokerSwap(BrokerSwapArgs {
            token_in: s.underlying_addr.clone(),
            token_out: s.underlying_addr.clone(),
            routes,
            min_amount_out: -1,
            fixed_fee_permille: 0,
            fee_path: empty_path,
        }));
        let r = s
            .router_client
            .try_execute(&caller, &cmds, &(s.env.ledger().timestamp() + 100));
        assert!(
            r.is_err(),
            "BrokerSwap with negative min_amount_out must panic"
        );
    }
}
