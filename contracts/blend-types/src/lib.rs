// SPDX-License-Identifier: MIT
#![no_std]

//! Vendored Blend v2 pool interface (`blend-contracts-v2`, soroban-sdk 22 —
//! cannot be linked into this workspace; types mirrored). Field names must
//! match the upstream `#[contracttype]` XDR exactly. Re-vendor on upstream change.
//!
//! Dedicated crate so both `blend-fungible-vault-wrapper` (the consumer) and
//! `spectra-mocks::mock_blend_pool` (the test mock) can depend on the same
//! types without creating a Cargo dependency cycle.

use soroban_sdk::unwrap::UnwrapOptimized;
use soroban_sdk::{Address, Env, I256, Map, Vec, contractclient, contracttype};

/// 12-decimal fixed-point scale of `b_rate`/`d_rate`.
pub const SCALAR_12: i128 = 1_000_000_000_000;

/// `RequestType` values used by the wrapper (pool/src/pool/actions.rs).
pub const REQUEST_SUPPLY: u32 = 0;
pub const REQUEST_WITHDRAW: u32 = 1;

/// Emission `reserve_token_id` for a reserve's bToken (dToken is `index * 2`).
pub fn b_token_emission_id(reserve_index: u32) -> u32 {
    reserve_index * 2 + 1
}

// ── bToken conversions, bit-identical to the pool's (pool/src/pool/reserve.rs) ──
// Math vendored from soroban-fixed-point-math 1.3.0 (MIT, Script3): i128 fast
// path, I256 retry on phantom overflow.

/// bTokens minted when supplying `amount`: `floor(amount * 1e12 / b_rate)`.
pub fn to_b_token_down(env: &Env, amount: i128, b_rate: i128) -> i128 {
    scaled_mul_div_floor(env, amount, SCALAR_12, b_rate)
}

/// bTokens burned when withdrawing `amount`: `ceil(amount * 1e12 / b_rate)`.
pub fn to_b_token_up(env: &Env, amount: i128, b_rate: i128) -> i128 {
    scaled_mul_div_ceil(env, amount, SCALAR_12, b_rate)
}

/// Underlying value of `b_tokens`: `floor(b_tokens * b_rate / 1e12)`.
pub fn to_asset_from_b_token(env: &Env, b_tokens: i128, b_rate: i128) -> i128 {
    scaled_mul_div_floor(env, b_tokens, b_rate, SCALAR_12)
}

/// Underlying value of `b_tokens`, rounded up: `ceil(b_tokens * b_rate / 1e12)`.
pub fn to_asset_from_b_token_up(env: &Env, b_tokens: i128, b_rate: i128) -> i128 {
    scaled_mul_div_ceil(env, b_tokens, b_rate, SCALAR_12)
}

/// Performs floor(x * y / z)
fn scaled_mul_div_floor(env: &Env, x: i128, y: i128, z: i128) -> i128 {
    match x.checked_mul(y) {
        Some(r) => div_floor(r, z).unwrap_optimized(),
        None => {
            // scale to i256 and retry
            let res = i256_mul_div_floor(
                env,
                &I256::from_i128(env, x),
                &I256::from_i128(env, y),
                &I256::from_i128(env, z),
            );
            // will panic if result is not representable in i128
            res.to_i128().unwrap_optimized()
        }
    }
}

/// Performs ceil(x * y / z)
fn scaled_mul_div_ceil(env: &Env, x: i128, y: i128, z: i128) -> i128 {
    match x.checked_mul(y) {
        Some(r) => div_ceil(r, z).unwrap_optimized(),
        None => {
            // scale to i256 and retry
            let res = i256_mul_div_ceil(
                env,
                &I256::from_i128(env, x),
                &I256::from_i128(env, y),
                &I256::from_i128(env, z),
            );
            // will panic if result is not representable in i128
            res.to_i128().unwrap_optimized()
        }
    }
}

/// Performs floor(r / z)
fn div_floor(r: i128, z: i128) -> Option<i128> {
    if r < 0 || (r > 0 && z < 0) {
        // ceiling is taken by default for a negative result
        let remainder = r.checked_rem_euclid(z)?;
        (r / z).checked_sub(if remainder > 0 { 1 } else { 0 })
    } else {
        // floor taken by default for a positive or zero result
        r.checked_div(z)
    }
}

/// Performs ceil(r / z)
#[allow(clippy::nonminimal_bool)] // verbatim upstream
fn div_ceil(r: i128, z: i128) -> Option<i128> {
    if r <= 0 || (r > 0 && z < 0) {
        // ceiling is taken by default for a negative or zero result
        r.checked_div(z)
    } else {
        // floor taken by default for a positive result
        let remainder = r.checked_rem_euclid(z)?;
        (r / z).checked_add(if remainder > 0 { 1 } else { 0 })
    }
}

/// Performs floor(x * y / z)
fn i256_mul_div_floor(env: &Env, x: &I256, y: &I256, z: &I256) -> I256 {
    let zero = I256::from_i32(env, 0);
    let r = x.mul(y);
    if r < zero || (r > zero && z.clone() < zero) {
        // ceiling is taken by default for a negative result
        let remainder = r.rem_euclid(z);
        let one = I256::from_i32(env, 1);
        r.div(z).sub(if remainder > zero { &one } else { &zero })
    } else {
        // floor taken by default for a positive or zero result
        r.div(z)
    }
}

/// Performs ceil(x * y / z)
#[allow(clippy::nonminimal_bool)] // verbatim upstream
fn i256_mul_div_ceil(env: &Env, x: &I256, y: &I256, z: &I256) -> I256 {
    let zero = I256::from_i32(env, 0);
    let r = x.mul(y);
    if r <= zero || (r > zero && z.clone() < zero) {
        // ceiling is taken by default for a negative or zero result
        r.div(z)
    } else {
        // floor taken by default for a positive result
        let remainder = r.rem_euclid(z);
        let one = I256::from_i32(env, 1);
        r.div(z).add(if remainder > zero { &one } else { &zero })
    }
}

/// A request a user makes against the pool.
#[derive(Clone, Debug)]
#[contracttype]
pub struct Request {
    pub request_type: u32,
    pub address: Address, // asset address or liquidatee
    pub amount: i128,
}

/// A user's positions with the pool, keyed by reserve index.
#[derive(Clone, Debug)]
#[contracttype]
pub struct Positions {
    pub liabilities: Map<u32, i128>, // dToken balances
    pub collateral: Map<u32, i128>,  // collateralized bToken balances
    pub supply: Map<u32, i128>,      // non-collateral bToken balances
}

/// The configuration of a reserve asset.
#[derive(Clone, Debug)]
#[contracttype]
pub struct ReserveConfig {
    pub index: u32,
    pub decimals: u32, // decimals of both the bToken and the underlying
    pub c_factor: u32,
    pub l_factor: u32,
    pub util: u32,
    pub max_util: u32,
    pub r_base: u32,
    pub r_one: u32,
    pub r_two: u32,
    pub r_three: u32,
    pub reactivity: u32,
    pub supply_cap: i128,
    pub enabled: bool,
}

/// The data of a reserve asset.
#[derive(Clone, Debug)]
#[contracttype]
pub struct ReserveData {
    pub d_rate: i128, // dToken -> underlying rate, 12 decimals
    pub b_rate: i128, // bToken -> underlying rate, 12 decimals
    pub ir_mod: i128,
    pub b_supply: i128,
    pub d_supply: i128,
    pub backstop_credit: i128,
    pub last_time: u64,
}

/// A reserve as returned by `get_reserve`, accrued to the current timestamp.
#[derive(Clone, Debug)]
#[contracttype]
pub struct Reserve {
    pub asset: Address,
    pub config: ReserveConfig,
    pub data: ReserveData,
    pub scalar: i128,
}

/// The Blend pool entry points the wrapper relies on.
#[contractclient(name = "BlendPoolClient")]
pub trait BlendPool {
    /// Execute `requests` against the pool. `spender` always authorizes;
    /// `from` (the position owner) also authorizes when different.
    fn submit(
        e: Env,
        from: Address,
        spender: Address,
        to: Address,
        requests: Vec<Request>,
    ) -> Positions;

    /// The reserve for `asset`, accrued to the current timestamp. Panics if
    /// `asset` is not a reserve.
    fn get_reserve(e: Env, asset: Address) -> Reserve;

    /// The positions of `address`.
    fn get_positions(e: Env, address: Address) -> Positions;

    /// Claim accrued BLND emissions of `from` and send them to `to`.
    fn claim(e: Env, from: Address, reserve_token_ids: Vec<u32>, to: Address) -> i128;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn b_token_down_floors() {
        let env = Env::default();
        // 100 * 1e12 / 1.5e12 = 66.67
        assert_eq!(to_b_token_down(&env, 100, 1_500_000_000_000), 66);
        assert_eq!(to_b_token_down(&env, 300, 1_500_000_000_000), 200);
    }

    #[test]
    fn b_token_up_ceils() {
        let env = Env::default();
        assert_eq!(to_b_token_up(&env, 100, 1_500_000_000_000), 67);
        assert_eq!(to_b_token_up(&env, 300, 1_500_000_000_000), 200);
    }

    #[test]
    fn asset_from_b_token_floors() {
        let env = Env::default();
        // 67 * 1.5e12 / 1e12 = 100.5
        assert_eq!(to_asset_from_b_token(&env, 67, 1_500_000_000_000), 100);
        assert_eq!(to_asset_from_b_token(&env, 66, 1_500_000_000_000), 99);
    }

    #[test]
    fn asset_from_b_token_up_ceils() {
        let env = Env::default();
        assert_eq!(to_asset_from_b_token_up(&env, 67, 1_500_000_000_000), 101);
        assert_eq!(to_asset_from_b_token_up(&env, 66, 1_500_000_000_000), 99);
    }

    #[test]
    fn falls_back_to_i256_on_phantom_overflow() {
        let env = Env::default();
        // amount * 1e12 ≈ 1e42 overflows i128; the quotient fits.
        let amount: i128 = 1_000_000_000_000_000_000_000_000_000_000; // 1e30
        assert_eq!(to_b_token_down(&env, amount, SCALAR_12), amount);
        assert_eq!(to_b_token_up(&env, amount, SCALAR_12), amount);
        assert_eq!(to_asset_from_b_token(&env, amount, SCALAR_12), amount);
    }

    #[test]
    #[should_panic]
    fn panics_when_result_exceeds_i128() {
        let env = Env::default();
        to_asset_from_b_token(&env, i128::MAX, 2_000_000_000_000);
    }
}
