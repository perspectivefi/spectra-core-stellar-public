// SPDX-License-Identifier: BUSL-1.1
//! PrincipalToken cross-contract surface.
//!
//! Mirrors Solidity's `IPrincipalToken`. The real `PrincipalTokenContract`
//! implements every method declared here, so any addition or signature
//! drift breaks compilation. `initialize` is intentionally NOT in this
//! trait — it lives in a direct `impl PrincipalTokenContract { ... }`
//! block, not in the trait impl. Cross-contract callers (Factory) reach
//! it via the separate `PrincipalTokenInitClient` below, which is
//! generated from a tiny trait that no contract implements — it exists
//! only to mint a typed client.
//!
//! SEP-41 / OZ `FungibleToken` methods (`decimals`, `name`, `symbol`,
//! `balance`, `total_supply`, `allowance`, `transfer`, `transfer_from`,
//! `approve`) are intentionally NOT redeclared here — cross-contract
//! callers hit those via `soroban_sdk::token::Client::new(env, &pt)`.
//!
//! `before_yt_transfer` and `update_yield` take pre-operation YT balances
//! as explicit parameters (spec-04 §17.1) so the caller never reenters YT
//! from inside PT. Soroban's host reentrancy guard (contract-scoped, always
//! on) would reject the back-call otherwise.

use soroban_sdk::{Address, BytesN, Env, Symbol, Val, Vec, contractclient};

/// Standalone init trait. No contract implements it — it exists only
/// to mint `PrincipalTokenInitClient`, which Factory calls right after
/// `env.deployer().deploy_v2(...)` to drive PT setup atomically.
/// Solidity: `initialize(_ibt, _duration, _initialAuthority)` plus
/// `registry` (immutable in Solidity, init param in Soroban).
#[contractclient(name = "PrincipalTokenInitClient")]
pub trait PrincipalTokenInit {
    fn initialize(env: &Env, ibt: Address, duration: u64, admin: Address, registry: Address);
}

/// Standalone client for PT's Soroban-specific `pricing_data` view (an inherent
/// method on `PrincipalTokenContract`, not part of the Solidity-mirroring
/// `PrincipalToken` trait). Returns `(ibt_rate, future_pt_value, maturity)` so
/// the limit-order engine can price a fill with one PT call instead of many.
#[contractclient(name = "PrincipalTokenPricingClient")]
pub trait PrincipalTokenPricing {
    fn pricing_data(env: &Env) -> (i128, i128, u64);
}

#[contractclient(name = "PrincipalTokenClient")]
pub trait PrincipalToken {
    // ── Admin: pause / fees / rate freeze ──────────────────────────────

    /// Pause PT and propagate the flag to its paired YT.
    /// Solidity: `pause()`.
    fn pause(env: &Env, caller: Address);

    /// Unpause PT and propagate to YT.
    /// Solidity: `unPause()`.
    fn unpause(env: &Env, caller: Address);

    /// Transfer accumulated IBT fees to the registry's fee collector.
    /// Solidity: `claimFees()`.
    fn claim_fees(env: &Env, caller: Address) -> i128;

    /// Freeze `pt_rate` / `ibt_rate` once expiry is reached. Permissionless.
    /// A second call panics with `RatesAtExpiryAlreadyStored`.
    /// Solidity: `storeRatesAtExpiry()`.
    fn store_rates_at_expiry(env: &Env);

    // ── YT-callbacks (yield system, spec-04 §3.5/§3.6 + §17.1/§17.3) ──

    /// Called by YT before a YT balance change from transfer or
    /// transfer_from. Settles yield for both sender and receiver using
    /// the pre-transfer balances.
    fn before_yt_transfer(
        env: &Env,
        from: Address,
        to: Address,
        from_balance: i128,
        to_balance: i128,
    );

    /// Called by YT before a public burn. Settles yield for the caller
    /// using their pre-burn balance. Returns the updated accumulated
    /// yield in IBT.
    fn update_yield(env: &Env, user: Address, user_yt_balance: i128) -> i128;

    // ── Vault: deposit (spec-04 §3.2, §5.1) ───────────────────────────

    /// Deposit `assets` of the underlying. PT pulls from `caller`, wraps to
    /// IBT, splits PT to `pt_receiver` and YT to `yt_receiver` minus the
    /// tokenization fee. Returns shares minted.
    fn deposit(
        env: &Env,
        caller: Address,
        assets: i128,
        pt_receiver: Address,
        yt_receiver: Address,
    ) -> i128;

    /// `deposit` + slippage guard on minimum shares.
    fn deposit_with_min_shares(
        env: &Env,
        caller: Address,
        assets: i128,
        pt_receiver: Address,
        yt_receiver: Address,
        min_shares: i128,
    ) -> i128;

    /// IBT-side deposit: caller already holds IBT shares.
    fn deposit_ibt(
        env: &Env,
        caller: Address,
        ibts: i128,
        pt_receiver: Address,
        yt_receiver: Address,
    ) -> i128;

    /// `deposit_ibt` + slippage guard on minimum shares.
    fn deposit_ibt_with_min_shares(
        env: &Env,
        caller: Address,
        ibts: i128,
        pt_receiver: Address,
        yt_receiver: Address,
        min_shares: i128,
    ) -> i128;

    // ── Vault: redeem (spec-04 §3.3, §5.2) ────────────────────────────

    /// Burn `shares` from `owner` (and matching YT pre-expiry), redeem the
    /// underlying through the IBT, transfer it to `receiver`. Returns the
    /// underlying delivered.
    fn redeem(env: &Env, caller: Address, shares: i128, receiver: Address, owner: Address) -> i128;

    /// `redeem` + slippage guard on minimum underlying.
    fn redeem_with_min_assets(
        env: &Env,
        caller: Address,
        shares: i128,
        receiver: Address,
        owner: Address,
        min_assets: i128,
    ) -> i128;

    /// Like `redeem` but transfers IBT shares to `receiver` instead of
    /// unwrapping to underlying.
    fn redeem_for_ibt(
        env: &Env,
        caller: Address,
        shares: i128,
        receiver: Address,
        owner: Address,
    ) -> i128;

    /// `redeem_for_ibt` + slippage guard on minimum IBT.
    fn redeem_for_ibt_with_min_ibts(
        env: &Env,
        caller: Address,
        shares: i128,
        receiver: Address,
        owner: Address,
        min_ibts: i128,
    ) -> i128;

    // ── Vault: withdraw (spec-04 §3.4, §5.3) ──────────────────────────

    /// Withdraw a specific amount of underlying. Burns matching shares from
    /// `owner`, transfers underlying to `receiver`. Returns shares burned.
    fn withdraw(
        env: &Env,
        caller: Address,
        assets: i128,
        receiver: Address,
        owner: Address,
    ) -> i128;

    /// `withdraw` + slippage guard on maximum shares burned.
    fn withdraw_with_max_shares(
        env: &Env,
        caller: Address,
        assets: i128,
        receiver: Address,
        owner: Address,
        max_shares: i128,
    ) -> i128;

    /// IBT-side withdraw: caller specifies an IBT amount to receive.
    fn withdraw_ibt(
        env: &Env,
        caller: Address,
        ibts: i128,
        receiver: Address,
        owner: Address,
    ) -> i128;

    /// `withdraw_ibt` + slippage guard on maximum shares burned.
    fn withdraw_ibt_with_max_shares(
        env: &Env,
        caller: Address,
        ibts: i128,
        receiver: Address,
        owner: Address,
        max_shares: i128,
    ) -> i128;

    // ── Yield claim (spec-04 §3.5, §8.4) ──────────────────────────────

    /// Claim accumulated yield as underlying.
    fn claim_yield(env: &Env, caller: Address, receiver: Address, min_assets: i128) -> i128;

    /// Claim accumulated yield as IBT (skip the IBT→underlying redeem).
    fn claim_yield_in_ibt(env: &Env, caller: Address, receiver: Address, min_ibts: i128) -> i128;

    // ── Admin: upgrade / rewards (spec-04 §3.7, §3.8) ─────────────────

    /// Swap the contract wasm. Role-gated to `UPGRADE`.
    fn upgrade(env: &Env, caller: Address, new_wasm_hash: BytesN<32>);

    /// Upgrade the paired YT's WASM. Gated by the `UPGRADE` role on PT
    /// because YT's own `upgrade` requires PT auth — this is the only
    /// legitimate path to roll YT forward.
    fn upgrade_yt(env: &Env, caller: Address, new_wasm_hash: BytesN<32>);

    /// The installed rewards proxy, or `None` when unset.
    fn rewards_proxy(env: &Env) -> Option<Address>;

    /// Install, replace, or clear the rewards proxy. `Some(addr)` installs;
    /// `None` clears (mirrors Solidity `setRewardsProxy(address(0))`).
    /// Role-gated to `RWD_PROX`.
    fn set_rewards_proxy(env: &Env, caller: Address, proxy: Option<Address>);

    /// Harvest rewards via the installed proxy. Role-gated to `RWD_HARV`.
    /// Dynamic dispatch, same surface as the wrapper: the caller names the
    /// proxy entry point (`function`) and passes its `args`.
    fn claim_rewards(env: &Env, caller: Address, function: Symbol, args: Vec<Val>) -> Val;

    // ── Vault views (spec-04 §3.10, §10) ──────────────────────────────

    /// Underlying value of PT's own IBT holdings. Solidity: `totalAssets()`.
    fn total_assets(env: &Env) -> i128;

    fn convert_to_principal(env: &Env, underlying: i128) -> i128;
    fn convert_to_underlying(env: &Env, principal: i128) -> i128;

    /// ERC-4626 `previewDeposit`. Returns the share count for `assets` of
    /// underlying assuming the **unreduced** registry tokenization fee — i.e.
    /// no per-user `fee_reduction`. Discounted users see a worse preview than
    /// they will actually receive; use `preview_deposit_for` for a faithful
    /// quote. See issue #50.
    fn preview_deposit(env: &Env, assets: i128) -> i128;
    /// ERC-4626-style preview in IBT terms. Same unreduced-fee assumption as
    /// `preview_deposit`. Use `preview_deposit_ibt_for` for a discount-aware
    /// quote.
    fn preview_deposit_ibt(env: &Env, ibts: i128) -> i128;
    /// Like `preview_deposit` but applies `user`'s per-PT `fee_reduction`.
    /// Solidity has no direct equivalent — discounted users on EVM `eth_call`
    /// with `from = user_with_discount`. Soroban view simulation has no such
    /// caller-spoof, so SDKs / discounted users call this variant directly to
    /// get a faithful quote. See issue #50.
    fn preview_deposit_for(env: &Env, assets: i128, user: Address) -> i128;
    /// Like `preview_deposit_ibt` but applies `user`'s per-PT `fee_reduction`.
    /// See `preview_deposit_for` for rationale.
    fn preview_deposit_ibt_for(env: &Env, ibts: i128, user: Address) -> i128;
    fn preview_redeem(env: &Env, shares: i128) -> i128;
    fn preview_redeem_for_ibt(env: &Env, shares: i128) -> i128;
    fn preview_withdraw(env: &Env, assets: i128) -> i128;
    fn preview_withdraw_ibt(env: &Env, ibts: i128) -> i128;

    fn max_deposit(env: &Env, receiver: Address) -> i128;
    fn max_redeem(env: &Env, owner: Address) -> i128;
    fn max_withdraw(env: &Env, owner: Address) -> i128;
    fn max_withdraw_ibt(env: &Env, owner: Address) -> i128;

    /// Registry-level tokenization fee (unreduced rate). Per-PT
    /// `fee_reduction` is applied separately. Solidity: `getTokenizationFee()`.
    fn get_tokenization_fee(env: &Env) -> i128;

    /// Current accumulated yield in IBT for `user` if they were to claim
    /// now, **net of the yield fee**. Solidity: `getCurrentYieldOfUserInIBT()`.
    fn get_current_yield_of_user_in_ibt(env: &Env, user: Address) -> i128;

    // ── Other views (spec-04 §3.10 — port of Solidity getters) ────────

    fn underlying(env: &Env) -> Address;

    /// IBT vault address.
    /// Solidity: `getIBT()`.
    fn get_ibt(env: &Env) -> Address;

    /// Paired YT address.
    /// Solidity: `getYT()`.
    fn get_yt(env: &Env) -> Address;

    /// Current PT rate in Ray (27-decimal).
    /// Solidity: `getPTRate()`.
    fn get_pt_rate(env: &Env) -> i128;

    /// Current IBT rate in Ray (27-decimal).
    /// Solidity: `getIBTRate()`.
    fn get_ibt_rate(env: &Env) -> i128;

    /// One unit of IBT (10^ibt_decimals), captured at init.
    /// Solidity: `getIBTUnit()`.
    fn get_ibt_unit(env: &Env) -> i128;

    /// Duration in seconds, captured at init.
    /// Solidity: `getDuration()`.
    fn get_duration(env: &Env) -> u64;

    /// Maturity timestamp (init time + duration).
    /// Solidity: `maturity()`.
    fn maturity(env: &Env) -> u64;

    /// True iff PT is paused.
    /// Solidity: `paused()`.
    fn paused(env: &Env) -> bool;

    /// Accumulated unclaimed protocol fees in IBT terms.
    /// Solidity: `getUnclaimedFeesInIBT()`.
    fn get_unclaimed_fees_in_ibt(env: &Env) -> i128;

    /// Total fees ever collected in IBT terms.
    fn get_total_fees_in_ibt(env: &Env) -> i128;

    // ── Admin rotation (2-step, delegated to stellar-access) ──────────

    /// Propose a new admin. Authorization is required from the current admin
    /// (enforced inside `stellar_access::access_control::transfer_admin_role`).
    /// `live_until_ledger == 0` cancels any prior proposal.
    fn transfer_admin_role(env: &Env, new_admin: Address, live_until_ledger: u32);

    /// Accept the pending admin rotation. Caller (`require_auth`) must equal
    /// the proposed new admin and the current ledger must not exceed
    /// `live_until_ledger`.
    fn accept_admin_transfer(env: &Env);
}
