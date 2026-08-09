// SPDX-License-Identifier: BUSL-1.1
//! Pricing math for the limit-order engine.
//!
//! All prices are 18-decimal fixed-point. The PT contract exposes its IBT
//! rate in Ray (27 decimals); we convert at the boundary via
//! `ray_math::from_ray`. Cross-contract reads happen at the top of
//! `get_price` and are then passed into the math helpers as values.
//!
//! Time math uses i128 directly (timestamps fit easily). Rate math goes
//! through `spectra_math::log_exp_math` which works on `I256`. Conversions
//! at the boundary use `I256::to_i128()` (asserts no overflow).

use soroban_sdk::{Address, Env, I256, panic_with_error};
use spectra_common::limit_order_engine::OrderType;
use spectra_common::principal_token::PrincipalTokenPricingClient;
use spectra_math::log_exp_math::log_exp_math;
use spectra_math::ray_math;

use crate::errors::LimitOrderEngineError;

/// 18-decimal fixed-point unit.
pub(crate) const UNIT_DECIMALS: u32 = 18;
pub(crate) const UNIT: i128 = 1_000_000_000_000_000_000;
const SECONDS_PER_YEAR: i128 = 31_536_000; // 365 * 24 * 60 * 60

#[inline(always)]
fn i256(env: &Env, v: i128) -> I256 {
    I256::from_i128(env, v)
}

#[inline(always)]
fn to_i128(env: &Env, v: I256) -> i128 {
    v.to_i128()
        .unwrap_or_else(|| panic_with_error!(env, LimitOrderEngineError::PriceOverflow))
}

// ---------------------------------------------------------------------------
// PricingLibrary port
// ---------------------------------------------------------------------------

/// P(t,T) = future_pt_value * e^(-r * (T-t) / year)
/// All arguments and the return value are 18-decimal fixed-point.
pub(crate) fn pt_price_in_underlying(
    env: &Env,
    implied_rate: i128,
    time_to_maturity: i128,
    future_pt_value: i128,
) -> i128 {
    // t = time_to_maturity * UNIT / SECONDS_PER_YEAR  (18 decimals)
    let t = i256(env, time_to_maturity)
        .mul(&i256(env, UNIT))
        .div(&i256(env, SECONDS_PER_YEAR));
    // base = UNIT + implied_rate
    let base = i256(env, UNIT).add(&i256(env, implied_rate));
    // rate_per_second = ln(base)
    let rate_per_second = log_exp_math::ln(env, &base);
    // denominator = exp(rate_per_second * t / UNIT)
    let exponent = rate_per_second.mul(&t).div(&i256(env, UNIT));
    let denominator = log_exp_math::exp(env, &exponent);
    // present_value = future_pt_value * UNIT / denominator
    let present_value = i256(env, future_pt_value)
        .mul(&i256(env, UNIT))
        .div(&denominator);
    to_i128(env, present_value)
}

/// Y(t,T) = future_pt_value - P(t,T)
pub(crate) fn yt_price_in_underlying(
    env: &Env,
    implied_rate: i128,
    time_to_maturity: i128,
    future_pt_value: i128,
) -> i128 {
    // implied_rate is signed here; reject negative rates explicitly.
    if implied_rate < 0 {
        panic_with_error!(env, LimitOrderEngineError::NegativeImpliedRate);
    }
    let pt_price = pt_price_in_underlying(env, implied_rate, time_to_maturity, future_pt_value);
    future_pt_value - pt_price
}

// ---------------------------------------------------------------------------
// LimitOrderPricingLibrary port
// ---------------------------------------------------------------------------

/// Compute spot and fee-adjusted price for an order. Returns `(spot, with_fee)`
/// in 18-decimal fixed-point.
///
/// Reads the PT's pricing inputs (ibt_rate, future_pt_value, maturity) in a
/// single bundled `pricing_data` call.
pub(crate) fn get_price(
    env: &Env,
    order_type: OrderType,
    pt: &Address,
    implied_rate: i128,
    implied_rate_fee: i128,
) -> (i128, i128) {
    // One bundled PT call instead of ~7 separate cross-contract reads (each of
    // which would re-instantiate the target contract, dominating per-fill memory
    // on a heavy IBT). Returns (ibt_rate Ray, future_pt_value 18-dec, maturity).
    let (ibt_rate, future_pt_value, maturity_u64) =
        PrincipalTokenPricingClient::new(env, pt).pricing_data();
    let ibt_rate_unit = ray_math::from_ray(ibt_rate, UNIT_DECIMALS);
    let now = env.ledger().timestamp() as i128;
    let maturity = maturity_u64 as i128;
    let time_to_maturity = maturity - now;

    match order_type {
        OrderType::PtForIbt => {
            // Maker sells PT for IBT. Buyer gets fee discount on the rate.
            let spot_pt =
                pt_price_in_underlying(env, implied_rate, time_to_maturity, future_pt_value);
            let spot = mul_div_unit(env, spot_pt, UNIT, ibt_rate_unit);
            let fee_rate = apply_fee_discount(env, implied_rate, implied_rate_fee);
            let with_fee_pt =
                pt_price_in_underlying(env, fee_rate, time_to_maturity, future_pt_value);
            let with_fee = mul_div_unit(env, with_fee_pt, UNIT, ibt_rate_unit);
            (spot, with_fee)
        }
        OrderType::IbtForPt => {
            // Maker sells IBT for PT. Buyer (taker pays IBT) gets fee discount.
            // Inverse of PT_FOR_IBT.
            let spot_pt =
                pt_price_in_underlying(env, implied_rate, time_to_maturity, future_pt_value);
            require_nonzero_divisor(env, spot_pt);
            let spot = mul_div_unit(env, UNIT, ibt_rate_unit, spot_pt);
            let fee_rate = apply_fee_premium(env, implied_rate, implied_rate_fee);
            let with_fee_pt =
                pt_price_in_underlying(env, fee_rate, time_to_maturity, future_pt_value);
            require_nonzero_divisor(env, with_fee_pt);
            let with_fee = mul_div_unit(env, UNIT, ibt_rate_unit, with_fee_pt);
            (spot, with_fee)
        }
        OrderType::YtForIbt => {
            // Maker sells YT for IBT. Seller forfeits fee.
            let spot_yt =
                yt_price_in_underlying(env, implied_rate, time_to_maturity, future_pt_value);
            let spot = mul_div_unit(env, spot_yt, UNIT, ibt_rate_unit);
            let fee_rate = apply_fee_premium(env, implied_rate, implied_rate_fee);
            let with_fee_yt =
                yt_price_in_underlying(env, fee_rate, time_to_maturity, future_pt_value);
            let with_fee = mul_div_unit(env, with_fee_yt, UNIT, ibt_rate_unit);
            (spot, with_fee)
        }
        OrderType::IbtForYt => {
            // Maker sells IBT for YT. Buyer (taker pays IBT) gets fee discount.
            let spot_yt =
                yt_price_in_underlying(env, implied_rate, time_to_maturity, future_pt_value);
            require_nonzero_divisor(env, spot_yt);
            let spot = mul_div_unit(env, UNIT, ibt_rate_unit, spot_yt);
            let fee_rate = apply_fee_discount(env, implied_rate, implied_rate_fee);
            let with_fee_yt =
                yt_price_in_underlying(env, fee_rate, time_to_maturity, future_pt_value);
            require_nonzero_divisor(env, with_fee_yt);
            let with_fee = mul_div_unit(env, UNIT, ibt_rate_unit, with_fee_yt);
            (spot, with_fee)
        }
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// A zero YT/PT price (dust-sized or zero implied rate) would divide by zero
/// when inverted.
fn require_nonzero_divisor(env: &Env, divisor: i128) {
    if divisor == 0 {
        panic_with_error!(env, LimitOrderEngineError::ZeroPriceDivisor);
    }
}

/// (a * b) / c via I256 — avoids i128 intermediate overflow on `a * b`.
fn mul_div_unit(env: &Env, a: i128, b: i128, c: i128) -> i128 {
    to_i128(env, i256(env, a).mul(&i256(env, b)).div(&i256(env, c)))
}

/// implied_rate * (UNIT - fee) / UNIT — buyer's discounted rate.
fn apply_fee_discount(env: &Env, implied_rate: i128, fee: i128) -> i128 {
    mul_div_unit(env, implied_rate, UNIT - fee, UNIT)
}

/// implied_rate * (UNIT + fee) / UNIT — seller's premium rate.
fn apply_fee_premium(env: &Env, implied_rate: i128, fee: i128) -> i128 {
    mul_div_unit(env, implied_rate, UNIT + fee, UNIT)
}

#[cfg(test)]
mod tests {
    //! Inline unit tests for the small pricing helpers (`mul_div_unit`,
    //! `apply_fee_discount`, `apply_fee_premium`). These are private to
    //! `pricing.rs` and not reachable from outside the crate; testing them
    //! here keeps the visibility tight while still pinning their behaviour.
    //!
    //! The heavy bit-for-bit equivalence vs Solidity lives in
    //! `test_pricing_vectors.rs` (~21k cases through pt_price/yt_price).

    use super::*;

    #[test]
    fn mul_div_unit_simple_identity() {
        let env = Env::default();
        // (a * UNIT) / UNIT == a
        assert_eq!(mul_div_unit(&env, 12345, UNIT, UNIT), 12345);
    }

    #[test]
    fn mul_div_unit_floor_rounding() {
        let env = Env::default();
        // (1 * 1) / 2 == 0 under integer floor division.
        assert_eq!(mul_div_unit(&env, 1, 1, 2), 0);
        // (3 * 1) / 2 == 1.
        assert_eq!(mul_div_unit(&env, 3, 1, 2), 1);
    }

    #[test]
    fn mul_div_unit_supports_large_intermediates_via_i256() {
        let env = Env::default();
        // a * b would overflow i128 (a, b near i128::MAX/2); via I256 it is fine.
        let half_max = i128::MAX / 2;
        let result = mul_div_unit(&env, half_max, 2, 2);
        // (half_max * 2) / 2 == half_max
        assert_eq!(result, half_max);
    }

    #[test]
    fn apply_fee_discount_with_zero_fee_returns_input() {
        let env = Env::default();
        let rate = 100_000_000_000_000_000i128; // 10% APY
        assert_eq!(apply_fee_discount(&env, rate, 0), rate);
    }

    #[test]
    fn apply_fee_discount_with_unit_fee_returns_zero() {
        let env = Env::default();
        // fee = UNIT means buyer's discounted rate is 0.
        let rate = 100_000_000_000_000_000i128;
        assert_eq!(apply_fee_discount(&env, rate, UNIT), 0);
    }

    #[test]
    fn apply_fee_premium_with_zero_fee_returns_input() {
        let env = Env::default();
        let rate = 100_000_000_000_000_000i128;
        assert_eq!(apply_fee_premium(&env, rate, 0), rate);
    }

    #[test]
    fn apply_fee_premium_with_unit_fee_doubles_input() {
        let env = Env::default();
        // fee = UNIT → rate * 2 (since (UNIT + UNIT) / UNIT == 2).
        let rate = 100_000_000_000_000_000i128;
        assert_eq!(apply_fee_premium(&env, rate, UNIT), rate * 2);
    }

    #[test]
    fn apply_fee_discount_and_premium_are_symmetric_around_zero_fee() {
        // discount(rate, 0) == premium(rate, 0) == rate.
        let env = Env::default();
        let rate = 50_000_000_000_000_000i128;
        assert_eq!(
            apply_fee_discount(&env, rate, 0),
            apply_fee_premium(&env, rate, 0),
        );
    }
}
