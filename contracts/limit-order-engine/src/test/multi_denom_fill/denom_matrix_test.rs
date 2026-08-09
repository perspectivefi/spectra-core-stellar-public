// SPDX-License-Identifier: BUSL-1.1
//! Exhaustive / property coverage for the multi-denomination fill pipeline.
//! Split out of `test.rs` to keep the core fill tests readable. A child of
//! `multi_denom_fill`, so it reuses that module's setup helpers via
//! `use super::*`. Holds the deterministic cross-decimal matrix and the
//! opt-in (`#[ignore]`) deposit-rate/floor fuzz.

use super::*;
use proptest::prelude::*;

/// Cross-decimal matrix: underlying at 6/8/12/18 decimals, with the deposit
/// rate scaling one underlying unit (10^d) to one 18-dec wrapper share. The
/// funding-Underlying fill must settle and drain the engine at every width.
#[test]
fn cross_decimal_funding_underlying_matrix() {
    for d in [6u32, 8, 12, 18] {
        // shares = assets * scale / 1e18 maps 10^d underlying → 1e18 shares.
        let scale = 10i128.pow(36 - d);
        let s = make_wrapper_setup_dec(d, 18, scale);
        s.env.cost_estimate().budget().reset_unlimited();
        s.engine.set_fee_rate(&s.admin, &0i128);

        let und_c = spectra_mocks::MockUnderlyingContractClient::new(&s.env, &s.und_addr);
        let maker = Address::generate(&s.env);
        let taker = Address::generate(&s.env);
        let receiver = Address::generate(&s.env);

        let making = 10i128.pow(d); // one underlying token
        und_c.mint(&maker, &making);
        approve(&s, &s.und_addr, &maker, making);
        let taker_in = give_ibt(&s, &taker, 2 * making); // ≈2e18 IBT shares

        let order = mk_order(
            &s,
            1,
            OrderType::IbtForPt,
            &maker,
            MakerTokenType::Underlying,
            0,
            making,
        );
        s.engine.register_order(&order);
        let params = one_param(&s, order, making);

        let results = s
            .engine
            .fill_orders(&taker, &receiver, &params, &s.wrapper_addr, &taker_in);
        let r0 = results.get(0).unwrap();
        assert_eq!(r0.actual_making, making, "decimals={d}");
        assert!(r0.actual_taking > 0, "decimals={d}");
        assert_eq!(
            bal(&s, &s.pt_addr, &maker),
            r0.actual_taking,
            "decimals={d}"
        );
        assert_engine_drained(&s);
    }
}

/// The documented `fail_safe_rate` convention, exercised across every
/// underlying/IBT decimal pair (6..=18, underlying ≤ IBT).
///
/// `fail_safe_rate = previewConversion(1e18)` — the raw output-token amount per
/// `1e18` raw input units — already carries the output token's decimals, so the
/// engine's `floor = amount * rate / 1e18` (no on-chain rescale, Solidity
/// parity) lands in the output raw scale for any decimal gap. This pins that
/// the floor is meaningful, not decimal-broken: a healthy conversion clears it
/// (`out == floor`) and a degraded one trips it — on both the funding
/// (underlying→IBT, `previewDeposit`) and payout (IBT→underlying,
/// `previewRedeem`) legs.
#[test]
fn fail_safe_rate_preview_convention_holds_cross_decimals() {
    const WAD: i128 = 1_000_000_000_000_000_000;
    for und_dec in 6u32..=18 {
        // Exhaustive: every underlying/IBT decimal pair with underlying <= IBT
        // (the enforced invariant) — all 91 combinations.
        for ibt_dec in und_dec..=18 {
            let parity = parity_rate(und_dec, ibt_dec);
            let s = make_wrapper_setup_dec(und_dec, ibt_dec, parity);
            s.env.cost_estimate().budget().reset_unlimited();
            s.engine.set_fee_rate(&s.admin, &0i128);
            let und_c = spectra_mocks::MockUnderlyingContractClient::new(&s.env, &s.und_addr);
            let wrapper =
                spectra_mocks::MockSpectraWrapperContractClient::new(&s.env, &s.wrapper_addr);

            // Maker sets the floor to previewConversion(1e18) at the healthy rate.
            let fs_fund = wrapper.preview_deposit(&WAD); // raw IBT per 1e18 raw underlying
            let fs_pay = wrapper.preview_redeem(&WAD); // raw underlying per 1e18 raw IBT

            // (1) FUNDING, healthy: realized deposit == floor → fill clears it.
            {
                let maker = Address::generate(&s.env);
                let taker = Address::generate(&s.env);
                let receiver = Address::generate(&s.env);
                let taker_in = give_ibt(&s, &taker, 8 * 10i128.pow(und_dec));
                let making = 10i128.pow(und_dec);
                und_c.mint(&maker, &making);
                approve(&s, &s.und_addr, &maker, making);
                let order = mk_order(
                    &s,
                    1,
                    OrderType::IbtForPt,
                    &maker,
                    MakerTokenType::Underlying,
                    fs_fund,
                    making,
                );
                s.engine.register_order(&order);
                let params = one_param(&s, order, making);
                let res =
                    s.engine
                        .fill_orders(&taker, &receiver, &params, &s.wrapper_addr, &taker_in);
                assert_eq!(
                    res.get(0).unwrap().actual_making,
                    making,
                    "healthy funding und={und_dec} ibt={ibt_dec}"
                );
            }

            // (2) PAYOUT, healthy: value-preserving redeem == floor → fill clears it.
            {
                let maker = Address::generate(&s.env);
                let taker = Address::generate(&s.env);
                let receiver = Address::generate(&s.env);
                let taker_in = give_ibt(&s, &taker, 8 * 10i128.pow(und_dec));
                let ibt_amt = give_ibt(&s, &maker, 10i128.pow(und_dec));
                let live = s.env.ledger().sequence() + 100;
                soroban_sdk::token::Client::new(&s.env, &s.wrapper_addr)
                    .approve(&maker, &s.pt_addr, &ibt_amt, &live);
                principal_token::PrincipalTokenContractClient::new(&s.env, &s.pt_addr)
                    .deposit_ibt(&maker, &ibt_amt, &maker, &maker);
                let making = ibt_amt;
                approve(&s, &s.pt_addr, &maker, making);
                let order = mk_order(
                    &s,
                    2,
                    OrderType::PtForIbt,
                    &maker,
                    MakerTokenType::Underlying,
                    fs_pay,
                    making,
                );
                s.engine.register_order(&order);
                let params = one_param(&s, order, making);
                let res =
                    s.engine
                        .fill_orders(&taker, &receiver, &params, &s.wrapper_addr, &taker_in);
                assert_eq!(
                    res.get(0).unwrap().actual_making,
                    making,
                    "healthy payout und={und_dec} ibt={ibt_dec}"
                );
                assert!(
                    und_c.balance(&maker) > 0,
                    "healthy payout must deliver underlying und={und_dec} ibt={ibt_dec}"
                );
            }

            // (3) FUNDING, degraded 50%: realized deposit < floor → fill reverts.
            {
                wrapper.set_deposit_rate(&(parity / 2));
                let maker = Address::generate(&s.env);
                let taker = Address::generate(&s.env);
                let receiver = Address::generate(&s.env);
                let taker_in = give_ibt(&s, &taker, 8 * 10i128.pow(und_dec));
                let making = 10i128.pow(und_dec);
                und_c.mint(&maker, &making);
                approve(&s, &s.und_addr, &maker, making);
                let order = mk_order(
                    &s,
                    3,
                    OrderType::IbtForPt,
                    &maker,
                    MakerTokenType::Underlying,
                    fs_fund,
                    making,
                );
                s.engine.register_order(&order);
                let params = one_param(&s, order, making);
                let r = s.engine.try_fill_orders(
                    &taker,
                    &receiver,
                    &params,
                    &s.wrapper_addr,
                    &taker_in,
                );
                assert_loe_error(r, LimitOrderEngineError::FailSafeRateNotMet);
            }

            // (4) PAYOUT, degraded: rate doubled → redeem delivers less
            // underlying per IBT share → realized < floor → fill reverts.
            {
                // First deposit IBT into PT at the healthy parity rate.
                let maker = Address::generate(&s.env);
                let taker = Address::generate(&s.env);
                let receiver = Address::generate(&s.env);
                let taker_in = give_ibt(&s, &taker, 8 * 10i128.pow(und_dec));
                let ibt_amt = give_ibt(&s, &maker, 10i128.pow(und_dec));
                let live = s.env.ledger().sequence() + 100;
                soroban_sdk::token::Client::new(&s.env, &s.wrapper_addr)
                    .approve(&maker, &s.pt_addr, &ibt_amt, &live);
                principal_token::PrincipalTokenContractClient::new(&s.env, &s.pt_addr)
                    .deposit_ibt(&maker, &ibt_amt, &maker, &maker);
                // Now degrade: doubling the rate means each IBT share redeems
                // for half the underlying, falling below the floor.
                wrapper.set_deposit_rate(&(parity * 2));
                let making = ibt_amt;
                approve(&s, &s.pt_addr, &maker, making);
                let order = mk_order(
                    &s,
                    4,
                    OrderType::PtForIbt,
                    &maker,
                    MakerTokenType::Underlying,
                    fs_pay,
                    making,
                );
                s.engine.register_order(&order);
                let params = one_param(&s, order, making);
                let r = s.engine.try_fill_orders(
                    &taker,
                    &receiver,
                    &params,
                    &s.wrapper_addr,
                    &taker_in,
                );
                assert_loe_error(r, LimitOrderEngineError::FailSafeRateNotMet);
            }
        }
    }
}

/// `deposit_rate` (18-dec) mapping one underlying unit (10^und_dec) to one
/// wrapper share (10^ibt_dec) — a 1:1-value vault across a decimal gap.
fn parity_rate(und_dec: u32, ibt_dec: u32) -> i128 {
    10i128.pow(18 + ibt_dec - und_dec)
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(16))]

    /// Fuzz the deposit rate and floor: an Underlying funding fill succeeds
    /// iff the realized IBT (making·rate/UNIT) is at least the floor
    /// (making·fail_safe/UNIT), and reverts otherwise. `making == UNIT`, so
    /// realized == rate and floor == fail_safe.
    #[test]
    #[ignore = "fuzz — opt in with: cargo test -p limit-order-engine -- --ignored"]
    fn prop_funding_underlying_floor_consistency(
        rate in 1_000_000_000_000_000i128..=2_000_000_000_000_000_000i128,
        fail_safe in 0i128..=2_000_000_000_000_000_000i128,
    ) {
        let s = make_wrapper_setup();
        s.env.cost_estimate().budget().reset_unlimited();
        s.engine.set_fee_rate(&s.admin, &0i128);

        let und_c = spectra_mocks::MockUnderlyingContractClient::new(&s.env, &s.und_addr);
        let wrapper =
            spectra_mocks::MockSpectraWrapperContractClient::new(&s.env, &s.wrapper_addr);
        let maker = Address::generate(&s.env);
        let taker = Address::generate(&s.env);
        let receiver = Address::generate(&s.env);

        let making = UNIT;
        und_c.mint(&maker, &making);
        approve(&s, &s.und_addr, &maker, making);
        give_ibt(&s, &taker, 4 * UNIT);

        wrapper.set_deposit_rate(&rate);
        let order = mk_order(
            &s,
            1,
            OrderType::IbtForPt,
            &maker,
            MakerTokenType::Underlying,
            fail_safe,
            making,
        );
        s.engine.register_order(&order);
        let params = one_param(&s, order, making);

        let realized = making * rate / UNIT;
        let floor = making * fail_safe / UNIT;
        let r = s
            .engine
            .try_fill_orders(&taker, &receiver, &params, &s.wrapper_addr, &(4 * UNIT));
        if realized >= floor {
            prop_assert!(r.is_ok(), "realized {} >= floor {} must fill", realized, floor);
        } else {
            prop_assert!(r.is_err(), "realized {} < floor {} must revert", realized, floor);
        }
    }
}
