// SPDX-License-Identifier: BUSL-1.1
#![no_std]

mod errors;
pub mod storage;
#[cfg(test)]
mod test;

use errors::YieldTokenError;
use soroban_sdk::{
    Address, BytesN, Env, MuxedAddress, String, contract, contractimpl, panic_with_error,
};
use stellar_contract_utils::pausable::{self as pausable, Pausable};
use stellar_macros::when_not_paused;
use stellar_tokens::fungible::{Base, FungibleToken, FungibleTokenError, burnable};

#[contract]
pub struct YieldTokenContract;

#[contractimpl]
impl YieldTokenContract {
    /// `decimals` and `maturity` are passed by PT (it already has both)
    /// instead of YT calling `pt.decimals()` / `pt.maturity()`. At init time,
    /// PT is on the call stack (since PT deploys+inits YT), so a YT→PT
    /// cross-call would hit the Soroban host reentrancy guard. Same pattern
    /// as `before_yt_transfer` in spec-04 §17.1.
    ///
    /// Caching `maturity` (immutable in PT) also decouples YT's liveness
    /// from PT's instance TTL — a YT read no longer requires PT to be
    /// alive in storage.
    pub fn initialize(
        env: &Env,
        name: String,
        symbol: String,
        decimals: u32,
        maturity: u64,
        pt: Address,
    ) {
        if storage::is_initialized(env) {
            panic_with_error!(env, YieldTokenError::AlreadyInitialized);
        }
        Base::set_metadata(env, decimals, name, symbol);
        storage::set_pt(env, &pt);
        storage::set_maturity(env, maturity);
        storage::extend_instance_ttl(env);
    }

    /// Returns the actual stored balance, ignoring maturity (spec-03 §3.6).
    /// Used by PT for yield accounting.
    pub fn actual_balance_of(env: &Env, account: Address) -> i128 {
        Base::balance(env, &account)
    }

    pub fn get_pt(env: &Env) -> Address {
        storage::get_pt(env)
    }

    /// PT-only: mint YT to `to` (spec-03 §3.4).
    #[when_not_paused]
    pub fn mint(env: &Env, to: Address, amount: i128) {
        let pt = storage::get_pt(env);
        pt.require_auth();
        Base::mint(env, &to, amount);
        storage::extend_instance_ttl(env);
    }

    /// PT-only: burn `amount` from `owner` without calling update_yield
    /// (spec-03 §3.4). PT is responsible for settling yield before calling.
    /// If `spender != owner`, spends spender's allowance from owner.
    ///
    /// # Auth invariant
    ///
    /// Gated by `pt.require_auth()`. When `owner == spender`, the inner
    /// `Base::burn` skips its allowance/auth bookkeeping because PT is
    /// expected to have already obtained `owner`'s consent on the PT
    /// side before reaching this branch:
    ///
    /// - Self-redeem / self-withdraw: PT's entrypoint sees
    ///   `caller.require_auth()` against `owner` before issuing the YT burn.
    /// - Behalf-of via approval: PT consumes a PT-side allowance via
    ///   `Base::burn_from` on the PT shares before the YT burn.
    ///
    /// Callers (PT only) MUST preserve this invariant on every code path
    /// that lands here; otherwise PT can burn arbitrary YT balances for
    /// any holder.
    #[when_not_paused]
    pub fn burn_without_yield_update(env: &Env, owner: Address, spender: Address, amount: i128) {
        let pt = storage::get_pt(env);
        pt.require_auth();
        if owner == spender {
            Base::update(env, Some(&owner), None, amount);
            burnable::emit_burn(env, &owner, amount);
        } else {
            Base::burn_from(env, &spender, &owner, amount);
        }
        storage::extend_instance_ttl(env);
    }

    /// Public burn (spec-03 §3.5). Settles yield via
    /// `pt.update_yield(caller, pre_burn_balance)` before burning. Panics with
    /// FungibleTokenError::InsufficientBalance if expired and amount != 0.
    /// `Base::burn` handles `caller.require_auth()` internally.
    #[when_not_paused]
    pub fn burn(env: &Env, caller: Address, amount: i128) {
        if is_expired(env) && amount != 0 {
            panic_with_error!(env, FungibleTokenError::InsufficientBalance);
        }
        if amount == 0 {
            return;
        }
        let caller_balance = Base::balance(env, &caller);
        let pt = storage::get_pt(env);
        spectra_common::principal_token::PrincipalTokenClient::new(env, &pt)
            .update_yield(&caller, &caller_balance);
        Base::burn(env, &caller, amount);
        storage::extend_instance_ttl(env);
    }

    /// PT-only: upgrade YT's WASM. Mirrors the `pause`/`unpause` gating
    /// pattern — PT is the policy authority for YT, so YT instances roll
    /// forward only when PT (whose admin holds UPGRADE_ROLE) drives the
    /// upgrade. Uses Soroban's native `update_current_contract_wasm` (no
    /// proxy/beacon).
    ///
    /// Intentionally not `#[when_not_paused]`: a paused YT must remain
    /// upgradeable so PT's admin can patch out the bug that caused the
    /// pause. Trades a small "upgrade hides a paused state" surface for
    /// being able to recover an otherwise wedged contract.
    pub fn upgrade(env: &Env, new_wasm_hash: BytesN<32>) {
        storage::get_pt(env).require_auth();
        env.deployer().update_current_contract_wasm(new_wasm_hash);
        storage::extend_instance_ttl(env);
    }
}

/// PT-only pause mirror (spec-04 §17.2, issue #31).
/// PT calls `yt.pause()` / `yt.unpause()` from its own pause/unpause
/// implementations. OZ's `pausable` module stores the flag in YT's own
/// storage, enabling the `#[when_not_paused]` macro on state-changing fns.
#[contractimpl]
impl Pausable for YieldTokenContract {
    fn paused(e: &Env) -> bool {
        pausable::paused(e)
    }

    // The `_caller` arg exists to match OZ's `Pausable` trait signature;
    // YT's policy is "only PT can pause" so we always require PT directly.
    fn pause(e: &Env, _caller: Address) {
        storage::get_pt(e).require_auth();
        pausable::pause(e);
        storage::extend_instance_ttl(e);
    }

    fn unpause(e: &Env, _caller: Address) {
        storage::get_pt(e).require_auth();
        pausable::unpause(e);
        storage::extend_instance_ttl(e);
    }
}

#[contractimpl]
impl FungibleToken for YieldTokenContract {
    type ContractType = Base;

    /// Returns 0 after expiry (YT becomes worthless at maturity).
    /// Use [`actual_balance_of`] to query the real stored balance.
    fn balance(e: &Env, account: Address) -> i128 {
        if is_expired(e) {
            return 0;
        }
        Base::balance(e, &account)
    }

    /// Returns 0 after expiry.
    fn total_supply(e: &Env) -> i128 {
        if is_expired(e) {
            return 0;
        }
        Base::total_supply(e)
    }

    /// Panics with InsufficientBalance post-expiry. Mirrors the post-expiry
    /// reverts on `transfer`/`transfer_from`/`burn`: once YT is expired the
    /// allowance is unspendable, so the query is hardened to fail loudly
    /// instead of returning a stale non-zero value (Solidity returns the
    /// raw value but its `transferFrom` reverts, making the allowance
    /// effectively dead — same outcome, surfaced earlier here).
    fn allowance(e: &Env, owner: Address, spender: Address) -> i128 {
        if is_expired(e) {
            panic_with_error!(e, FungibleTokenError::InsufficientBalance);
        }
        Base::allowance(e, &owner, &spender)
    }

    /// Panics with InsufficientBalance if expired and amount != 0. Reads
    /// pre-transfer balances and pushes them into
    /// `pt.before_yt_transfer(from, to, from_bal, to_bal)` so yield is settled
    /// on pre-transfer state (spec-04 §17.1).
    #[when_not_paused]
    fn transfer(e: &Env, from: Address, to: MuxedAddress, amount: i128) {
        if is_expired(e) && amount != 0 {
            panic_with_error!(e, FungibleTokenError::InsufficientBalance);
        }
        // Zero-amount short-circuit before yield settlement: at amount==0
        // `before_yt_transfer` would settle both sides' yield with no allowance
        // or balance change, letting a caller force a checkpoint.
        if amount == 0 {
            return;
        }
        let to_addr = to.address();
        let from_bal = Base::balance(e, &from);
        let to_bal = Base::balance(e, &to_addr);
        let pt = storage::get_pt(e);
        spectra_common::principal_token::PrincipalTokenClient::new(e, &pt)
            .before_yt_transfer(&from, &to_addr, &from_bal, &to_bal);
        Base::transfer(e, &from, &to, amount);
        storage::extend_instance_ttl(e);
    }

    #[when_not_paused]
    fn transfer_from(e: &Env, spender: Address, from: Address, to: Address, amount: i128) {
        if is_expired(e) && amount != 0 {
            panic_with_error!(e, FungibleTokenError::InsufficientBalance);
        }
        // Zero-amount short-circuit before yield settlement — see transfer.
        if amount == 0 {
            return;
        }
        let from_bal = Base::balance(e, &from);
        let to_bal = Base::balance(e, &to);
        let pt = storage::get_pt(e);
        spectra_common::principal_token::PrincipalTokenClient::new(e, &pt)
            .before_yt_transfer(&from, &to, &from_bal, &to_bal);
        Base::transfer_from(e, &spender, &from, &to, amount);
        storage::extend_instance_ttl(e);
    }

    fn approve(e: &Env, owner: Address, spender: Address, amount: i128, live_until_ledger: u32) {
        Base::approve(e, &owner, &spender, amount, live_until_ledger);
        storage::extend_instance_ttl(e);
    }

    fn decimals(e: &Env) -> u32 {
        Base::decimals(e)
    }

    fn name(e: &Env) -> String {
        Base::name(e)
    }

    fn symbol(e: &Env) -> String {
        Base::symbol(e)
    }
}

fn is_expired(env: &Env) -> bool {
    env.ledger().timestamp() >= storage::get_maturity(env)
}
