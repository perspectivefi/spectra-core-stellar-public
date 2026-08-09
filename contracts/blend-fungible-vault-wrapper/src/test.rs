// SPDX-License-Identifier: BUSL-1.1
#![cfg(test)]

extern crate std;

use crate::errors::BlendWrapperError;
use crate::{BlendFungibleVaultWrapperContract, BlendFungibleVaultWrapperContractClient};
use blend_types::{
    REQUEST_SUPPLY, Request, SCALAR_12, to_asset_from_b_token, to_b_token_down, to_b_token_up,
};
use soroban_sdk::testutils::Address as _;
use soroban_sdk::testutils::Ledger as _;
use soroban_sdk::{Address, Env, String, vec};
use spectra_math::mul_div::Rounding;
use spectra_mocks::{
    MockBlendPoolContract, MockBlendPoolContractClient, MockUnderlyingContract,
    MockUnderlyingContractClient,
};

/// Assert that a `try_*` call returned the given structured `BlendWrapperError`
/// (i.e. `Err(Ok(error))`), so an unstructured panic does not satisfy RED tests.
fn assert_blend_error<T: core::fmt::Debug>(
    result: Result<T, Result<soroban_sdk::Error, soroban_sdk::InvokeError>>,
    expected: BlendWrapperError,
) {
    match result {
        Err(Ok(actual)) => {
            let expected_val: soroban_sdk::Error = expected.into();
            assert_eq!(
                actual, expected_val,
                "expected error {expected:?}, got {actual:?}"
            );
        }
        other => panic!("expected structured BlendWrapperError::{expected:?}, got {other:?}"),
    }
}

struct Fix {
    env: Env,
    wc: BlendFungibleVaultWrapperContractClient<'static>,
    und: MockUnderlyingContractClient<'static>,
    pool: MockBlendPoolContractClient<'static>,
    und_addr: Address,
    pool_addr: Address,
    wrapper_addr: Address,
    admin: Address,
    user: Address,
}

fn setup_with_rate(b_rate: i128) -> Fix {
    let env = Env::default();
    env.mock_all_auths();
    build_fix(env, b_rate)
}

/// Fixture without any mocked auths — contract-side auth runs for real.
fn setup_enforcing() -> Fix {
    build_fix(Env::default(), SCALAR_12)
}

fn build_fix(env: Env, b_rate: i128) -> Fix {
    let und_addr = env.register(MockUnderlyingContract, ());
    let und = MockUnderlyingContractClient::new(&env, &und_addr);
    und.initialize(
        &String::from_str(&env, "Mock USD"),
        &String::from_str(&env, "USDC"),
        &7,
    );

    let pool_addr = env.register(MockBlendPoolContract, ());
    let pool = MockBlendPoolContractClient::new(&env, &pool_addr);
    pool.add_reserve(&und_addr, &b_rate);

    let admin = Address::generate(&env);
    let wrapper_addr = env.register(
        BlendFungibleVaultWrapperContract,
        (pool_addr.clone(), und_addr.clone(), admin.clone()),
    );
    let wc = BlendFungibleVaultWrapperContractClient::new(&env, &wrapper_addr);

    let user = Address::generate(&env);
    und.mint(&user, &1_000_000);

    Fix {
        env,
        wc,
        und,
        pool,
        und_addr,
        pool_addr,
        wrapper_addr,
        admin,
        user,
    }
}

fn setup() -> Fix {
    setup_with_rate(SCALAR_12)
}

/// Change the pool's `b_rate` and advance the ledger one second. Real Blend only
/// moves `b_rate` by accruing over elapsed time, so a rate change at a frozen
/// timestamp is unreachable on-chain; the wrapper caches `b_rate` per timestamp,
/// so a test that moves the rate must also move the clock to stay faithful.
fn set_rate(f: &Fix, rate: i128) {
    f.env.ledger().set_timestamp(f.env.ledger().timestamp() + 1);
    f.pool.set_b_rate(&f.und_addr, &rate);
}

/// Supply `amount` underlying to the pool on behalf of the wrapper without
/// going through `deposit` — grows the position, not the wrapper supply.
fn donate_position(f: &Fix, amount: i128) {
    let donor = Address::generate(&f.env);
    f.und.mint(&donor, &amount);
    f.pool.submit(
        &f.wrapper_addr,
        &donor,
        &donor,
        &vec![
            &f.env,
            Request {
                request_type: REQUEST_SUPPLY,
                address: f.und_addr.clone(),
                amount,
            },
        ],
    );
}

mod init {
    use super::*;

    #[test]
    fn metadata_derived_from_underlying() {
        let f = setup();
        assert_eq!(f.wc.decimals(), 7);
        assert_eq!(f.wc.query_asset(), f.und_addr);
        assert_eq!(f.wc.pool(), f.pool_addr);
        assert_eq!(
            f.wc.name(),
            String::from_str(&f.env, "Spectra FungibleVault Wrapper: Mock USD")
        );
        assert_eq!(f.wc.symbol(), String::from_str(&f.env, "sw-USDC"));
    }

    // removed double_init_fails — the wrapper is built via `__constructor`,
    // which runs exactly once at deploy and is not a callable entry point, so
    // re-initialization is impossible by construction.

    // construction now happens in `__constructor`, so a non-reserve asset
    // is rejected at deploy time (the constructor calls `pool.get_reserve`,
    // which panics). This asserts the register itself panics.
    #[test]
    #[should_panic]
    fn init_rejects_non_reserve_asset() {
        let f = setup();
        let stray = f.env.register(MockUnderlyingContract, ());
        MockUnderlyingContractClient::new(&f.env, &stray).initialize(
            &String::from_str(&f.env, "Stray"),
            &String::from_str(&f.env, "STR"),
            &7,
        );
        f.env.register(
            BlendFungibleVaultWrapperContract,
            (f.pool_addr.clone(), stray, f.admin.clone()),
        );
    }

    #[test]
    fn vault_share_not_supported() {
        let f = setup();
        assert_blend_error(f.wc.try_vault_share(), BlendWrapperError::NotSupported);
    }
}

mod conversions {
    use super::*;

    #[test]
    fn one_to_one_at_unit_rate() {
        let f = setup();
        assert_eq!(f.wc.convert_to_shares(&100), 100);
        assert_eq!(f.wc.convert_to_assets(&100), 100);
    }

    #[test]
    fn shares_floor_at_rate_3_2() {
        let f = setup_with_rate(1_500_000_000_000);
        // 100 * 1e12 / 1.5e12 = 66.67
        assert_eq!(f.wc.convert_to_shares(&100), 66);
    }

    #[test]
    fn assets_floor_at_rate_3_2() {
        let f = setup_with_rate(1_500_000_000_000);
        // 67 * 1.5e12 / 1e12 = 100.5
        assert_eq!(f.wc.convert_to_assets(&67), 100);
    }

    #[test]
    fn negative_amounts_panic() {
        let f = setup();
        assert_blend_error(
            f.wc.try_convert_to_shares(&-1),
            BlendWrapperError::NegativeAmount,
        );
        assert_blend_error(
            f.wc.try_convert_to_assets(&-1),
            BlendWrapperError::NegativeAmount,
        );
    }

    #[test]
    fn ceil_variants_round_up() {
        let f = setup_with_rate(1_500_000_000_000);
        assert_eq!(
            f.wc.convert_to_shares_with_rounding(&100, &Rounding::Ceil),
            67
        );
        assert_eq!(
            f.wc.convert_to_assets_with_rounding(&67, &Rounding::Ceil),
            101
        );
    }

    #[test]
    fn round_trip_returns_at_most_input() {
        let f = setup_with_rate(1_500_000_000_000);
        for assets in [1i128, 99, 100, 101, 12_345] {
            let back = f.wc.convert_to_assets(&f.wc.convert_to_shares(&assets));
            assert!(back <= assets);
        }
    }
}

mod views {
    use super::*;

    #[test]
    fn total_vault_shares_reads_pool_position() {
        let f = setup();
        assert_eq!(f.wc.total_vault_shares(), 0);
        donate_position(&f, 500);
        assert_eq!(f.wc.total_vault_shares(), 500);
    }

    #[test]
    fn total_assets_values_position_at_b_rate() {
        let f = setup_with_rate(2_000_000_000_000);
        donate_position(&f, 500); // 250 bTokens
        assert_eq!(f.wc.total_vault_shares(), 250);
        assert_eq!(f.wc.total_assets(), 500);
    }
}

mod deposit {
    use super::*;

    #[test]
    fn mints_floor_shares_and_supplies_pool() {
        let f = setup_with_rate(1_500_000_000_000);
        let shares = f.wc.deposit(&300, &f.user, &f.user, &f.user);
        assert_eq!(shares, 200);
        assert_eq!(f.wc.balance(&f.user), 200);
        assert_eq!(f.wc.total_supply(), 200);
        assert_eq!(f.wc.total_vault_shares(), 200);
        assert_eq!(f.wc.total_assets(), 300);
        assert_eq!(f.und.balance(&f.user), 999_700);
        assert_eq!(f.und.balance(&f.pool_addr), 300);
        assert_eq!(f.und.balance(&f.wrapper_addr), 0);
    }

    #[test]
    fn matches_preview() {
        let f = setup_with_rate(1_500_000_000_000);
        let preview = f.wc.preview_deposit(&301);
        assert_eq!(preview, 200);
        assert_eq!(f.wc.deposit(&301, &f.user, &f.user, &f.user), preview);
    }

    #[test]
    fn operator_pulls_via_allowance() {
        let f = setup();
        let operator = Address::generate(&f.env);
        let receiver = Address::generate(&f.env);
        let expiry = f.env.ledger().sequence() + 100;
        f.und.approve(&f.user, &operator, &400, &expiry);
        let shares = f.wc.deposit(&400, &receiver, &f.user, &operator);
        assert_eq!(shares, 400);
        assert_eq!(f.wc.balance(&receiver), 400);
        assert_eq!(f.und.balance(&f.user), 999_600);
    }

    #[test]
    fn zero_amount_mints_zero() {
        let f = setup();
        assert_eq!(f.wc.deposit(&0, &f.user, &f.user, &f.user), 0);
        assert_eq!(f.wc.total_supply(), 0);
        assert_eq!(f.und.balance(&f.user), 1_000_000);
    }

    #[test]
    fn negative_amount_panics() {
        let f = setup();
        assert_blend_error(
            f.wc.try_deposit(&-5, &f.user, &f.user, &f.user),
            BlendWrapperError::NegativeAmount,
        );
    }

    #[test]
    fn paused_blocks_deposit() {
        let f = setup();
        assert_eq!(f.wc.max_deposit(&f.user), i128::MAX);
        f.wc.pause(&f.admin);
        assert_eq!(f.wc.max_deposit(&f.user), 0);
        assert!(f.wc.try_deposit(&100, &f.user, &f.user, &f.user).is_err());
        // previews are views and stay callable while paused
        assert!(f.wc.try_preview_deposit(&100).is_ok());
    }

    #[test]
    fn zero_b_rate_returns_zero() {
        let f = setup();
        f.pool.force_b_rate(&f.und_addr, &0);
        assert_eq!(f.wc.max_deposit(&f.user), 0);
    }

    #[test]
    fn negative_b_rate_returns_zero() {
        let f = setup();
        f.pool.force_b_rate(&f.und_addr, &-1);
        assert_eq!(f.wc.max_deposit(&f.user), 0);
    }
}

mod redeem {
    use super::*;

    #[test]
    fn burns_shares_and_pays_floor_assets() {
        let f = setup_with_rate(1_500_000_000_000);
        let shares = f.wc.deposit(&300, &f.user, &f.user, &f.user);
        assert_eq!(f.wc.total_vault_shares(), 200);
        let assets = f.wc.redeem(&shares, &f.user, &f.user, &f.user);
        assert_eq!(assets, 300);
        assert_eq!(f.und.balance(&f.user), 1_000_000);
        assert_eq!(f.wc.balance(&f.user), 0);
        assert_eq!(f.wc.total_supply(), 0);
        assert_eq!(f.wc.total_vault_shares(), 0);
    }

    #[test]
    fn accrual_increases_redeem_value() {
        let f = setup();
        f.wc.deposit(&500, &f.user, &f.user, &f.user);
        set_rate(&f, 1_200_000_000_000);
        f.und.mint(&f.pool_addr, &1_000); // accrued interest liquidity
        assert_eq!(f.wc.preview_redeem(&500), 600);
        let assets = f.wc.redeem(&500, &f.user, &f.user, &f.user);
        assert_eq!(assets, 600);
        assert_eq!(f.und.balance(&f.user), 1_000_100);
    }

    #[test]
    fn pays_receiver_directly() {
        let f = setup();
        let receiver = Address::generate(&f.env);
        f.wc.deposit(&250, &f.user, &f.user, &f.user);
        let assets = f.wc.redeem(&250, &receiver, &f.user, &f.user);
        assert_eq!(assets, 250);
        assert_eq!(f.und.balance(&receiver), 250);
        assert_eq!(f.und.balance(&f.wrapper_addr), 0);
    }

    #[test]
    fn operator_spends_share_allowance() {
        let f = setup();
        f.wc.deposit(&500, &f.user, &f.user, &f.user);
        let operator = Address::generate(&f.env);
        let expiry = f.env.ledger().sequence() + 100;
        f.wc.approve(&f.user, &operator, &200, &expiry);
        let assets = f.wc.redeem(&200, &operator, &f.user, &operator);
        assert_eq!(assets, 200);
        assert_eq!(f.wc.balance(&f.user), 300);
        assert_eq!(f.wc.allowance(&f.user, &operator), 0);
    }

    #[test]
    fn exceeding_balance_panics() {
        let f = setup();
        let shares = f.wc.deposit(&100, &f.user, &f.user, &f.user);
        assert!(
            f.wc.try_redeem(&(shares + 1), &f.user, &f.user, &f.user)
                .is_err()
        );
    }

    #[test]
    fn negative_shares_panic() {
        let f = setup();
        assert_blend_error(
            f.wc.try_redeem(&-1, &f.user, &f.user, &f.user),
            BlendWrapperError::NegativeAmount,
        );
    }

    #[test]
    fn zero_shares_redeems_zero() {
        let f = setup();
        f.wc.deposit(&100, &f.user, &f.user, &f.user);
        assert_eq!(f.wc.redeem(&0, &f.user, &f.user, &f.user), 0);
        assert_eq!(f.wc.balance(&f.user), 100);
        assert_eq!(f.und.balance(&f.user), 999_900);
    }

    #[test]
    fn paused_blocks_redeem() {
        let f = setup();
        let shares = f.wc.deposit(&100, &f.user, &f.user, &f.user);
        assert_eq!(f.wc.max_redeem(&f.user), shares);
        f.wc.pause(&f.admin);
        assert_eq!(f.wc.max_redeem(&f.user), 0);
        assert!(f.wc.try_redeem(&shares, &f.user, &f.user, &f.user).is_err());
        // previews are views and stay callable while paused
        assert!(f.wc.try_preview_redeem(&shares).is_ok());
    }
}

mod withdraw {
    use super::*;

    #[test]
    fn burns_ceil_shares_and_pays_exact_assets() {
        let f = setup_with_rate(1_500_000_000_000);
        f.wc.deposit(&300, &f.user, &f.user, &f.user); // 200 shares
        let shares = f.wc.withdraw(&100, &f.user, &f.user, &f.user);
        assert_eq!(shares, 67); // ceil(100 / 1.5)
        assert_eq!(f.und.balance(&f.user), 999_800);
        assert_eq!(f.wc.balance(&f.user), 133);
        assert_eq!(f.wc.total_vault_shares(), 133);
    }

    #[test]
    fn matches_preview() {
        let f = setup_with_rate(1_500_000_000_000);
        f.wc.deposit(&300, &f.user, &f.user, &f.user);
        let preview = f.wc.preview_withdraw(&100);
        assert_eq!(preview, 67);
        assert_eq!(f.wc.withdraw(&100, &f.user, &f.user, &f.user), preview);
    }

    #[test]
    fn max_withdraw_drains_position_exactly() {
        let f = setup_with_rate(1_500_000_000_000);
        f.wc.deposit(&300, &f.user, &f.user, &f.user);
        let max = f.wc.max_withdraw(&f.user);
        assert_eq!(max, 300); // floor(200 * 1.5)
        let shares = f.wc.withdraw(&max, &f.user, &f.user, &f.user);
        assert_eq!(shares, 200);
        assert_eq!(f.wc.balance(&f.user), 0);
        assert_eq!(f.und.balance(&f.user), 1_000_000);
    }

    #[test]
    fn exceeding_max_panics() {
        let f = setup_with_rate(1_500_000_000_000);
        f.wc.deposit(&300, &f.user, &f.user, &f.user);
        assert!(f.wc.try_withdraw(&301, &f.user, &f.user, &f.user).is_err());
    }

    #[test]
    fn pays_receiver_directly() {
        let f = setup();
        let receiver = Address::generate(&f.env);
        f.wc.deposit(&250, &f.user, &f.user, &f.user);
        f.wc.withdraw(&100, &receiver, &f.user, &f.user);
        assert_eq!(f.und.balance(&receiver), 100);
    }

    #[test]
    fn operator_spends_share_allowance() {
        let f = setup_with_rate(1_500_000_000_000);
        f.wc.deposit(&300, &f.user, &f.user, &f.user);
        let operator = Address::generate(&f.env);
        let expiry = f.env.ledger().sequence() + 100;
        f.wc.approve(&f.user, &operator, &67, &expiry);
        let shares = f.wc.withdraw(&100, &operator, &f.user, &operator);
        assert_eq!(shares, 67);
        assert_eq!(f.wc.allowance(&f.user, &operator), 0);
    }

    #[test]
    fn negative_assets_panic() {
        let f = setup();
        assert_blend_error(
            f.wc.try_withdraw(&-1, &f.user, &f.user, &f.user),
            BlendWrapperError::NegativeAmount,
        );
    }

    #[test]
    fn zero_assets_withdraws_zero() {
        let f = setup();
        f.wc.deposit(&100, &f.user, &f.user, &f.user);
        assert_eq!(f.wc.withdraw(&0, &f.user, &f.user, &f.user), 0);
        assert_eq!(f.wc.balance(&f.user), 100);
    }

    #[test]
    fn paused_blocks_withdraw() {
        let f = setup();
        f.wc.deposit(&100, &f.user, &f.user, &f.user);
        f.wc.pause(&f.admin);
        assert_eq!(f.wc.max_withdraw(&f.user), 0);
        assert!(f.wc.try_withdraw(&50, &f.user, &f.user, &f.user).is_err());
        // previews are views and stay callable while paused
        assert!(f.wc.try_preview_withdraw(&50).is_ok());
    }
}

mod invariants {
    use super::*;

    const RATES: [i128; 6] = [
        1_000_000_000_000,
        1_100_000_000_000,
        1_500_000_000_000,
        2_000_000_000_000,
        3_777_777_777_777,
        9_990_000_000_001,
    ];
    const AMOUNTS: [i128; 6] = [1, 7, 100, 999, 1_234_567, 99_999_999];

    fn lcg(seed: &mut u64) -> i128 {
        *seed = seed
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        (*seed >> 33) as i128
    }

    #[test]
    fn redeem_burns_exact_shares_and_pays_exactly_preview() {
        for rate in RATES {
            let f = setup_with_rate(rate);
            f.und.mint(&f.user, &1_000_000_000);
            f.wc.deposit(&500_000_000, &f.user, &f.user, &f.user);
            for s in [1i128, 9, 1_000, 123_456] {
                let preview = f.wc.preview_redeem(&s);
                let balance_before = f.wc.balance(&f.user);
                let supply_before = f.wc.total_supply();
                let assets = f.wc.redeem(&s, &f.user, &f.user, &f.user);
                assert_eq!(balance_before - f.wc.balance(&f.user), s, "exact burn");
                assert_eq!(supply_before - f.wc.total_supply(), s);
                assert!(assets >= preview, "4626: actual must cover preview");
                assert_eq!(assets, preview, "no surplus beyond preview");
            }
        }
    }

    #[test]
    fn convert_round_trips_lose_bounded_dust() {
        for rate in RATES {
            let f = setup_with_rate(rate);
            for x in AMOUNTS {
                // assets -> shares -> assets: loss < rate/1e12 + 1
                let back = f.wc.convert_to_assets(&f.wc.convert_to_shares(&x));
                assert!(back <= x, "round trip must never profit");
                assert!(
                    x - back <= rate / SCALAR_12 + 1,
                    "loss {} at rate {rate}",
                    x - back
                );
                // shares -> assets -> shares: loss < 1e12/rate + 1
                let s2 = f.wc.convert_to_shares(&f.wc.convert_to_assets(&x));
                assert!(s2 <= x);
                assert!(x - s2 <= SCALAR_12 / rate + 1);
            }
        }
    }

    #[test]
    fn deposit_redeem_round_trip_never_profits() {
        for rate in RATES {
            for x in [1i128, 100, 999, 1_234_567] {
                let f = setup_with_rate(rate);
                f.und.mint(&f.user, &10_000_000);
                let preview = f.wc.preview_deposit(&x);
                let shares = f.wc.deposit(&x, &f.user, &f.user, &f.user);
                assert_eq!(shares, preview, "deposit mints exactly preview");
                let back = f.wc.redeem(&shares, &f.user, &f.user, &f.user);
                assert!(back <= x, "round trip must never profit");
                assert!(x - back <= rate / SCALAR_12 + 1);
                assert!(f.wc.total_vault_shares() >= f.wc.total_supply());
            }
        }
    }

    #[test]
    fn deposit_withdraw_round_trip_drains_exactly() {
        for rate in RATES {
            for x in [100i128, 999, 1_234_567] {
                let f = setup_with_rate(rate);
                f.und.mint(&f.user, &10_000_000);
                let shares = f.wc.deposit(&x, &f.user, &f.user, &f.user);
                let max_w = f.wc.max_withdraw(&f.user);
                assert_eq!(max_w, f.wc.convert_to_assets(&shares));
                let preview = f.wc.preview_withdraw(&max_w);
                let burned = f.wc.withdraw(&max_w, &f.user, &f.user, &f.user);
                assert_eq!(burned, preview, "withdraw burns exactly preview");
                // with b_rate >= 1e12 the max-withdraw burns the whole position
                assert_eq!(burned, shares);
                assert_eq!(f.wc.balance(&f.user), 0);
                assert!(x - max_w <= rate / SCALAR_12 + 1);
                assert!(f.wc.total_vault_shares() >= f.wc.total_supply());
            }
        }
    }

    #[test]
    fn preview_relations_hold() {
        for rate in RATES {
            let f = setup_with_rate(rate);
            for x in AMOUNTS {
                assert_eq!(f.wc.preview_deposit(&x), f.wc.convert_to_shares(&x));
                assert_eq!(f.wc.preview_redeem(&x), f.wc.convert_to_assets(&x));
                let ceil = f.wc.preview_withdraw(&x);
                let floor = f.wc.convert_to_shares(&x);
                assert!(ceil >= floor && ceil - floor <= 1, "ceil-floor delta");
            }
        }
    }

    /// Deterministic random walk of deposit/redeem/withdraw with b_rate drift
    /// (incl. drops below 1e12): the position must always cover the supply.
    #[test]
    fn solvency_holds_across_random_walk() {
        let f = setup();
        f.und.mint(&f.user, &10_i128.pow(15));
        f.und.mint(&f.pool_addr, &10_i128.pow(15)); // interest payout buffer
        let mut rate = SCALAR_12;
        let mut seed: u64 = 42;
        for step in 0..60u32 {
            // Each step is a distinct moment; advancing the clock lets the
            // wrapper's per-timestamp b_rate cache observe the rate drift
            // (Blend only changes b_rate as time elapses).
            f.env.ledger().with_mut(|li| li.timestamp += 1);
            match step % 4 {
                0 => {
                    let amt = lcg(&mut seed) % 1_000_000 + 1;
                    f.wc.deposit(&amt, &f.user, &f.user, &f.user);
                }
                1 => {
                    rate = if step % 12 == 1 {
                        (rate * 95 / 100).max(900_000_000_000)
                    } else {
                        rate * (101 + lcg(&mut seed) % 5) / 100
                    };
                    // The clock already advanced at the top of this step, so set
                    // the rate directly rather than via `set_rate` (which would
                    // advance it a second time).
                    f.pool.set_b_rate(&f.und_addr, &rate);
                }
                2 => {
                    let max = f.wc.max_redeem(&f.user);
                    if max > 0 {
                        let s = lcg(&mut seed) % max + 1;
                        f.wc.redeem(&s, &f.user, &f.user, &f.user);
                    }
                }
                _ => {
                    let max = f.wc.max_withdraw(&f.user);
                    if max > 0 {
                        let a = lcg(&mut seed) % max + 1;
                        f.wc.withdraw(&a, &f.user, &f.user, &f.user);
                    }
                }
            }
            assert!(
                f.wc.total_vault_shares() >= f.wc.total_supply(),
                "solvency broke at step {step}"
            );
        }
        // a full exit must always be possible
        let max = f.wc.max_redeem(&f.user);
        f.wc.redeem(&max, &f.user, &f.user, &f.user);
        assert_eq!(f.wc.total_supply(), 0);
        assert!(f.wc.total_vault_shares() >= 0);
    }
}

// ── 4626 events: deposit emits Deposit, withdraw/redeem emit Withdraw (topics
// + data), the surface off-chain indexers and the PT rely on. ──

mod events {
    use super::*;
    use soroban_sdk::Event;
    use soroban_sdk::testutils::Events as _;
    use stellar_tokens::vault::{Deposit, Withdraw};

    /// Whether the wrapper emitted `ev` (matched on topics + data via XDR).
    fn wrapper_emitted<E: Event>(f: &Fix, ev: E) -> bool {
        f.env
            .events()
            .all()
            .filter_by_contract(&f.wrapper_addr)
            .events()
            .contains(&ev.to_xdr(&f.env, &f.wrapper_addr))
    }

    #[test]
    fn deposit_emits_deposit_event() {
        let f = setup();
        f.wc.deposit(&400, &f.user, &f.user, &f.user);
        assert!(wrapper_emitted(
            &f,
            Deposit {
                operator: f.user.clone(),
                from: f.user.clone(),
                receiver: f.user.clone(),
                assets: 400,
                shares: 400,
            }
        ));
    }

    #[test]
    fn redeem_emits_withdraw_event() {
        let f = setup();
        f.wc.deposit(&400, &f.user, &f.user, &f.user);
        f.wc.redeem(&400, &f.user, &f.user, &f.user);
        assert!(wrapper_emitted(
            &f,
            Withdraw {
                operator: f.user.clone(),
                receiver: f.user.clone(),
                owner: f.user.clone(),
                assets: 400,
                shares: 400,
            }
        ));
    }

    #[test]
    fn withdraw_emits_withdraw_event() {
        let f = setup();
        f.wc.deposit(&400, &f.user, &f.user, &f.user);
        f.wc.withdraw(&100, &f.user, &f.user, &f.user);
        assert!(wrapper_emitted(
            &f,
            Withdraw {
                operator: f.user.clone(),
                receiver: f.user.clone(),
                owner: f.user.clone(),
                assets: 100,
                shares: 100,
            }
        ));
    }
}

// ── multi-user: accrual is shared strictly proportionally, and one holder's
// exit never touches another's claim. ──

mod multi_user {
    use super::*;

    #[test]
    fn accrual_shared_proportionally_no_cross_drain() {
        let f = setup();
        let user2 = Address::generate(&f.env);
        f.und.mint(&user2, &1_000_000);

        // equal deposits at unit rate → equal shares
        f.wc.deposit(&1_000, &f.user, &f.user, &f.user);
        f.wc.deposit(&1_000, &user2, &user2, &user2);
        assert_eq!(f.wc.total_supply(), 2_000);
        assert_eq!(f.wc.balance(&f.user), 1_000);
        assert_eq!(f.wc.balance(&user2), 1_000);

        // 50% Blend accrual; fund the pool's payout liquidity
        set_rate(&f, 1_500_000_000_000);
        f.und.mint(&f.pool_addr, &10_000);

        // each holder's share is worth 1.5 underlying — shared proportionally
        assert_eq!(f.wc.convert_to_assets(&1_000), 1_500);
        assert!(f.wc.total_vault_shares() >= f.wc.total_supply());

        // user1 fully exits at the accrued value
        assert_eq!(f.wc.redeem(&1_000, &f.user, &f.user, &f.user), 1_500);

        // user2's claim is untouched by user1's exit
        assert_eq!(f.wc.max_redeem(&user2), 1_000);
        assert_eq!(f.wc.convert_to_assets(&1_000), 1_500);
        assert_eq!(f.wc.redeem(&1_000, &user2, &user2, &user2), 1_500);

        // vault fully drained, solvent throughout
        assert_eq!(f.wc.total_supply(), 0);
        assert!(f.wc.total_vault_shares() >= 0);
    }
}

// ── rewards: BLND emissions are claimed through the existing rewards-proxy
// mechanism with the pool itself as proxy — the wrapper is the direct invoker
// of `claim`, so the pool's `from.require_auth()` passes as invoker auth. ──

mod rewards {
    use super::*;
    use blend_types::b_token_emission_id;
    use soroban_sdk::{IntoVal, TryFromVal, Val, Vec, symbol_short};

    fn blnd_fixture(f: &Fix, amount: i128) -> MockUnderlyingContractClient<'static> {
        let blnd_addr = f.env.register(MockUnderlyingContract, ());
        let blnd = MockUnderlyingContractClient::new(&f.env, &blnd_addr);
        blnd.initialize(
            &String::from_str(&f.env, "Blend"),
            &String::from_str(&f.env, "BLND"),
            &7,
        );
        blnd.mint(&f.pool_addr, &amount);
        f.pool.set_claim(&blnd_addr, &amount);
        blnd
    }

    #[test]
    fn claim_rewards_via_pool_proxy() {
        let f = setup();
        let blnd = blnd_fixture(&f, 500);
        f.wc.set_rewards_proxy(&f.admin, &Some(f.pool_addr.clone()));

        let treasury = Address::generate(&f.env);
        let ids: Vec<u32> = vec![&f.env, b_token_emission_id(0)];
        let args: Vec<Val> = vec![
            &f.env,
            f.wrapper_addr.into_val(&f.env),
            ids.into_val(&f.env),
            treasury.into_val(&f.env),
        ];
        let claimed = f.wc.claim_rewards(&f.admin, &symbol_short!("claim"), &args);
        assert_eq!(i128::try_from_val(&f.env, &claimed).unwrap(), 500);
        assert_eq!(blnd.balance(&treasury), 500);

        let last = f.pool.last_claim().unwrap();
        assert_eq!(last.from, f.wrapper_addr);
        assert_eq!(last.reserve_token_ids, vec![&f.env, 1u32]);
        assert_eq!(last.to, treasury);
    }

    #[test]
    fn non_harvester_cannot_claim() {
        let f = setup();
        blnd_fixture(&f, 100);
        f.wc.set_rewards_proxy(&f.admin, &Some(f.pool_addr.clone()));
        let rando = Address::generate(&f.env);
        let args: Vec<Val> = vec![&f.env, f.wrapper_addr.into_val(&f.env)];
        assert!(
            f.wc.try_claim_rewards(&rando, &symbol_short!("claim"), &args)
                .is_err()
        );
    }

    #[test]
    fn non_setter_cannot_set_proxy() {
        let f = setup();
        let rando = Address::generate(&f.env);
        assert!(
            f.wc.try_set_rewards_proxy(&rando, &Some(f.pool_addr.clone()))
                .is_err()
        );
    }
}

// ── enforcing auth: only the user's signature is mocked; the wrapper→pool
// invoker auth and the pool's nested pull (covered by `authorize_pull`) run
// against the real host auth logic — `mock_all_auths` would mask a missing
// pre-authorization. ──

mod enforcing_auth {
    use super::*;
    use soroban_sdk::IntoVal;
    use soroban_sdk::testutils::{MockAuth, MockAuthInvoke};

    fn mock_deposit_auth(f: &Fix, amount: i128) {
        f.env.mock_auths(&[MockAuth {
            address: &f.user,
            invoke: &MockAuthInvoke {
                contract: &f.wrapper_addr,
                fn_name: "deposit",
                args: (amount, f.user.clone(), f.user.clone(), f.user.clone()).into_val(&f.env),
                sub_invokes: &[MockAuthInvoke {
                    contract: &f.und_addr,
                    fn_name: "transfer",
                    args: (f.user.clone(), f.wrapper_addr.clone(), amount).into_val(&f.env),
                    sub_invokes: &[],
                }],
            },
        }]);
    }

    #[test]
    fn deposit_with_real_contract_auth_chain() {
        let f = setup_enforcing();
        mock_deposit_auth(&f, 400);
        let shares = f.wc.deposit(&400, &f.user, &f.user, &f.user);
        assert_eq!(shares, 400);
        assert_eq!(f.und.balance(&f.pool_addr), 400);
        assert_eq!(f.wc.total_vault_shares(), 400);
    }

    #[test]
    fn redeem_with_real_contract_auth_chain() {
        let f = setup_enforcing();
        mock_deposit_auth(&f, 400);
        f.wc.deposit(&400, &f.user, &f.user, &f.user);

        // redeem needs only the operator's root signature: the pool pays the
        // receiver from its own balance.
        f.env.mock_auths(&[MockAuth {
            address: &f.user,
            invoke: &MockAuthInvoke {
                contract: &f.wrapper_addr,
                fn_name: "redeem",
                args: (150i128, f.user.clone(), f.user.clone(), f.user.clone()).into_val(&f.env),
                sub_invokes: &[],
            },
        }]);
        let assets = f.wc.redeem(&150, &f.user, &f.user, &f.user);
        assert_eq!(assets, 150);
        assert_eq!(f.und.balance(&f.user), 1_000_000 - 400 + 150);
    }

    #[test]
    fn withdraw_with_real_contract_auth_chain() {
        let f = setup_enforcing();
        mock_deposit_auth(&f, 400);
        f.wc.deposit(&400, &f.user, &f.user, &f.user);

        f.env.mock_auths(&[MockAuth {
            address: &f.user,
            invoke: &MockAuthInvoke {
                contract: &f.wrapper_addr,
                fn_name: "withdraw",
                args: (150i128, f.user.clone(), f.user.clone(), f.user.clone()).into_val(&f.env),
                sub_invokes: &[],
            },
        }]);
        let burned = f.wc.withdraw(&150, &f.user, &f.user, &f.user);
        assert_eq!(burned, 150);
        assert_eq!(f.und.balance(&f.user), 1_000_000 - 400 + 150);
    }

    #[test]
    fn claim_rewards_pool_invoker_auth_is_real() {
        use soroban_sdk::{Val, Vec, symbol_short};

        let f = setup_enforcing();
        let blnd_addr = f.env.register(MockUnderlyingContract, ());
        let blnd = MockUnderlyingContractClient::new(&f.env, &blnd_addr);
        blnd.initialize(
            &String::from_str(&f.env, "Blend"),
            &String::from_str(&f.env, "BLND"),
            &7,
        );
        blnd.mint(&f.pool_addr, &500);
        f.pool.set_claim(&blnd_addr, &500);

        f.env.mock_auths(&[MockAuth {
            address: &f.admin,
            invoke: &MockAuthInvoke {
                contract: &f.wrapper_addr,
                fn_name: "set_rewards_proxy",
                args: (f.admin.clone(), Some(f.pool_addr.clone())).into_val(&f.env),
                sub_invokes: &[],
            },
        }]);
        f.wc.set_rewards_proxy(&f.admin, &Some(f.pool_addr.clone()));

        let treasury = Address::generate(&f.env);
        let args: Vec<Val> = vec![
            &f.env,
            f.wrapper_addr.into_val(&f.env),
            vec![&f.env, 1u32].into_val(&f.env),
            treasury.into_val(&f.env),
        ];
        // only the harvester's root signature is mocked; the pool's
        // `from.require_auth()` (from = wrapper) passes as real invoker auth
        f.env.mock_auths(&[MockAuth {
            address: &f.admin,
            invoke: &MockAuthInvoke {
                contract: &f.wrapper_addr,
                fn_name: "claim_rewards",
                args: (f.admin.clone(), symbol_short!("claim"), args.clone()).into_val(&f.env),
                sub_invokes: &[],
            },
        }]);
        f.wc.claim_rewards(&f.admin, &symbol_short!("claim"), &args);
        assert_eq!(blnd.balance(&treasury), 500);
    }
}

// ── PT integration: underlying → Blend wrapper → PrincipalToken ──
// The wrapper serves as PT's IBT: PT deposits route underlying into the Blend
// pool, b_rate growth shows up as PT yield, and PT redeem/withdraw exit
// through the pool.

mod pt_integration {
    use super::*;
    use principal_token::{PrincipalTokenContract, PrincipalTokenContractClient};

    const ONE_YEAR_SECS: u64 = 31_536_000;
    const UNIT: i128 = 10_000_000; // 1.0 at 7 decimals

    const YT_WASM: &[u8] = include_bytes!("../../../target/wasm32v1-none/release/yield_token.wasm");

    struct PtFix {
        f: Fix,
        pt: PrincipalTokenContractClient<'static>,
        seeded: i128,
    }

    fn setup_pt() -> PtFix {
        let env = Env::default();
        env.mock_all_auths_allowing_non_root_auth();
        // the user→PT→wrapper→pool→token call tree exceeds the default test budget
        env.cost_estimate().budget().reset_unlimited();
        let f = build_fix(env, SCALAR_12);
        f.und.mint(&f.user, &(1_000 * UNIT));

        // Seed the wrapper before PT init: PT rejects an IBT with zero total_assets.
        let seeded = f.wc.deposit(&(50 * UNIT), &f.user, &f.user, &f.user);

        let registry_addr = f
            .env
            .register(registry::RegistryContract, (f.admin.clone(),));
        let registry_c = registry::RegistryContractClient::new(&f.env, &registry_addr);
        registry_c.set_fee_collector(&f.admin, &Address::generate(&f.env));
        registry_c.set_yt_wasm_hash(&f.admin, &f.env.deployer().upload_contract_wasm(YT_WASM));

        let pt_addr = f.env.register(PrincipalTokenContract, ());
        let pt = PrincipalTokenContractClient::new(&f.env, &pt_addr);
        pt.initialize(&f.wrapper_addr, &ONE_YEAR_SECS, &f.admin, &registry_addr);

        PtFix { f, pt, seeded }
    }

    /// PT pulls `assets` underlying from the user and routes them into Blend.
    fn deposit_via_pt(p: &PtFix, assets: i128) -> i128 {
        p.f.und.approve(
            &p.f.user,
            &p.pt.address,
            &assets,
            &(p.f.env.ledger().sequence() + 1000),
        );
        p.pt.deposit(&p.f.user, &assets, &p.f.user, &p.f.user)
    }

    #[test]
    fn pt_reads_wrapper_as_ibt() {
        let p = setup_pt();
        assert_eq!(p.pt.underlying(), p.f.und_addr);
        assert_eq!(p.pt.decimals(), 7);
        // b_rate 1e12 → IBT rate exactly 1.0 Ray
        assert_eq!(p.pt.get_ibt_rate(), 10_i128.pow(27));
    }

    #[test]
    fn pt_deposit_routes_underlying_into_blend() {
        let p = setup_pt();
        let pool_before = p.f.und.balance(&p.f.pool_addr);
        let shares = deposit_via_pt(&p, 10 * UNIT);
        assert_eq!(shares, 10 * UNIT); // 1:1 rates, no fees
        assert_eq!(p.pt.balance(&p.f.user), shares);
        // underlying went user → PT → wrapper → pool
        assert_eq!(p.f.und.balance(&p.f.pool_addr), pool_before + 10 * UNIT);
        assert_eq!(p.f.und.balance(&p.pt.address), 0);
        assert_eq!(p.f.und.balance(&p.f.wrapper_addr), 0);
        // PT holds the wrapper shares; YT minted 1:1
        assert_eq!(p.f.wc.balance(&p.pt.address), 10 * UNIT);
        let yt = soroban_sdk::token::Client::new(&p.f.env, &p.pt.get_yt());
        assert_eq!(yt.balance(&p.f.user), shares);
    }

    #[test]
    fn pt_deposit_ibt_takes_wrapper_shares() {
        let p = setup_pt();
        p.f.wc.approve(
            &p.f.user,
            &p.pt.address,
            &p.seeded,
            &(p.f.env.ledger().sequence() + 1000),
        );
        let shares = p.pt.deposit_ibt(&p.f.user, &p.seeded, &p.f.user, &p.f.user);
        assert_eq!(shares, p.seeded);
        assert_eq!(p.f.wc.balance(&p.f.user), 0);
        assert_eq!(p.f.wc.balance(&p.pt.address), p.seeded);
    }

    #[test]
    fn blend_accrual_shows_up_as_pt_yield_on_redeem() {
        let p = setup_pt();
        let shares = deposit_via_pt(&p, 10 * UNIT);

        // 10% Blend accrual; fund the pool's payout liquidity.
        set_rate(&p.f, 1_100_000_000_000);
        p.f.und.mint(&p.f.pool_addr, &(100 * UNIT));

        let before = p.f.und.balance(&p.f.user);
        // redeem returns the principal; the 10% accrual is YT yield
        let back = p.pt.redeem(&p.f.user, &shares, &p.f.user, &p.f.user);
        assert!((10 * UNIT - 10..=10 * UNIT).contains(&back));
        assert_eq!(p.pt.balance(&p.f.user), 0);

        let yield_assets = p.pt.claim_yield(&p.f.user, &p.f.user, &0);
        assert!(yield_assets > 0);
        assert!(back + yield_assets > 10 * UNIT);
        assert_eq!(p.f.und.balance(&p.f.user), before + back + yield_assets);
        // the flow refreshed PT's stored IBT rate from the wrapper
        assert!(p.pt.get_ibt_rate() > 10_i128.pow(27));
    }

    #[test]
    fn pt_withdraw_exact_assets_works() {
        let p = setup_pt();
        deposit_via_pt(&p, 10 * UNIT);
        let before = p.f.und.balance(&p.f.user);
        let burned = p.pt.withdraw(&p.f.user, &(3 * UNIT), &p.f.user, &p.f.user);
        assert!(burned > 0);
        assert_eq!(p.f.und.balance(&p.f.user), before + 3 * UNIT);
    }
}

// ── admin 2-step: the wrapper inherits `admin`/`transfer_admin_role`/
// `accept_admin_transfer` as default trait methods from
// `SpectraFungibleVaultWrapper`; these guard that they are exposed on the
// deployed Blend contract and operate on its `stellar-access` admin storage. ──

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

        // New admin can grant roles.
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
        // Enforcing fixture: no auths mocked. The current admin's
        // require_auth() (enforced inside stellar-access) has no matching
        // AuthorizationEntry, so the transfer must revert.
        let f = setup_enforcing();
        let new_admin = Address::generate(&f.env);
        assert!(
            f.wc.try_transfer_admin_role(&new_admin, &live_until(&f.env))
                .is_err()
        );
    }
}

mod unsupported {
    use super::*;

    #[test]
    fn flows_panic_not_supported() {
        let f = setup();
        let u = &f.user;
        assert_blend_error(
            f.wc.try_mint(&100, u, u, u),
            BlendWrapperError::NotSupported,
        );
        assert_blend_error(f.wc.try_wrap(&100, u, u), BlendWrapperError::NotSupported);
        assert_blend_error(
            f.wc.try_unwrap(&100, u, u, u),
            BlendWrapperError::NotSupported,
        );
        assert_blend_error(
            f.wc.try_wrap_with_min_shares(&100, u, u, &0),
            BlendWrapperError::NotSupported,
        );
        assert_blend_error(
            f.wc.try_unwrap_with_min_vault_shares(&100, u, u, u, &0),
            BlendWrapperError::NotSupported,
        );
    }

    #[test]
    fn previews_panic_not_supported() {
        let f = setup();
        assert_blend_error(f.wc.try_preview_mint(&100), BlendWrapperError::NotSupported);
        assert_blend_error(f.wc.try_preview_wrap(&100), BlendWrapperError::NotSupported);
        assert_blend_error(
            f.wc.try_preview_unwrap(&100),
            BlendWrapperError::NotSupported,
        );
    }

    #[test]
    fn max_mint_is_zero() {
        let f = setup();
        assert_eq!(f.wc.max_mint(&f.user), 0);
    }
}

// ===========================================================================
// BREACH-HUNT: blend-fvw additional coverage
// ===========================================================================
//
// Targets gaps in the 60-test baseline: upgrade auth, role rotation,
// transfer pause-gating, claim_rewards permission boundary, slippage-style
// assertions, and pause cascade smoke.
// ===========================================================================

mod breach_hunt {
    use super::*;
    use soroban_sdk::{MuxedAddress, Symbol, Val, Vec as SVec};
    use spectra_common::constants::roles;

    /// Upgrade requires `UPGRADE` role.
    #[test]
    fn test_attack_non_upgrade_role_cannot_upgrade() {
        let f = setup();
        let attacker = Address::generate(&f.env);
        let hash = soroban_sdk::BytesN::<32>::from_array(&f.env, &[1u8; 32]);
        let r = f.wc.try_upgrade(&attacker, &hash);
        assert!(r.is_err(), "non-UPGRADE role must not upgrade");
    }

    /// Admin can rotate PAUSER. Old admin loses pause power once their role
    /// is revoked AFTER a new pauser is granted (so we never strip the only
    /// holder).
    #[test]
    fn test_pauser_role_rotation_admin_loses_after_revoke() {
        let f = setup();
        let new_pauser = Address::generate(&f.env);
        f.wc.grant_role(&f.admin, &new_pauser, &roles::PAUSER);
        f.wc.revoke_role(&f.admin, &f.admin, &roles::PAUSER);

        assert!(f.wc.try_pause(&f.admin).is_err(), "old admin loses pause");
        f.wc.pause(&new_pauser);
        assert!(f.wc.paused());
    }

    /// Wrapper share `transfer` is gated by `when_not_paused`.
    #[test]
    fn test_share_transfer_blocked_while_paused() {
        let f = setup();
        // Mint some wrapper shares to user.
        f.und.approve(
            &f.user,
            &f.wrapper_addr,
            &500,
            &(f.env.ledger().sequence() + 100),
        );
        let _ = f.wc.deposit(&500, &f.user, &f.user, &f.user);

        f.wc.pause(&f.admin);

        let other = Address::generate(&f.env);
        let other_muxed: MuxedAddress = other.clone().into();
        let r = f.wc.try_transfer(&f.user, &other_muxed, &10);
        assert!(r.is_err(), "share transfer must panic while paused");
    }

    /// `claim_rewards` without a proxy panics — fail-fast guard.
    #[test]
    fn test_claim_rewards_without_proxy_panics() {
        let f = setup();
        let bogus_fn = Symbol::new(&f.env, "x");
        let args: SVec<Val> = SVec::new(&f.env);
        let r = f.wc.try_claim_rewards(&f.admin, &bogus_fn, &args);
        assert!(r.is_err());
    }

    /// Set rewards proxy to `Some` then `None` clears it.
    #[test]
    fn test_rewards_proxy_set_then_clear() {
        let f = setup();
        let proxy = Address::generate(&f.env);
        f.wc.set_rewards_proxy(&f.admin, &Some(proxy));
        assert!(f.wc.rewards_proxy().is_some());
        f.wc.set_rewards_proxy(&f.admin, &None);
        assert!(f.wc.rewards_proxy().is_none());
    }

    /// Round-trip deposit → redeem never profits the user (rounding favors
    /// the protocol). Pin this — broader version of the existing
    /// `deposit_redeem_round_trip_never_profits` but at multiple amounts.
    #[test]
    fn test_round_trip_floor_bias_holds_across_amounts() {
        let f = setup();
        for amount in [1i128, 100, 10_000, 999_999] {
            // Mint per-iteration to keep balances independent.
            let u = Address::generate(&f.env);
            f.und.mint(&u, &amount);
            f.und.approve(
                &u,
                &f.wrapper_addr,
                &amount,
                &(f.env.ledger().sequence() + 100),
            );
            let shares = f.wc.deposit(&amount, &u, &u, &u);
            let back = f.wc.redeem(&shares, &u, &u, &u);
            assert!(
                back <= amount,
                "round trip must not profit user: amount={amount} back={back}"
            );
        }
    }

    /// b_token rounding sandwich (#7): across a non-trivial b_rate
    /// (just above SCALAR_12), the round-trip
    /// `to_asset_from_b_token(to_b_token_down(x))` MUST be `<= x`. Pin the
    /// Floor-down → Floor-up dust direction.
    #[test]
    fn test_b_token_rounding_sandwich_never_profits_user() {
        let f = setup_with_rate(SCALAR_12 + 1); // just above 1.0
        // Multiple amounts to sweep the rounding boundary.
        for amount in [1i128, 7, 1_000, 999_999, 100_000_000] {
            let u = Address::generate(&f.env);
            f.und.mint(&u, &amount);
            f.und.approve(
                &u,
                &f.wrapper_addr,
                &amount,
                &(f.env.ledger().sequence() + 100),
            );
            let shares = f.wc.deposit(&amount, &u, &u, &u);
            let back = f.wc.redeem(&shares, &u, &u, &u);
            assert!(
                back <= amount,
                "b_token sandwich at rate>1.0 must not profit: amount={amount} back={back}"
            );
        }
    }
}

// ===========================================================================
// Differential equivalence: the optimized wrapper (per-timestamp `b_rate`
// cache + single deposit reserve read) must produce byte-identical results to
// the pre-optimization code, which re-read the pool on every conversion.
//
// The oracle is `to_b_token_*(x, b_rate)` applied to the b_rate the pool
// reports — exactly what `load_reserve().data.b_rate` fed the math before the
// cache existed. If the cached wrapper ever diverges from this oracle, the
// optimization changed behavior. Equivalence holds because Blend's b_rate is
// invariant within a ledger timestamp; these tests pin every place the cache is
// read or primed (cold miss, deposit-priming, across a submit, across accrual).
// ===========================================================================

/// Amounts chosen to exercise both rounding directions and large magnitudes.
const EQUIV_AMOUNTS: [i128; 8] = [0, 1, 2, 67, 100, 301, 999_999, 100_000_000];

/// Representative b_rates: exactly 1.0, just above, and non-round appreciations.
const EQUIV_RATES: [i128; 5] = [
    SCALAR_12,
    SCALAR_12 + 1,
    1_200_000_000_000,
    1_500_000_000_000,
    3_333_333_333_333,
];

/// Assert every conversion/preview on `f.wc` equals a fresh-read computation at
/// `b_rate` — the pre-cache behavior.
fn assert_matches_fresh_read(f: &Fix, b_rate: i128) {
    for &x in EQUIV_AMOUNTS.iter() {
        assert_eq!(
            f.wc.convert_to_shares(&x),
            to_b_token_down(&f.env, x, b_rate),
            "convert_to_shares diverged from fresh read: x={x} b_rate={b_rate}"
        );
        assert_eq!(
            f.wc.preview_deposit(&x),
            to_b_token_down(&f.env, x, b_rate),
            "preview_deposit diverged from fresh read: x={x} b_rate={b_rate}"
        );
        assert_eq!(
            f.wc.convert_to_assets(&x),
            to_asset_from_b_token(&f.env, x, b_rate),
            "convert_to_assets diverged from fresh read: x={x} b_rate={b_rate}"
        );
        assert_eq!(
            f.wc.preview_redeem(&x),
            to_asset_from_b_token(&f.env, x, b_rate),
            "preview_redeem diverged from fresh read: x={x} b_rate={b_rate}"
        );
    }
}

/// Cold cache: the first conversion is a miss that reads the pool, the rest are
/// hits. All must equal the fresh-read oracle, across a range of rates.
#[test]
fn cache_equiv_cold_then_hits() {
    for &r in EQUIV_RATES.iter() {
        let f = setup_with_rate(r);
        assert_matches_fresh_read(&f, r);
    }
}

/// `deposit` primes the cache from its single reserve read; later conversions
/// at the same timestamp (cache hits) must still equal fresh reads.
#[test]
fn cache_equiv_after_deposit_primes_same_ts() {
    for &r in EQUIV_RATES.iter() {
        let f = setup_with_rate(r);
        f.wc.deposit(&500, &f.user, &f.user, &f.user);
        assert_matches_fresh_read(&f, r);
    }
}

/// Two state-changing submits (deposit then redeem) at one timestamp must not
/// perturb the result: b_rate is invariant within a timestamp, so the cached
/// value still equals a fresh read.
#[test]
fn cache_equiv_across_submit_same_ts() {
    let r = 1_200_000_000_000;
    let f = setup_with_rate(r);
    let shares = f.wc.deposit(&400, &f.user, &f.user, &f.user);
    f.wc.redeem(&(shares / 2), &f.user, &f.user, &f.user);
    assert_matches_fresh_read(&f, r);
}

/// Across accrual (clock advances, b_rate grows) the cache must invalidate and
/// track the NEW rate — never serve the stale one.
#[test]
fn cache_equiv_tracks_accrual() {
    let f = setup_with_rate(SCALAR_12);
    assert_matches_fresh_read(&f, SCALAR_12);

    // Prime at the old rate, then accrue (Blend only moves b_rate with time).
    f.wc.deposit(&500, &f.user, &f.user, &f.user);
    let new_rate = 1_750_000_000_000;
    f.env.ledger().with_mut(|li| li.timestamp += 1);
    f.pool.set_b_rate(&f.und_addr, &new_rate);

    assert_matches_fresh_read(&f, new_rate);
    assert_ne!(
        f.wc.convert_to_assets(&1_000_000),
        to_asset_from_b_token(&f.env, 1_000_000, SCALAR_12),
        "stale b_rate served after accrual"
    );

    // A deposit at the new timestamp must prime with the new rate too.
    f.wc.deposit(&300, &f.user, &f.user, &f.user);
    assert_matches_fresh_read(&f, new_rate);
}

// ===========================================================================
// Adversarial tests for the per-timestamp `b_rate` cache.
//
// The cache stores `(ledger_timestamp, b_rate)` in temporary storage and serves
// it for every convert/preview call sharing that timestamp. Its correctness
// rests on ONE invariant: Blend's `b_rate` only advances with elapsed time, so
// it is constant within a single ledger timestamp (verified in the pool source:
// `Reserve::load` short-circuits when `timestamp == last_time`). These tests
// attack that boundary: they force a rate change at a FROZEN timestamp (which
// real Blend cannot do, but the mock can) to prove (a) the cache is genuinely
// bound to the timestamp, and (b) that even a violated invariant cannot mint a
// wrong amount — `deposit` prices off a fresh reserve read it takes itself, so
// the stale cache value never reaches the mint path.
// ===========================================================================
mod cache_attacks {
    use super::*;

    /// Populate the cache by issuing one conversion at the current timestamp.
    fn warm_cache(f: &Fix) {
        let _ = f.wc.convert_to_assets(&SCALAR_12);
    }

    /// ATTACK: move `b_rate` WITHOUT advancing the clock, then read again.
    /// The cache must return the STALE (pre-change) value — proving the
    /// optimization is bound to the timestamp invariant. If this ever returns
    /// the fresh value, the cache keying changed and the security argument
    /// (b_rate constant per timestamp) must be re-derived.
    #[test]
    fn test_frozen_timestamp_rate_change_serves_stale() {
        let f = setup(); // rate = SCALAR_12 (1.0)
        warm_cache(&f);
        let before = f.wc.convert_to_assets(&1_000_000);

        // Double the pool rate at the SAME timestamp (unreachable on real Blend).
        f.pool.set_b_rate(&f.und_addr, &(2 * SCALAR_12));

        let after = f.wc.convert_to_assets(&1_000_000);
        assert_eq!(
            before, after,
            "cache must serve the stale rate within a frozen timestamp"
        );
    }

    /// SAFETY: once the ledger clock advances, the cache key misses and the
    /// fresh rate is read. No stale value survives across timestamps.
    #[test]
    fn test_rate_refreshes_when_timestamp_advances() {
        let f = setup();
        warm_cache(&f);
        let before = f.wc.convert_to_assets(&1_000_000);

        // Advance the clock AND move the rate (the faithful Blend accrual model).
        set_rate(&f, 2 * SCALAR_12);

        let after = f.wc.convert_to_assets(&1_000_000);
        assert!(
            after > before,
            "after the clock advances the fresh (higher) rate must be used: \
             before={before} after={after}"
        );
    }

    /// SAFETY: repeated reads at the same timestamp are internally consistent
    /// (the cache hit returns a byte-identical value).
    #[test]
    fn test_repeated_reads_same_timestamp_identical() {
        let f = setup();
        let a = f.wc.convert_to_assets(&123_456);
        let b = f.wc.convert_to_assets(&123_456);
        let c = f.wc.convert_to_assets(&123_456);
        assert_eq!(a, b);
        assert_eq!(b, c);
    }

    /// DESYNC-IMMUNE: `deposit` prices off a fresh reserve read it takes itself
    /// (re-priming the cache), so even a warmed-then-changed same-timestamp rate
    /// can never make it mint wrong — it mints at the REAL (changed) rate and
    /// succeeds. The stale cache value never reaches the mint path.
    #[test]
    fn test_stale_cache_deposit_prices_fresh_rate() {
        let f = setup();
        warm_cache(&f); // cache rate = 1.0 at this timestamp

        // Rate jumps at the SAME timestamp; the pool will now mint at 2.0.
        f.pool.set_b_rate(&f.und_addr, &(2 * SCALAR_12));

        // deposit re-reads the reserve: 500 assets at 2.0 ⇒ 250 shares minted.
        let minted = f.wc.deposit(&500, &f.user, &f.user, &f.user);
        assert_eq!(minted, 250, "deposit must mint at the fresh 2.0 rate");
    }

    /// SAFETY: with a warmed cache and NO rate change, deposit still succeeds
    /// (the guard does not false-positive on a legitimate cache hit).
    #[test]
    fn test_warmed_cache_deposit_succeeds_when_rate_unchanged() {
        let f = setup();
        warm_cache(&f);
        let shares = f.wc.deposit(&500, &f.user, &f.user, &f.user);
        assert_eq!(shares, 500, "1.0 rate ⇒ shares == assets");
    }

    /// ATTACK (solvency-breaking): Blend socialises an unbackstopped loss by
    /// discretely *reducing* `b_rate` (default_liabilities) — a downward move
    /// that is NOT accrual and can happen within one ledger timestamp. If
    /// `withdraw` prices the burn off a stale (pre-loss) cached `b_rate` while
    /// the pool removes bTokens at the fresh (post-loss) rate, the withdrawer
    /// dodges the loss and the supply/position invariant breaks — the wrapper
    /// goes insolvent for the remaining holders.
    ///
    /// Sequence at a single timestamp: deposit warms the cache at 1.0, the pool
    /// `b_rate` drops to 0.5, then `withdraw` must refresh (like `redeem`) so it
    /// burns shares at the real 0.5 rate. Post-withdraw the pool position must
    /// still cover the wrapper supply.
    #[test]
    fn test_frozen_timestamp_rate_drop_withdraw_preserves_solvency() {
        let f = setup(); // rate = 1.0

        // supply == position == 300 bTokens, cache primed to 1.0 by deposit.
        f.wc.deposit(&300, &f.user, &f.user, &f.user);
        assert_eq!(f.wc.total_supply(), 300);
        assert_eq!(f.wc.total_vault_shares(), 300);

        // Unbackstopped-loss socialisation: b_rate halves at the SAME timestamp.
        f.pool.set_b_rate(&f.und_addr, &(SCALAR_12 / 2));

        // Withdraw 100 underlying. At the real 0.5 rate this must burn 200
        // shares and remove 200 bTokens; against the stale 1.0 cache it would
        // burn only 100 shares while the pool removes 200 bTokens.
        let burned = f.wc.withdraw(&100, &f.user, &f.user, &f.user);
        assert_eq!(
            burned, 200,
            "withdraw must price the burn at the fresh 0.5 rate"
        );

        // Core invariant: the pool position must still cover the wrapper supply.
        assert!(
            f.wc.total_vault_shares() >= f.wc.total_supply(),
            "position {} must cover supply {} after withdraw",
            f.wc.total_vault_shares(),
            f.wc.total_supply(),
        );
    }
}

// ===========================================================================
// RED-TEAM: cache vs pool activity, multi-user and multi-tx interleaving.
//
// Attack model: warm the wrapper's b_rate cache, then act on the pool / as
// other users / across transactions, and try to make the cached rate desync
// from the pool so a deposit mints too much or a redeem over-withdraws.
//
// Blend's b_rate is invariant within a ledger timestamp (supply/withdraw are
// rate-neutral; it only accrues over elapsed time). The cache is keyed by
// timestamp, so: (a) pool activity at the same timestamp cannot desync it,
// (b) a real rate change always advances the clock and misses the cache, and
// (c) `deposit` prices off its own fresh reserve read, so even a
// (mainnet-impossible) forced same-timestamp desync mints at the real rate
// rather than a stale one.
// ===========================================================================
mod cache_pool_multiuser_attacks {
    use super::*;

    fn warm(f: &Fix) {
        let _ = f.wc.convert_to_assets(&SCALAR_12);
    }

    /// ATTACK: warm cache → supply directly into the POOL (same timestamp) →
    /// deposit through the wrapper. Pool supply is rate-neutral, so the cached
    /// b_rate stays valid and the deposit's mint matches (guard does not trip).
    #[test]
    fn pool_supply_same_timestamp_keeps_cache_valid() {
        let f = setup_with_rate(1_500_000_000_000); // 1.5
        f.wc.deposit(&3_000, &f.user, &f.user, &f.user); // establish position
        warm(&f);
        donate_position(&f, 50_000); // direct pool SUPPLY for the wrapper, same ts
        // If the cache had desynced, this would panic BTokenMismatch.
        let minted = f.wc.deposit(&3_000, &f.user, &f.user, &f.user);
        assert_eq!(minted, f.wc.convert_to_shares(&3_000));
    }

    /// ATTACK (requested vector): warm cache, force b_rate to change at a FROZEN
    /// timestamp (unreachable on real Blend), then deposit through the wrapper.
    /// `deposit` re-reads the reserve and prices the mint off the fresh rate, so
    /// the stale cache can never cause a wrong mint: 1_000 assets at 2.0 ⇒ 500.
    #[test]
    fn frozen_timestamp_desync_then_deposit_prices_fresh() {
        let f = setup();
        warm(&f); // cache = 1.0
        f.pool.set_b_rate(&f.und_addr, &2_000_000_000_000); // desync at same ts
        let minted = f.wc.deposit(&1_000, &f.user, &f.user, &f.user);
        assert_eq!(
            minted, 500,
            "deposit prices the mint off its own fresh read"
        );
    }

    /// Multi-user, same timestamp: A warms the cache, B rides it. The b_rate is
    /// global, so equal assets mint equal shares and the position covers both —
    /// no cross-user drain from the shared cache.
    #[test]
    fn multi_user_same_timestamp_no_cross_drain() {
        let f = setup_with_rate(1_500_000_000_000);
        let b = Address::generate(&f.env);
        f.und.mint(&b, &1_000_000);
        let ma = f.wc.deposit(&6_000, &f.user, &f.user, &f.user); // warms
        let mb = f.wc.deposit(&6_000, &b, &b, &b); // cached rate
        assert_eq!(ma, mb, "same assets+rate ⇒ same shares");
        assert_eq!(f.wc.balance(&f.user), ma);
        assert_eq!(f.wc.balance(&b), mb);
        assert!(
            f.wc.total_vault_shares() >= ma + mb,
            "position must cover both"
        );
        // both can fully exit (fund pool payout liquidity first)
        f.und.mint(&f.pool_addr, &10_000_000);
        let xa = f.wc.redeem(&ma, &f.user, &f.user, &f.user);
        let xb = f.wc.redeem(&mb, &b, &b, &b);
        assert_eq!(xa, xb, "symmetric redeem");
    }

    /// Multi-tx across a timestamp advance + accrual: the cache misses and the
    /// later deposit prices at the NEW rate (fewer shares for equal assets).
    #[test]
    fn multi_tx_accrual_refreshes_cache() {
        let f = setup(); // 1.0
        let m1 = f.wc.deposit(&10_000, &f.user, &f.user, &f.user);
        set_rate(&f, 1_250_000_000_000); // advance ledger + accrue to 1.25
        f.und.mint(&f.pool_addr, &10_000_000);
        let m2 = f.wc.deposit(&10_000, &f.user, &f.user, &f.user);
        assert!(
            m2 < m1,
            "post-accrual deposit must mint fewer shares: m1={m1} m2={m2}"
        );
        assert_eq!(m2, f.wc.convert_to_shares(&10_000));
    }

    /// Redeem after a faithful accrual is sized by the refreshed rate, never a
    /// stale one: 10_000 bTokens at 2.0 must pay exactly 20_000 underlying.
    #[test]
    fn redeem_after_accrual_uses_fresh_rate() {
        let f = setup(); // 1.0 ⇒ 10_000 assets = 10_000 bTokens
        let shares = f.wc.deposit(&10_000, &f.user, &f.user, &f.user);
        assert_eq!(shares, 10_000);
        f.und.mint(&f.pool_addr, &10_000_000);
        set_rate(&f, 2_000_000_000_000); // advance + double
        let assets = f.wc.redeem(&shares, &f.user, &f.user, &f.user);
        assert_eq!(assets, 20_000, "redeem must use the accrued (fresh) rate");
    }
}

// the Blend adapter overrides the liquidity-blind `max_*` base views with
// pool-aware ceilings. All fixtures use b_rate = 1e12, so 1 share = 1 underlying.
mod max_liquidity_i06 {
    use super::*;

    #[test]
    fn max_deposit_is_supply_cap_headroom() {
        let f = setup();
        f.pool.set_supply_cap(&f.und_addr, &1_000);
        f.wc.deposit(&300, &f.user, &f.user, &f.user); // supplied 300 underlying
        assert_eq!(f.wc.max_deposit(&f.user), 700);
    }

    #[test]
    fn max_deposit_zero_at_cap() {
        let f = setup();
        f.pool.set_supply_cap(&f.und_addr, &300);
        f.wc.deposit(&300, &f.user, &f.user, &f.user);
        assert_eq!(f.wc.max_deposit(&f.user), 0);
    }

    #[test]
    fn max_deposit_zero_when_reserve_disabled() {
        let f = setup();
        f.pool.set_enabled(&f.und_addr, &false);
        assert_eq!(f.wc.max_deposit(&f.user), 0);
    }

    #[test]
    fn max_withdraw_and_redeem_clamp_to_available_liquidity() {
        let f = setup();
        f.wc.deposit(&500, &f.user, &f.user, &f.user); // owns 500 shares, supplied 500
        // borrow 300 underlying → keep ceil((300+1)·1e12/1e12)=301 bTokens so
        // utilization stays strictly below 100% (Blend rejects >= 100%): 199.
        f.pool.set_debt(&f.und_addr, &300, &SCALAR_12);
        assert_eq!(f.wc.max_withdraw(&f.user), 199);
        assert_eq!(f.wc.max_redeem(&f.user), 199);
    }

    #[test]
    fn max_withdraw_and_redeem_are_full_position_when_fully_liquid() {
        let f = setup();
        f.wc.deposit(&500, &f.user, &f.user, &f.user);
        // no debt → serviceable = supplied; clamp is the owner's own position.
        assert_eq!(f.wc.max_withdraw(&f.user), 500);
        assert_eq!(f.wc.max_redeem(&f.user), 500);
    }

    // b_rate < 1.0 (possible after a Blend loss): a fixed 1-share/1-unit buffer
    // is insufficient because Blend measures utilization as
    // floor(remaining_b_supply × b_rate) vs ceil-rounded liabilities. The buffer
    // must be computed in bToken space. Cases from the auditor's follow-up.
    #[test]
    fn max_redeem_stays_fillable_when_brate_below_one() {
        let f = setup();
        f.wc.deposit(&5, &f.user, &f.user, &f.user); // 5 shares (b_supply = 5) at rate 1.0
        set_rate(&f, SCALAR_12 / 2); // b_rate = 0.5
        f.pool.set_debt(&f.und_addr, &1, &SCALAR_12); // liabilities = 1 underlying
        // min_remaining = ceil((1+1)·1e12 / 0.5e12) = 4 → redeemable = 5 - 4 = 1.
        // Old 1-share buffer quoted 2, which reverts at 100% utilization.
        assert_eq!(f.wc.max_redeem(&f.user), 1);
    }

    #[test]
    fn max_withdraw_stays_fillable_when_brate_below_one() {
        let f = setup();
        f.wc.deposit(&10, &f.user, &f.user, &f.user); // 10 shares (b_supply = 10) at rate 1.0
        set_rate(&f, 300_000_000_000); // b_rate = 0.3
        f.pool.set_debt(&f.und_addr, &1, &SCALAR_12); // liabilities = 1 underlying
        // redeemable = 10 - ceil((1+1)·1e12 / 0.3e12) = 10 - 7 = 3 bTokens
        // → floor(3 × 0.3) = 0 underlying. Old code quoted 1, which reverts.
        assert_eq!(f.wc.max_withdraw(&f.user), 0);
    }

    // Sweep b_rate × b_supply × liabilities and assert every quote is actually
    // fillable: redeeming max_redeem bTokens must leave supplied underlying
    // strictly above liabilities (utilization < 100%), and withdrawing
    // max_withdraw underlying must do the same after Blend's ceil-rounded burn.
    #[test]
    fn max_quotes_are_always_fillable() {
        let rates = [
            300_000_000_000i128,
            SCALAR_12 / 2,
            SCALAR_12,
            1_500_000_000_000,
        ];
        let supplies = [5i128, 10, 100, 500];
        let liabs = [0i128, 1, 3, 7];
        for &rate in &rates {
            for &supply in &supplies {
                for &liab in &liabs {
                    let f = setup();
                    f.wc.deposit(&supply, &f.user, &f.user, &f.user); // b_supply = supply
                    set_rate(&f, rate);
                    if liab > 0 {
                        f.pool.set_debt(&f.und_addr, &liab, &SCALAR_12);
                    }

                    let redeem = f.wc.max_redeem(&f.user);
                    assert!(redeem <= supply, "redeem {redeem} exceeds owner shares");
                    // With no liabilities a full drain is legal (utilization 0).
                    if redeem > 0 && liab > 0 {
                        let remaining = supply - redeem;
                        let supplied = to_asset_from_b_token(&f.env, remaining, rate);
                        assert!(
                            supplied > liab,
                            "redeem {redeem} @rate {rate} supply {supply} liab {liab}: \
                             leaves {supplied} underlying, utilization >= 100%"
                        );
                    }

                    let withdraw = f.wc.max_withdraw(&f.user);
                    if withdraw > 0 && liab > 0 {
                        // Blend burns ceil(withdraw × 1e12 / b_rate) bTokens.
                        let burned = to_b_token_up(&f.env, withdraw, rate);
                        let remaining = supply - burned;
                        let supplied = to_asset_from_b_token(&f.env, remaining, rate);
                        assert!(
                            supplied > liab,
                            "withdraw {withdraw} @rate {rate} supply {supply} liab {liab}: \
                             leaves {supplied} underlying, utilization >= 100%"
                        );
                    }
                }
            }
        }
    }

    #[test]
    fn paused_zeroes_all_max_views() {
        let f = setup();
        f.wc.deposit(&500, &f.user, &f.user, &f.user);
        f.wc.pause(&f.admin);
        assert_eq!(f.wc.max_deposit(&f.user), 0);
        assert_eq!(f.wc.max_withdraw(&f.user), 0);
        assert_eq!(f.wc.max_redeem(&f.user), 0);
    }

    // Blend can clamp b_rate to 0 after bad debt (pool/src/pool/user.rs). A
    // zero (or negative) b_rate would divide by zero in the bToken conversion,
    // so the exit views must short-circuit to 0 rather than panic.
    #[test]
    fn max_exit_views_zero_when_brate_zeroed_by_bad_debt() {
        let f = setup();
        f.wc.deposit(&500, &f.user, &f.user, &f.user);
        f.pool.set_debt(&f.und_addr, &1, &SCALAR_12); // liabilities > 0
        f.env.ledger().set_timestamp(f.env.ledger().timestamp() + 1);
        f.pool.force_b_rate(&f.und_addr, &0); // bad-debt clamp
        assert_eq!(f.wc.max_redeem(&f.user), 0);
        assert_eq!(f.wc.max_withdraw(&f.user), 0);
    }
}
