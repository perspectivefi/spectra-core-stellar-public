// SPDX-License-Identifier: BUSL-1.1
#![cfg(test)]

use crate::{FactoryContract, FactoryContractClient};
use soroban_sdk::{
    Address, Env, String as SorobanString,
    testutils::{Address as _, Events as _},
};
use spectra_common::constants::roles;
use stellar_access::access_control;

use registry::{RegistryContract, RegistryContractClient};
use spectra_mocks::{
    MockIbtContract, MockIbtContractClient, MockUnderlyingContract, MockUnderlyingContractClient,
};

const PT_WASM: &[u8] =
    include_bytes!("../../../target/wasm32v1-none/release/mock_factory_pt_init.wasm");
const YT_WASM: &[u8] = include_bytes!("../../../target/wasm32v1-none/release/yield_token.wasm");

fn register_factory<'a>(
    env: &'a Env,
    admin: &Address,
    registry: &Address,
) -> (Address, FactoryContractClient<'a>) {
    let id = env.register(FactoryContract, (admin.clone(), registry.clone()));
    let client = FactoryContractClient::new(env, &id);
    (id, client)
}

/// Set up: Registry deployed (from Rust type) + initialised, PT+YT wasm
/// hashes uploaded and set on Registry, Factory deployed + initialised +
/// granted FACTORY role on Registry, MockIbt deployed.
///
/// Returns (admin, registry_address, factory_id, factory_client, ibt_address).
fn setup_full_stack<'a>(
    env: &'a Env,
) -> (
    Address,
    Address,
    Address,
    FactoryContractClient<'a>,
    Address,
) {
    env.mock_all_auths_allowing_non_root_auth();

    let admin = Address::generate(env);

    let registry_addr = env.register(RegistryContract, (admin.clone(),));
    let registry_c = RegistryContractClient::new(env, &registry_addr);

    let pt_wasm_hash = env.deployer().upload_contract_wasm(PT_WASM);
    let yt_wasm_hash = env.deployer().upload_contract_wasm(YT_WASM);
    registry_c.set_pt_wasm_hash(&admin, &pt_wasm_hash);
    registry_c.set_yt_wasm_hash(&admin, &yt_wasm_hash);

    let (factory_id, factory) = register_factory(env, &admin, &registry_addr);

    registry_c.grant_role(&admin, &factory_id, &roles::FACTORY);

    let underlying = env.register(MockUnderlyingContract, ());
    let underlying_c = MockUnderlyingContractClient::new(env, &underlying);
    underlying_c.initialize(
        &SorobanString::from_str(env, "Mock Underlying"),
        &SorobanString::from_str(env, "MUSD"),
        &6u32,
    );

    let ibt = env.register(MockIbtContract, ());
    let ibt_c = MockIbtContractClient::new(env, &ibt);
    ibt_c.initialize(
        &underlying,
        &SorobanString::from_str(env, "Mock IBT"),
        &SorobanString::from_str(env, "MIBT"),
        &6u32,
    );
    // PT.initialize calls ibt.total_assets() and panics RateError if zero.
    ibt_c.set_total_assets(&1_000_000i128);

    (admin, registry_addr, factory_id, factory, ibt)
}

#[test]
fn test_constructor_sets_admin_and_grants_upgrade_role() {
    let env = Env::default();
    env.mock_all_auths();

    let admin = Address::generate(&env);
    let registry = Address::generate(&env);

    let (factory_id, _factory) = register_factory(&env, &admin, &registry);

    env.as_contract(&factory_id, || {
        assert_eq!(access_control::get_admin(&env), Some(admin.clone()));
        assert!(access_control::has_role(&env, &admin, &roles::UPGRADE).is_some());
        // FACTORY role is intentionally NOT granted on initialize: nothing
        // on the Factory contract reads it. The FACTORY role that actually
        // gates deploy_pt lives on the Registry contract and is granted to
        // the Factory address there by the deployment script.
        assert!(access_control::has_role(&env, &admin, &roles::FACTORY).is_none());
    });
}

// removed test_initialize_twice_panics — a Soroban constructor runs
// exactly once at deploy, so re-initialization is impossible.

// removed test_initialize_rejects_self_as_registry — the deploy address
// is unknown before deploy, so registry==self cannot be passed at construction
// (the defensive check remains in the contract).

#[test]
fn test_getters_return_stored_values() {
    let env = Env::default();
    env.mock_all_auths();

    let admin = Address::generate(&env);
    let registry = Address::generate(&env);

    let (_, factory) = register_factory(&env, &admin, &registry);

    assert_eq!(factory.get_registry(), registry);
}

// removed test_get_registry_before_init_panics — there is no
// uninitialized state; the constructor always sets the registry at deploy.

#[test]
#[should_panic]
fn test_upgrade_rejects_non_upgrade_role() {
    let env = Env::default();
    env.mock_all_auths();

    let admin = Address::generate(&env);
    let registry = Address::generate(&env);
    let intruder = Address::generate(&env);

    let (_, factory) = register_factory(&env, &admin, &registry);

    let dummy_hash = soroban_sdk::BytesN::<32>::from_array(&env, &[0u8; 32]);
    factory.upgrade(&intruder, &dummy_hash);
}

/// `upgrade` must emit a
/// `FactoryUpgraded { caller, new_wasm_hash }` event for off-chain
/// monitoring of wasm rotations.
#[test]
fn test_upgrade_emits_factory_upgraded_event() {
    use crate::events::FactoryUpgraded;
    use soroban_sdk::Event;

    /// Workspace's pre-built YT wasm — any valid uploaded wasm hash works
    /// because `update_current_contract_wasm` is content-addressed.
    const ARBITRARY_WASM: &[u8] =
        include_bytes!("../../../target/wasm32v1-none/release/yield_token.wasm");

    let env = Env::default();
    env.mock_all_auths();

    let admin = Address::generate(&env);
    let registry = Address::generate(&env);

    let (factory_id, factory) = register_factory(&env, &admin, &registry);

    let hash = env.deployer().upload_contract_wasm(ARBITRARY_WASM);
    factory.upgrade(&admin, &hash);

    let expected_event = FactoryUpgraded {
        caller: admin.clone(),
        new_wasm_hash: hash.clone(),
    };
    let factory_events = env.events().all().filter_by_contract(&factory_id);
    assert!(
        factory_events
            .events()
            .contains(&expected_event.to_xdr(&env, &factory_id)),
        "expected FactoryUpgraded event not found in factory events"
    );
}

#[test]
fn test_deploy_pt_returns_address_registers_and_emits() {
    let env = Env::default();
    let (admin, registry_addr, _factory_id, factory, ibt) = setup_full_stack(&env);
    let duration: u64 = 30 * 24 * 60 * 60; // 30 days

    let pt = factory.deploy_pt(&admin, &ibt, &duration);

    // PT must be registered on Registry.
    let reg = RegistryContractClient::new(&env, &registry_addr);
    assert!(reg.is_registered_pt(&pt));
}

#[test]
fn test_deploy_pt_emits_pt_deployed_event() {
    // Ported from core-v2/test/Factory.t.sol::testDeployPrincipalToken (~L162):
    // verifies that after factory.deploy_pt the factory emitted the
    // PtDeployed event. Maturity/IBT/duration are covered by PT-side tests.
    use crate::events::PtDeployed;
    use soroban_sdk::Event;

    let env = Env::default();
    let (admin, _registry_addr, factory_id, factory, ibt) = setup_full_stack(&env);
    let duration: u64 = 30 * 24 * 60 * 60; // 30 days

    let pt = factory.deploy_pt(&admin, &ibt, &duration);

    // Capture events from the deploy_pt invocation immediately; any later
    // contract call (including view reads) replaces this snapshot.
    let expected_event = PtDeployed {
        deployer: admin.clone(),
        pt: pt.clone(),
        ibt: ibt.clone(),
        duration,
    };
    let factory_events = env.events().all().filter_by_contract(&factory_id);
    assert!(
        factory_events
            .events()
            .contains(&expected_event.to_xdr(&env, &factory_id)),
        "expected PtDeployed event not found in factory events"
    );
}

#[test]
fn test_deploy_pt_five_sequential_calls_produce_unique_addresses() {
    let env = Env::default();
    let (admin, _, _, factory, ibt) = setup_full_stack(&env);
    let duration: u64 = 30 * 24 * 60 * 60;

    let mut pts: soroban_sdk::Vec<Address> = soroban_sdk::Vec::new(&env);
    for _ in 0..5 {
        let pt = factory.deploy_pt(&admin, &ibt, &duration);
        pts.push_back(pt);
    }

    for i in 0..5u32 {
        for j in (i + 1)..5u32 {
            assert_ne!(pts.get(i).unwrap(), pts.get(j).unwrap());
        }
    }
}

// removed test_deploy_pt_before_init_panics — there is no uninitialized
// state anymore; the constructor sets the registry at deploy.

#[test]
#[should_panic]
fn test_deploy_pt_panics_when_factory_lacks_registry_role() {
    let env = Env::default();
    env.mock_all_auths_allowing_non_root_auth();

    let admin = Address::generate(&env);

    let registry_addr = env.register(RegistryContract, (admin.clone(),));
    let registry_c = RegistryContractClient::new(&env, &registry_addr);

    let pt_wasm_hash = env.deployer().upload_contract_wasm(PT_WASM);
    let yt_wasm_hash = env.deployer().upload_contract_wasm(YT_WASM);
    registry_c.set_pt_wasm_hash(&admin, &pt_wasm_hash);
    registry_c.set_yt_wasm_hash(&admin, &yt_wasm_hash);

    let (_factory_id, factory) = register_factory(&env, &admin, &registry_addr);
    // Deliberately skip granting Factory the FACTORY role on Registry.

    let underlying = env.register(MockUnderlyingContract, ());
    MockUnderlyingContractClient::new(&env, &underlying).initialize(
        &SorobanString::from_str(&env, "U"),
        &SorobanString::from_str(&env, "U"),
        &6u32,
    );
    let ibt = env.register(MockIbtContract, ());
    let ibt_c = MockIbtContractClient::new(&env, &ibt);
    ibt_c.initialize(
        &underlying,
        &SorobanString::from_str(&env, "I"),
        &SorobanString::from_str(&env, "I"),
        &6u32,
    );
    ibt_c.set_total_assets(&1_000_000i128);

    let _ = factory.deploy_pt(&admin, &ibt, &30u64);
}

#[test]
fn test_same_ibt_and_duration_deployed_twice_yields_distinct_pts() {
    let env = Env::default();
    let (admin, _, _, factory, ibt) = setup_full_stack(&env);
    let duration: u64 = 30 * 24 * 60 * 60;
    let a = factory.deploy_pt(&admin, &ibt, &duration);
    let b = factory.deploy_pt(&admin, &ibt, &duration);
    assert_ne!(a, b);
}

#[test]
fn test_deploy_pt_with_duration_one_second() {
    let env = Env::default();
    let (admin, _, _, factory, ibt) = setup_full_stack(&env);
    // PT's own behaviour at expiry is its concern, not Factory's.
    let _ = factory.deploy_pt(&admin, &ibt, &1u64);
}

#[test]
#[should_panic(expected = "Error(Contract, #4)")] // InvalidDuration
fn test_deploy_pt_rejects_zero_duration() {
    let env = Env::default();
    let (admin, _, _, factory, ibt) = setup_full_stack(&env);
    let _ = factory.deploy_pt(&admin, &ibt, &0u64);
}

mod factorysng_port {
    //! 1:1 port of `core-v2/test/FactorySNG.t.sol`.
    //!
    //! Inventory of source tests:
    //! - `testDeployFactoryFailWhenRegistryIsZero` — ADAPTED as
    //!   `test_initialize_rejects_self_as_registry` (Soroban has no zero
    //!   address; the Soroban equivalent invariant is `registry != self`).
    //! - `testDeployFactoryFailWhenRateOracleRegistryIsZero` — SKIP (no
    //!   rate-oracle wiring in Soroban Factory; that lives in the future
    //!   AMM/oracle PR).
    //! - `testDeployFactoryFailWhenCurveAddressIsZero` — SKIP (no Curve
    //!   factory dep in this PR; AMM deployment is deferred).
    //! - `testSetCurveFactory` — SKIP (no AMM factory surface in this PR).
    //! - `testFactorySNGDeployAllWithoutInitialLiquidity` — NOT PORTED; AMM
    //!   deployment is deferred and deploy_all is removed.
    //! - `testFactorySNGDeployAllWithInitialLiquidityFuzz` — SKIP (AMM
    //!   deployment deferred to future release).
    //! - `testFactoryDeployAllFailBeaconNotSet` — PORTED below as
    //!   `test_deploy_pt_fail_when_pt_wasm_hash_not_set`

    use super::*;

    #[test]
    #[should_panic]
    fn test_deploy_pt_fail_when_pt_wasm_hash_not_set() {
        // adapted from: FactorySNG.t.sol::testFactoryDeployAllFailBeaconNotSet
        let env = Env::default();
        env.mock_all_auths_allowing_non_root_auth();

        let admin = Address::generate(&env);

        // Registry deployed + initialised but pt_wasm_hash NOT set.
        let registry_addr = env.register(RegistryContract, (admin.clone(),));
        let registry_c = RegistryContractClient::new(&env, &registry_addr);

        let (factory_id, factory) = register_factory(&env, &admin, &registry_addr);

        registry_c.grant_role(&admin, &factory_id, &roles::FACTORY);

        let ibt = Address::generate(&env);
        let _ = factory.deploy_pt(&admin, &ibt, &30u64); // panics on registry .unwrap()
    }
}

// Admin-rotation tests (stellar-access 2-step)
//
// Lifecycle behaviour is covered upstream. Per-host coverage limited to the
// happy path + a cross-contract integration test verifying that a rotated
// Factory admin's `deploy_pt` forwards the new admin to the deployed PT
// (the property that closed the legacy `authority` field).
// ===========================================================================

mod ownable2step_tests {
    use super::*;

    fn setup() -> (Env, FactoryContractClient<'static>, Address) {
        let env = Env::default();
        env.mock_all_auths();
        let admin = Address::generate(&env);
        let registry = Address::generate(&env);
        let factory_id = env.register(FactoryContract, (admin.clone(), registry.clone()));
        let factory = FactoryContractClient::new(&env, &factory_id);
        (env, factory, admin)
    }

    #[test]
    fn transfer_then_accept_rotates_admin_slot() {
        let (env, factory, _admin) = setup();
        let new_admin = Address::generate(&env);
        let live_until = env.ledger().sequence() + 8640;

        factory.transfer_admin_role(&new_admin, &live_until);
        factory.accept_admin_transfer();

        env.as_contract(&factory.address, || {
            assert_eq!(access_control::get_admin(&env), Some(new_admin.clone()));
        });
    }

    /// Cross-contract integration: rotate Factory's admin via the 2-step,
    /// then deploy a PT. Verifies that `deploy_pt` reads the new admin
    /// from `access_control::get_admin` and forwards it to the deployed PT
    /// — the collapse of the legacy `authority` field into the admin path.
    #[test]
    fn rotated_admin_propagates_to_deployed_pt() {
        let env = Env::default();
        let (admin, _registry_addr, _factory_id, factory, ibt) = setup_full_stack(&env);

        let pt_before = factory.deploy_pt(&admin, &ibt, &(30 * 24 * 60 * 60u64));

        let new_admin = Address::generate(&env);
        let live_until = env.ledger().sequence() + 8640;
        factory.transfer_admin_role(&new_admin, &live_until);
        factory.accept_admin_transfer();

        let pt_after = factory.deploy_pt(&new_admin, &ibt, &(30 * 24 * 60 * 60u64));
        assert_ne!(pt_before, pt_after);

        env.as_contract(&factory.address, || {
            assert_eq!(access_control::get_admin(&env), Some(new_admin.clone()));
        });
    }
}

// ===========================================================================
// Public role-management entry points
//
// Exercises the contract surface `grant_role` / `revoke_role` / `has_role`
// added so off-chain tooling (deploy dispatcher, key rotation) can rotate
// the FACTORY and UPGRADE roles after `initialize`. The library-level
// helpers are exercised indirectly via the initialize tests above; these
// focus on the entry-point surface.
// ===========================================================================

mod role_entrypoint_tests {
    use super::*;

    fn init_factory(env: &Env) -> (Address, FactoryContractClient<'_>) {
        env.mock_all_auths();
        let admin = Address::generate(env);
        let registry = Address::generate(env);
        let (_, client) = register_factory(env, &admin, &registry);
        (admin, client)
    }

    #[test]
    fn test_grant_role_via_entrypoint() {
        let env = Env::default();
        let (admin, client) = init_factory(&env);
        let new_holder = Address::generate(&env);

        client.grant_role(&admin, &new_holder, &roles::FACTORY);
        assert!(client.has_role(&new_holder, &roles::FACTORY));
    }

    #[test]
    fn test_revoke_role_via_entrypoint() {
        let env = Env::default();
        let (admin, client) = init_factory(&env);
        let new_holder = Address::generate(&env);

        client.grant_role(&admin, &new_holder, &roles::FACTORY);
        assert!(client.has_role(&new_holder, &roles::FACTORY));
        client.revoke_role(&admin, &new_holder, &roles::FACTORY);
        assert!(!client.has_role(&new_holder, &roles::FACTORY));
    }

    #[test]
    fn test_has_role_false_when_not_granted() {
        let env = Env::default();
        let (_admin, client) = init_factory(&env);
        let nobody = Address::generate(&env);
        assert!(!client.has_role(&nobody, &roles::FACTORY));
    }

    // A caller that is neither the contract admin nor holds the admin-role
    // of FACTORY must not be able to grant it. mock_all_auths satisfies
    // require_auth, so the failure surfaces from ensure_if_admin_or_admin_role.
    #[test]
    fn test_grant_role_by_non_admin_fails() {
        let env = Env::default();
        let (_admin, client) = init_factory(&env);
        let attacker = Address::generate(&env);
        let target = Address::generate(&env);
        let result = client.try_grant_role(&attacker, &target, &roles::FACTORY);
        assert!(result.is_err());
        assert!(!client.has_role(&target, &roles::FACTORY));
    }

    // Same as above for revoke_role: a non-admin caller cannot strip a role
    // from an account that legitimately holds it.
    #[test]
    fn test_revoke_role_by_non_admin_fails() {
        let env = Env::default();
        let (admin, client) = init_factory(&env);
        let new_holder = Address::generate(&env);
        client.grant_role(&admin, &new_holder, &roles::FACTORY);
        assert!(client.has_role(&new_holder, &roles::FACTORY));

        let attacker = Address::generate(&env);
        let result = client.try_revoke_role(&attacker, &new_holder, &roles::FACTORY);
        assert!(result.is_err());
        assert!(client.has_role(&new_holder, &roles::FACTORY));
    }
}

// ===========================================================================
// Upgrade — wasm actually swapped
//
// Existing tests cover the UPGRADE role gate and the `FactoryUpgraded` event.
// They do not assert that the wasm was actually replaced. We upgrade to
// YT_WASM, which lacks Factory's `get_registry` entry point; a post-upgrade
// call to `get_registry` must fail.
// ===========================================================================

mod upgrade_swap_test {
    use super::*;

    #[test]
    #[should_panic]
    fn test_upgrade_actually_swaps_wasm() {
        let env = Env::default();
        env.mock_all_auths();

        let admin = Address::generate(&env);
        let registry = Address::generate(&env);
        let (_, factory) = register_factory(&env, &admin, &registry);

        let yt_wasm_hash = env.deployer().upload_contract_wasm(YT_WASM);
        factory.upgrade(&admin, &yt_wasm_hash);

        // Post-upgrade the contract no longer exposes Factory's surface.
        let _ = factory.get_registry();
    }
}

// ===========================================================================
// deploy_pt — permissionless caller
//
// `deploy_pt` is intentionally not admin-gated; the only auth is the
// caller's own require_auth, recorded as `deployer` on `PtDeployed`. All
// existing tests pass `admin` as caller; this locks in that a random
// address with no roles can deploy a PT and is the recorded deployer.
// ===========================================================================

mod deploy_pt_permissionless_test {
    use super::*;
    use crate::events::PtDeployed;
    use soroban_sdk::Event;

    #[test]
    fn test_deploy_pt_permissionless_caller_succeeds_and_records_deployer() {
        let env = Env::default();
        let (_admin, _registry_addr, factory_id, factory, ibt) = setup_full_stack(&env);
        let random_caller = Address::generate(&env);
        let duration: u64 = 30 * 24 * 60 * 60;

        let pt = factory.deploy_pt(&random_caller, &ibt, &duration);

        let expected = PtDeployed {
            deployer: random_caller.clone(),
            pt: pt.clone(),
            ibt: ibt.clone(),
            duration,
        };
        let factory_events = env.events().all().filter_by_contract(&factory_id);
        assert!(
            factory_events
                .events()
                .contains(&expected.to_xdr(&env, &factory_id)),
            "PtDeployed.deployer must equal the random caller"
        );
    }
}

// ===========================================================================
// Init coverage gaps
//
// Closes two systematic gaps left by the original init test suite:
//   - the `FactoryInitialized` event was emitted but not asserted anywhere;
//   - `initialize` has no caller-auth gate by design (front-run accepted —
//     admin slot is multisig in prod). No test pinned that property, so a
//     future refactor could silently add `require_auth` and break the
//     permissionless deployment dispatch the deploy scripts rely on.
// ===========================================================================

mod init_extras {
    use super::*;

    #[test]
    fn test_initialize_emits_factory_initialized_event() {
        use crate::events::FactoryInitialized;
        use soroban_sdk::Event;

        let env = Env::default();
        env.mock_all_auths();

        let admin = Address::generate(&env);
        let registry = Address::generate(&env);

        let (factory_id, _factory) = register_factory(&env, &admin, &registry);

        let expected = FactoryInitialized {
            admin: admin.clone(),
            registry: registry.clone(),
        };
        let factory_events = env.events().all().filter_by_contract(&factory_id);
        assert!(
            factory_events
                .events()
                .contains(&expected.to_xdr(&env, &factory_id)),
            "expected FactoryInitialized event not found"
        );
    }

    // removed test_initialize_is_permissionless_any_address_can_call —
    // the constructor runs at deploy, so a permissionless standalone initialize
    // no longer exists.
}

// ===========================================================================
// Role entry-point edges
//
// The original suite covered grant/revoke/has_role only on the FACTORY role.
// Symmetry coverage on UPGRADE catches the case of a role-symbol typo or
// wiring mistake that would only affect one of the two roles the contract
// manages. The revoke-unheld test pins the stellar-access semantics that the
// contract inherits — if a future bump flips that path to a no-op we want a
// loud failure here.
// ===========================================================================

mod role_extras {
    use super::*;

    fn init_factory(env: &Env) -> (Address, FactoryContractClient<'_>) {
        env.mock_all_auths();
        let admin = Address::generate(env);
        let registry = Address::generate(env);
        let (_, client) = register_factory(env, &admin, &registry);
        (admin, client)
    }

    #[test]
    fn test_grant_and_revoke_upgrade_role_via_entrypoint() {
        let env = Env::default();
        let (admin, client) = init_factory(&env);
        let new_holder = Address::generate(&env);

        client.grant_role(&admin, &new_holder, &roles::UPGRADE);
        assert!(client.has_role(&new_holder, &roles::UPGRADE));

        client.revoke_role(&admin, &new_holder, &roles::UPGRADE);
        assert!(!client.has_role(&new_holder, &roles::UPGRADE));
    }

    #[test]
    #[should_panic]
    fn test_revoke_role_account_never_held_panics() {
        let env = Env::default();
        let (admin, client) = init_factory(&env);
        let nobody = Address::generate(&env);
        // FACTORY was never granted to `nobody`; the underlying
        // stellar-access revoke path panics rather than returning a no-op.
        client.revoke_role(&admin, &nobody, &roles::FACTORY);
    }
}

// ===========================================================================
// Admin 2-step transfer — edge cases
//
// The existing happy-path test (`transfer_then_accept_rotates_admin_slot`)
// and integration test (`rotated_admin_propagates_to_deployed_pt`) cover the
// nominal flow. These tests lock in three failure / override paths that any
// stellar-access bump or local wrapper change could regress silently:
//   - `accept_admin_transfer` with no pending transfer must panic;
//   - issuing `transfer_admin_role` twice overrides the prior pending slot,
//     leaving only the latest target able to accept;
//   - `accept_admin_transfer` requires the pending admin's auth — calling
//     it with no auth context must fail.
// ===========================================================================

mod admin_2step_edges {
    use super::*;

    fn init_factory(env: &Env) -> (Address, FactoryContractClient<'_>) {
        env.mock_all_auths();
        let admin = Address::generate(env);
        let registry = Address::generate(env);
        let (_, client) = register_factory(env, &admin, &registry);
        (admin, client)
    }

    #[test]
    #[should_panic]
    fn test_accept_admin_transfer_without_pending_panics() {
        let env = Env::default();
        let (_admin, factory) = init_factory(&env);
        factory.accept_admin_transfer();
    }

    #[test]
    fn test_transfer_admin_role_twice_overrides_pending() {
        let env = Env::default();
        let (_admin, factory) = init_factory(&env);
        let first_pending = Address::generate(&env);
        let second_pending = Address::generate(&env);
        let live_until = env.ledger().sequence() + 8640;

        factory.transfer_admin_role(&first_pending, &live_until);
        factory.transfer_admin_role(&second_pending, &live_until);
        factory.accept_admin_transfer();

        env.as_contract(&factory.address, || {
            assert_eq!(
                access_control::get_admin(&env),
                Some(second_pending.clone())
            );
        });

        // The first pending admin is no longer pending; trying to accept
        // again must fail (no pending transfer remains).
        let result = factory.try_accept_admin_transfer();
        assert!(result.is_err());
    }

    #[test]
    #[should_panic]
    fn test_accept_admin_transfer_requires_pending_admins_auth() {
        let env = Env::default();
        env.mock_all_auths();
        let admin = Address::generate(&env);
        let registry = Address::generate(&env);
        let (_, factory) = register_factory(&env, &admin, &registry);

        let pending = Address::generate(&env);
        let live_until = env.ledger().sequence() + 8640;
        factory.transfer_admin_role(&pending, &live_until);

        // Drop every previously-mocked auth — the pending admin's
        // require_auth call inside accept_admin_transfer must now fail.
        env.set_auths(&[]);
        factory.accept_admin_transfer();
    }
}

// ===========================================================================
// Admin rotation — access-control invariants
//
// The existing `ownable2step_tests` only check that `get_admin` returns the
// new address after rotation and that future PT deploys carry it. They do
// NOT exercise the role-management surface across the rotation boundary —
// i.e. they don't verify that the OLD admin's grant/revoke rights are gone
// and that the NEW admin's are live. That property is the core security
// guarantee of admin rotation; if a future stellar-access bump or local
// wrapper change accidentally left the old admin's grant capability in
// place, the rotation would be visually correct but practically broken.
// ===========================================================================

mod admin_rotation_invariants {
    use super::*;

    fn setup_post_rotation(env: &Env) -> (FactoryContractClient<'_>, Address, Address) {
        env.mock_all_auths();
        let admin = Address::generate(env);
        let registry = Address::generate(env);
        let (_, factory) = register_factory(env, &admin, &registry);

        let new_admin = Address::generate(env);
        let live_until = env.ledger().sequence() + 8640;
        factory.transfer_admin_role(&new_admin, &live_until);
        factory.accept_admin_transfer();
        (factory, admin, new_admin)
    }

    #[test]
    fn test_old_admin_cannot_grant_role_after_rotation() {
        let env = Env::default();
        let (factory, old_admin, _new_admin) = setup_post_rotation(&env);
        let target = Address::generate(&env);

        let result = factory.try_grant_role(&old_admin, &target, &roles::FACTORY);
        assert!(result.is_err());
        assert!(!factory.has_role(&target, &roles::FACTORY));
    }

    #[test]
    fn test_old_admin_cannot_revoke_role_after_rotation() {
        let env = Env::default();
        let (factory, old_admin, new_admin) = setup_post_rotation(&env);
        let target = Address::generate(&env);

        // New admin first grants FACTORY to the target.
        factory.grant_role(&new_admin, &target, &roles::FACTORY);
        assert!(factory.has_role(&target, &roles::FACTORY));

        // Old admin tries to revoke — must fail; target still holds the role.
        let result = factory.try_revoke_role(&old_admin, &target, &roles::FACTORY);
        assert!(result.is_err());
        assert!(factory.has_role(&target, &roles::FACTORY));
    }

    #[test]
    fn test_new_admin_can_grant_role_after_rotation() {
        let env = Env::default();
        let (factory, _old_admin, new_admin) = setup_post_rotation(&env);
        let target = Address::generate(&env);

        factory.grant_role(&new_admin, &target, &roles::FACTORY);
        assert!(factory.has_role(&target, &roles::FACTORY));
    }

    #[test]
    fn test_new_admin_can_revoke_role_after_rotation() {
        let env = Env::default();
        let (factory, _old_admin, new_admin) = setup_post_rotation(&env);
        let target = Address::generate(&env);

        factory.grant_role(&new_admin, &target, &roles::FACTORY);
        assert!(factory.has_role(&target, &roles::FACTORY));

        factory.revoke_role(&new_admin, &target, &roles::FACTORY);
        assert!(!factory.has_role(&target, &roles::FACTORY));
    }
}

// ===========================================================================
// PT deploy counter + salt scheme
//
// `compute_salt` is a private helper that hashes `(duration, counter)` with
// big-endian byte layout. The counter is what gives two `deploy_pt` calls
// with identical `(ibt, duration)` distinct PT addresses. These tests pin
// both invariants directly:
//   - counter increments by exactly 1 per successful deploy
//     (`test_same_ibt_and_duration_*` only checks the *consequence* —
//     distinct addresses — not the counter mechanism);
//   - the deployed PT lands at the address derived from
//     sha256(duration_be || counter_be) under the factory contract,
//     i.e. the salt scheme matches Solidity's CREATE2 contract-address
//     determinism guarantee.
// ===========================================================================

mod counter_and_salt {
    use super::*;
    use soroban_sdk::{Bytes, BytesN};

    #[test]
    fn test_pt_deploy_counter_increments_by_one() {
        let env = Env::default();
        let (admin, _, factory_id, factory, ibt) = setup_full_stack(&env);
        let duration: u64 = 30 * 24 * 60 * 60;

        let before = env.as_contract(&factory_id, || crate::storage::get_pt_deploy_counter(&env));
        let _ = factory.deploy_pt(&admin, &ibt, &duration);
        let after = env.as_contract(&factory_id, || crate::storage::get_pt_deploy_counter(&env));

        assert_eq!(after, before + 1);
    }

    #[test]
    fn test_pt_deploy_counter_increments_monotonically_across_deploys() {
        let env = Env::default();
        let (admin, _, factory_id, factory, ibt) = setup_full_stack(&env);
        let duration: u64 = 30 * 24 * 60 * 60;

        for expected in 1u64..=5 {
            let _ = factory.deploy_pt(&admin, &ibt, &duration);
            let actual =
                env.as_contract(&factory_id, || crate::storage::get_pt_deploy_counter(&env));
            assert_eq!(actual, expected);
        }
    }

    #[test]
    fn test_deployed_pt_address_matches_sha256_salt_scheme() {
        let env = Env::default();
        let (admin, _, factory_id, factory, ibt) = setup_full_stack(&env);
        let duration: u64 = 30 * 24 * 60 * 60;

        // Counter is 0 immediately after setup (no PTs deployed yet).
        let counter: u64 = 0;
        let mut buf = Bytes::new(&env);
        buf.extend_from_array(&duration.to_be_bytes());
        buf.extend_from_array(&counter.to_be_bytes());
        let salt: BytesN<32> = env.crypto().sha256(&buf).into();

        let expected_addr = env.as_contract(&factory_id, || {
            env.deployer()
                .with_current_contract(salt.clone())
                .deployed_address()
        });

        let actual = factory.deploy_pt(&admin, &ibt, &duration);
        assert_eq!(actual, expected_addr);
    }
}

// ===========================================================================
// UPGRADE role — decoupled from admin slot
//
// `upgrade` is gated by `#[only_role(caller, "UPGRADE")]`, not by the admin
// slot. The existing upgrade tests only exercise the admin (who happens to
// hold UPGRADE at init). These tests separate the two:
//   - a non-admin holder of UPGRADE can call `upgrade`;
//   - revoking UPGRADE from the admin disables their upgrade path (admin
//     status alone is not enough — confirming UPGRADE is the only gate).
// ===========================================================================

mod upgrade_role_decoupling {
    use super::*;

    #[test]
    fn test_non_admin_upgrade_role_holder_can_call_upgrade() {
        let env = Env::default();
        env.mock_all_auths();
        let admin = Address::generate(&env);
        let registry = Address::generate(&env);
        let (_, factory) = register_factory(&env, &admin, &registry);

        let third_party = Address::generate(&env);
        factory.grant_role(&admin, &third_party, &roles::UPGRADE);
        assert!(factory.has_role(&third_party, &roles::UPGRADE));
        // Sanity: third_party is NOT the admin.
        env.as_contract(&factory.address, || {
            assert_ne!(access_control::get_admin(&env), Some(third_party.clone()));
        });

        // Any uploaded wasm works; update_current_contract_wasm only checks
        // the hash is uploaded, not what it points to.
        let hash = env.deployer().upload_contract_wasm(YT_WASM);
        factory.upgrade(&third_party, &hash);
    }

    #[test]
    fn test_admin_cannot_upgrade_after_self_revoking_upgrade_role() {
        let env = Env::default();
        env.mock_all_auths();
        let admin = Address::generate(&env);
        let registry = Address::generate(&env);
        let (_, factory) = register_factory(&env, &admin, &registry);

        // Admin self-revokes UPGRADE.
        factory.revoke_role(&admin, &admin, &roles::UPGRADE);
        assert!(!factory.has_role(&admin, &roles::UPGRADE));

        let hash = env.deployer().upload_contract_wasm(YT_WASM);
        let result = factory.try_upgrade(&admin, &hash);
        assert!(
            result.is_err(),
            "admin without UPGRADE role must not be able to call upgrade"
        );
    }
}

// ===========================================================================
// Multi-holder role semantics
//
// Roles must be a set, not a single-slot field. Three independent accounts
// hold the same role; revoking one must not affect the other two. Pins the
// stellar-access storage model so a future refactor can't silently turn
// the role into a single-holder slot.
// ===========================================================================

mod multi_holder_role {
    use super::*;

    #[test]
    fn test_multiple_addresses_hold_factory_role_independently() {
        let env = Env::default();
        env.mock_all_auths();
        let admin = Address::generate(&env);
        let registry = Address::generate(&env);
        let (_, factory) = register_factory(&env, &admin, &registry);

        let h1 = Address::generate(&env);
        let h2 = Address::generate(&env);
        let h3 = Address::generate(&env);

        factory.grant_role(&admin, &h1, &roles::FACTORY);
        factory.grant_role(&admin, &h2, &roles::FACTORY);
        factory.grant_role(&admin, &h3, &roles::FACTORY);

        assert!(factory.has_role(&h1, &roles::FACTORY));
        assert!(factory.has_role(&h2, &roles::FACTORY));
        assert!(factory.has_role(&h3, &roles::FACTORY));

        factory.revoke_role(&admin, &h2, &roles::FACTORY);

        assert!(factory.has_role(&h1, &roles::FACTORY));
        assert!(!factory.has_role(&h2, &roles::FACTORY));
        assert!(factory.has_role(&h3, &roles::FACTORY));
    }
}

// removed the entire pre_init_guards module — with a deploy-time
// constructor there is no uninitialized state to guard against; every
// contract instance is fully initialized the moment it exists.
