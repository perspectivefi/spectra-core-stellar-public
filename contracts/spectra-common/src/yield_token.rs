// SPDX-License-Identifier: BUSL-1.1
//! YieldToken cross-contract surface.
//!
//! Mirrors Solidity's `IYieldToken`. Soroban host dispatches by function
//! name, so the trait declares only what cross-contract callers need to
//! reach. The real `YieldTokenContract` provides the bodies.

use soroban_sdk::{Address, BytesN, Env, String, contractclient};

#[contractclient(name = "YieldTokenClient")]
pub trait YieldToken {
    fn initialize(
        env: Env,
        name: String,
        symbol: String,
        decimals: u32,
        maturity: u64,
        pt: Address,
    );
    fn pause(env: Env, caller: Address);
    fn unpause(env: Env, caller: Address);
    fn mint(env: Env, to: Address, amount: i128);
    fn burn(env: Env, from: Address, amount: i128);
    fn burn_without_yield_update(env: Env, owner: Address, spender: Address, amount: i128);
    /// PT-only: drives YT's `update_current_contract_wasm`. YT requires the
    /// stored `pt` to have authorized, so the only legitimate caller is
    /// `PrincipalToken::upgrade_yt` (which is itself `UPGRADE`-gated).
    fn upgrade(env: Env, new_wasm_hash: BytesN<32>);
    /// Maturity-ignoring stored balance (spec-03 §3.6) — `balance()` returns 0
    /// after expiry, but PT's yield accounting needs the real ledger value.
    fn actual_balance_of(env: Env, account: Address) -> i128;
}
