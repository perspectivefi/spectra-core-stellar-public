// SPDX-License-Identifier: BUSL-1.1
pub const RAY_UNIT: i128 = 1_000_000_000_000_000_000_000_000_000; // 1e27
pub const RAY_DECIMALS: u32 = 27;

fn decimals_ratio(decimals: u32) -> i128 {
    assert!(
        decimals <= RAY_DECIMALS,
        "decimals_ratio: decimals exceeds RAY"
    );
    let exp = RAY_DECIMALS - decimals;
    10i128.pow(exp)
}

pub fn from_ray(a: i128, decimals: u32) -> i128 {
    a / decimals_ratio(decimals)
}

pub fn from_ray_rounded(a: i128, decimals: u32, round_up: bool) -> i128 {
    assert!(a >= 0, "from_ray_rounded: negative input");
    let ratio = decimals_ratio(decimals);
    let result = a / ratio;
    if round_up && a % ratio != 0 {
        result + 1
    } else {
        result
    }
}

pub fn to_ray(a: i128, decimals: u32) -> i128 {
    let ratio = decimals_ratio(decimals);
    let result = a * ratio;
    assert!(result / ratio == a, "to_ray: overflow");
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ray_math::{from_ray, from_ray_rounded, to_ray};

    #[test]
    fn test_to_ray_18_decimals() {
        let amount: i128 = 1_000_000_000_000_000_000; // 1e18
        let result = to_ray(amount, 18);
        assert_eq!(result, RAY_UNIT); // 1e27
    }

    #[test]
    fn test_to_ray_6_decimals() {
        let amount: i128 = 1_000_000; // 1e6 (1 USDC)
        let result = to_ray(amount, 6);
        assert_eq!(result, RAY_UNIT);
    }

    #[test]
    fn test_from_ray_18_decimals() {
        let result = from_ray(RAY_UNIT, 18);
        assert_eq!(result, 1_000_000_000_000_000_000);
    }

    #[test]
    fn test_from_ray_6_decimals() {
        let result = from_ray(RAY_UNIT, 6);
        assert_eq!(result, 1_000_000);
    }

    #[test]
    fn test_from_ray_rounds_down() {
        let value = RAY_UNIT + 500_000_000; // 1e27 + some dust
        let result = from_ray(value, 18);
        assert_eq!(result, 1_000_000_000_000_000_000); // truncated
    }

    #[test]
    fn test_from_ray_rounded_up() {
        let value = RAY_UNIT + 500_000_000; // 1e27 + some dust
        let result = from_ray_rounded(value, 18, true);
        assert_eq!(result, 1_000_000_000_000_000_001); // rounded up
    }

    #[test]
    fn test_from_ray_rounded_down() {
        let value = RAY_UNIT + 500_000_000;
        let result = from_ray_rounded(value, 18, false);
        assert_eq!(result, 1_000_000_000_000_000_000); // same as from_ray
    }

    #[test]
    fn test_roundtrip_18() {
        let original: i128 = 123_456_789_000_000_000; // some 18-decimal value
        let ray = to_ray(original, 18);
        let back = from_ray(ray, 18);
        assert_eq!(back, original);
    }

    #[test]
    fn test_roundtrip_6() {
        let original: i128 = 1_234_567; // some 6-decimal value
        let ray = to_ray(original, 6);
        let back = from_ray(ray, 6);
        assert_eq!(back, original);
    }

    #[test]
    fn test_to_ray_zero() {
        assert_eq!(to_ray(0, 18), 0);
    }

    #[test]
    fn test_from_ray_zero() {
        assert_eq!(from_ray(0, 18), 0);
    }

    // Edge cases for from_ray_rounded (item 4).

    #[test]
    fn test_from_ray_rounded_exact_multiple_with_round_up() {
        // Exact multiple of ratio: remainder is 0, so round_up must not add 1.
        let result = from_ray_rounded(RAY_UNIT, 18, true);
        assert_eq!(result, 1_000_000_000_000_000_000);
    }

    #[test]
    fn test_from_ray_rounded_zero_round_up() {
        assert_eq!(from_ray_rounded(0, 18, true), 0);
        assert_eq!(from_ray_rounded(0, 6, true), 0);
    }

    #[test]
    fn test_from_ray_rounded_dust_round_up_for_6_decimals() {
        // Value smaller than the 6-decimal ratio (10^21) rounds up to 1.
        let dust: i128 = 1;
        let result = from_ray_rounded(dust, 6, true);
        assert_eq!(result, 1);
    }

    #[test]
    fn test_from_ray_rounded_dust_round_down_for_6_decimals() {
        // Same dust floors to 0.
        let dust: i128 = 1;
        let result = from_ray_rounded(dust, 6, false);
        assert_eq!(result, 0);
    }

    #[test]
    fn test_from_ray_rounded_sub_unit_round_up() {
        // Ratio for 18-decimal from_ray is 10^(27-18) = 10^9.
        // Any value 0 < v < 10^9 floors to 0 and ceils to 1.
        let half_ratio: i128 = 500_000_000; // 0.5 * 10^9
        assert_eq!(from_ray_rounded(half_ratio, 18, false), 0);
        assert_eq!(from_ray_rounded(half_ratio, 18, true), 1);
    }

    #[test]
    fn test_from_ray_rounded_ratio_minus_one() {
        // value = ratio - 1: floors to 0, ceils to 1.
        let ratio = 10i128.pow(RAY_DECIMALS - 18);
        let value = ratio - 1;
        assert_eq!(from_ray_rounded(value, 18, false), 0);
        assert_eq!(from_ray_rounded(value, 18, true), 1);
    }

    // to_ray overflow guard — see also discussion: the `a * ratio` multiply
    // already panics under workspace `overflow-checks = true`. The explicit
    // `result / ratio == a` assert is a belt-and-suspenders check for builds
    // that disable overflow-checks. This test only confirms the function
    // panics (doesn't pin the exact message).
    #[test]
    #[should_panic]
    fn test_to_ray_overflow_panics() {
        // With 6-decimal ratio = 10^21, any value >= i128::MAX / 10^21 will
        // overflow. i128::MAX is ~1.7e38, so ratio * 1e18 overflows.
        let huge: i128 = i128::MAX / 1_000_000_000;
        to_ray(huge, 6);
    }

    // ── Additional regression / boundary tests ──────────────────────────

    /// `decimals == 0` — raw integer is multiplied by 10^27. Used in the
    /// rare token-with-no-decimals case.
    #[test]
    fn test_to_ray_decimals_zero() {
        assert_eq!(to_ray(1, 0), RAY_UNIT);
        assert_eq!(to_ray(5, 0), 5 * RAY_UNIT);
    }

    /// `decimals == 27` (RAY_DECIMALS) — identity transformation.
    #[test]
    fn test_to_ray_decimals_equals_ray_decimals_is_identity() {
        let value: i128 = 12_345;
        assert_eq!(to_ray(value, RAY_DECIMALS), value);
        assert_eq!(from_ray(value, RAY_DECIMALS), value);
    }

    /// `decimals > RAY_DECIMALS` panics — the `RAY_DECIMALS - decimals`
    /// subtraction would underflow.
    #[test]
    #[should_panic(expected = "decimals_ratio: decimals exceeds RAY")]
    fn test_to_ray_decimals_above_27_panics() {
        to_ray(1, 28);
    }

    #[test]
    #[should_panic(expected = "decimals_ratio: decimals exceeds RAY")]
    fn test_from_ray_decimals_above_27_panics() {
        from_ray(RAY_UNIT, 28);
    }

    /// `from_ray_rounded` rejects negative input via raw assert.
    #[test]
    #[should_panic(expected = "from_ray_rounded: negative input")]
    fn test_from_ray_rounded_negative_panics() {
        from_ray_rounded(-1, 18, false);
    }

    /// `from_ray` with negative input: Rust integer division truncates
    /// toward zero, so the result is non-Solidity-floor for negatives.
    /// Pin the current behavior so a refactor that changes it is caught.
    #[test]
    fn test_from_ray_negative_input_truncates_toward_zero() {
        // -RAY_UNIT / 10^9 = -10^18 exact
        let result = from_ray(-RAY_UNIT, 18);
        assert_eq!(result, -1_000_000_000_000_000_000);
        // Non-exact negative: -RAY_UNIT - 1 → truncates toward zero
        let result2 = from_ray(-(RAY_UNIT + 1), 18);
        // = -(1e18 + 1 wei in ray) / 1e9 = trunc(-1.000...0001 × 1e18)
        // Rust truncated: -1_000_000_000_000_000_000
        assert_eq!(result2, -1_000_000_000_000_000_000);
    }

    /// Round-trip integrity for several decimals/value combinations,
    /// skipping pairs that would overflow `to_ray` (large values at low
    /// decimals — `value * 10^(27-d)` exceeds i128).
    #[test]
    fn test_roundtrip_integrity_sweep() {
        // (decimals, max_value_that_fits)
        let cases: &[(u32, i128)] = &[
            (0, 100),                            // ratio = 10^27 → max ≈ 1e11
            (6, 1_000_000_000),                  // ratio = 10^21
            (9, 1_000_000_000_000),              // ratio = 10^18
            (12, 1_000_000_000_000_000),         // ratio = 10^15
            (18, 1_000_000_000_000_000_000),     // ratio = 10^9
            (27, 1_000_000_000_000_000_000_000), // ratio = 1
        ];
        let small_values: &[i128] = &[1, 7, 100];
        for &(d, big) in cases {
            for &v in small_values {
                let ray = to_ray(v, d);
                assert_eq!(
                    from_ray(ray, d),
                    v,
                    "roundtrip(small) broken at d={d} v={v}"
                );
            }
            let ray = to_ray(big, d);
            assert_eq!(from_ray(ray, d), big, "roundtrip(big) broken at d={d}");
        }
    }
}
