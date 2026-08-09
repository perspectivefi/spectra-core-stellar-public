// SPDX-License-Identifier: BUSL-1.1
/// Cross-validation: compare Rust LogExpMath output against Solidity-generated vectors.
#[cfg(test)]
mod tests {
    extern crate std;

    use crate::log_exp_math::log_exp_math;
    use serde::Deserialize;
    use soroban_sdk::{Env, I256, U256};
    use std::string::String;
    use std::vec::Vec;

    #[derive(Deserialize, Debug)]
    struct TestVector {
        #[serde(rename = "fn")]
        function: String,
        label: String,
        #[serde(default)]
        input: Option<String>,
        #[serde(default)]
        input_x: Option<String>,
        #[serde(default)]
        input_y: Option<String>,
        expected: String,
    }

    fn i256_from_str(env: &Env, s: &str) -> I256 {
        let negative = s.starts_with('-');
        let digits = if negative { &s[1..] } else { s };

        if digits.len() <= 38 {
            let val: i128 = digits.parse().expect("value doesn't fit i128");
            let result = I256::from_i128(env, val);
            if negative {
                I256::from_i128(env, 0).sub(&result)
            } else {
                result
            }
        } else {
            let ten = I256::from_i128(env, 10);
            let mut result = I256::from_i128(env, 0);
            for byte in digits.bytes() {
                let digit = (byte - b'0') as i128;
                result = result.mul(&ten).add(&I256::from_i128(env, digit));
            }
            if negative {
                I256::from_i128(env, 0).sub(&result)
            } else {
                result
            }
        }
    }

    fn u256_from_str(env: &Env, s: &str) -> U256 {
        if s.len() <= 38 {
            let val: u128 = s.parse().expect("value doesn't fit u128");
            U256::from_u128(env, val)
        } else {
            let ten = U256::from_u128(env, 10);
            let mut result = U256::from_u128(env, 0);
            for byte in s.bytes() {
                let digit = (byte - b'0') as u128;
                result = result.mul(&ten).add(&U256::from_u128(env, digit));
            }
            result
        }
    }

    fn i256_display(env: &Env, v: &I256) -> String {
        let zero = I256::from_i128(env, 0);
        if *v < zero {
            let abs = zero.sub(v);
            if let Some(val) = abs.to_i128() {
                std::format!("-{}", val)
            } else {
                std::string::String::from("<large negative>")
            }
        } else if let Some(val) = v.to_i128() {
            std::format!("{}", val)
        } else {
            std::string::String::from("<large positive>")
        }
    }

    fn u256_display(v: &U256) -> String {
        if let Some(val) = v.to_u128() {
            std::format!("{}", val)
        } else {
            std::string::String::from("<large>")
        }
    }

    #[test]
    fn test_cross_validate() {
        let json_str = include_str!("../test-vectors/cross_validation.json");
        let vectors: Vec<TestVector> =
            serde_json::from_str(json_str).expect("failed to parse vectors");

        let env = Env::default();
        let mut passed = 0;
        let mut failed = 0;

        for v in &vectors {
            match v.function.as_str() {
                "exp" => {
                    let input = v.input.as_ref().expect("exp missing input");
                    let x = i256_from_str(&env, input);
                    let expected = i256_from_str(&env, &v.expected);
                    let result = log_exp_math::exp(&env, &x);
                    if result != expected {
                        std::eprintln!(
                            "FAIL [{}]: exp({}) = {} (Rust) vs {} (Solidity)",
                            v.label,
                            input,
                            i256_display(&env, &result),
                            v.expected
                        );
                        failed += 1;
                    } else {
                        passed += 1;
                    }
                }
                "ln" => {
                    let input = v.input.as_ref().expect("ln missing input");
                    let a = i256_from_str(&env, input);
                    let expected = i256_from_str(&env, &v.expected);
                    let result = log_exp_math::ln(&env, &a);
                    if result != expected {
                        std::eprintln!(
                            "FAIL [{}]: ln({}) = {} (Rust) vs {} (Solidity)",
                            v.label,
                            input,
                            i256_display(&env, &result),
                            v.expected
                        );
                        failed += 1;
                    } else {
                        passed += 1;
                    }
                }
                "pow" => {
                    let x_str = v.input_x.as_ref().expect("pow missing input_x");
                    let y_str = v.input_y.as_ref().expect("pow missing input_y");
                    let x = u256_from_str(&env, x_str);
                    let y = u256_from_str(&env, y_str);
                    let expected = u256_from_str(&env, &v.expected);
                    let result = log_exp_math::pow(&env, &x, &y);
                    if result != expected {
                        std::eprintln!(
                            "FAIL [{}]: pow({}, {}) = {} (Rust) vs {} (Solidity)",
                            v.label,
                            x_str,
                            y_str,
                            u256_display(&result),
                            v.expected
                        );
                        failed += 1;
                    } else {
                        passed += 1;
                    }
                }
                other => panic!("Unknown function: {}", other),
            }
        }

        std::eprintln!(
            "\nCross-validation: {} passed, {} failed, {} total",
            passed,
            failed,
            vectors.len()
        );
        assert_eq!(failed, 0, "{} vectors failed cross-validation", failed);
    }
}
