// SPDX-License-Identifier: MIT
/// Fixed-point math library for log and exponential functions.
///
/// Exponentiation and logarithm functions for 18 decimal fixed point numbers
/// (both base and exponent/argument).
///
/// Exponentiation and logarithm with arbitrary bases (x^y and log_x(y)) are
/// implemented by conversion to natural exponentiation and logarithm (where
/// the base is Euler's number).
///
/// This is a strict port of Balancer's Solidity `LogExpMath` library, using
/// `soroban_sdk::I256` to match Solidity's `int256` semantics exactly —
/// same constants, same domain, same intermediate precision strategy
/// (20-decimal for exp/_ln, 36-decimal for _ln_36).
///
/// All public functions take an `&Env` (required by I256/U256) plus I256
/// arguments / return values to match Solidity's int256 exactly.
/// `pow` takes and returns U256, matching Solidity's uint256.
///
/// @author Fernando Martinelli - @fernandomartinelli
/// @author Sergio Yuhjtman - @sergioyuhjtman
/// @author Daniel Fernandez - @dmf7z
#[allow(clippy::module_inception)]
pub mod log_exp_math {
    use soroban_sdk::{Env, I256, U256};

    // -----------------------------------------------------------------------
    // Inline helpers: construct I256/U256 from primitives
    // -----------------------------------------------------------------------

    #[inline(always)]
    fn i(env: &Env, v: i128) -> I256 {
        I256::from_i128(env, v)
    }

    #[inline(always)]
    fn u(env: &Env, v: u128) -> U256 {
        U256::from_u128(env, v)
    }

    // -----------------------------------------------------------------------
    // Raw constants (used to construct I256/U256 at runtime)
    // -----------------------------------------------------------------------

    // All fixed point multiplications and divisions are inlined. This means we
    // need to divide by ONE when multiplying two numbers, and multiply by ONE
    // when dividing them.

    // All arguments and return values are 18 decimal fixed point numbers.
    const ONE_18: i128 = 1_000_000_000_000_000_000; // 1e18

    // Internally, intermediate values are computed with higher precision as
    // 20 decimal fixed point numbers, and in the case of ln36, 36 decimals.
    const ONE_20: i128 = 100_000_000_000_000_000_000; // 1e20
    const ONE_36: i128 = 1_000_000_000_000_000_000_000_000_000_000_000_000; // 1e36

    // The domain of natural exponentiation is bound by the word size and number
    // of decimals used.
    //
    // Because internally the result will be stored using 20 decimals, the
    // largest possible result is (2^255 - 1) / 10^20, which makes the largest
    // exponent ln((2^255 - 1) / 10^20) = 130.700829182905140221.
    // The smallest possible result is 10^(-18), which makes the largest
    // negative argument ln(10^(-18)) = -41.446531673892822312.
    // We use 130.0 and -41.0 to have some safety margin.
    const MAX_NATURAL_EXPONENT: i128 = 130_000_000_000_000_000_000; // 130e18
    const MIN_NATURAL_EXPONENT: i128 = -41_000_000_000_000_000_000; // -41e18

    // Bounds for ln_36's argument. Both ln(0.9) and ln(1.1) can be represented
    // with 36 decimal places in a fixed point 256 bit integer.
    const LN_36_LOWER_BOUND: i128 = ONE_18 - 100_000_000_000_000_000; // ONE_18 - 1e17
    const LN_36_UPPER_BOUND: i128 = ONE_18 + 100_000_000_000_000_000; // ONE_18 + 1e17

    // MILD_EXPONENT_BOUND = 2^254 / uint256(ONE_20)
    // 2^254 = 28948022309329048855892746252171976963317496166410141009864396001978282409984
    // 2^254 / 1e20 = 289480223093290488558927462521719769633174961664101410098643960019782824
    // Since this is larger than u128::MAX, we build it from 64-bit limbs at runtime.

    // 18-decimal constants
    const X0: i128 = 128_000_000_000_000_000_000; // 2^7
    // a0 = e^(x0) (no decimals) — too large for i128, constructed at runtime
    // a0 = 38877084059945950922200000000000000000000000000000000000
    const X1: i128 = 64_000_000_000_000_000_000; // 2^6
    // a1 = e^(x1) (no decimals)
    const A1: i128 = 6_235_149_080_811_616_882_910_000_000;

    // 20-decimal constants
    const X2: i128 = 3_200_000_000_000_000_000_000; // 2^5
    const A2: i128 = 7_896_296_018_268_069_516_100_000_000_000_000; // e^(x2)
    const X3: i128 = 1_600_000_000_000_000_000_000; // 2^4
    const A3: i128 = 888_611_052_050_787_263_676_000_000; // e^(x3)
    const X4: i128 = 800_000_000_000_000_000_000; // 2^3
    const A4: i128 = 298_095_798_704_172_827_474_000; // e^(x4)
    const X5: i128 = 400_000_000_000_000_000_000; // 2^2
    const A5: i128 = 5_459_815_003_314_423_907_810; // e^(x5)
    const X6: i128 = 200_000_000_000_000_000_000; // 2^1
    const A6: i128 = 738_905_609_893_065_022_723; // e^(x6)
    const X7: i128 = 100_000_000_000_000_000_000; // 2^0
    const A7: i128 = 271_828_182_845_904_523_536; // e^(x7)
    const X8: i128 = 50_000_000_000_000_000_000; // 2^(-1)
    const A8: i128 = 164_872_127_070_012_814_685; // e^(x8)
    const X9: i128 = 25_000_000_000_000_000_000; // 2^(-2)
    const A9: i128 = 128_402_541_668_774_148_407; // e^(x9)
    const X10: i128 = 12_500_000_000_000_000_000; // 2^(-3)
    const A10: i128 = 113_314_845_306_682_631_683; // e^(x10)
    const X11: i128 = 6_250_000_000_000_000_000; // 2^(-4)
    const A11: i128 = 106_449_445_891_785_942_956; // e^(x11)

    // -----------------------------------------------------------------------
    // Runtime constant constructors
    // -----------------------------------------------------------------------

    /// Construct a0 as I256. Too large for i128, so we build from known decimal string.
    /// a0 = e^(x0) = e^128 (no decimals) = 38877084059945950922200000000000000000000000000000000000
    /// Split as 38877084059945950922200000000 * 10^27.
    #[inline(always)]
    fn a0(env: &Env) -> I256 {
        // 38877084059945950922200000000000000000000000000000000000
        // = 38877084059945950922200000000 * 10^27
        // 38877084059945950922200000000 fits i128
        let hi = i(env, 38_877_084_059_945_950_922_200_000_000);
        let scale = i(env, 1_000_000_000_000_000_000_000_000_000); // 10^27
        hi.mul(&scale)
    }

    /// MILD_EXPONENT_BOUND = 2^254 / uint256(ONE_20)
    /// Precomputed as four u64 limbs (big-endian).
    #[inline(always)]
    fn mild_exponent_bound(env: &Env) -> U256 {
        U256::from_parts(
            env,
            0x0000000000000000,
            0x0BCE5086492111AE,
            0xA88F4BB1CA6BCF58,
            0x4181EA8059F76532,
        )
    }

    // -----------------------------------------------------------------------
    // I256 <-> U256 conversions
    // -----------------------------------------------------------------------

    /// Convert U256 to I256 (panics if value >= 2^255).
    #[inline(always)]
    fn u256_to_i256(env: &Env, v: &U256) -> I256 {
        let bytes = v.to_be_bytes();
        I256::from_be_bytes(env, &bytes)
    }

    /// Convert I256 to U256 (panics if value is negative).
    ///
    /// The negative-input assert is defensive — `pow`'s `exp` path computes
    /// the value via `exp(y * ln(x))` where `x > 0` guarantees a non-negative
    /// intermediate. The assert traps refactors that would route a negative
    /// I256 here.
    #[inline(always)]
    fn i256_to_u256(env: &Env, v: &I256) -> U256 {
        let zero = i(env, 0);
        assert!(*v >= zero, "negative I256 cannot convert to U256");
        let bytes = v.to_be_bytes();
        U256::from_be_bytes(env, &bytes)
    }

    // -----------------------------------------------------------------------
    // Public API
    // -----------------------------------------------------------------------

    /// Natural exponentiation (e^x) with signed 18-decimal fixed-point exponent.
    ///
    /// Reverts if `x` is smaller than MIN_NATURAL_EXPONENT, or larger than
    /// `MAX_NATURAL_EXPONENT`.
    pub fn exp(env: &Env, x: &I256) -> I256 {
        let min_exp = i(env, MIN_NATURAL_EXPONENT);
        let max_exp = i(env, MAX_NATURAL_EXPONENT);
        assert!(*x >= min_exp && *x <= max_exp, "Invalid exponent");

        let zero = i(env, 0);
        if *x < zero {
            // We only handle positive exponents: e^(-x) is computed as 1 / e^x.
            // We can safely make x positive since it fits in the signed 256 bit
            // range (as it is larger than MIN_NATURAL_EXPONENT).
            // Fixed point division requires multiplying by ONE_18.
            let one_18 = i(env, ONE_18);
            let neg_x = zero.sub(x);
            let exp_pos = exp(env, &neg_x);
            return one_18.mul(&one_18).div(&exp_pos);
        }

        // First, we use the fact that e^(x+y) = e^x * e^y to decompose x into a
        // sum of powers of two, which we call x_n, where x_n == 2^(7 - n), and
        // e^x_n = a_n has been precomputed. We choose the first x_n, x0, to equal
        // 2^7 because all larger powers are larger than MAX_NATURAL_EXPONENT, and
        // therefore not present in the decomposition.
        // At the end of this process we will have the product of all e^x_n = a_n
        // that apply, and the remainder of this decomposition, which will be lower
        // than the smallest x_n.
        // exp(x) = k_0 * a_0 * k_1 * a_1 * ... + k_n * a_n * exp(remainder),
        // where each k_n equals either 0 or 1.
        // We mutate x by subtracting x_n, making it the remainder of the
        // decomposition.

        // The first two a_n (e^(2^7) and e^(2^6)) are too large if stored as 18
        // decimal numbers, and could cause intermediate overflows. Instead we store
        // them as plain integers, with 0 decimals.
        // Additionally, x0 + x1 is larger than MAX_NATURAL_EXPONENT, which means
        // they will not both be present in the decomposition.

        // For each x_n, we test if that term is present in the decomposition (if x
        // is larger than it), and if so deduct it and compute the accumulated
        // product.
        let mut x = x.clone();
        let x0_val = i(env, X0);
        let x1_val = i(env, X1);

        let first_an;
        if x >= x0_val {
            x = x.sub(&x0_val);
            first_an = a0(env);
        } else if x >= x1_val {
            x = x.sub(&x1_val);
            first_an = i(env, A1);
        } else {
            first_an = i(env, 1); // One with no decimal places
        }

        // We now transform x into a 20 decimal fixed point number, to have
        // enhanced precision when computing the smaller terms.
        let hundred = i(env, 100);
        x = x.mul(&hundred);

        // `product` is the accumulated product of all a_n (except a0 and a1),
        // which starts at 20 decimal fixed point one. Recall that fixed point
        // multiplication requires dividing by ONE_20.
        let one_20 = i(env, ONE_20);
        let mut product = one_20.clone();

        let x2_val = i(env, X2);
        let x3_val = i(env, X3);
        let x4_val = i(env, X4);
        let x5_val = i(env, X5);
        let x6_val = i(env, X6);
        let x7_val = i(env, X7);
        let x8_val = i(env, X8);
        let x9_val = i(env, X9);

        if x >= x2_val {
            x = x.sub(&x2_val);
            product = product.mul(&i(env, A2)).div(&one_20);
        }
        if x >= x3_val {
            x = x.sub(&x3_val);
            product = product.mul(&i(env, A3)).div(&one_20);
        }
        if x >= x4_val {
            x = x.sub(&x4_val);
            product = product.mul(&i(env, A4)).div(&one_20);
        }
        if x >= x5_val {
            x = x.sub(&x5_val);
            product = product.mul(&i(env, A5)).div(&one_20);
        }
        if x >= x6_val {
            x = x.sub(&x6_val);
            product = product.mul(&i(env, A6)).div(&one_20);
        }
        if x >= x7_val {
            x = x.sub(&x7_val);
            product = product.mul(&i(env, A7)).div(&one_20);
        }
        if x >= x8_val {
            x = x.sub(&x8_val);
            product = product.mul(&i(env, A8)).div(&one_20);
        }
        if x >= x9_val {
            x = x.sub(&x9_val);
            product = product.mul(&i(env, A9)).div(&one_20);
        }

        // x10 and x11 are unnecessary here since we have high enough precision
        // already.

        // Now we need to compute e^x, where x is small (in particular, it is
        // smaller than x9). We use the Taylor series expansion for e^x:
        // 1 + x + (x^2 / 2!) + (x^3 / 3!) + ... + (x^n / n!).
        let mut series_sum = one_20.clone(); // The initial one in the sum, with 20 decimal places.
        let mut term; // Each term in the sum, where the nth term is (x^n / n!).

        // The first term is simply x.
        term = x.clone();
        series_sum = series_sum.add(&term);

        // Each term (x^n / n!) equals the previous one times x, divided by n.
        // Since x is a fixed point number, multiplying by it requires dividing by
        // ONE_20, but dividing by the non-fixed point n values does not.

        term = term.mul(&x).div(&one_20).div(&i(env, 2));
        series_sum = series_sum.add(&term);

        term = term.mul(&x).div(&one_20).div(&i(env, 3));
        series_sum = series_sum.add(&term);

        term = term.mul(&x).div(&one_20).div(&i(env, 4));
        series_sum = series_sum.add(&term);

        term = term.mul(&x).div(&one_20).div(&i(env, 5));
        series_sum = series_sum.add(&term);

        term = term.mul(&x).div(&one_20).div(&i(env, 6));
        series_sum = series_sum.add(&term);

        term = term.mul(&x).div(&one_20).div(&i(env, 7));
        series_sum = series_sum.add(&term);

        term = term.mul(&x).div(&one_20).div(&i(env, 8));
        series_sum = series_sum.add(&term);

        term = term.mul(&x).div(&one_20).div(&i(env, 9));
        series_sum = series_sum.add(&term);

        term = term.mul(&x).div(&one_20).div(&i(env, 10));
        series_sum = series_sum.add(&term);

        term = term.mul(&x).div(&one_20).div(&i(env, 11));
        series_sum = series_sum.add(&term);

        term = term.mul(&x).div(&one_20).div(&i(env, 12));
        series_sum = series_sum.add(&term);

        // 12 Taylor terms are sufficient for 18 decimal precision.

        // We now have the first a_n (with no decimals), and the product of all
        // other a_n present, and the Taylor approximation of the exponentiation of
        // the remainder (both with 20 decimals). All that remains is to multiply
        // all three (one 20 decimal fixed point multiplication, dividing by ONE_20,
        // and one integer multiplication), and then drop two digits to return an 18
        // decimal value.

        // result = ((product * seriesSum) / ONE_20) * firstAN / 100
        product
            .mul(&series_sum)
            .div(&one_20)
            .mul(&first_an)
            .div(&hundred)
    }

    /// Natural logarithm (ln(a)) with signed 18-decimal fixed-point argument.
    ///
    /// The real natural logarithm is not defined for negative numbers or zero.
    /// Panics if `a <= 0`.
    pub fn ln(env: &Env, a: &I256) -> I256 {
        let zero = i(env, 0);
        assert!(*a > zero, "out of bounds");

        let lower = i(env, LN_36_LOWER_BOUND);
        let upper = i(env, LN_36_UPPER_BOUND);
        let one_18 = i(env, ONE_18);

        if *a > lower && *a < upper {
            ln_36(env, a).div(&one_18)
        } else {
            ln_internal(env, a)
        }
    }

    /// Exponentiation (x^y) with unsigned 18-decimal fixed-point base and exponent.
    ///
    /// Reverts if `ln(x) * y` is smaller than `MIN_NATURAL_EXPONENT`, or larger
    /// than `MAX_NATURAL_EXPONENT`.
    ///
    /// Instead of computing x^y directly, we rely on the properties of logarithms
    /// and exponentiation: exp(ln(x)) = x, and ln(x^y) = y * ln(x). This means
    /// x^y = exp(y * ln(x)).
    pub fn pow(env: &Env, x: &U256, y: &U256) -> U256 {
        let zero_u = u(env, 0);
        let one_18_u = u(env, ONE_18 as u128);

        if *y == zero_u {
            // We solve the 0^0 indetermination by making it equal one.
            return one_18_u;
        }
        if *x == zero_u {
            return zero_u;
        }

        // The ln function takes a signed value, so we need to make sure x fits in
        // the signed 256 bit range.
        let two_255 = U256::from_parts(env, 0x8000000000000000, 0, 0, 0);
        assert!(*x < two_255, "x out of bounds");
        let x_int256 = u256_to_i256(env, x);

        // We will compute y * ln(x) in a single step. Depending on the value of x,
        // we can either use ln or ln_36. In both cases, we leave the division by
        // ONE_18 (due to fixed point multiplication) to the end.

        // This prevents y * ln(x) from overflowing, and at the same time guarantees
        // y fits in the signed 256 bit range.
        assert!(*y < mild_exponent_bound(env), "y out of bounds");
        let y_int256 = u256_to_i256(env, y);

        let lower = i(env, LN_36_LOWER_BOUND);
        let upper = i(env, LN_36_UPPER_BOUND);
        let one_18 = i(env, ONE_18);

        let logx_times_y = if x_int256 > lower && x_int256 < upper {
            let ln_36_x = ln_36(env, &x_int256);

            // ln_36_x has 36 decimal places, so multiplying by y_int256 isn't as
            // straightforward, since we can't just bring y_int256 to 36 decimal
            // places, as it might overflow. Instead, we perform two 18 decimal
            // multiplications and add the results: one with the first 18 decimals
            // of ln_36_x, and one with the (downscaled) last 18 decimals.
            let hi = ln_36_x.div(&one_18).mul(&y_int256);
            let lo = rem_trunc(&ln_36_x, &one_18).mul(&y_int256).div(&one_18);
            hi.add(&lo)
        } else {
            ln_internal(env, &x_int256).mul(&y_int256)
        };

        let logx_times_y = logx_times_y.div(&one_18);

        // Finally, we compute exp(y * ln(x)) to arrive at x^y.
        let min_exp = i(env, MIN_NATURAL_EXPONENT);
        let max_exp = i(env, MAX_NATURAL_EXPONENT);
        assert!(
            logx_times_y >= min_exp && logx_times_y <= max_exp,
            "product out of bounds"
        );

        i256_to_u256(env, &exp(env, &logx_times_y))
    }

    // -----------------------------------------------------------------------
    // Private helpers
    // -----------------------------------------------------------------------

    /// Truncated (C-style / Solidity-style) remainder: sign follows the dividend.
    /// Solidity's `%` operator truncates toward zero, unlike Rust's `rem_euclid`.
    #[inline(always)]
    fn rem_trunc(a: &I256, b: &I256) -> I256 {
        // a - (a / b) * b
        a.sub(&a.div(b).mul(b))
    }

    /// Internal natural logarithm (ln(a)) with signed 18-decimal fixed-point
    /// argument.
    ///
    /// Mirrors `LogExpMath._ln(int256 a)` exactly.
    fn ln_internal(env: &Env, a: &I256) -> I256 {
        let one_18 = i(env, ONE_18);

        if *a < one_18 {
            // Since ln(a^k) = k * ln(a), we can compute ln(a) as
            // ln(a) = ln((1/a)^(-1)) = -ln(1/a).
            // If a is less than one, 1/a will be greater than one, and this branch
            // will not be entered in the recursive call.
            // Fixed point division requires multiplying by ONE_18.
            let inv = one_18.mul(&one_18).div(a);
            let zero = i(env, 0);
            return zero.sub(&ln_internal(env, &inv));
        }

        // First, we use the fact that ln(a * b) = ln(a) + ln(b) to decompose ln(a)
        // into a sum of powers of two, which we call x_n, where x_n == 2^(7 - n),
        // which are the natural logarithm of precomputed quantities a_n (that is,
        // ln(a_n) = x_n). We choose the first x_n, x0, to equal 2^7 because the
        // exponential of all larger powers cannot be represented as 18 fixed point
        // decimal numbers in 256 bits, and are therefore larger than a.
        // At the end of this process we will have the sum of all x_n = ln(a_n) that
        // apply, and the remainder of this decomposition, which will be lower than
        // the smallest a_n.
        // ln(a) = k_0 * x_0 + k_1 * x_1 + ... + k_n * x_n + ln(remainder),
        // where each k_n equals either 0 or 1.
        // We mutate a by subtracting a_n, making it the remainder of the
        // decomposition.

        // For reasons related to how `exp` works, the first two a_n (e^(2^7) and
        // e^(2^6)) are not stored as fixed point numbers with 18 decimals, but
        // instead as plain integers with 0 decimals, so we need to multiply them by
        // ONE_18 to convert them to fixed point.
        // For each a_n, we test if that term is present in the decomposition (if a
        // is larger than it), and if so divide by it and compute the accumulated sum.

        let mut a = a.clone();
        let mut sum = i(env, 0);

        let a0_val = a0(env);
        let a0_threshold = a0_val.mul(&one_18);
        if a >= a0_threshold {
            a = a.div(&a0_val); // Integer, not fixed point division
            sum = sum.add(&i(env, X0));
        }

        let a1_val = i(env, A1);
        let a1_threshold = a1_val.mul(&one_18);
        if a >= a1_threshold {
            a = a.div(&a1_val); // Integer, not fixed point division
            sum = sum.add(&i(env, X1));
        }

        // All other a_n and x_n are stored as 20 digit fixed point numbers, so we
        // convert the sum and a to this format.
        let hundred = i(env, 100);
        sum = sum.mul(&hundred);
        a = a.mul(&hundred);

        // Because further a_n are 20 digit fixed point numbers, we multiply by
        // ONE_20 when dividing by them.
        let one_20 = i(env, ONE_20);

        let a2_val = i(env, A2);
        if a >= a2_val {
            a = a.mul(&one_20).div(&a2_val);
            sum = sum.add(&i(env, X2));
        }

        let a3_val = i(env, A3);
        if a >= a3_val {
            a = a.mul(&one_20).div(&a3_val);
            sum = sum.add(&i(env, X3));
        }

        let a4_val = i(env, A4);
        if a >= a4_val {
            a = a.mul(&one_20).div(&a4_val);
            sum = sum.add(&i(env, X4));
        }

        let a5_val = i(env, A5);
        if a >= a5_val {
            a = a.mul(&one_20).div(&a5_val);
            sum = sum.add(&i(env, X5));
        }

        let a6_val = i(env, A6);
        if a >= a6_val {
            a = a.mul(&one_20).div(&a6_val);
            sum = sum.add(&i(env, X6));
        }

        let a7_val = i(env, A7);
        if a >= a7_val {
            a = a.mul(&one_20).div(&a7_val);
            sum = sum.add(&i(env, X7));
        }

        let a8_val = i(env, A8);
        if a >= a8_val {
            a = a.mul(&one_20).div(&a8_val);
            sum = sum.add(&i(env, X8));
        }

        let a9_val = i(env, A9);
        if a >= a9_val {
            a = a.mul(&one_20).div(&a9_val);
            sum = sum.add(&i(env, X9));
        }

        let a10_val = i(env, A10);
        if a >= a10_val {
            a = a.mul(&one_20).div(&a10_val);
            sum = sum.add(&i(env, X10));
        }

        let a11_val = i(env, A11);
        if a >= a11_val {
            a = a.mul(&one_20).div(&a11_val);
            sum = sum.add(&i(env, X11));
        }

        // a is now a small number (smaller than a_11, which roughly equals 1.06).
        // This means we can use a Taylor series that converges rapidly for values of
        // `a` close to one - the same one used in ln_36.
        // Let z = (a - 1) / (a + 1).
        // ln(a) = 2 * (z + z^3 / 3 + z^5 / 5 + z^7 / 7 + ... + z^(2*n+1) / (2*n+1))

        // Recall that 20 digit fixed point division requires multiplying by ONE_20,
        // and multiplication requires division by ONE_20.
        let z = a.sub(&one_20).mul(&one_20).div(&a.add(&one_20));
        let z_squared = z.mul(&z).div(&one_20);

        // num is the numerator of the series: the z^(2*n+1) term.
        let mut num = z.clone();

        // series_sum holds the accumulated sum of each term in the series,
        // starting with the initial z.
        let mut series_sum = num.clone();

        // In each step, the numerator is multiplied by z^2.
        num = num.mul(&z_squared).div(&one_20);
        series_sum = series_sum.add(&num.div(&i(env, 3)));

        num = num.mul(&z_squared).div(&one_20);
        series_sum = series_sum.add(&num.div(&i(env, 5)));

        num = num.mul(&z_squared).div(&one_20);
        series_sum = series_sum.add(&num.div(&i(env, 7)));

        num = num.mul(&z_squared).div(&one_20);
        series_sum = series_sum.add(&num.div(&i(env, 9)));

        num = num.mul(&z_squared).div(&one_20);
        series_sum = series_sum.add(&num.div(&i(env, 11)));

        // 6 Taylor terms are sufficient for 36 decimal precision.

        // Finally, we multiply by 2 (non fixed point) to compute ln(remainder).
        series_sum = series_sum.mul(&i(env, 2));

        // We now have the sum of all x_n present, and the Taylor approximation of
        // the logarithm of the remainder (both with 20 decimals). All that remains
        // is to sum these two, and then drop two digits to return an 18 decimal
        // value.

        // (sum + seriesSum) / 100
        sum.add(&series_sum).div(&hundred)
    }

    /// Internal high-precision (36 decimal places) natural logarithm (ln(x))
    /// with signed 18-decimal fixed-point argument, for x close to one.
    ///
    /// Since ln(1) = 0, a value of x close to one will yield a very small result,
    /// which makes using 36 digits worthwhile.
    ///
    /// Should only be used if x is between LN_36_LOWER_BOUND and LN_36_UPPER_BOUND.
    /// Returns a 36-decimal fixed-point I256.
    fn ln_36(env: &Env, x: &I256) -> I256 {
        let one_18 = i(env, ONE_18);
        let one_36 = i(env, ONE_36);

        // First, we transform x to a 36 digit fixed point value.
        let x = x.mul(&one_18);

        // We will use the following Taylor expansion, which converges very rapidly.
        // Let z = (x - 1) / (x + 1).
        // ln(x) = 2 * (z + z^3 / 3 + z^5 / 5 + z^7 / 7 + ... + z^(2*n+1) / (2*n+1))

        // Recall that 36 digit fixed point division requires multiplying by ONE_36,
        // and multiplication requires division by ONE_36.
        let z = x.sub(&one_36).mul(&one_36).div(&x.add(&one_36));
        let z_squared = z.mul(&z).div(&one_36);

        // num is the numerator of the series: the z^(2*n+1) term.
        let mut num = z.clone();

        // series_sum holds the accumulated sum of each term in the series,
        // starting with the initial z.
        let mut series_sum = num.clone();

        // In each step, the numerator is multiplied by z^2.
        num = num.mul(&z_squared).div(&one_36);
        series_sum = series_sum.add(&num.div(&i(env, 3)));

        num = num.mul(&z_squared).div(&one_36);
        series_sum = series_sum.add(&num.div(&i(env, 5)));

        num = num.mul(&z_squared).div(&one_36);
        series_sum = series_sum.add(&num.div(&i(env, 7)));

        num = num.mul(&z_squared).div(&one_36);
        series_sum = series_sum.add(&num.div(&i(env, 9)));

        num = num.mul(&z_squared).div(&one_36);
        series_sum = series_sum.add(&num.div(&i(env, 11)));

        num = num.mul(&z_squared).div(&one_36);
        series_sum = series_sum.add(&num.div(&i(env, 13)));

        num = num.mul(&z_squared).div(&one_36);
        series_sum = series_sum.add(&num.div(&i(env, 15)));

        // 8 Taylor terms are sufficient for 36 decimal precision.

        // All that remains is multiplying by 2 (non fixed point).
        series_sum.mul(&i(env, 2))
    }

    // -----------------------------------------------------------------------
    // Tests
    // -----------------------------------------------------------------------

    #[cfg(test)]
    #[allow(
        clippy::unwrap_used,
        clippy::arithmetic_side_effects,
        clippy::unnecessary_cast
    )]
    mod tests {
        use super::*;
        use soroban_sdk::Env;

        /// Approximate equality check for unsigned results.
        fn assert_approx(result: i128, expected: i128, rel_tol_denom: i128, label: &str) {
            let diff = (result - expected).abs();
            assert!(
                diff <= expected.abs() / rel_tol_denom,
                "{}: got {}, expected {}, diff {}",
                label,
                result,
                expected,
                diff
            );
        }

        // --- exp tests ---

        #[test]
        fn test_exp_zero() {
            let env = Env::default();
            let result = exp(&env, &i(&env, 0));
            assert_eq!(result, i(&env, ONE_18));
        }

        #[test]
        fn test_exp_one() {
            let env = Env::default();
            // e^1 ≈ 2.71828... * 1e18
            // a7 / 100 = 271828182845904523536 / 100 = 2718281828459045235
            let expected = A7 / 100;
            let result = exp(&env, &i(&env, ONE_18));
            assert_approx(result.to_i128().unwrap(), expected, 1_000_000, "exp(1)");
        }

        #[test]
        fn test_exp_half() {
            let env = Env::default();
            // e^0.5 ≈ 1.64872... * 1e18
            // a8 / 100 = 164872127070012814685 / 100 = 1648721270700128146
            let expected = A8 / 100;
            let result = exp(&env, &i(&env, ONE_18 / 2));
            assert_approx(result.to_i128().unwrap(), expected, 1_000_000, "exp(0.5)");
        }

        #[test]
        fn test_exp_small() {
            let env = Env::default();
            // e^0.1 ≈ 1.10517... * 1e18
            let expected: i128 = 1_105_170_918_075_647_624;
            let result = exp(&env, &i(&env, ONE_18 / 10));
            assert_approx(result.to_i128().unwrap(), expected, 1_000_000, "exp(0.1)");
        }

        #[test]
        fn test_exp_two() {
            let env = Env::default();
            // e^2 ≈ 7.38905... * 1e18
            // a6 / 100 = 738905609893065022723 / 100 = 7389056098930650227
            let expected = A6 / 100;
            let result = exp(&env, &i(&env, 2 * ONE_18));
            assert_approx(result.to_i128().unwrap(), expected, 1_000_000, "exp(2)");
        }

        #[test]
        fn test_exp_near_zero_positive() {
            let env = Env::default();
            // e^0.05 ≈ 1.05127... * 1e18
            let expected: i128 = 1_051_271_096_376_024_124;
            let result = exp(&env, &i(&env, ONE_18 / 20));
            assert_approx(result.to_i128().unwrap(), expected, 1_000_000, "exp(0.05)");
        }

        #[test]
        fn test_exp_large() {
            let env = Env::default();
            // e^10 ≈ 22026.46... * 1e18
            let expected: i128 = 22_026_465_794_806_716_516_957;
            let result = exp(&env, &i(&env, 10 * ONE_18));
            assert_approx(result.to_i128().unwrap(), expected, 1_000_000, "exp(10)");
        }

        #[test]
        fn test_exp_negative() {
            let env = Env::default();
            // e^(-1) = 1/e ≈ 0.36787944... * 1e18
            let expected: i128 = 367_879_441_171_442_321;
            let result = exp(&env, &i(&env, -ONE_18));
            assert_approx(result.to_i128().unwrap(), expected, 1_000_000, "exp(-1)");
        }

        // --- ln tests ---

        #[test]
        fn test_ln_one() {
            let env = Env::default();
            let result = ln(&env, &i(&env, ONE_18));
            assert_eq!(result, i(&env, 0));
        }

        #[test]
        fn test_ln_e() {
            let env = Env::default();
            // ln(e) = 1. We use a7/100 as our best e approximation.
            let e_approx = A7 / 100; // 2718281828459045235
            let result = ln(&env, &i(&env, e_approx));
            assert_approx(result.to_i128().unwrap(), ONE_18, 1_000_000, "ln(e)");
        }

        #[test]
        fn test_ln_2() {
            let env = Env::default();
            let expected: i128 = 693_147_180_559_945_309;
            let result = ln(&env, &i(&env, 2 * ONE_18));
            assert_approx(result.to_i128().unwrap(), expected, 1_000_000, "ln(2)");
        }

        #[test]
        fn test_ln_close_to_one_above() {
            let env = Env::default();
            // ln(1.05)
            let expected: i128 = 48_790_164_169_432_058;
            let result = ln(&env, &i(&env, ONE_18 + ONE_18 / 20));
            assert_approx(result.to_i128().unwrap(), expected, 1_000_000, "ln(1.05)");
        }

        #[test]
        fn test_ln_close_to_one_below() {
            let env = Env::default();
            // ln(0.95)
            let expected: i128 = -51_293_294_387_550_584;
            let result = ln(&env, &i(&env, ONE_18 - ONE_18 / 20));
            let diff = (result.to_i128().unwrap() - expected).abs();
            assert!(
                diff <= expected.abs() / 1_000_000,
                "ln(0.95): got {}, expected {}, diff {}",
                result.to_i128().unwrap(),
                expected,
                diff
            );
        }

        #[test]
        fn test_ln_less_than_one() {
            let env = Env::default();
            // ln(0.5) ≈ -0.693147... * 1e18
            let expected: i128 = -693_147_180_559_945_309;
            let result = ln(&env, &i(&env, ONE_18 / 2));
            let diff = (result.to_i128().unwrap() - expected).abs();
            assert!(
                diff <= expected.abs() / 1_000_000,
                "ln(0.5): got {}, expected {}, diff {}",
                result.to_i128().unwrap(),
                expected,
                diff
            );
        }

        #[test]
        fn test_ln_exp_roundtrip() {
            let env = Env::default();
            for &x_raw in &[
                ONE_18 / 10,
                ONE_18 / 2,
                ONE_18,
                2 * ONE_18,
                ONE_18 * 69 / 100,
            ] {
                let x_val = i(&env, x_raw);
                let exp_result = exp(&env, &x_val);
                let result = ln(&env, &exp_result).to_i128().unwrap();
                let diff = (result - x_raw).abs();
                assert!(
                    diff <= x_raw.abs() / 1_000_000,
                    "ln(exp({})): got {}, diff {}",
                    x_raw,
                    result,
                    diff
                );
            }
        }

        // --- pow tests ---

        #[test]
        fn test_pow_zero_exponent() {
            let env = Env::default();
            assert_eq!(
                pow(&env, &u(&env, 2 * ONE_18 as u128), &u(&env, 0)),
                u(&env, ONE_18 as u128)
            );
            assert_eq!(pow(&env, &u(&env, 0), &u(&env, 0)), u(&env, ONE_18 as u128));
        }

        #[test]
        fn test_pow_zero_base() {
            let env = Env::default();
            assert_eq!(pow(&env, &u(&env, 0), &u(&env, ONE_18 as u128)), u(&env, 0));
        }

        #[test]
        fn test_pow_one_exponent() {
            let env = Env::default();
            let x: u128 = 15 * ONE_18 as u128 / 10;
            let result = pow(&env, &u(&env, x), &u(&env, ONE_18 as u128));
            assert_approx(
                result.to_u128().unwrap() as i128,
                x as i128,
                1_000_000,
                "pow(1.5, 1)",
            );
        }

        #[test]
        fn test_pow_half_exponent() {
            let env = Env::default();
            // sqrt(4) = 2
            let result = pow(
                &env,
                &u(&env, 4 * ONE_18 as u128),
                &u(&env, ONE_18 as u128 / 2),
            );
            assert_approx(
                result.to_u128().unwrap() as i128,
                2 * ONE_18 as i128,
                1_000_000,
                "pow(4, 0.5)",
            );
        }

        #[test]
        fn test_pow_zcb_typical() {
            let env = Env::default();
            // pow(1.1, 1) ≈ 1.1
            let base: u128 = ONE_18 as u128 + ONE_18 as u128 / 10;
            let result = pow(&env, &u(&env, base), &u(&env, ONE_18 as u128));
            assert_approx(
                result.to_u128().unwrap() as i128,
                base as i128,
                1_000_000,
                "pow(1.1, 1)",
            );
        }

        #[test]
        fn test_pow_zcb_half_year() {
            let env = Env::default();
            // pow(1.1, 0.5) ≈ 1.04880884817...
            let base: u128 = ONE_18 as u128 + ONE_18 as u128 / 10;
            let exponent: u128 = ONE_18 as u128 / 2;
            let expected: i128 = 1_048_808_848_170_151_551;
            let result = pow(&env, &u(&env, base), &u(&env, exponent));
            let result_i128 = result.to_u128().unwrap() as i128;
            let diff = (result_i128 - expected).abs();
            assert!(
                diff <= expected / 10_000,
                "pow(1.1, 0.5): got {}, expected ~{}, diff {}",
                result_i128,
                expected,
                diff
            );
        }

        #[test]
        fn test_zcb_scenario_exp_ln() {
            let env = Env::default();
            let base = ONE_18 + ONE_18 / 10;
            let base_i = i(&env, base);
            let result = exp(&env, &ln(&env, &base_i));
            assert_approx(result.to_i128().unwrap(), base, 1_000_000, "exp(ln(1.1))");
        }
    }
}
