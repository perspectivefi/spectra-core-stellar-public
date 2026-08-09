// SPDX-License-Identifier: BUSL-1.1
#![cfg(test)]

use crate::SpectraFungibleVaultWrapper;
use soroban_sdk::testutils::Address as _;
use soroban_sdk::{
    Address, BytesN, Env, MuxedAddress, String, Symbol, Val, Vec, contract, contractimpl,
};
use spectra_math::mul_div::Rounding;
use spectra_mocks::{
    MockIbtContract, MockIbtContractClient, MockUnderlyingContract, MockUnderlyingContractClient,
};
use stellar_macros::when_not_paused;
use stellar_tokens::fungible::{Base, ContractOverrides, FungibleToken};
use stellar_tokens::vault::Vault;

/// Compiled YT wasm — see `scripts/build-wasm.sh`. Doubles as a valid wasm hash
/// for the upgrade-rotation tests.
const YT_WASM: &[u8] = include_bytes!("../../../target/wasm32v1-none/release/yield_token.wasm");

// ── Reference instance ──────────────────────────────────────────────────────
// The base is abstract: it has no deployable contract, so a minimal in-test
// instance supplies the required seams (the rate conversions and the 4626
// flows) and inherits the rest. Its conversions are the pure wrap ratio
// (`_previewWrap`/`_previewUnwrap`), which is all the base scaffolding tests
// need; the deposit/redeem flows are a concrete instance's concern and are
// exercised by `blend-fungible-vault-wrapper`, so here they are stubs.

#[contract]
pub struct RefWrapper;

#[contractimpl]
impl RefWrapper {
    pub fn initialize(
        e: &Env,
        asset: Address,
        vault_share: Address,
        decimals_offset: u32,
        admin: Address,
    ) {
        crate::init_base(e, asset, vault_share, decimals_offset, admin);
    }
}

#[contractimpl(contracttrait)]
impl SpectraFungibleVaultWrapper for RefWrapper {
    fn convert_to_shares_with_rounding(e: &Env, assets: i128, rounding: Rounding) -> i128 {
        crate::preview_wrap_internal(e, assets, rounding)
    }

    fn convert_to_assets_with_rounding(e: &Env, shares: i128, rounding: Rounding) -> i128 {
        crate::preview_unwrap_internal(e, shares, rounding)
    }

    fn deposit(
        _e: &Env,
        _assets: i128,
        _receiver: Address,
        _from: Address,
        _operator: Address,
    ) -> i128 {
        panic!("RefWrapper: flows live in the instance; see blend")
    }
    fn mint(
        _e: &Env,
        _shares: i128,
        _receiver: Address,
        _from: Address,
        _operator: Address,
    ) -> i128 {
        panic!("RefWrapper: flows live in the instance; see blend")
    }
    fn withdraw(
        _e: &Env,
        _assets: i128,
        _receiver: Address,
        _owner: Address,
        _operator: Address,
    ) -> i128 {
        panic!("RefWrapper: flows live in the instance; see blend")
    }
    fn redeem(
        _e: &Env,
        _shares: i128,
        _receiver: Address,
        _owner: Address,
        _operator: Address,
    ) -> i128 {
        panic!("RefWrapper: flows live in the instance; see blend")
    }
}

#[contractimpl(contracttrait)]
impl FungibleToken for RefWrapper {
    type ContractType = Vault;

    #[when_not_paused]
    fn transfer(e: &Env, from: Address, to: MuxedAddress, amount: i128) {
        <Vault as ContractOverrides>::transfer(e, &from, &to, amount);
    }
    #[when_not_paused]
    fn transfer_from(e: &Env, spender: Address, from: Address, to: Address, amount: i128) {
        <Vault as ContractOverrides>::transfer_from(e, &spender, &from, &to, amount);
    }
    fn decimals(e: &Env) -> u32 {
        Base::decimals(e)
    }
}

// ── fixture ─────────────────────────────────────────────────────────────────

struct Fix {
    env: Env,
    wc: RefWrapperClient<'static>,
    vs: MockIbtContractClient<'static>,
    und_addr: Address,
    vs_addr: Address,
    admin: Address,
    user: Address,
}

fn setup_with(offset: u32, vs_decimals: u32) -> Fix {
    let env = Env::default();
    env.mock_all_auths();
    let und_addr = env.register(MockUnderlyingContract, ());
    let und = MockUnderlyingContractClient::new(&env, &und_addr);
    und.initialize(
        &String::from_str(&env, "Mock USD"),
        &String::from_str(&env, "USDC"),
        &7,
    );
    let vs_addr = env.register(MockIbtContract, ());
    let vs = MockIbtContractClient::new(&env, &vs_addr);
    vs.initialize(
        &und_addr,
        &String::from_str(&env, "Vault Share"),
        &String::from_str(&env, "vsTKN"),
        &vs_decimals,
    );
    let admin = Address::generate(&env);
    let waddr = env.register(RefWrapper, ());
    let wc = RefWrapperClient::new(&env, &waddr);
    wc.initialize(&und_addr, &vs_addr, &offset, &admin);
    let user = Address::generate(&env);
    und.mint(&user, &2_000_000);
    // acquire vault shares at rate 1.0 — user holds 1_000_000 und + 1_000_000 vs
    vs.deposit(&1_000_000, &user, &user, &user);
    Fix {
        env,
        wc,
        vs,
        und_addr,
        vs_addr,
        admin,
        user,
    }
}

fn setup() -> Fix {
    setup_with(0, 7)
}

mod init {
    use super::*;

    #[test]
    fn metadata_derived_from_vault_share() {
        let f = setup();
        assert_eq!(f.wc.decimals(), 7);
        assert_eq!(f.wc.vault_share(), f.vs_addr);
        assert_eq!(f.wc.query_asset(), f.und_addr);
        assert_eq!(
            f.wc.name(),
            String::from_str(&f.env, "Spectra FungibleVault Wrapper: Vault Share")
        );
        assert_eq!(f.wc.symbol(), String::from_str(&f.env, "sw-vsTKN"));
    }

    #[test]
    fn decimals_include_offset() {
        let f = setup_with(2, 7);
        assert_eq!(f.wc.decimals(), 9);
    }

    #[test]
    fn decimals_follow_vault_share_not_asset() {
        let f = setup_with(0, 6); // underlying has 7 decimals
        assert_eq!(f.wc.decimals(), 6);
    }

    #[test]
    fn double_init_fails() {
        let f = setup();
        let other = Address::generate(&f.env);
        assert!(
            f.wc.try_initialize(&f.und_addr, &f.vs_addr, &0, &other)
                .is_err()
        );
    }
}

mod wrapping {
    use super::*;

    #[test]
    fn wrap_mints_shares_and_pulls_vault_shares() {
        let f = setup();
        let shares = f.wc.wrap(&500, &f.user, &f.user);
        assert_eq!(shares, 500); // first wrap, offset 0
        assert_eq!(f.wc.balance(&f.user), 500);
        assert_eq!(f.wc.total_vault_shares(), 500);
        assert_eq!(f.vs.balance(&f.user), 999_500);
    }

    #[test]
    fn preview_wrap_matches_wrap() {
        let f = setup();
        let preview = f.wc.preview_wrap(&500);
        let shares = f.wc.wrap(&500, &f.user, &f.user);
        assert_eq!(preview, shares);
    }

    #[test]
    fn round_trip_returns_at_most_input() {
        let f = setup();
        f.wc.wrap(&500, &f.user, &f.user);
        let back = f.wc.unwrap(&500, &f.user, &f.user, &f.user);
        assert!(back <= 500);
        assert_eq!(f.vs.balance(&f.user), 1_000_000 - 500 + back);
    }

    #[test]
    fn unwrap_via_operator_spends_allowance() {
        let f = setup();
        f.wc.wrap(&500, &f.user, &f.user);
        let operator = Address::generate(&f.env);
        f.wc.approve(&f.user, &operator, &500, &(f.env.ledger().sequence() + 100));
        let back = f.wc.unwrap(&200, &operator, &f.user, &operator);
        assert!(back <= 200);
        assert_eq!(f.wc.balance(&f.user), 300);
    }

    #[test]
    fn wrap_with_min_shares_enforces_floor() {
        let f = setup();
        // Exact minimum is satisfied (first wrap, offset 0 → 1:1).
        let shares = f.wc.wrap_with_min_shares(&500, &f.user, &f.user, &500);
        assert_eq!(shares, 500);
        // Asking for one more share than mintable reverts.
        assert!(
            f.wc.try_wrap_with_min_shares(&500, &f.user, &f.user, &501)
                .is_err()
        );
    }

    #[test]
    fn unwrap_with_min_vault_shares_enforces_floor() {
        let f = setup();
        f.wc.wrap(&500, &f.user, &f.user);
        // Exact minimum is satisfied.
        let back =
            f.wc.unwrap_with_min_vault_shares(&200, &f.user, &f.user, &f.user, &200);
        assert_eq!(back, 200);
        // An unreachable minimum reverts.
        assert!(
            f.wc.try_unwrap_with_min_vault_shares(&100, &f.user, &f.user, &f.user, &101)
                .is_err()
        );
    }

    #[test]
    fn total_assets_tracks_supply() {
        let f = setup();
        f.wc.wrap(&500, &f.user, &f.user);
        assert!(f.wc.total_assets() > 0);
    }
}

mod pause {
    use super::*;

    #[test]
    fn pause_blocks_wrap_and_zeroes_max() {
        let f = setup();
        f.wc.pause(&f.admin);
        assert!(f.wc.paused());
        assert_eq!(f.wc.max_deposit(&f.user), 0);
        assert_eq!(f.wc.max_redeem(&f.user), 0);
        assert!(f.wc.try_wrap(&100, &f.user, &f.user).is_err());
        // previews are views and stay callable while paused
        assert!(f.wc.try_preview_wrap(&100).is_ok());
    }

    #[test]
    fn unpause_restores() {
        let f = setup();
        f.wc.pause(&f.admin);
        f.wc.unpause(&f.admin);
        assert!(!f.wc.paused());
        assert_eq!(f.wc.wrap(&100, &f.user, &f.user), 100);
    }

    #[test]
    fn non_pauser_cannot_pause() {
        let f = setup();
        let rando = Address::generate(&f.env);
        assert!(f.wc.try_pause(&rando).is_err());
    }
}

// ── role rotation: every role moves to a new holder post-init, and rotation
// is safe — only the admin can move roles, and a role grants exactly its own
// capability. ──

mod role_rotation {
    use super::*;
    use soroban_sdk::{IntoVal, TryFromVal, symbol_short, vec};
    use spectra_common::constants::roles;

    #[test]
    fn rotated_pauser_works_and_old_holders_lose() {
        let f = setup();
        let pauser = Address::generate(&f.env);
        f.wc.grant_role(&f.admin, &pauser, &roles::PAUSER);
        f.wc.revoke_role(&f.admin, &f.admin, &roles::PAUSER);
        assert!(f.wc.has_role(&pauser, &roles::PAUSER));
        assert!(!f.wc.has_role(&f.admin, &roles::PAUSER));

        // New holder pauses; the ex-holder is locked out even though it is
        // still the contract admin — only_role checks the role, not adminship.
        f.wc.pause(&pauser);
        assert!(f.wc.try_unpause(&f.admin).is_err());
        f.wc.unpause(&pauser);

        // Revoked, the new holder loses the capability too.
        f.wc.revoke_role(&f.admin, &pauser, &roles::PAUSER);
        assert!(f.wc.try_pause(&pauser).is_err());
    }

    #[test]
    fn rotated_rewards_roles_work_and_admin_loses_them() {
        let f = setup();
        let ops = Address::generate(&f.env);
        f.wc.grant_role(&f.admin, &ops, &roles::REWARDS_PROXY_SETTER);
        f.wc.grant_role(&f.admin, &ops, &roles::REWARDS_HARVESTER);
        f.wc.revoke_role(&f.admin, &f.admin, &roles::REWARDS_PROXY_SETTER);
        f.wc.revoke_role(&f.admin, &f.admin, &roles::REWARDS_HARVESTER);

        let proxy = f.env.register(super::rewards::MockRewardsProxy, ());
        f.wc.set_rewards_proxy(&ops, &Some(proxy));
        let args: Vec<Val> = vec![&f.env, 1_i128.into_val(&f.env)];
        let r = f.wc.claim_rewards(&ops, &symbol_short!("collect"), &args);
        assert_eq!(i128::try_from_val(&f.env, &r).unwrap(), 2);

        assert!(f.wc.try_set_rewards_proxy(&f.admin, &None).is_err());
        assert!(
            f.wc.try_claim_rewards(&f.admin, &symbol_short!("collect"), &args)
                .is_err()
        );
    }

    #[test]
    fn rotated_upgrader_can_upgrade_and_admin_cannot() {
        let f = setup();
        let upgrader = Address::generate(&f.env);
        f.wc.grant_role(&f.admin, &upgrader, &roles::UPGRADE);
        f.wc.revoke_role(&f.admin, &f.admin, &roles::UPGRADE);
        let hash = f.env.deployer().upload_contract_wasm(YT_WASM);
        assert!(f.wc.try_upgrade(&f.admin, &hash).is_err());
        f.wc.upgrade(&upgrader, &hash);
    }

    #[test]
    fn role_grants_only_its_own_capability() {
        let f = setup();
        // Pre-install a proxy so the harvest check below fails on the role,
        // not on a missing proxy.
        let proxy = f.env.register(super::rewards::MockRewardsProxy, ());
        f.wc.set_rewards_proxy(&f.admin, &Some(proxy));

        let pauser = Address::generate(&f.env);
        f.wc.grant_role(&f.admin, &pauser, &roles::PAUSER);
        let hash = f.env.deployer().upload_contract_wasm(YT_WASM);
        assert!(f.wc.try_upgrade(&pauser, &hash).is_err());
        assert!(f.wc.try_set_rewards_proxy(&pauser, &None).is_err());
        let args: Vec<Val> = vec![&f.env, 1_i128.into_val(&f.env)];
        assert!(
            f.wc.try_claim_rewards(&pauser, &symbol_short!("collect"), &args)
                .is_err()
        );
    }

    #[test]
    fn role_holder_cannot_grant_or_revoke_onward() {
        let f = setup();
        let pauser = Address::generate(&f.env);
        f.wc.grant_role(&f.admin, &pauser, &roles::PAUSER);
        let accomplice = Address::generate(&f.env);
        // Holding a role does not confer the power to move it.
        assert!(
            f.wc.try_grant_role(&pauser, &accomplice, &roles::PAUSER)
                .is_err()
        );
        assert!(
            f.wc.try_revoke_role(&pauser, &f.admin, &roles::PAUSER)
                .is_err()
        );
        assert!(!f.wc.has_role(&accomplice, &roles::PAUSER));
        assert!(f.wc.has_role(&f.admin, &roles::PAUSER));
    }

    #[test]
    fn stranger_cannot_grant_or_revoke() {
        let f = setup();
        let attacker = Address::generate(&f.env);
        assert!(
            f.wc.try_grant_role(&attacker, &attacker, &roles::PAUSER)
                .is_err()
        );
        assert!(
            f.wc.try_revoke_role(&attacker, &f.admin, &roles::PAUSER)
                .is_err()
        );
        assert!(!f.wc.has_role(&attacker, &roles::PAUSER));
        assert!(f.wc.has_role(&f.admin, &roles::PAUSER));
    }
}

mod rewards {
    use super::*;
    use crate::errors::SpectraFungibleVaultError;
    use soroban_sdk::{IntoVal, TryFromVal, symbol_short, vec};

    // Stand-in rewards proxy: `claim_rewards` invokes `collect` on it, and the
    // `+ 1` lets the test observe that the dynamic cross-contract call ran.
    #[contract]
    pub struct MockRewardsProxy;

    #[contractimpl]
    impl MockRewardsProxy {
        pub fn collect(_e: &Env, x: i128) -> i128 {
            x + 1
        }
    }

    #[contract]
    pub struct MockPanickingRewardsProxy;

    #[contractimpl]
    impl MockPanickingRewardsProxy {
        pub fn fail(_e: &Env) -> i128 {
            panic!("proxy boom")
        }
    }

    #[test]
    fn set_get_and_clear_proxy() {
        let f = setup();
        assert_eq!(f.wc.rewards_proxy(), None);
        let proxy = Address::generate(&f.env);
        f.wc.set_rewards_proxy(&f.admin, &Some(proxy.clone()));
        assert_eq!(f.wc.rewards_proxy(), Some(proxy));
        f.wc.set_rewards_proxy(&f.admin, &None);
        assert_eq!(f.wc.rewards_proxy(), None);
    }

    #[test]
    fn claim_rewards_invokes_proxy_function() {
        let f = setup();
        let proxy = f.env.register(MockRewardsProxy, ());
        f.wc.set_rewards_proxy(&f.admin, &Some(proxy));
        let args: Vec<Val> = vec![&f.env, 41_i128.into_val(&f.env)];
        let result =
            f.wc.claim_rewards(&f.admin, &symbol_short!("collect"), &args);
        assert_eq!(i128::try_from_val(&f.env, &result).unwrap(), 42);
    }

    #[test]
    fn claim_without_proxy_reverts() {
        let f = setup();
        let args: Vec<Val> = vec![&f.env, 1_i128.into_val(&f.env)];
        assert!(
            f.wc.try_claim_rewards(&f.admin, &symbol_short!("collect"), &args)
                .is_err()
        );
    }

    #[test]
    fn proxy_panic_surfaces_as_claim_rewards_failed() {
        let f = setup();
        let proxy = f.env.register(MockPanickingRewardsProxy, ());
        f.wc.set_rewards_proxy(&f.admin, &Some(proxy));
        let args: Vec<Val> = vec![&f.env];
        let err =
            f.wc.try_claim_rewards(&f.admin, &symbol_short!("fail"), &args)
                .expect_err("proxy panic must surface")
                .expect("error must be a SpectraFungibleVaultError");
        assert_eq!(err, SpectraFungibleVaultError::ClaimRewardsFailed.into());
    }

    #[test]
    fn non_harvester_cannot_claim() {
        let f = setup();
        let proxy = f.env.register(MockRewardsProxy, ());
        f.wc.set_rewards_proxy(&f.admin, &Some(proxy));
        let rando = Address::generate(&f.env);
        let args: Vec<Val> = vec![&f.env, 1_i128.into_val(&f.env)];
        assert!(
            f.wc.try_claim_rewards(&rando, &symbol_short!("collect"), &args)
                .is_err()
        );
    }

    #[test]
    fn non_setter_cannot_set_proxy() {
        let f = setup();
        let rando = Address::generate(&f.env);
        let proxy = Address::generate(&f.env);
        assert!(f.wc.try_set_rewards_proxy(&rando, &Some(proxy)).is_err());
    }
}

mod missing_negatives {
    //! Negative coverage for paths that previously had only happy-path tests:
    //! - pause-gating on the SEP-41 transfer surface (`transfer` / `transfer_from`)
    //! - auth-not-signed on the wrap/unwrap entry points
    //! - non-admin can not grant / revoke roles
    //!
    //! Pause-gating on the 4626 flows (`deposit` / `mint` / `withdraw` / `redeem`)
    //! is not testable on the abstract base because `RefWrapper` stubs those out;
    //! see `blend-fungible-vault-wrapper` for instance-level coverage.

    use super::*;
    use spectra_common::constants::roles;

    // ── transfer / transfer_from pause-gated ──────────────────────────────

    #[test]
    fn transfer_when_paused_reverts() {
        let f = setup();
        // Mint wrapper shares so transfer has something to move.
        f.wc.wrap(&500, &f.user, &f.user);
        let receiver = Address::generate(&f.env);
        let mreceiver: MuxedAddress = receiver.into();
        // Unpaused transfer works.
        f.wc.transfer(&f.user, &mreceiver, &100);
        // Paused transfer reverts.
        f.wc.pause(&f.admin);
        assert!(f.wc.try_transfer(&f.user, &mreceiver, &100).is_err());
    }

    #[test]
    fn transfer_from_when_paused_reverts() {
        let f = setup();
        f.wc.wrap(&500, &f.user, &f.user);
        let spender = Address::generate(&f.env);
        let receiver = Address::generate(&f.env);
        f.wc.approve(&f.user, &spender, &500, &(f.env.ledger().sequence() + 100));
        // Unpaused transfer_from works.
        f.wc.transfer_from(&spender, &f.user, &receiver, &50);
        // Paused transfer_from reverts.
        f.wc.pause(&f.admin);
        assert!(
            f.wc.try_transfer_from(&spender, &f.user, &receiver, &50)
                .is_err()
        );
    }

    // ── wrap / unwrap require caller's auth ───────────────────────────────

    #[test]
    fn wrap_without_caller_auth_reverts() {
        let f = setup();
        // Drop the universal mock; no AuthorizationEntry is supplied for
        // `caller`, so the inner `caller.require_auth()` in `wrap` must fail.
        f.env.set_auths(&[]);
        assert!(f.wc.try_wrap(&100, &f.user, &f.user).is_err());
    }

    #[test]
    fn unwrap_without_operator_auth_reverts() {
        let f = setup();
        // Mint wrapper shares first (still under mock_all_auths).
        f.wc.wrap(&500, &f.user, &f.user);
        // Then drop auth and attempt unwrap — `operator.require_auth()` fails.
        f.env.set_auths(&[]);
        assert!(f.wc.try_unwrap(&100, &f.user, &f.user, &f.user).is_err());
    }

    // ── grant_role / revoke_role: non-admin can not move roles ────────────

    #[test]
    fn non_admin_cannot_grant_role() {
        let f = setup();
        let attacker = Address::generate(&f.env);
        let target = Address::generate(&f.env);
        // Even under mock_all_auths, stellar-access enforces the
        // caller-is-admin check internally.
        assert!(
            f.wc.try_grant_role(&attacker, &target, &roles::PAUSER)
                .is_err()
        );
        assert!(!f.wc.has_role(&target, &roles::PAUSER));
    }

    #[test]
    fn non_admin_cannot_revoke_role() {
        let f = setup();
        // Grant first as admin, so there is something to revoke.
        let holder = Address::generate(&f.env);
        f.wc.grant_role(&f.admin, &holder, &roles::PAUSER);
        // A third-party cannot revoke it.
        let attacker = Address::generate(&f.env);
        assert!(
            f.wc.try_revoke_role(&attacker, &holder, &roles::PAUSER)
                .is_err()
        );
        assert!(f.wc.has_role(&holder, &roles::PAUSER));
    }
}

mod fuzz {
    //! Property tests for the wrap/unwrap conversion math.
    //!
    //! The wrap formula is monotone in the floor direction, so a single
    //! round-trip can lose at most a few least-significant units to rounding
    //! and must never return more than was deposited. The property tests
    //! below randomize `(offset, vs_decimals, x)` (and for the multi-step
    //! version a sequence of amounts) and lock in:
    //!
    //! 1. **No free shares:** `unwrap(wrap(x)) ≤ x` for any single deposit.
    //! 2. **Genesis exactness:** at a fresh wrapper, `wrap(x) == x * 10^offset`.
    //! 3. **Multi-step dust bound:** after a sequence of wraps, fully unwrapping
    //!    returns at most the total deposited.
    //!
    //! Case count kept low (32) because each iteration sets up a fresh Soroban
    //! env, which is the dominant cost.
    use super::*;
    use proptest::prelude::*;

    proptest! {
        #![proptest_config(ProptestConfig {
            cases: 32,
            .. ProptestConfig::default()
        })]

        #[test]
        fn single_wrap_unwrap_round_trip_does_not_create_value(
            offset in 0u32..=10,
            vs_decimals in 4u32..=18,
            x in 1i128..1_000_000,
        ) {
            let f = setup_with(offset, vs_decimals);
            let shares = f.wc.wrap(&x, &f.user, &f.user);
            let back = f.wc.unwrap(&shares, &f.user, &f.user, &f.user);
            prop_assert!(
                back <= x,
                "round-trip returned more than deposited: back={} > x={} (offset={}, vs_dec={})",
                back, x, offset, vs_decimals
            );
        }

        #[test]
        fn wrap_is_exact_at_genesis(
            offset in 0u32..=10,
            x in 1i128..1_000_000,
        ) {
            // At a fresh wrapper: total_supply = 0, vault_share_balance = 0.
            //   shares = x * (0 + 10^offset) / (0 + 1) = x * 10^offset.
            let f = setup_with(offset, 7);
            let shares = f.wc.wrap(&x, &f.user, &f.user);
            let expected = x.checked_mul(10_i128.pow(offset)).unwrap();
            prop_assert_eq!(shares, expected);
        }

        #[test]
        fn multi_wrap_then_full_unwrap_does_not_create_value(
            offset in 0u32..=10,
            amounts in prop::collection::vec(1i128..100_000, 1..6),
        ) {
            let f = setup_with(offset, 7);
            let total: i128 = amounts.iter().sum();
            let mut total_shares = 0i128;
            for &a in &amounts {
                total_shares += f.wc.wrap(&a, &f.user, &f.user);
            }
            let back = f.wc.unwrap(&total_shares, &f.user, &f.user, &f.user);
            prop_assert!(
                back <= total,
                "multi-wrap round-trip returned more than deposited: \
                back={} > total={} (offset={}, amounts={:?})",
                back, total, offset, amounts
            );
        }
    }
}

// ===========================================================================
// BREACH-HUNT: spec-driven invariants on the abstract base
// ===========================================================================
//
// The trait's default methods route every preview/convert through the two
// override seams. These tests pin the rounding directions and the SFVW spec
// invariants (ERC-4626 conventions adapted to Soroban).
// ===========================================================================

mod spec_invariants {
    use super::*;

    /// `preview_deposit` MUST round Floor (favors protocol on share mint).
    /// `preview_withdraw` MUST round Ceil (favors protocol on share burn).
    /// `preview_mint` MUST round Ceil (Solidity ERC4626 convention).
    /// `preview_redeem` MUST round Floor.
    ///
    /// Pin all four with a wrap-induced non-exact ratio so Floor != Ceil.
    #[test]
    fn test_preview_rounding_directions_match_erc4626_convention() {
        let f = setup_with(0, 7);
        // Seed wrapper with some shares so that subsequent previews use a
        // non-trivial ratio.
        f.vs.approve(
            &f.user,
            &f.wc.address,
            &500_000,
            &(f.env.ledger().sequence() + 100),
        );
        f.wc.wrap(&500_000, &f.user, &f.user);

        // Pick an asset value that doesn't divide evenly.
        let assets: i128 = 333_333;
        let floor_dep = f.wc.preview_deposit(&assets);
        let ceil_wd = f.wc.preview_withdraw(&assets);
        assert!(
            ceil_wd >= floor_dep,
            "preview_withdraw (ceil) must be >= preview_deposit (floor) for same assets"
        );

        let shares: i128 = 333_333;
        let ceil_mint = f.wc.preview_mint(&shares);
        let floor_red = f.wc.preview_redeem(&shares);
        assert!(
            ceil_mint >= floor_red,
            "preview_mint (ceil) must be >= preview_redeem (floor) for same shares"
        );
    }

    /// preview_wrap and preview_unwrap are inverse with rounding bias:
    /// `unwrap(wrap(x))` <= `x` always (Floor on both).
    #[test]
    fn test_wrap_unwrap_roundtrip_is_lossy_or_exact() {
        let f = setup_with(0, 7);
        let x: i128 = 7_777;
        f.vs.approve(
            &f.user,
            &f.wc.address,
            &500_000,
            &(f.env.ledger().sequence() + 100),
        );
        // Seed wrapper.
        f.wc.wrap(&500_000, &f.user, &f.user);

        let shares = f.wc.preview_wrap(&x);
        let back = f.wc.preview_unwrap(&shares);
        assert!(
            back <= x,
            "preview_unwrap(preview_wrap(x)) must be <= x (Floor bias); got back={back} x={x}"
        );
    }

    /// `convert_to_shares` is the Floor variant.
    /// `convert_to_assets` is the Floor variant.
    /// Both must satisfy: convert_to_assets(convert_to_shares(x)) <= x.
    #[test]
    fn test_convert_roundtrip_floor_floor_lossy_or_exact() {
        let f = setup_with(0, 7);
        f.vs.approve(
            &f.user,
            &f.wc.address,
            &500_000,
            &(f.env.ledger().sequence() + 100),
        );
        f.wc.wrap(&500_000, &f.user, &f.user);

        let x: i128 = 12_345;
        let shares = f.wc.convert_to_shares(&x);
        let back = f.wc.convert_to_assets(&shares);
        assert!(back <= x);
    }

    /// max_deposit/max_mint return i128::MAX when not paused, 0 when paused.
    #[test]
    fn test_max_deposit_max_mint_zero_when_paused_full_when_active() {
        let f = setup();
        let other = Address::generate(&f.env);
        assert_eq!(f.wc.max_deposit(&other), i128::MAX);
        assert_eq!(f.wc.max_mint(&other), i128::MAX);
        f.wc.pause(&f.admin);
        assert_eq!(f.wc.max_deposit(&other), 0);
        assert_eq!(f.wc.max_mint(&other), 0);
    }

    /// `wrap_with_min_shares` panics when slippage is not met; the underlying
    /// `wrap` already executed mutations — but Soroban tx-level revert undoes
    /// them. Verify the user's vault-share balance is restored.
    #[test]
    fn test_wrap_with_min_shares_atomic_revert_restores_balance() {
        let f = setup();
        f.vs.approve(
            &f.user,
            &f.wc.address,
            &100_000,
            &(f.env.ledger().sequence() + 100),
        );
        let bal_before = f.vs.balance(&f.user);
        let r =
            f.wc.try_wrap_with_min_shares(&100_000, &f.user, &f.user, &i128::MAX);
        assert!(r.is_err(), "impossible min_shares must revert");
        let bal_after = f.vs.balance(&f.user);
        assert_eq!(bal_before, bal_after, "tx revert must restore balance");
    }

    /// `unwrap` by non-owner without allowance fails.
    #[test]
    fn test_unwrap_by_third_party_without_allowance_fails() {
        let f = setup();
        f.vs.approve(
            &f.user,
            &f.wc.address,
            &100_000,
            &(f.env.ledger().sequence() + 100),
        );
        f.wc.wrap(&100_000, &f.user, &f.user);

        let attacker = Address::generate(&f.env);
        let r = f.wc.try_unwrap(&10_000, &attacker, &f.user, &attacker);
        assert!(r.is_err(), "non-owner without allowance must not unwrap");
    }

    /// `claim_rewards` without proxy installed panics `NoRewardsProxy`.
    #[test]
    fn test_claim_rewards_without_proxy_panics() {
        let f = setup();
        let bogus = Symbol::new(&f.env, "nope");
        let args: Vec<Val> = Vec::new(&f.env);
        let r = f.wc.try_claim_rewards(&f.admin, &bogus, &args);
        assert!(r.is_err(), "claim_rewards without proxy must panic");
    }

    /// Setting rewards proxy to `None` clears it.
    #[test]
    fn test_set_rewards_proxy_none_clears_storage() {
        let f = setup();
        let proxy = Address::generate(&f.env);
        f.wc.set_rewards_proxy(&f.admin, &Some(proxy));
        assert!(f.wc.rewards_proxy().is_some());
        f.wc.set_rewards_proxy(&f.admin, &None);
        assert!(f.wc.rewards_proxy().is_none());
    }

    /// `init_base` with admin == user is allowed; the admin gets all roles.
    #[test]
    fn test_init_admin_holds_all_documented_roles() {
        let f = setup();
        use spectra_common::constants::roles;
        assert!(f.wc.has_role(&f.admin, &roles::PAUSER));
        assert!(f.wc.has_role(&f.admin, &roles::UPGRADE));
        assert!(f.wc.has_role(&f.admin, &roles::REWARDS_PROXY_SETTER));
        assert!(f.wc.has_role(&f.admin, &roles::REWARDS_HARVESTER));
    }

    /// FINDING (#6): with `decimals_offset == 0`, the first-depositor
    /// inflation attack still works. Pin this so any future instance
    /// initialized with offset=0 is flagged.
    ///
    /// Attack sequence:
    /// 1. Attacker wraps 1 wei of vault_share, gets 1 wrapper-share.
    /// 2. Attacker donates a large vault_share balance directly to the
    ///    wrapper (bypassing wrap).
    /// 3. Victim wraps 100_000 vault_share. With offset=0, the math
    ///    `vault_shares * (total_supply + 1) / (vault_share_balance + 1)`
    ///    floors to 0 because vault_share_balance is now 500_001 while
    ///    total_supply is still 1.
    ///
    /// Mitigation: deploy instances with `decimals_offset > 0`. The
    /// next test pins that offset=6 defeats the same attack.
    #[test]
    fn test_finding_inflation_attack_succeeds_when_offset_is_zero() {
        let f = setup_with(0, 7);
        f.vs.approve(
            &f.user,
            &f.wc.address,
            &1_000_000,
            &(f.env.ledger().sequence() + 100),
        );
        let _ = f.wc.wrap(&1, &f.user, &f.user);
        f.vs.transfer(&f.user, &f.wc.address, &500_000);

        let victim = Address::generate(&f.env);
        f.vs.transfer(&f.user, &victim, &100_000);
        f.vs.approve(
            &victim,
            &f.wc.address,
            &100_000,
            &(f.env.ledger().sequence() + 100),
        );
        let victim_shares = f.wc.wrap(&100_000, &victim, &victim);

        // With offset=0 the victim is rugged to 0.
        assert_eq!(
            victim_shares, 0,
            "FINDING: offset=0 leaves first-depositor inflation surface open"
        );
    }

    /// Mitigation pin: `decimals_offset >= 6` defeats the inflation attack
    /// at the same magnitudes — the `+10^offset` term in the numerator is
    /// large enough that the victim still receives a meaningful share.
    #[test]
    fn test_inflation_attack_defeated_by_nonzero_decimals_offset() {
        let f = setup_with(6, 7); // offset = 6 → +10^6 boost on numerator
        f.vs.approve(
            &f.user,
            &f.wc.address,
            &1_000_000,
            &(f.env.ledger().sequence() + 100),
        );
        let _ = f.wc.wrap(&1, &f.user, &f.user);
        f.vs.transfer(&f.user, &f.wc.address, &500_000);

        let victim = Address::generate(&f.env);
        f.vs.transfer(&f.user, &victim, &100_000);
        f.vs.approve(
            &victim,
            &f.wc.address,
            &100_000,
            &(f.env.ledger().sequence() + 100),
        );
        let victim_shares = f.wc.wrap(&100_000, &victim, &victim);

        assert!(
            victim_shares > 0,
            "decimals_offset > 0 must mitigate inflation attack; got victim_shares={victim_shares}"
        );
    }

    /// Slippage off-by-one (#9) on `wrap_with_min_shares`: `min == actual`
    /// must pass; `min == actual + 1` must fail.
    #[test]
    fn test_slippage_off_by_one_wrap_with_min_shares() {
        let f = setup();
        f.vs.approve(
            &f.user,
            &f.wc.address,
            &500_000,
            &(f.env.ledger().sequence() + 100),
        );
        // Capture exact shares via a peek-then-revert pattern: use preview.
        let expected = f.wc.preview_wrap(&100_000);

        // Equal: passes.
        f.vs.approve(
            &f.user,
            &f.wc.address,
            &100_000,
            &(f.env.ledger().sequence() + 100),
        );
        let actual =
            f.wc.wrap_with_min_shares(&100_000, &f.user, &f.user, &expected);
        assert_eq!(actual, expected);

        // One above expected: fails.
        f.vs.approve(
            &f.user,
            &f.wc.address,
            &100_000,
            &(f.env.ledger().sequence() + 100),
        );
        let r =
            f.wc.try_wrap_with_min_shares(&100_000, &f.user, &f.user, &(expected + 1));
        assert!(r.is_err(), "min == actual + 1 must trip slippage");
    }
}

mod admin_2step {
    use super::*;

    fn live_until(env: &Env) -> u32 {
        env.ledger().sequence() + 1000
    }

    #[test]
    fn admin_rotates_via_two_step() {
        let f = setup();
        let new_admin = Address::generate(&f.env);

        assert_eq!(f.wc.admin(), Some(f.admin.clone()));

        // Current admin proposes; pending state is set but admin is unchanged.
        f.wc.transfer_admin_role(&new_admin, &live_until(&f.env));
        assert_eq!(f.wc.admin(), Some(f.admin.clone()));

        // Proposed admin accepts; rotation completes atomically.
        f.wc.accept_admin_transfer();
        assert_eq!(f.wc.admin(), Some(new_admin));
    }

    #[test]
    fn new_admin_holds_admin_privileges_old_does_not() {
        let f = setup();
        let new_admin = Address::generate(&f.env);
        f.wc.transfer_admin_role(&new_admin, &live_until(&f.env));
        f.wc.accept_admin_transfer();

        // New admin can grant/revoke roles.
        let bystander = Address::generate(&f.env);
        f.wc.grant_role(
            &new_admin,
            &bystander,
            &spectra_common::constants::roles::PAUSER,
        );
        assert!(f.wc.has_role(&bystander, &spectra_common::constants::roles::PAUSER));

        // The old admin can no longer grant roles — stellar-access checks
        // caller-is-admin internally and rejects.
        assert!(
            f.wc.try_grant_role(
                &f.admin,
                &Address::generate(&f.env),
                &spectra_common::constants::roles::PAUSER
            )
            .is_err()
        );
    }

    #[test]
    fn accept_without_pending_reverts() {
        let f = setup();
        // No prior `transfer_admin_role` call — there is nothing to accept.
        assert!(f.wc.try_accept_admin_transfer().is_err());
    }

    #[test]
    fn transfer_admin_role_unauth_reverts() {
        // Drop the universal mock and assert no caller can transfer without
        // the current admin's explicit auth. `try_transfer_admin_role` returns
        // Err because no AuthorizationEntry was supplied.
        let env = Env::default();
        env.mock_all_auths();
        let und_addr = env.register(MockUnderlyingContract, ());
        let und = MockUnderlyingContractClient::new(&env, &und_addr);
        und.initialize(
            &String::from_str(&env, "Mock USD"),
            &String::from_str(&env, "USDC"),
            &7,
        );
        let vs_addr = env.register(MockIbtContract, ());
        let vs = MockIbtContractClient::new(&env, &vs_addr);
        vs.initialize(
            &und_addr,
            &String::from_str(&env, "Vault Share"),
            &String::from_str(&env, "vsTKN"),
            &7,
        );
        let admin = Address::generate(&env);
        let waddr = env.register(RefWrapper, ());
        let wc = RefWrapperClient::new(&env, &waddr);
        wc.initialize(&und_addr, &vs_addr, &0, &admin);

        // Now drop all auth mocking. Subsequent require_auth() must come from
        // a real AuthorizationEntry — none is supplied here.
        env.set_auths(&[]);
        let new_admin = Address::generate(&env);
        let live = env.ledger().sequence() + 1000;
        assert!(wc.try_transfer_admin_role(&new_admin, &live).is_err());
    }
}
