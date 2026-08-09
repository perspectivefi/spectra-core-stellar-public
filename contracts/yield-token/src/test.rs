// SPDX-License-Identifier: BUSL-1.1
use crate::{YieldTokenContract, YieldTokenContractClient};
use soroban_sdk::{
    Address, Env, MuxedAddress, String,
    testutils::{Address as _, Ledger as _},
};
use spectra_mocks::mock_principal_token::{
    LastBeforeYtTransfer, LastUpdateYield, MockKey, MockPrincipalTokenContract,
    MockPrincipalTokenContractClient,
};

fn setup() -> (Env, YieldTokenContractClient<'static>, Address) {
    let env = Env::default();
    env.mock_all_auths();
    let pt = env.register(MockPrincipalTokenContract, ());
    let contract_id = env.register(YieldTokenContract, ());
    let client = YieldTokenContractClient::new(&env, &contract_id);
    let name = String::from_str(&env, "Test YT");
    let symbol = String::from_str(&env, "TYT");
    client.initialize(&name, &symbol, &18u32, &10_000_000_000u64, &pt);
    (env, client, pt)
}

fn advance_to_expiry(env: &Env) {
    // MockPrincipalTokenContract.maturity() returns 10_000_000_000.
    env.ledger().with_mut(|li| {
        li.timestamp = 10_000_000_001;
    });
}

mod init_tests {
    use super::*;

    #[test]
    fn test_initialize_succeeds() {
        let (_env, _client, _pt) = setup();
    }

    #[test]
    fn test_double_initialize_fails() {
        let (env, client, _pt) = setup();
        let pt2 = Address::generate(&env);
        let name = String::from_str(&env, "Other");
        let symbol = String::from_str(&env, "OTH");
        let result = client.try_initialize(&name, &symbol, &18u32, &10_000_000_000u64, &pt2);
        assert!(result.is_err());
    }

    #[test]
    fn test_initialize_stores_pt_name_symbol() {
        let (env, client, pt) = setup();
        assert_eq!(client.get_pt(), pt);
        assert_eq!(client.name(), String::from_str(&env, "Test YT"));
        assert_eq!(client.symbol(), String::from_str(&env, "TYT"));
    }
}

mod sep41_read_tests {
    use super::*;

    #[test]
    fn test_name_returns_stored_name() {
        let (env, client, _pt) = setup();
        assert_eq!(client.name(), String::from_str(&env, "Test YT"));
    }

    #[test]
    fn test_symbol_returns_stored_symbol() {
        let (env, client, _pt) = setup();
        assert_eq!(client.symbol(), String::from_str(&env, "TYT"));
    }

    #[test]
    fn test_decimals_returns_pt_decimals() {
        let (_env, client, _pt) = setup();
        assert_eq!(client.decimals(), 18);
    }
}

mod pause_tests {
    use super::*;

    #[test]
    fn test_pause_unpause_flips_flag() {
        let (_env, client, pt) = setup();
        assert!(!client.paused());
        client.pause(&pt);
        assert!(client.paused());
        client.unpause(&pt);
        assert!(!client.paused());
    }
}

mod mint_tests {
    use super::*;

    #[test]
    fn test_mint_by_pt_increases_balance_and_total_supply() {
        let (env, client, _pt) = setup();
        let recipient = Address::generate(&env);
        client.mint(&recipient, &1000i128);
        assert_eq!(client.actual_balance_of(&recipient), 1000);
        // total_supply is maturity-aware; MockPT's maturity is far future so
        // it reports the real value.
        assert_eq!(client.total_supply(), 1000);
    }
}

mod burn_without_yield_update_tests {
    use super::*;

    #[test]
    fn test_burn_without_yield_update_by_pt_decreases_balance() {
        let (env, client, _pt) = setup();
        let owner = Address::generate(&env);
        client.mint(&owner, &1000i128);
        client.burn_without_yield_update(&owner, &owner, &400i128);
        assert_eq!(client.actual_balance_of(&owner), 600);
        assert_eq!(client.total_supply(), 600);
    }
}

mod burn_tests {
    use super::*;

    #[test]
    fn test_burn_by_owner_decreases_balance() {
        let (env, client, _pt) = setup();
        let owner = Address::generate(&env);
        client.mint(&owner, &1000i128);
        client.burn(&owner, &400i128);
        assert_eq!(client.actual_balance_of(&owner), 600);
        assert_eq!(client.total_supply(), 600);
    }

    #[test]
    fn test_burn_zero_at_expiry_is_noop() {
        let (env, client, _pt) = setup();
        let owner = Address::generate(&env);
        client.mint(&owner, &1000i128);
        advance_to_expiry(&env);
        client.burn(&owner, &0i128);
        assert_eq!(client.actual_balance_of(&owner), 1000);
    }

    #[test]
    fn test_burn_nonzero_at_expiry_fails() {
        let (env, client, _pt) = setup();
        let owner = Address::generate(&env);
        client.mint(&owner, &1000i128);
        advance_to_expiry(&env);
        let result = client.try_burn(&owner, &100i128);
        assert!(result.is_err());
    }
}

// ===========================================================================
// Ports from Solidity: core-v2/test/PrincipalToken/PrincipalToken.t.sol
// ===========================================================================

mod solidity_ports {
    use super::*;

    // Solidity: testYTgetPT
    #[test]
    fn test_yt_get_pt() {
        let (_env, client, pt) = setup();
        assert_eq!(client.get_pt(), pt);
    }

    // Solidity: testYTDecimals
    #[test]
    fn test_yt_decimals_matches_pt() {
        let (_env, client, _pt) = setup();
        // MockPrincipalTokenContract.decimals() returns 18; YT caches it at init.
        assert_eq!(client.decimals(), 18);
    }

    // Solidity: testYTBalanceOfAndTotalSupplyWhenNotExpired
    #[test]
    fn test_balance_and_total_supply_not_expired() {
        let (env, client, _pt) = setup();
        let user = Address::generate(&env);
        client.mint(&user, &1_000_000_000_000_000_000i128); // 1e18
        assert_eq!(client.actual_balance_of(&user), 1_000_000_000_000_000_000);
        assert_eq!(client.balance(&user), 1_000_000_000_000_000_000);
        assert_eq!(client.total_supply(), 1_000_000_000_000_000_000);
    }

    // Solidity: testYTBalanceOfAndTotalSupplyWhenExpired
    #[test]
    fn test_balance_and_total_supply_after_expiry() {
        let (env, client, _pt) = setup();
        let user = Address::generate(&env);
        client.mint(&user, &1_000_000_000_000_000_000i128);

        // Pre-expiry snapshot.
        assert_eq!(client.balance(&user), 1_000_000_000_000_000_000);
        assert_eq!(client.total_supply(), 1_000_000_000_000_000_000);

        advance_to_expiry(&env);

        // Post-expiry: maturity-aware views return 0, actual stays.
        assert_eq!(client.balance(&user), 0);
        assert_eq!(client.total_supply(), 0);
        assert_eq!(client.actual_balance_of(&user), 1_000_000_000_000_000_000);
    }

    // Solidity: testYTTransferFrom
    #[test]
    fn test_transfer_from_moves_balance() {
        let (env, client, _pt) = setup();
        let owner = Address::generate(&env);
        let recipient = Address::generate(&env);
        let spender = Address::generate(&env);

        client.mint(&owner, &1_000_000_000_000_000_000i128);
        client.approve(&owner, &spender, &1_000_000_000_000_000_000i128, &1000u32);
        client.transfer_from(&spender, &owner, &recipient, &1_000_000_000_000_000_000i128);

        assert_eq!(client.actual_balance_of(&owner), 0);
        assert_eq!(
            client.actual_balance_of(&recipient),
            1_000_000_000_000_000_000
        );
    }

    // Solidity: testYTTransferFails — expired + amount > balance
    #[test]
    fn test_transfer_insufficient_balance_fails() {
        let (env, client, _pt) = setup();
        let from = Address::generate(&env);
        let to = Address::generate(&env);
        client.mint(&from, &100i128);
        let result = client.try_transfer(&from, MuxedAddress::from(to.clone()), &200i128);
        assert!(result.is_err());
    }

    // Solidity: testYTTransferFails — post-expiry with amount != 0
    #[test]
    fn test_transfer_at_expiry_nonzero_fails() {
        let (env, client, _pt) = setup();
        let from = Address::generate(&env);
        let to = Address::generate(&env);
        client.mint(&from, &1000i128);
        advance_to_expiry(&env);
        let result = client.try_transfer(&from, MuxedAddress::from(to.clone()), &100i128);
        assert!(result.is_err());
    }

    // Solidity: testYTTransferFromFails — expired + amount > balance via transferFrom
    #[test]
    fn test_transfer_from_at_expiry_nonzero_fails() {
        let (env, client, _pt) = setup();
        let owner = Address::generate(&env);
        let recipient = Address::generate(&env);
        let spender = Address::generate(&env);
        client.mint(&owner, &1000i128);
        client.approve(&owner, &spender, &1000i128, &1000u32);
        advance_to_expiry(&env);
        let result = client.try_transfer_from(&spender, &owner, &recipient, &100i128);
        assert!(result.is_err());
    }

    // Solidity: testburnWithoutYieldUpdateOnBehalf
    #[test]
    fn test_burn_without_yield_update_on_behalf() {
        let (env, client, _pt) = setup();
        let owner = Address::generate(&env);
        let spender = Address::generate(&env);
        client.mint(&owner, &1000i128);
        client.approve(&owner, &spender, &500i128, &1000u32);
        client.burn_without_yield_update(&owner, &spender, &500i128);
        assert_eq!(client.actual_balance_of(&owner), 500);
    }

    // Solidity: testburnWithoutYieldUpdateOnBehalfWithInsufficientAllowanceFail
    #[test]
    fn test_burn_without_yield_update_on_behalf_without_allowance_fails() {
        let (env, client, _pt) = setup();
        let owner = Address::generate(&env);
        let spender = Address::generate(&env);
        client.mint(&owner, &1000i128);
        // No approve.
        let result = client.try_burn_without_yield_update(&owner, &spender, &500i128);
        assert!(result.is_err());
    }
}

// ===========================================================================
// Extra tests (beyond the Solidity set)
// ===========================================================================

mod extra_tests {
    use super::*;

    #[test]
    fn test_transfer_zero_at_expiry_succeeds() {
        // Solidity doesn't test this path — spec-03 §8 lists it.
        let (env, client, _pt) = setup();
        let from = Address::generate(&env);
        let to = Address::generate(&env);
        client.mint(&from, &100i128);
        advance_to_expiry(&env);
        client.transfer(&from, MuxedAddress::from(to.clone()), &0i128); // no-op, should not panic
    }

    #[test]
    fn test_mint_when_paused_fails() {
        let (env, client, pt) = setup();
        let recipient = Address::generate(&env);
        client.pause(&pt);
        let result = client.try_mint(&recipient, &100i128);
        assert!(result.is_err());
    }

    #[test]
    fn test_burn_when_paused_fails() {
        let (env, client, pt) = setup();
        let owner = Address::generate(&env);
        client.mint(&owner, &100i128);
        client.pause(&pt);
        let result = client.try_burn(&owner, &10i128);
        assert!(result.is_err());
    }

    #[test]
    fn test_transfer_when_paused_fails() {
        let (env, client, pt) = setup();
        let from = Address::generate(&env);
        let to = Address::generate(&env);
        client.mint(&from, &100i128);
        client.pause(&pt);
        let result = client.try_transfer(&from, MuxedAddress::from(to.clone()), &10i128);
        assert!(result.is_err());
    }

    #[test]
    fn test_maturity_boundary_expiry_minus_one_still_live() {
        // One second before expiry, YT is still live.
        let (env, client, _pt) = setup();
        let user = Address::generate(&env);
        client.mint(&user, &1000i128);
        env.ledger().with_mut(|li| {
            li.timestamp = 9_999_999_999; // maturity - 1
        });
        assert_eq!(client.balance(&user), 1000);
        assert_eq!(client.total_supply(), 1000);
    }

    #[test]
    fn test_maturity_boundary_exact_expiry_is_expired() {
        // At exactly maturity timestamp, YT is expired.
        let (env, client, _pt) = setup();
        let user = Address::generate(&env);
        client.mint(&user, &1000i128);
        env.ledger().with_mut(|li| {
            li.timestamp = 10_000_000_000;
        });
        assert_eq!(client.balance(&user), 0);
        assert_eq!(client.total_supply(), 0);
    }
}

mod allowance_tests {
    use super::*;

    #[test]
    fn test_approve_sets_allowance() {
        let (env, client, _pt) = setup();
        let owner = Address::generate(&env);
        let spender = Address::generate(&env);
        client.approve(&owner, &spender, &500i128, &1000u32);
        assert_eq!(client.allowance(&owner, &spender), 500);
    }

    #[test]
    fn test_allowance_decrements_after_transfer_from() {
        let (env, client, _pt) = setup();
        let owner = Address::generate(&env);
        let recipient = Address::generate(&env);
        let spender = Address::generate(&env);
        client.mint(&owner, &1000i128);
        client.approve(&owner, &spender, &500i128, &1000u32);
        client.transfer_from(&spender, &owner, &recipient, &300i128);
        assert_eq!(client.allowance(&owner, &spender), 200);
    }

    #[test]
    fn test_expired_allowance_cannot_be_spent() {
        let (env, client, _pt) = setup();
        let owner = Address::generate(&env);
        let recipient = Address::generate(&env);
        let spender = Address::generate(&env);
        client.mint(&owner, &1000i128);
        // live_until_ledger = current sequence; allowance expires before
        // spender can use it.
        let current = env.ledger().sequence();
        client.approve(&owner, &spender, &500i128, &current);
        // Advance one ledger past expiry.
        env.ledger().with_mut(|li| li.sequence_number = current + 1);
        let result = client.try_transfer_from(&spender, &owner, &recipient, &100i128);
        assert!(result.is_err());
    }

    #[test]
    fn test_burn_from_decrements_allowance_and_balance() {
        let (env, client, _pt) = setup();
        let owner = Address::generate(&env);
        let spender = Address::generate(&env);
        client.mint(&owner, &1000i128);
        client.approve(&owner, &spender, &600i128, &1000u32);

        // on-behalf burn via burn_without_yield_update — PT path
        client.burn_without_yield_update(&owner, &spender, &400i128);

        assert_eq!(client.actual_balance_of(&owner), 600);
        assert_eq!(client.allowance(&owner, &spender), 200);
    }
}

// ===========================================================================
// Pause propagation: PT pushes set_paused through to YT
// ===========================================================================

mod pause_propagation_tests {
    use super::*;

    fn mock_pt_client<'a>(env: &Env, pt: &Address) -> MockPrincipalTokenContractClient<'a> {
        MockPrincipalTokenContractClient::new(env, pt)
    }

    #[test]
    fn test_pt_forward_pause_sets_yt_paused() {
        let (env, yt_client, pt) = setup();
        let pt_client = mock_pt_client(&env, &pt);

        assert!(!yt_client.paused());
        pt_client.forward_pause(&yt_client.address);
        assert!(yt_client.paused());
    }

    #[test]
    fn test_pt_forward_unpause_clears_yt_paused() {
        let (env, yt_client, pt) = setup();
        let pt_client = mock_pt_client(&env, &pt);

        pt_client.forward_pause(&yt_client.address);
        assert!(yt_client.paused());
        pt_client.forward_unpause(&yt_client.address);
        assert!(!yt_client.paused());
    }

    #[test]
    fn test_pause_blocks_state_changing_ops_after_pt_push() {
        let (env, yt_client, pt) = setup();
        let pt_client = mock_pt_client(&env, &pt);
        let user = Address::generate(&env);
        yt_client.mint(&user, &100i128);

        pt_client.forward_pause(&yt_client.address);

        let mint_result = yt_client.try_mint(&user, &50i128);
        assert!(mint_result.is_err());

        let burn_result = yt_client.try_burn(&user, &10i128);
        assert!(burn_result.is_err());
    }
}

// ===========================================================================
// Recording tests: YT passes correct pre-op balances to PT callbacks
// (spec-04 §17.1)
// ===========================================================================

mod callback_arg_tests {
    use super::*;

    fn read_last_before_yt_transfer(env: &Env, pt: &Address) -> LastBeforeYtTransfer {
        env.as_contract(pt, || {
            env.storage()
                .instance()
                .get(&MockKey::LastBeforeYtTransfer)
                .unwrap()
        })
    }

    fn read_last_update_yield(env: &Env, pt: &Address) -> LastUpdateYield {
        env.as_contract(pt, || {
            env.storage()
                .instance()
                .get(&MockKey::LastUpdateYield)
                .unwrap()
        })
    }

    #[test]
    fn test_transfer_passes_pre_transfer_balances_to_pt() {
        let (env, client, pt) = setup();
        let alice = Address::generate(&env);
        let bob = Address::generate(&env);
        client.mint(&alice, &1000i128);
        client.mint(&bob, &500i128);

        client.transfer(&alice, MuxedAddress::from(bob.clone()), &200i128);

        let last = read_last_before_yt_transfer(&env, &pt);
        assert_eq!(last.from, alice);
        assert_eq!(last.to, bob);
        // Pre-transfer balances — NOT post-transfer.
        assert_eq!(last.from_balance, 1000);
        assert_eq!(last.to_balance, 500);
    }

    #[test]
    fn test_transfer_from_passes_pre_transfer_balances_to_pt() {
        let (env, client, pt) = setup();
        let alice = Address::generate(&env);
        let bob = Address::generate(&env);
        let spender = Address::generate(&env);
        client.mint(&alice, &1000i128);
        client.mint(&bob, &500i128);
        client.approve(&alice, &spender, &300i128, &1000u32);

        client.transfer_from(&spender, &alice, &bob, &200i128);

        let last = read_last_before_yt_transfer(&env, &pt);
        assert_eq!(last.from, alice);
        assert_eq!(last.to, bob);
        assert_eq!(last.from_balance, 1000);
        assert_eq!(last.to_balance, 500);
    }

    // a zero-amount `transfer_from` needs no allowance, so before the fix
    // `yt.transfer_from(attacker, victim, attacker, 0)` invoked
    // `before_yt_transfer` and settled the victim's yield on the attacker's
    // signature alone. The short-circuit must skip the callback entirely.
    #[test]
    fn test_zero_amount_transfer_from_does_not_settle_yield() {
        let (env, client, pt) = setup();
        let victim = Address::generate(&env);
        let bob = Address::generate(&env);
        let attacker = Address::generate(&env);
        client.mint(&victim, &1000i128);
        client.mint(&bob, &500i128);
        // Prime the recorded callback with a distinctive `to`.
        client.transfer(&victim, MuxedAddress::from(bob.clone()), &1i128);
        assert_eq!(read_last_before_yt_transfer(&env, &pt).to, bob);

        // Attacker holds no allowance; the zero transfer must be a no-op.
        client.transfer_from(&attacker, &victim, &attacker, &0i128);

        // The callback record is unchanged: `before_yt_transfer` never fired.
        let last = read_last_before_yt_transfer(&env, &pt);
        assert_eq!(
            last.to, bob,
            "zero-amount transfer_from must not invoke before_yt_transfer"
        );
    }

    #[test]
    fn test_zero_amount_transfer_does_not_settle_yield() {
        let (env, client, pt) = setup();
        let victim = Address::generate(&env);
        let bob = Address::generate(&env);
        client.mint(&victim, &1000i128);
        // Prime the recorded callback with a distinctive `to`.
        client.transfer(&victim, MuxedAddress::from(bob.clone()), &1i128);
        assert_eq!(read_last_before_yt_transfer(&env, &pt).to, bob);

        // Zero transfer must be a no-op.
        client.transfer(&victim, MuxedAddress::from(bob.clone()), &0i128);

        // The callback record is unchanged: `before_yt_transfer` never fired.
        let last = read_last_before_yt_transfer(&env, &pt);
        assert_eq!(
            last.to, bob,
            "zero-amount transfer must not invoke before_yt_transfer"
        );
    }

    #[test]
    fn test_burn_passes_pre_burn_balance_to_update_yield() {
        let (env, client, pt) = setup();
        let user = Address::generate(&env);
        client.mint(&user, &750i128);

        client.burn(&user, &100i128);

        let last = read_last_update_yield(&env, &pt);
        assert_eq!(last.user, user);
        // Pre-burn balance — NOT post-burn (650).
        assert_eq!(last.user_yt_balance, 750);
    }
}

// ===========================================================================
// Auth-scope: PT-only functions reject non-PT callers
// ===========================================================================

mod auth_scope_tests {
    use super::*;
    use soroban_sdk::{
        IntoVal,
        testutils::{MockAuth, MockAuthInvoke},
    };

    /// Setup WITHOUT mock_all_auths — individual calls must specify their auth.
    fn setup_scoped() -> (Env, YieldTokenContractClient<'static>, Address) {
        let env = Env::default();
        let pt = env.register(MockPrincipalTokenContract, ());
        let yt_id = env.register(YieldTokenContract, ());
        let client = YieldTokenContractClient::new(&env, &yt_id);
        // initialize has no require_auth calls, so no auth needed.
        let name = String::from_str(&env, "Test YT");
        let symbol = String::from_str(&env, "TYT");
        client.initialize(&name, &symbol, &18u32, &10_000_000_000u64, &pt);
        (env, client, pt)
    }

    #[test]
    fn test_mint_without_pt_auth_fails() {
        let (env, client, _pt) = setup_scoped();
        let non_pt = Address::generate(&env);
        let recipient = Address::generate(&env);

        let args = (recipient.clone(), 100i128).into_val(&env);
        let result = client
            .mock_auths(&[MockAuth {
                address: &non_pt,
                invoke: &MockAuthInvoke {
                    contract: &client.address,
                    fn_name: "mint",
                    args,
                    sub_invokes: &[],
                },
            }])
            .try_mint(&recipient, &100i128);
        assert!(result.is_err());
    }

    #[test]
    fn test_burn_without_yield_update_without_pt_auth_fails() {
        let (env, client, pt) = setup_scoped();
        let non_pt = Address::generate(&env);
        let owner = Address::generate(&env);

        // Mint first (PT-authorized).
        let mint_args = (owner.clone(), 1000i128).into_val(&env);
        client
            .mock_auths(&[MockAuth {
                address: &pt,
                invoke: &MockAuthInvoke {
                    contract: &client.address,
                    fn_name: "mint",
                    args: mint_args,
                    sub_invokes: &[],
                },
            }])
            .mint(&owner, &1000i128);

        // Attempt burn_without_yield_update with non-PT caller.
        let burn_args = (owner.clone(), owner.clone(), 100i128).into_val(&env);
        let result = client
            .mock_auths(&[MockAuth {
                address: &non_pt,
                invoke: &MockAuthInvoke {
                    contract: &client.address,
                    fn_name: "burn_without_yield_update",
                    args: burn_args,
                    sub_invokes: &[],
                },
            }])
            .try_burn_without_yield_update(&owner, &owner, &100i128);
        assert!(result.is_err());
    }

    #[test]
    fn test_pause_without_pt_auth_fails() {
        let (env, client, _pt) = setup_scoped();
        let non_pt = Address::generate(&env);

        let args = (non_pt.clone(),).into_val(&env);
        let result = client
            .mock_auths(&[MockAuth {
                address: &non_pt,
                invoke: &MockAuthInvoke {
                    contract: &client.address,
                    fn_name: "pause",
                    args,
                    sub_invokes: &[],
                },
            }])
            .try_pause(&non_pt);
        assert!(result.is_err());
    }

    // Mirror of test_pause_without_pt_auth_fails for the sister entry point.
    // `unpause` shares the same `pt.require_auth()` gate (lib.rs:134) and the
    // same ignored `_caller` arg; without an explicit negative test, a future
    // refactor that drops or weakens the gate would not be caught.
    #[test]
    fn test_unpause_without_pt_auth_fails() {
        let (env, client, _pt) = setup_scoped();
        let non_pt = Address::generate(&env);

        let args = (non_pt.clone(),).into_val(&env);
        let result = client
            .mock_auths(&[MockAuth {
                address: &non_pt,
                invoke: &MockAuthInvoke {
                    contract: &client.address,
                    fn_name: "unpause",
                    args,
                    sub_invokes: &[],
                },
            }])
            .try_unpause(&non_pt);
        assert!(result.is_err());
    }

    // `burn` is gated by `Base::burn`'s internal `caller.require_auth()` — not
    // a YT-side `require_auth` call. The positive paths run under blanket
    // `mock_all_auths` and therefore cannot distinguish "owner auth required"
    // from "no auth check at all". Lock the upstream invariant in: supply
    // auth for a non-owner, call burn for the owner, expect rejection.
    #[test]
    fn test_burn_without_caller_auth_fails() {
        let (env, client, pt) = setup_scoped();
        let owner = Address::generate(&env);

        // Mint with PT auth so the owner has a balance to burn.
        let mint_args = (owner.clone(), 1000i128).into_val(&env);
        client
            .mock_auths(&[MockAuth {
                address: &pt,
                invoke: &MockAuthInvoke {
                    contract: &client.address,
                    fn_name: "mint",
                    args: mint_args,
                    sub_invokes: &[],
                },
            }])
            .mint(&owner, &1000i128);

        // Attempt burn with auth provided by a non-owner address. Base::burn
        // calls owner.require_auth() internally — supplying a different
        // address's signature must not satisfy that gate.
        let non_owner = Address::generate(&env);
        let burn_args = (owner.clone(), 100i128).into_val(&env);
        let result = client
            .mock_auths(&[MockAuth {
                address: &non_owner,
                invoke: &MockAuthInvoke {
                    contract: &client.address,
                    fn_name: "burn",
                    args: burn_args,
                    sub_invokes: &[],
                },
            }])
            .try_burn(&owner, &100i128);
        assert!(result.is_err());
    }

    /// Self-path burn_without_yield_update with proper auths.
    /// Owner authorizes the burn themselves (they are the one initiating the
    /// redeem via PT). `Base::burn(env, &owner, amount)` requires owner.
    #[test]
    fn test_burn_without_yield_update_self_with_proper_auths() {
        let (env, client, pt) = setup_scoped();
        let owner = Address::generate(&env);

        // Mint (PT-authorized).
        let mint_args = (owner.clone(), 1000i128).into_val(&env);
        client
            .mock_auths(&[MockAuth {
                address: &pt,
                invoke: &MockAuthInvoke {
                    contract: &client.address,
                    fn_name: "mint",
                    args: mint_args,
                    sub_invokes: &[],
                },
            }])
            .mint(&owner, &1000i128);

        // burn_without_yield_update in self-path: auth chain needs both
        // PT (gate) and owner (Base::burn's internal require_auth).
        let burn_args: soroban_sdk::Vec<soroban_sdk::Val> =
            (owner.clone(), owner.clone(), 500i128).into_val(&env);
        client
            .mock_auths(&[
                MockAuth {
                    address: &pt,
                    invoke: &MockAuthInvoke {
                        contract: &client.address,
                        fn_name: "burn_without_yield_update",
                        args: burn_args.clone(),
                        sub_invokes: &[],
                    },
                },
                MockAuth {
                    address: &owner,
                    invoke: &MockAuthInvoke {
                        contract: &client.address,
                        fn_name: "burn_without_yield_update",
                        args: burn_args,
                        sub_invokes: &[],
                    },
                },
            ])
            .burn_without_yield_update(&owner, &owner, &500i128);

        assert_eq!(client.actual_balance_of(&owner), 500);
    }

    /// On-behalf burn with proper PT auth + spender auth + prior approval.
    /// Would have failed with the old impl that used `Base::burn(env, &owner)`
    /// (since owner didn't authorize). Passes with the new impl that uses
    /// `Base::burn_from(env, &spender, &owner)` — spender auth + allowance.
    #[test]
    fn test_burn_without_yield_update_on_behalf_with_proper_auths() {
        let (env, client, pt) = setup_scoped();
        let owner = Address::generate(&env);
        let spender = Address::generate(&env);

        // Mint + approve need mocked auths (owner / PT).
        let mint_args = (owner.clone(), 1000i128).into_val(&env);
        client
            .mock_auths(&[MockAuth {
                address: &pt,
                invoke: &MockAuthInvoke {
                    contract: &client.address,
                    fn_name: "mint",
                    args: mint_args,
                    sub_invokes: &[],
                },
            }])
            .mint(&owner, &1000i128);

        let approve_args = (owner.clone(), spender.clone(), 500i128, 1000u32).into_val(&env);
        client
            .mock_auths(&[MockAuth {
                address: &owner,
                invoke: &MockAuthInvoke {
                    contract: &client.address,
                    fn_name: "approve",
                    args: approve_args,
                    sub_invokes: &[],
                },
            }])
            .approve(&owner, &spender, &500i128, &1000u32);

        // Actual test: PT invokes burn_without_yield_update on spender's behalf.
        // Auth chain must include PT (gate) and spender (for Base::burn_from's
        // internal require_auth). Owner is NOT in the chain — the bug surface.
        let burn_args: soroban_sdk::Vec<soroban_sdk::Val> =
            (owner.clone(), spender.clone(), 500i128).into_val(&env);
        client
            .mock_auths(&[
                MockAuth {
                    address: &pt,
                    invoke: &MockAuthInvoke {
                        contract: &client.address,
                        fn_name: "burn_without_yield_update",
                        args: burn_args.clone(),
                        sub_invokes: &[],
                    },
                },
                MockAuth {
                    address: &spender,
                    invoke: &MockAuthInvoke {
                        contract: &client.address,
                        fn_name: "burn_without_yield_update",
                        args: burn_args,
                        sub_invokes: &[],
                    },
                },
            ])
            .burn_without_yield_update(&owner, &spender, &500i128);

        assert_eq!(client.actual_balance_of(&owner), 500);
    }
}

// ===========================================================================
// YT.DoS.2 — YT caches `maturity` at `initialize`; later changes to the
// PT's maturity must not affect YT's view of expiry. RED on the live-call
// implementation (which would re-read `pt.maturity()` on every `is_expired`).
// ===========================================================================

mod cached_maturity_tests {
    use super::*;

    fn override_pt_maturity(env: &Env, pt: &Address, value: u64) {
        env.as_contract(pt, || {
            env.storage().instance().set(&MockKey::Maturity, &value);
        });
    }

    #[test]
    fn test_balance_uses_cached_maturity_after_pt_changes() {
        let (env, client, pt) = setup();
        let user = Address::generate(&env);
        client.mint(&user, &1000i128);

        env.ledger().with_mut(|li| li.timestamp = 100);
        // PT reports a maturity in the past — would force `is_expired = true`
        // on a live read, but YT cached 10_000_000_000 at init.
        override_pt_maturity(&env, &pt, 50);

        assert_eq!(client.balance(&user), 1000);
    }

    #[test]
    fn test_total_supply_uses_cached_maturity_after_pt_changes() {
        let (env, client, pt) = setup();
        let user = Address::generate(&env);
        client.mint(&user, &1000i128);

        env.ledger().with_mut(|li| li.timestamp = 100);
        override_pt_maturity(&env, &pt, 50);

        assert_eq!(client.total_supply(), 1000);
    }

    #[test]
    fn test_transfer_uses_cached_maturity_after_pt_changes() {
        let (env, client, pt) = setup();
        let from = Address::generate(&env);
        let to = Address::generate(&env);
        client.mint(&from, &1000i128);

        env.ledger().with_mut(|li| li.timestamp = 100);
        override_pt_maturity(&env, &pt, 50);

        // Cached maturity = 10_000_000_000; ledger.timestamp = 100 < cached.
        // Live read would see 50, decide expired, and panic via is_expired.
        client.transfer(&from, MuxedAddress::from(to.clone()), &10i128);
        assert_eq!(client.actual_balance_of(&from), 990);
    }
}

// ===========================================================================
// YT.Info.2 — `allowance` panics post-expiry (Soroban-only hardening beyond
// Solidity parity; transferFrom already reverts post-expiry).
// ===========================================================================

mod allowance_expiry_tests {
    use super::*;

    #[test]
    fn test_allowance_panics_post_expiry() {
        let (env, client, _pt) = setup();
        let owner = Address::generate(&env);
        let spender = Address::generate(&env);
        client.approve(&owner, &spender, &500i128, &1_000_000u32);
        advance_to_expiry(&env);

        let result = client.try_allowance(&owner, &spender);
        assert!(result.is_err());
    }

    #[test]
    fn test_allowance_panics_post_expiry_with_no_prior_approve() {
        let (env, client, _pt) = setup();
        let owner = Address::generate(&env);
        let spender = Address::generate(&env);
        advance_to_expiry(&env);

        let result = client.try_allowance(&owner, &spender);
        assert!(result.is_err());
    }
}

mod upgrade_tests {
    use super::*;
    use soroban_sdk::{
        BytesN, IntoVal,
        testutils::{MockAuth, MockAuthInvoke},
    };

    fn setup_scoped() -> (Env, YieldTokenContractClient<'static>, Address) {
        let env = Env::default();
        let pt = env.register(MockPrincipalTokenContract, ());
        let yt_id = env.register(YieldTokenContract, ());
        let client = YieldTokenContractClient::new(&env, &yt_id);
        let name = String::from_str(&env, "Test YT");
        let symbol = String::from_str(&env, "TYT");
        client.initialize(&name, &symbol, &18u32, &10_000_000_000u64, &pt);
        (env, client, pt)
    }

    #[test]
    fn test_upgrade_without_pt_auth_fails() {
        let (env, client, _pt) = setup_scoped();
        let non_pt = Address::generate(&env);
        let hash = BytesN::from_array(&env, &[1u8; 32]);

        let args = (hash.clone(),).into_val(&env);
        let result = client
            .mock_auths(&[MockAuth {
                address: &non_pt,
                invoke: &MockAuthInvoke {
                    contract: &client.address,
                    fn_name: "upgrade",
                    args,
                    sub_invokes: &[],
                },
            }])
            .try_upgrade(&hash);
        assert!(result.is_err());
    }
}

// ===========================================================================
// BREACH-HUNT: yield-token red-team + regression
// ===========================================================================
//
// 36 tests covering: PT-only auth surface, post-expiry hardening,
// yield-settlement ordering, pause cascade, init edges, expiry boundary,
// composability, and adversarial inputs.
// ===========================================================================

mod breach_hunt {
    use super::*;
    use soroban_sdk::{IntoVal, Val, Vec as SVec, testutils::MockAuth, testutils::MockAuthInvoke};

    // ---------------------------------------------------------------------
    // Helpers
    // ---------------------------------------------------------------------

    /// Setup with custom maturity (default is 10_000_000_000).
    fn setup_with_maturity(maturity: u64) -> (Env, YieldTokenContractClient<'static>, Address) {
        let env = Env::default();
        env.mock_all_auths();
        let pt = env.register(MockPrincipalTokenContract, ());
        env.as_contract(&pt, || {
            env.storage().instance().set(&MockKey::Maturity, &maturity);
        });
        let yt = env.register(YieldTokenContract, ());
        let client = YieldTokenContractClient::new(&env, &yt);
        let name = String::from_str(&env, "Test YT");
        let symbol = String::from_str(&env, "TYT");
        client.initialize(&name, &symbol, &18u32, &maturity, &pt);
        (env, client, pt)
    }

    /// Pre-expiry timestamp (well before default maturity).
    fn pre_expiry_ts() -> u64 {
        1_000
    }

    fn at_expiry_ts() -> u64 {
        10_000_000_000
    }

    fn post_expiry_ts() -> u64 {
        10_000_000_001
    }

    // ---------------------------------------------------------------------
    // 1-5. Authorization / spoofing red-team
    // ---------------------------------------------------------------------

    /// T1: Non-PT cannot mint. Drop mock_all_auths, attempt mint without PT
    /// signature. Must fail.
    #[test]
    fn test_t1_non_pt_cannot_mint() {
        let (env, client, _pt) = setup();
        let attacker = Address::generate(&env);
        env.set_auths(&[]);
        let r = client.try_mint(&attacker, &1_000);
        assert!(r.is_err(), "mint without PT auth must fail");
    }

    /// T2: Non-PT cannot burn_without_yield_update.
    #[test]
    fn test_t2_non_pt_cannot_burn_without_yield_update() {
        let (env, client, pt) = setup();
        let victim = Address::generate(&env);
        client.mint(&victim, &1_000);

        env.set_auths(&[]);
        let r = client.try_burn_without_yield_update(&victim, &victim, &500);
        assert!(
            r.is_err(),
            "burn_without_yield_update without PT auth must fail"
        );
        let _ = pt;
    }

    /// T3: Non-PT cannot pause / unpause / upgrade.
    #[test]
    fn test_t3_non_pt_cannot_pause_unpause_upgrade() {
        let (env, client, _pt) = setup();
        let attacker = Address::generate(&env);
        env.set_auths(&[]);

        let r_pause = client.try_pause(&attacker);
        assert!(r_pause.is_err(), "non-PT must not pause");

        // For upgrade: same — no PT auth.
        let hash = soroban_sdk::BytesN::<32>::from_array(&env, &[1u8; 32]);
        let r_upg = client.try_upgrade(&hash);
        assert!(r_upg.is_err(), "non-PT must not upgrade");
    }

    /// T4: PT-driven self-burn path. `burn_without_yield_update(owner=A,
    /// spender=A)` skips Base's allowance bookkeeping, deferring auth to PT.
    /// Under mock_all_auths PT auth is satisfied; we pin that the YT side
    /// does NOT independently require the owner's signature, by invoking
    /// the function with set_auths providing only PT auth.
    #[test]
    fn test_t4_burn_without_yield_update_self_path_pt_only_auth() {
        let (env, client, pt) = setup();
        let user = Address::generate(&env);
        client.mint(&user, &1_000);

        // Auth only PT (not user). The self-path skips owner auth.
        let burn_args = (user.clone(), user.clone(), 500i128).into_val(&env);
        client
            .mock_auths(&[MockAuth {
                address: &pt,
                invoke: &MockAuthInvoke {
                    contract: &client.address,
                    fn_name: "burn_without_yield_update",
                    args: burn_args,
                    sub_invokes: &[],
                },
            }])
            .burn_without_yield_update(&user, &user, &500);

        assert_eq!(client.actual_balance_of(&user), 500);
    }

    /// T5: Behalf-of `burn_without_yield_update` consumes spender's
    /// allowance from owner exactly.
    #[test]
    fn test_t5_burn_without_yield_update_behalf_consumes_allowance_exactly() {
        let (env, client, _pt) = setup();
        let owner = Address::generate(&env);
        let spender = Address::generate(&env);
        client.mint(&owner, &1_000);
        client.approve(&owner, &spender, &600, &(env.ledger().sequence() + 1000));
        assert_eq!(client.allowance(&owner, &spender), 600);

        client.burn_without_yield_update(&owner, &spender, &400);
        assert_eq!(client.actual_balance_of(&owner), 600);
        assert_eq!(
            client.allowance(&owner, &spender),
            200,
            "allowance must decrement by exactly 400"
        );
    }

    // ---------------------------------------------------------------------
    // 6-14. Post-expiry hardening
    // ---------------------------------------------------------------------

    /// T6: `balance` returns 0 post-expiry while `actual_balance_of` still
    /// returns the storage value.
    #[test]
    fn test_t6_balance_zero_post_expiry_but_actual_balance_intact() {
        let (env, client, _pt) = setup();
        let user = Address::generate(&env);
        client.mint(&user, &1_000);

        env.ledger().with_mut(|li| li.timestamp = post_expiry_ts());
        assert_eq!(client.balance(&user), 0, "balance reads 0 post-expiry");
        assert_eq!(
            client.actual_balance_of(&user),
            1_000,
            "actual_balance_of stays intact post-expiry"
        );
    }

    /// T7: `total_supply` returns 0 post-expiry.
    #[test]
    fn test_t7_total_supply_zero_post_expiry() {
        let (env, client, _pt) = setup();
        let u = Address::generate(&env);
        client.mint(&u, &1_000);

        env.ledger().with_mut(|li| li.timestamp = post_expiry_ts());
        assert_eq!(client.total_supply(), 0);
    }

    /// T8: `allowance` panics post-expiry with InsufficientBalance.
    #[test]
    fn test_t8_allowance_panics_post_expiry_with_insufficient_balance() {
        let (env, client, _pt) = setup();
        let owner = Address::generate(&env);
        let spender = Address::generate(&env);
        client.approve(&owner, &spender, &100, &(env.ledger().sequence() + 1000));

        env.ledger().with_mut(|li| li.timestamp = post_expiry_ts());
        let r = client.try_allowance(&owner, &spender);
        assert!(r.is_err(), "allowance post-expiry must panic");
    }

    /// T9: `transfer(amount != 0)` post-expiry panics with InsufficientBalance.
    #[test]
    fn test_t9_transfer_nonzero_post_expiry_panics() {
        let (env, client, _pt) = setup();
        let from = Address::generate(&env);
        let to: MuxedAddress = Address::generate(&env).into();
        client.mint(&from, &1_000);

        env.ledger().with_mut(|li| li.timestamp = post_expiry_ts());
        let r = client.try_transfer(&from, &to, &1);
        assert!(r.is_err(), "transfer(nonzero) post-expiry must panic");
    }

    /// T10: `transfer(amount == 0)` post-expiry succeeds.
    #[test]
    fn test_t10_transfer_zero_post_expiry_succeeds() {
        let (env, client, _pt) = setup();
        let from = Address::generate(&env);
        let to_addr = Address::generate(&env);
        let to: MuxedAddress = to_addr.clone().into();
        client.mint(&from, &1_000);

        env.ledger().with_mut(|li| li.timestamp = post_expiry_ts());
        client.transfer(&from, &to, &0);
        assert_eq!(client.actual_balance_of(&from), 1_000);
        assert_eq!(client.actual_balance_of(&to_addr), 0);
    }

    /// T11: `transfer_from(amount != 0)` post-expiry panics.
    #[test]
    fn test_t11_transfer_from_nonzero_post_expiry_panics() {
        let (env, client, _pt) = setup();
        let owner = Address::generate(&env);
        let spender = Address::generate(&env);
        let to = Address::generate(&env);
        client.mint(&owner, &1_000);
        client.approve(&owner, &spender, &500, &(env.ledger().sequence() + 1000));

        env.ledger().with_mut(|li| li.timestamp = post_expiry_ts());
        let r = client.try_transfer_from(&spender, &owner, &to, &1);
        assert!(r.is_err(), "transfer_from(nonzero) post-expiry must panic");
    }

    /// T12: `approve()` still works post-expiry (approval is allowed even
    /// though it's effectively dead).
    #[test]
    fn test_t12_approve_works_post_expiry() {
        let (env, client, _pt) = setup();
        let owner = Address::generate(&env);
        let spender = Address::generate(&env);

        env.ledger().with_mut(|li| li.timestamp = post_expiry_ts());
        client.approve(&owner, &spender, &100, &(env.ledger().sequence() + 1000));
        // We can't read it back via allowance() because that panics post-expiry,
        // but the call itself succeeded.
    }

    /// T13: `burn(amount != 0)` post-expiry panics with InsufficientBalance.
    #[test]
    fn test_t13_burn_nonzero_post_expiry_panics() {
        let (env, client, _pt) = setup();
        let u = Address::generate(&env);
        client.mint(&u, &1_000);

        env.ledger().with_mut(|li| li.timestamp = post_expiry_ts());
        let r = client.try_burn(&u, &1);
        assert!(r.is_err(), "burn(nonzero) post-expiry must panic");
    }

    /// T14: `burn(0)` post-expiry is a no-op. The `amount == 0` short-circuit
    /// in `burn` returns before the expiry guard *and* before
    /// `pt.update_yield`, so no balance changes and no cross-contract call is
    /// made. Pins that the short-circuit skips the yield update.
    #[test]
    fn test_t14_burn_zero_post_expiry_is_noop() {
        let (env, client, pt) = setup();
        let u = Address::generate(&env);
        client.mint(&u, &1_000);

        env.ledger().with_mut(|li| li.timestamp = post_expiry_ts());
        client.burn(&u, &0);
        assert_eq!(client.actual_balance_of(&u), 1_000);

        // burn(0) short-circuits: PT.update_yield is NOT called.
        let last: Option<LastUpdateYield> = env.as_contract(&pt, || {
            env.storage().instance().get(&MockKey::LastUpdateYield)
        });
        assert!(
            last.is_none(),
            "burn(0) post-expiry must short-circuit before PT.update_yield"
        );
    }

    // ---------------------------------------------------------------------
    // 15-19. Yield-settlement E2E
    // ---------------------------------------------------------------------

    /// T15: `transfer` settles via `before_yt_transfer` with PRE-transfer
    /// balances (not post). Pin ordering.
    #[test]
    fn test_t15_transfer_settles_with_pre_transfer_balances() {
        let (env, client, pt) = setup();
        let from = Address::generate(&env);
        let to_addr = Address::generate(&env);
        let to: MuxedAddress = to_addr.clone().into();
        client.mint(&from, &1_000);
        client.mint(&to_addr, &300);

        client.transfer(&from, &to, &200);

        let last: LastBeforeYtTransfer = env.as_contract(&pt, || {
            env.storage()
                .instance()
                .get(&MockKey::LastBeforeYtTransfer)
                .unwrap()
        });
        assert_eq!(last.from, from);
        assert_eq!(last.to, to_addr);
        assert_eq!(
            last.from_balance, 1_000,
            "must capture from's PRE-transfer balance"
        );
        assert_eq!(
            last.to_balance, 300,
            "must capture to's PRE-transfer balance"
        );
    }

    /// T16: Self-transfer A→A must pass both balance args equal to A's
    /// balance (settles A twice, no corruption).
    #[test]
    fn test_t16_self_transfer_settles_a_with_equal_balances() {
        let (env, client, pt) = setup();
        let a = Address::generate(&env);
        let a_muxed: MuxedAddress = a.clone().into();
        client.mint(&a, &500);

        client.transfer(&a, &a_muxed, &100);

        let last: LastBeforeYtTransfer = env.as_contract(&pt, || {
            env.storage()
                .instance()
                .get(&MockKey::LastBeforeYtTransfer)
                .unwrap()
        });
        assert_eq!(last.from, a);
        assert_eq!(last.to, a);
        assert_eq!(last.from_balance, 500);
        assert_eq!(last.to_balance, 500);
        // A's balance unchanged by self-transfer.
        assert_eq!(client.actual_balance_of(&a), 500);
    }

    /// T17: `transfer_from` settles yield against `from` (owner), not
    /// `spender`. Pin auth/yield separation.
    #[test]
    fn test_t17_transfer_from_settles_against_owner_not_spender() {
        let (env, client, pt) = setup();
        let owner = Address::generate(&env);
        let spender = Address::generate(&env);
        let receiver = Address::generate(&env);
        client.mint(&owner, &1_000);
        client.approve(&owner, &spender, &500, &(env.ledger().sequence() + 1000));

        client.transfer_from(&spender, &owner, &receiver, &200);

        let last: LastBeforeYtTransfer = env.as_contract(&pt, || {
            env.storage()
                .instance()
                .get(&MockKey::LastBeforeYtTransfer)
                .unwrap()
        });
        assert_eq!(
            last.from, owner,
            "yield-settlement must hit owner, not spender"
        );
        assert_eq!(last.to, receiver);
    }

    /// T18: Public `burn` settles via `pt.update_yield(caller, pre_burn_balance)`
    /// then burns.
    #[test]
    fn test_t18_burn_settles_yield_with_pre_burn_balance() {
        let (env, client, pt) = setup();
        let user = Address::generate(&env);
        client.mint(&user, &1_000);

        client.burn(&user, &400);

        let last: LastUpdateYield = env.as_contract(&pt, || {
            env.storage()
                .instance()
                .get(&MockKey::LastUpdateYield)
                .unwrap()
        });
        assert_eq!(last.user, user);
        assert_eq!(
            last.user_yt_balance, 1_000,
            "update_yield called with PRE-burn balance"
        );
        assert_eq!(client.actual_balance_of(&user), 600);
    }

    /// T19: Multi-hop A→B→C triggers exactly two `before_yt_transfer`s,
    /// the second capturing the post-hop1 balances.
    #[test]
    fn test_t19_multihop_chain_triggers_yield_hook_each_hop() {
        let (env, client, pt) = setup();
        let a = Address::generate(&env);
        let b = Address::generate(&env);
        let c = Address::generate(&env);
        let b_muxed: MuxedAddress = b.clone().into();
        let c_muxed: MuxedAddress = c.clone().into();

        client.mint(&a, &1_000);

        client.transfer(&a, &b_muxed, &400);
        // After hop 1: a=600, b=400.
        let last1: LastBeforeYtTransfer = env.as_contract(&pt, || {
            env.storage()
                .instance()
                .get(&MockKey::LastBeforeYtTransfer)
                .unwrap()
        });
        assert_eq!(last1.from, a);
        assert_eq!(last1.from_balance, 1_000);
        assert_eq!(last1.to_balance, 0);

        client.transfer(&b, &c_muxed, &300);
        let last2: LastBeforeYtTransfer = env.as_contract(&pt, || {
            env.storage()
                .instance()
                .get(&MockKey::LastBeforeYtTransfer)
                .unwrap()
        });
        assert_eq!(last2.from, b);
        assert_eq!(last2.to, c);
        assert_eq!(last2.from_balance, 400, "B's pre-hop2 balance");
        assert_eq!(last2.to_balance, 0);
    }

    // ---------------------------------------------------------------------
    // 20-22. Pause cascade
    // ---------------------------------------------------------------------

    /// T20: Pause cascade — all mutating ops panic.
    #[test]
    fn test_t20_paused_state_blocks_all_mutating_ops() {
        let (env, client, pt) = setup();
        let user = Address::generate(&env);
        let other = Address::generate(&env);
        let other_muxed: MuxedAddress = other.clone().into();
        client.mint(&user, &500);

        client.pause(&pt);

        assert!(client.try_mint(&user, &1).is_err());
        assert!(client.try_burn(&user, &1).is_err());
        assert!(client.try_transfer(&user, &other_muxed, &1).is_err());
        assert!(
            client
                .try_burn_without_yield_update(&user, &user, &1)
                .is_err()
        );

        client.unpause(&pt);
        client.mint(&user, &1);
    }

    /// T21: `upgrade` works WHILE paused (intentional per lib.rs:121-124).
    #[test]
    fn test_t21_upgrade_works_while_paused() {
        let (env, client, pt) = setup();
        client.pause(&pt);

        // Use any wasm hash that's uploadable. Re-upload YT's own bytes
        // — the test env allows it as a no-op-ish upgrade.
        let yt_wasm: &[u8] =
            include_bytes!("../../../target/wasm32v1-none/release/yield_token.wasm");
        let hash = env.deployer().upload_contract_wasm(yt_wasm);
        client.upgrade(&hash); // must not panic
    }

    /// T22: Read-only views work while paused.
    #[test]
    fn test_t22_read_views_work_while_paused() {
        let (env, client, pt) = setup();
        let u = Address::generate(&env);
        client.mint(&u, &500);

        client.pause(&pt);
        // None of these panic:
        let _ = client.balance(&u);
        let _ = client.total_supply();
        let _ = client.actual_balance_of(&u);
        let _ = client.get_pt();
        let _ = client.allowance(&u, &u);
    }

    // ---------------------------------------------------------------------
    // 23-27. Initialize edge cases
    // ---------------------------------------------------------------------

    /// T23: maturity == current ledger ts → YT born expired.
    #[test]
    fn test_t23_born_expired_when_maturity_equals_current_ts() {
        let env = Env::default();
        env.mock_all_auths();
        env.ledger().with_mut(|li| li.timestamp = 100);

        let pt = env.register(MockPrincipalTokenContract, ());
        env.as_contract(&pt, || {
            env.storage().instance().set(&MockKey::Maturity, &100u64);
        });
        let yt = env.register(YieldTokenContract, ());
        let client = YieldTokenContractClient::new(&env, &yt);
        client.initialize(
            &String::from_str(&env, "Y"),
            &String::from_str(&env, "Y"),
            &18u32,
            &100u64,
            &pt,
        );

        let u = Address::generate(&env);
        // Mint succeeds (no expiry guard on mint).
        client.mint(&u, &500);
        // But balance reads 0.
        assert_eq!(client.balance(&u), 0);
        // And transfer-nonzero panics.
        let to: MuxedAddress = Address::generate(&env).into();
        assert!(client.try_transfer(&u, &to, &1).is_err());
    }

    /// T24: maturity == 0 → expired from the start.
    #[test]
    fn test_t24_maturity_zero_means_born_expired() {
        let (_env, client, _pt) = setup_with_maturity(0);
        let u = Address::generate(&_env);
        client.mint(&u, &100);
        assert_eq!(client.balance(&u), 0, "maturity=0 → always expired");
    }

    /// T25: maturity == u64::MAX → never expires.
    #[test]
    fn test_t25_maturity_u64_max_never_expires() {
        let (env, client, _pt) = setup_with_maturity(u64::MAX);
        env.ledger().with_mut(|li| li.timestamp = u64::MAX - 1);
        let u = Address::generate(&env);
        client.mint(&u, &100);
        assert_eq!(client.balance(&u), 100, "before u64::MAX → not expired");
    }

    /// T26: double-init panics with `AlreadyInitialized`.
    #[test]
    fn test_t26_double_init_panics() {
        let (env, client, _pt) = setup();
        let pt2 = Address::generate(&env);
        let r = client.try_initialize(
            &String::from_str(&env, "X"),
            &String::from_str(&env, "X"),
            &18u32,
            &10_000_000_000u64,
            &pt2,
        );
        assert!(r.is_err());
    }

    /// T27: decimals round-trip.
    #[test]
    fn test_t27_decimals_roundtrip_through_init_and_view() {
        let env = Env::default();
        env.mock_all_auths();
        let pt = env.register(MockPrincipalTokenContract, ());
        let yt = env.register(YieldTokenContract, ());
        let client = YieldTokenContractClient::new(&env, &yt);
        client.initialize(
            &String::from_str(&env, "Y"),
            &String::from_str(&env, "Y"),
            &7u32,
            &10_000_000_000u64,
            &pt,
        );
        assert_eq!(client.decimals(), 7);
    }

    // ---------------------------------------------------------------------
    // 28-30. Boundary pins
    // ---------------------------------------------------------------------

    /// T28: `is_expired` boundary — at t == maturity is expired.
    #[test]
    fn test_t28_expiry_boundary_at_equals_maturity_is_expired() {
        let (env, client, _pt) = setup();
        let u = Address::generate(&env);
        client.mint(&u, &100);

        env.ledger().with_mut(|li| li.timestamp = at_expiry_ts());
        assert_eq!(client.balance(&u), 0, "t == maturity → expired");

        env.ledger()
            .with_mut(|li| li.timestamp = at_expiry_ts() - 1);
        assert_eq!(client.balance(&u), 100, "t == maturity - 1 → not expired");
    }

    /// T29: `actual_balance_of` at expiry exact returns stored value.
    #[test]
    fn test_t29_actual_balance_of_at_expiry_exact_returns_stored() {
        let (env, client, _pt) = setup();
        let u = Address::generate(&env);
        client.mint(&u, &777);
        env.ledger().with_mut(|li| li.timestamp = at_expiry_ts());
        assert_eq!(client.actual_balance_of(&u), 777);
    }

    /// T30: `balance` at exactly maturity-1 returns stored; at maturity 0.
    #[test]
    fn test_t30_balance_at_boundary_transitions_correctly() {
        let (env, client, _pt) = setup();
        let u = Address::generate(&env);
        client.mint(&u, &500);

        env.ledger()
            .with_mut(|li| li.timestamp = at_expiry_ts() - 1);
        assert_eq!(client.balance(&u), 500);

        env.ledger().with_mut(|li| li.timestamp = at_expiry_ts());
        assert_eq!(client.balance(&u), 0);
    }

    // ---------------------------------------------------------------------
    // 31-33. Composability E2E
    // ---------------------------------------------------------------------

    /// T31: Lifecycle smoke — mint, transfer, burn, expire, balance reads 0.
    #[test]
    fn test_t31_full_lifecycle_smoke() {
        let (env, client, _pt) = setup();
        let a = Address::generate(&env);
        let b = Address::generate(&env);
        let b_muxed: MuxedAddress = b.clone().into();

        client.mint(&a, &1_000);
        client.transfer(&a, &b_muxed, &400);
        assert_eq!(client.actual_balance_of(&a), 600);
        assert_eq!(client.actual_balance_of(&b), 400);

        client.burn(&a, &100);
        assert_eq!(client.actual_balance_of(&a), 500);

        env.ledger().with_mut(|li| li.timestamp = post_expiry_ts());
        assert_eq!(client.balance(&a), 0);
        assert_eq!(client.balance(&b), 0);
        assert_eq!(client.total_supply(), 0);
        // But actual_balance_of preserves history.
        assert_eq!(client.actual_balance_of(&a), 500);
        assert_eq!(client.actual_balance_of(&b), 400);
    }

    /// T32: Frozen-allowance via post-expiry — pre-expiry approve, then
    /// cross expiry, transfer_from panics, balance untouched.
    #[test]
    fn test_t32_frozen_allowance_via_post_expiry() {
        let (env, client, _pt) = setup();
        let owner = Address::generate(&env);
        let spender = Address::generate(&env);
        let receiver = Address::generate(&env);
        client.mint(&owner, &1_000);
        client.approve(
            &owner,
            &spender,
            &500,
            &(env.ledger().sequence() + 1_000_000),
        );

        env.ledger().with_mut(|li| li.timestamp = post_expiry_ts());

        let r = client.try_transfer_from(&spender, &owner, &receiver, &100);
        assert!(r.is_err());
        assert_eq!(client.actual_balance_of(&owner), 1_000);
        assert_eq!(client.actual_balance_of(&receiver), 0);
    }

    /// T33: YT.burn at t == maturity-1 succeeds; at t == maturity panics.
    #[test]
    fn test_t33_burn_at_maturity_boundary() {
        let (env, client, _pt) = setup();
        let u = Address::generate(&env);
        client.mint(&u, &1_000);

        env.ledger()
            .with_mut(|li| li.timestamp = at_expiry_ts() - 1);
        client.burn(&u, &100); // pre-expiry, ok
        assert_eq!(client.actual_balance_of(&u), 900);

        env.ledger().with_mut(|li| li.timestamp = at_expiry_ts());
        let r = client.try_burn(&u, &100);
        assert!(r.is_err(), "burn at exactly maturity must panic");
    }

    // ---------------------------------------------------------------------
    // 34-36. Adversarial inputs
    // ---------------------------------------------------------------------

    /// T34: Negative amounts rejected on every mutator.
    #[test]
    fn test_t34_negative_amounts_rejected_on_all_mutators() {
        let (env, client, _pt) = setup();
        let u = Address::generate(&env);
        let to: MuxedAddress = Address::generate(&env).into();
        client.mint(&u, &1_000);

        assert!(client.try_mint(&u, &-1).is_err());
        assert!(client.try_burn(&u, &-1).is_err());
        assert!(client.try_burn_without_yield_update(&u, &u, &-1).is_err());
        assert!(client.try_transfer(&u, &to, &-1).is_err());
    }

    /// T35: i128::MAX amounts overflow on accumulation.
    #[test]
    fn test_t35_i128_max_mint_does_not_silently_wrap() {
        let (env, client, _pt) = setup();
        let u = Address::generate(&env);
        client.mint(&u, &1);
        let r = client.try_mint(&u, &i128::MAX);
        assert!(r.is_err(), "mint(MAX) must overflow-panic, not wrap");
    }

    /// T36: Transfer to MuxedAddress with embedded ID preserves yield-hook
    /// routing — the from balance comes from the un-muxed address.
    #[test]
    fn test_t36_muxed_transfer_routes_through_yield_hook_correctly() {
        let (env, client, pt) = setup();
        let from = Address::generate(&env);
        let to_addr = Address::generate(&env);
        let to_muxed: MuxedAddress = to_addr.clone().into();
        client.mint(&from, &1_000);

        client.transfer(&from, &to_muxed, &100);
        let last: LastBeforeYtTransfer = env.as_contract(&pt, || {
            env.storage()
                .instance()
                .get(&MockKey::LastBeforeYtTransfer)
                .unwrap()
        });
        assert_eq!(
            last.to, to_addr,
            "muxed → resolved underlying address for yield hook"
        );
    }

    // Suppress unused-import warnings for helpers shared across tests.
    #[allow(dead_code)]
    fn _suppress(_: SVec<Val>) {}
    #[allow(dead_code)]
    fn _pre() -> u64 {
        pre_expiry_ts()
    }
}
