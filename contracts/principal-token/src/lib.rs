// SPDX-License-Identifier: BUSL-1.1
#![no_std]

pub mod errors;
pub mod events;
pub mod storage;
#[cfg(test)]
mod test;

use crate::errors::PrincipalTokenError;
use crate::events::{FeeClaimed, YtDeployed};
use soroban_sdk::{
    Address, BytesN, Env, MuxedAddress, String, Symbol, Val, Vec, contract, contractimpl,
    panic_with_error,
};
use spectra_common::constants::{ONE, roles};
use spectra_common::naming;
use spectra_common::principal_token::PrincipalToken;
use spectra_common::yield_token::YieldTokenClient;
use spectra_math::mul_div::{Rounding, mul_div_wide};
use spectra_math::ray_math;
use stellar_access::access_control;
use stellar_contract_utils::pausable;
use stellar_macros::{only_role, when_not_paused};
use stellar_tokens::fungible::{Base, FungibleToken, FungibleTokenClient};
use stellar_tokens::vault::FungibleVaultClient;

const MIN_DECIMALS: u32 = 6;
const MAX_DECIMALS: u32 = 18;
const RAY: i128 = 1_000_000_000_000_000_000_000_000_000;

/// Dust threshold in underlying token-decimals. Yield below this rounds to
/// zero so the rounding error favors the protocol.
const SAFETY_BOUND: i128 = 100;

#[contract]
pub struct PrincipalTokenContract;

#[contractimpl]
impl PrincipalTokenContract {
    /// Initialize the Principal Token and its paired Yield Token.
    ///
    /// Requires `admin` authorization and must be called only once. Reads IBT and
    /// underlying metadata, validates decimals, computes initial rates, sets PT
    /// metadata, grants roles, deploys the paired YT, and persists instance state.
    /// Panics with `AlreadyInitialized`, `RateError`, or `InvalidDecimals` when
    /// preconditions fail.
    pub fn initialize(env: &Env, ibt: Address, duration: u64, admin: Address, registry: Address) {
        if storage::is_initialized(env) {
            panic_with_error!(env, PrincipalTokenError::AlreadyInitialized);
        }

        // 1. Read IBT state. A bogus IBT panics naturally on the first call —
        //    explicit zero-address checks are unnecessary here.
        let ibt_client = FungibleVaultClient::new(env, &ibt);
        if ibt_client.total_assets() == 0 {
            panic_with_error!(env, PrincipalTokenError::RateError);
        }
        let underlying = ibt_client.query_asset();
        let ibt_token = FungibleTokenClient::new(env, &ibt);
        let ibt_decimals = ibt_token.decimals();
        let ibt_symbol = ibt_token.symbol();

        // 2. Read underlying state via the SEP-41 token client.
        let und_client = soroban_sdk::token::Client::new(env, &underlying);
        let underlying_decimals = und_client.decimals();
        let underlying_symbol = und_client.symbol();

        // 3. Validate decimal bounds.
        if underlying_decimals < MIN_DECIMALS
            || ibt_decimals > MAX_DECIMALS
            || underlying_decimals > ibt_decimals
        {
            panic_with_error!(env, PrincipalTokenError::InvalidDecimals);
        }

        // 4. Compute timing.
        let expiry = env.ledger().timestamp() + duration;
        let ibt_unit = 10_i128.pow(ibt_decimals);

        // 5. Compute initial rates. ibt_rate is preview_redeem(unit) lifted to Ray;
        //    pt_rate starts at exactly 1.0 in Ray.
        let ibt_rate_raw = ibt_client.preview_redeem(&ibt_unit);
        // Reject a non-positive initial rate
        if ibt_rate_raw <= 0 {
            panic_with_error!(env, PrincipalTokenError::RateError);
        }
        let ibt_rate = ray_math::to_ray(ibt_rate_raw, underlying_decimals);
        let pt_rate = RAY;

        // 6. Compose names + set PT metadata. PT's decimals match the IBT's;
        //    YT shares the same decimals — passed to YT.initialize below.
        let pt_name = naming::gen_pt_name(env, &ibt_symbol, &underlying_symbol, expiry);
        let pt_symbol = naming::gen_pt_symbol(env, &ibt_symbol, &underlying_symbol, expiry);
        Base::set_metadata(env, ibt_decimals, pt_name, pt_symbol);

        // 7. Grant admin the default admin role.
        access_control::set_admin(env, &admin);
        access_control::grant_role_no_auth(env, &admin, &roles::PAUSER, &admin);
        access_control::grant_role_no_auth(env, &admin, &roles::UPGRADE, &admin);
        access_control::grant_role_no_auth(env, &admin, &roles::REWARDS_HARVESTER, &admin);
        access_control::grant_role_no_auth(env, &admin, &roles::REWARDS_PROXY_SETTER, &admin);

        // 8. Deploy the paired YieldToken. PT deploys exactly one YT in its
        //    lifetime, and the deployer's address (PT's own) varies per
        //    PT instance, so a content-derived salt adds no uniqueness.
        //    A fixed zero salt is sufficient.
        let yt_wasm_hash =
            spectra_common::registry::RegistryClient::new(env, &registry).get_yt_wasm_hash();
        let yt_address = env
            .deployer()
            .with_current_contract(BytesN::from_array(env, &[0u8; 32]))
            .deploy_v2(yt_wasm_hash, ());
        let yt_name = naming::gen_yt_name(env, &ibt_symbol, &underlying_symbol, expiry);
        let yt_symbol = naming::gen_yt_symbol(env, &ibt_symbol, &underlying_symbol, expiry);
        // Pass ibt_decimals and expiry explicitly so YT skips the YT->PT
        // call back during its own initialize (host reentrancy guard rejects
        // re-entry while PT is on the call stack).
        YieldTokenClient::new(env, &yt_address).initialize(
            &yt_name,
            &yt_symbol,
            &ibt_decimals,
            &expiry,
            &env.current_contract_address(),
        );

        // 9. Persist all instance state.
        storage::set_ibt(env, &ibt);
        storage::set_underlying(env, &underlying);
        storage::set_yt(env, &yt_address);
        storage::set_registry(env, &registry);
        storage::set_ibt_unit(env, ibt_unit);
        storage::set_ibt_rate(env, ibt_rate);
        storage::set_pt_rate(env, pt_rate);
        storage::set_ibt_decimals(env, ibt_decimals);
        storage::set_underlying_decimals(env, underlying_decimals);
        storage::set_duration(env, duration);
        storage::set_expiry(env, expiry);
        storage::set_unclaimed_fees_ibt(env, 0);
        storage::set_total_fees_ibt(env, 0);

        // 10. Emit YtDeployed.
        YtDeployed {
            address: yt_address,
        }
        .publish(env);
    }

    // -----------------------------------------------------------------------
    // Role management — thin wrappers over OZ stellar-access.
    //
    // The library functions enforce auth + admin-role checks themselves; we
    // expose them as contract entry points so off-chain tooling (deploy
    // dispatcher, key rotation) can rotate the PAUSER, FEE_SETTER, UPGRADE,
    // and REWARDS_HARVESTER roles after `initialize`.
    // -----------------------------------------------------------------------

    /// Grant `role` to `account`. `caller` must be the contract admin or hold
    /// the admin-role of `role`. Auth + admin checks are enforced by
    /// `access_control::grant_role`.
    pub fn grant_role(env: &Env, caller: Address, account: Address, role: Symbol) {
        access_control::grant_role(env, &account, &role, &caller);
        storage::extend_instance_ttl(env);
    }

    /// Revoke `role` from `account`. Same auth model as `grant_role`.
    pub fn revoke_role(env: &Env, caller: Address, account: Address, role: Symbol) {
        access_control::revoke_role(env, &account, &role, &caller);
        storage::extend_instance_ttl(env);
    }

    /// View: whether `account` currently holds `role`.
    pub fn has_role(env: &Env, account: Address, role: Symbol) -> bool {
        access_control::has_role(env, &account, &role).is_some()
    }

    /// Bundled pricing snapshot for the limit-order engine:
    /// `(ibt_rate, future_pt_value, maturity)` in a single cross-contract call.
    ///
    /// The engine otherwise reads these via ~7 separate calls (PT `get_ibt`,
    /// IBT `decimals`, vault `query_asset`, underlying `decimals`, PT
    /// `get_ibt_rate`, PT `preview_redeem`, PT `maturity`) — each re-instantiating
    /// the target contract, which on a heavy IBT (Blend pool / Upshift vault)
    /// dominates per-fill memory. Folding them into one PT invocation (with one
    /// internal vault read) removes those redundant instantiations.
    #[when_not_paused]
    pub fn pricing_data(env: &Env) -> (i128, i128, u64) {
        let (pt_rate, ibt_rate) = _get_pt_and_ibt_rates(env, false);
        let ibts = _convert_shares_to_ibts_with(env, ONE, Rounding::Floor, pt_rate, ibt_rate);
        let preview = FungibleVaultClient::new(env, &storage::get_ibt(env)).preview_redeem(&ibts);
        let ibt_decimals = storage::get_ibt_decimals(env);
        let underlying_decimals = storage::get_underlying_decimals(env);
        let scale = 10i128.pow(ibt_decimals - underlying_decimals);
        let future_pt_value = preview * scale;
        (ibt_rate, future_pt_value, storage::get_expiry(env))
    }
}

#[contractimpl]
impl PrincipalToken for PrincipalTokenContract {
    // ── Pause / unpause — role-gated to PAUSER ─────────
    // PT is the initiator; it propagates the flag to YT so YT's
    // `#[when_not_paused]` guards take effect across the pair.

    /// Pause both PT and the paired YT. Requires the `PAUSER` role.
    #[only_role(caller, "PAUSER")]
    fn pause(env: &Env, caller: Address) {
        pausable::pause(env);
        let yt = storage::get_yt(env);
        YieldTokenClient::new(env, &yt).pause(&env.current_contract_address());
    }

    /// Unpause both PT and the paired YT. Requires the `PAUSER` role.
    #[only_role(caller, "PAUSER")]
    fn unpause(env: &Env, caller: Address) {
        pausable::unpause(env);
        let yt = storage::get_yt(env);
        YieldTokenClient::new(env, &yt).unpause(&env.current_contract_address());
    }

    // ── Fees ───────────────────────────────────────────
    // Only the fee collector itself can call (direct address check against the
    // registry's stored fee_collector).

    /// Freeze PT and IBT rates after expiry so all vault flows use fixed values.
    /// Panics with `NotExpired` before maturity or `RatesAtExpiryAlreadyStored`
    /// on a repeat call.
    #[when_not_paused]
    fn store_rates_at_expiry(env: &Env) {
        if env.ledger().timestamp() < storage::get_expiry(env) {
            panic_with_error!(env, PrincipalTokenError::NotExpired);
        }
        if storage::get_rates_at_expiry_stored(env) {
            panic_with_error!(env, PrincipalTokenError::RatesAtExpiryAlreadyStored);
        }
        store_rates_at_expiry_inner(env);
    }

    /// Transfer accumulated fees to the registry fee collector as IBT shares.
    /// Panics with `UnauthorizedFeeCollector` when caller is not the collector and
    /// `NoFeesToClaim` when the accumulated fee balance is zero.
    #[when_not_paused]
    fn claim_fees(env: &Env, caller: Address) -> i128 {
        caller.require_auth();
        let registry_addr = storage::get_registry(env);
        let registry_client = spectra_common::registry::RegistryClient::new(env, &registry_addr);
        let fee_collector = match registry_client.get_fee_collector_opt() {
            Some(c) => c,
            None => panic_with_error!(env, PrincipalTokenError::FeeCollectorNotSet),
        };
        if caller != fee_collector {
            panic_with_error!(env, PrincipalTokenError::UnauthorizedFeeCollector);
        }
        let ibts = storage::get_unclaimed_fees_ibt(env);
        if ibts == 0 {
            panic_with_error!(env, PrincipalTokenError::NoFeesToClaim);
        }
        storage::set_unclaimed_fees_ibt(env, 0);
        let ibt_addr = storage::get_ibt(env);
        soroban_sdk::token::Client::new(env, &ibt_addr).transfer(
            &env.current_contract_address(),
            &caller,
            &ibts,
        );
        FeeClaimed {
            fee_collector: caller,
            ibts,
        }
        .publish(env);
        ibts
    }

    // ── YT-callbacks ───────────────────────────────────
    // Only the paired YT can call these. `update_yield` and
    // `before_yt_transfer` accept the user's pre-op YT balance as an
    // explicit parameter so PT never reenters YT.

    /// Settle yield for both sides of a YT transfer using pre-transfer balances.
    /// Callable only by the paired YT.
    #[when_not_paused]
    fn before_yt_transfer(
        env: &Env,
        from: Address,
        to: Address,
        from_balance: i128,
        to_balance: i128,
    ) {
        storage::get_yt(env).require_auth();
        // Resolve + persist the PT/IBT rates once, then settle both sides with
        // them. Equivalent to two `update_yield_inner` calls — the second read
        // would return the rate the first just persisted — but skips the
        // duplicate IBT-rate read that re-instantiates the wrapper + vault.
        let (pt_rate, ibt_rate) = _update_pt_and_ibt_rates(env);
        settle_user_yield(env, &from, from_balance, pt_rate, ibt_rate);
        settle_user_yield(env, &to, to_balance, pt_rate, ibt_rate);
    }

    /// Settle yield for a user based on their pre-op YT balance.
    /// Callable only by the paired YT.
    #[when_not_paused]
    fn update_yield(env: &Env, user: Address, user_yt_balance: i128) -> i128 {
        storage::get_yt(env).require_auth();
        update_yield_inner(env, &user, user_yt_balance).0
    }

    // ── Views — pure reads, no auth, no TTL bump.
    // `decimals` ships via the `FungibleToken` impl below to avoid duplicate host
    // function definitions.

    /// Return the underlying asset address.
    fn underlying(env: &Env) -> Address {
        storage::get_underlying(env)
    }

    /// Return the IBT vault address.
    fn get_ibt(env: &Env) -> Address {
        storage::get_ibt(env)
    }

    /// Return the paired YT address.
    fn get_yt(env: &Env) -> Address {
        storage::get_yt(env)
    }

    /// Return the current PT rate (Ray).
    fn get_pt_rate(env: &Env) -> i128 {
        storage::get_pt_rate(env)
    }

    /// Return the current IBT rate (Ray).
    fn get_ibt_rate(env: &Env) -> i128 {
        storage::get_ibt_rate(env)
    }

    /// Return the IBT unit (10^decimals).
    fn get_ibt_unit(env: &Env) -> i128 {
        storage::get_ibt_unit(env)
    }

    /// Return the PT duration in seconds.
    fn get_duration(env: &Env) -> u64 {
        storage::get_duration(env)
    }

    /// Return the maturity timestamp.
    fn maturity(env: &Env) -> u64 {
        storage::get_expiry(env)
    }

    /// Return pause state.
    fn paused(env: &Env) -> bool {
        pausable::paused(env)
    }

    /// Return unclaimed fees in IBT shares.
    fn get_unclaimed_fees_in_ibt(env: &Env) -> i128 {
        storage::get_unclaimed_fees_ibt(env)
    }

    /// Return total accumulated fees in IBT shares.
    fn get_total_fees_in_ibt(env: &Env) -> i128 {
        storage::get_total_fees_ibt(env)
    }

    // -----------------------------------------------------------------------
    // Vault: deposit
    // -----------------------------------------------------------------------

    /// Deposit underlying assets. PT pulls from `caller`, wraps to IBT, mints PT
    /// to `pt_receiver` and YT to `yt_receiver` after the tokenization fee.
    #[when_not_paused]
    fn deposit(
        env: &Env,
        caller: Address,
        assets: i128,
        pt_receiver: Address,
        yt_receiver: Address,
    ) -> i128 {
        caller.require_auth();
        let underlying = storage::get_underlying(env);
        let ibt = storage::get_ibt(env);
        let und = soroban_sdk::token::Client::new(env, &underlying);
        let pt = env.current_contract_address();
        und.transfer_from(&pt, &caller, &pt, &assets);
        // The vault pulls assets via `transfer(pt → ibt)` one frame below this
        // call, out of reach of PT's direct-invoker auto-auth.
        spectra_common::auth::authorize_pull(env, &underlying, &pt, &ibt, assets);
        let ibts = FungibleVaultClient::new(env, &ibt).deposit(&assets, &pt, &pt, &pt);
        _deposit_ibt(env, &caller, ibts, &pt_receiver, &yt_receiver)
    }

    /// Deposit underlying with a minimum-share guard.
    /// Panics with `SlippageProtectionFailed` if minted shares are below `min_shares`.
    #[when_not_paused]
    fn deposit_with_min_shares(
        env: &Env,
        caller: Address,
        assets: i128,
        pt_receiver: Address,
        yt_receiver: Address,
        min_shares: i128,
    ) -> i128 {
        let shares = Self::deposit(env, caller, assets, pt_receiver, yt_receiver);
        if shares < min_shares {
            panic_with_error!(env, PrincipalTokenError::SlippageProtectionFailed);
        }
        shares
    }

    /// Deposit using IBT shares directly. PT pulls IBT from `caller` and settles
    /// minting through the same internal IBT deposit flow.
    #[when_not_paused]
    fn deposit_ibt(
        env: &Env,
        caller: Address,
        ibts: i128,
        pt_receiver: Address,
        yt_receiver: Address,
    ) -> i128 {
        caller.require_auth();
        let ibt = storage::get_ibt(env);
        let pt = env.current_contract_address();
        soroban_sdk::token::Client::new(env, &ibt).transfer_from(&pt, &caller, &pt, &ibts);
        _deposit_ibt(env, &caller, ibts, &pt_receiver, &yt_receiver)
    }

    /// Deposit IBT with a minimum-share guard.
    #[when_not_paused]
    fn deposit_ibt_with_min_shares(
        env: &Env,
        caller: Address,
        ibts: i128,
        pt_receiver: Address,
        yt_receiver: Address,
        min_shares: i128,
    ) -> i128 {
        let shares = Self::deposit_ibt(env, caller, ibts, pt_receiver, yt_receiver);
        if shares < min_shares {
            panic_with_error!(env, PrincipalTokenError::SlippageProtectionFailed);
        }
        shares
    }

    // -----------------------------------------------------------------------
    // Vault: redeem
    // -----------------------------------------------------------------------

    /// Redeem PT shares for underlying. Burns PT from `owner`, burns YT pre-expiry,
    /// unwraps IBT, and transfers underlying to `receiver`.
    #[when_not_paused]
    fn redeem(env: &Env, caller: Address, shares: i128, receiver: Address, owner: Address) -> i128 {
        // Guard: zero-amount before yield settlement — see README.
        if shares == 0 {
            return 0;
        }
        let (pt_rate, ibt_rate) = _before_redeem(env, &caller, shares, &owner);
        crate::events::Redeem {
            owner: owner.clone(),
            receiver: receiver.clone(),
            shares,
        }
        .publish(env);
        let ibts = _convert_shares_to_ibts_with(env, shares, Rounding::Floor, pt_rate, ibt_rate);
        let pt = env.current_contract_address();
        FungibleVaultClient::new(env, &storage::get_ibt(env)).redeem(&ibts, &receiver, &pt, &pt)
    }

    /// Redeem with a minimum-asset guard.
    /// Panics with `SlippageProtectionFailed` if delivered assets are below `min_assets`.
    #[when_not_paused]
    fn redeem_with_min_assets(
        env: &Env,
        caller: Address,
        shares: i128,
        receiver: Address,
        owner: Address,
        min_assets: i128,
    ) -> i128 {
        let assets = Self::redeem(env, caller, shares, receiver, owner);
        if assets < min_assets {
            panic_with_error!(env, PrincipalTokenError::SlippageProtectionFailed);
        }
        assets
    }

    /// Redeem PT shares for IBT shares instead of underlying.
    #[when_not_paused]
    fn redeem_for_ibt(
        env: &Env,
        caller: Address,
        shares: i128,
        receiver: Address,
        owner: Address,
    ) -> i128 {
        // Guard: zero-amount before yield settlement — see README.
        if shares == 0 {
            return 0;
        }
        let (pt_rate, ibt_rate) = _before_redeem(env, &caller, shares, &owner);
        let ibts = _convert_shares_to_ibts_with(env, shares, Rounding::Floor, pt_rate, ibt_rate);
        crate::events::Redeem {
            owner: owner.clone(),
            receiver: receiver.clone(),
            shares,
        }
        .publish(env);
        if ibts != 0 {
            let pt = env.current_contract_address();
            soroban_sdk::token::Client::new(env, &storage::get_ibt(env))
                .transfer(&pt, &receiver, &ibts);
        }
        ibts
    }

    /// Redeem for IBT with a minimum-IBT guard.
    #[when_not_paused]
    fn redeem_for_ibt_with_min_ibts(
        env: &Env,
        caller: Address,
        shares: i128,
        receiver: Address,
        owner: Address,
        min_ibts: i128,
    ) -> i128 {
        let ibts = Self::redeem_for_ibt(env, caller, shares, receiver, owner);
        if ibts < min_ibts {
            panic_with_error!(env, PrincipalTokenError::SlippageProtectionFailed);
        }
        ibts
    }

    // -----------------------------------------------------------------------
    // Vault: withdraw
    // -----------------------------------------------------------------------

    /// Withdraw a fixed amount of underlying. Redeems IBT for `assets`, burns the
    /// corresponding PT (and YT pre-expiry), and transfers underlying to `receiver`.
    #[when_not_paused]
    fn withdraw(
        env: &Env,
        caller: Address,
        assets: i128,
        receiver: Address,
        owner: Address,
    ) -> i128 {
        // Guard: zero-amount before yield settlement — see README.
        if assets == 0 {
            return 0;
        }
        _before_withdraw(env, assets, &owner);
        // Snapshot rates BEFORE the IBT side-effect so the share count uses the
        // pre-withdrawal IBT state.
        let (pt_rate, ibt_rate) = _get_pt_and_ibt_rates(env, false);
        let pt = env.current_contract_address();
        let ibts = FungibleVaultClient::new(env, &storage::get_ibt(env))
            .withdraw(&assets, &receiver, &pt, &pt);
        _burn_shares_for_withdraw(env, &caller, ibts, &receiver, &owner, pt_rate, ibt_rate)
    }

    /// Withdraw with a maximum-share guard.
    #[when_not_paused]
    fn withdraw_with_max_shares(
        env: &Env,
        caller: Address,
        assets: i128,
        receiver: Address,
        owner: Address,
        max_shares: i128,
    ) -> i128 {
        let shares = Self::withdraw(env, caller, assets, receiver, owner);
        if shares > max_shares {
            panic_with_error!(env, PrincipalTokenError::SlippageProtectionFailed);
        }
        shares
    }

    /// Withdraw IBT shares directly. Burns the matching PT (and YT pre-expiry) and
    /// transfers IBT to `receiver`.
    #[when_not_paused]
    fn withdraw_ibt(
        env: &Env,
        caller: Address,
        ibts: i128,
        receiver: Address,
        owner: Address,
    ) -> i128 {
        // Convert IBTs to assets via preview redeem before the cap check so
        // `_before_withdraw` sees an asset amount.
        //
        // Guard: zero-amount before yield settlement — see README.
        if ibts == 0 {
            return 0;
        }
        let assets = FungibleVaultClient::new(env, &storage::get_ibt(env)).preview_redeem(&ibts);
        _before_withdraw(env, assets, &owner);
        // Snapshot rates BEFORE the IBT.transfer side-effect.
        let (pt_rate, ibt_rate) = _get_pt_and_ibt_rates(env, false);
        let shares =
            _burn_shares_for_withdraw(env, &caller, ibts, &receiver, &owner, pt_rate, ibt_rate);
        if ibts != 0 {
            let pt = env.current_contract_address();
            soroban_sdk::token::Client::new(env, &storage::get_ibt(env))
                .transfer(&pt, &receiver, &ibts);
        }
        shares
    }

    // -----------------------------------------------------------------------
    // Upgrade — Soroban-native contract upgrade.
    // -----------------------------------------------------------------------

    /// Upgrade the contract code to `new_wasm_hash`. Requires the `UPGRADE` role.
    #[only_role(caller, "UPGRADE")]
    fn upgrade(env: &Env, caller: Address, new_wasm_hash: BytesN<32>) {
        crate::events::PtUpgraded {
            caller,
            new_wasm_hash: new_wasm_hash.clone(),
        }
        .publish(env);
        env.deployer().update_current_contract_wasm(new_wasm_hash);
    }

    /// Upgrade the paired YT's WASM to `new_wasm_hash`. Requires the
    /// `UPGRADE` role on PT. The PT is YT's policy authority — YT's own
    /// `upgrade` entry point requires `pt.require_auth()`, so this is the
    /// only legitimate path to roll YT forward.
    #[only_role(caller, "UPGRADE")]
    fn upgrade_yt(env: &Env, caller: Address, new_wasm_hash: BytesN<32>) {
        let yt = storage::get_yt(env);
        spectra_common::yield_token::YieldTokenClient::new(env, &yt).upgrade(&new_wasm_hash);
    }

    // -----------------------------------------------------------------------
    // Rewards — role-gated proxy install + harvest. Same surface as the
    // wrapper's rewards proxy.
    // -----------------------------------------------------------------------

    /// The installed rewards proxy, or `None` when unset.
    fn rewards_proxy(env: &Env) -> Option<Address> {
        storage::try_get_rewards_proxy(env)
    }

    /// Install, replace, or clear the rewards proxy. Requires `RWD_PROX`.
    #[only_role(caller, "RWD_PROX")]
    fn set_rewards_proxy(env: &Env, caller: Address, proxy: Option<Address>) {
        let old = storage::try_get_rewards_proxy(env);
        match &proxy {
            Some(addr) => storage::set_rewards_proxy(env, addr),
            None => storage::remove_rewards_proxy(env),
        }
        crate::events::RewardsProxyChange { old, new: proxy }.publish(env);
    }

    /// Claim rewards through the installed proxy. Requires `RWD_HARV`.
    /// Soroban has no `delegatecall`, so like the wrapper this is a dynamic
    /// cross-contract call: the caller names the proxy entry point
    /// (`function`) and passes its `args`.
    #[only_role(caller, "RWD_HARV")]
    #[when_not_paused]
    fn claim_rewards(env: &Env, caller: Address, function: Symbol, args: Vec<Val>) -> Val {
        let proxy = match storage::try_get_rewards_proxy(env) {
            Some(p) => p,
            None => panic_with_error!(env, PrincipalTokenError::NoRewardsProxy),
        };
        let result = env.invoke_contract::<Val>(&proxy, &function, args);
        crate::events::RewardsClaimed { caller }.publish(env);
        result
    }

    /// Withdraw IBT with a maximum-share guard.
    #[when_not_paused]
    fn withdraw_ibt_with_max_shares(
        env: &Env,
        caller: Address,
        ibts: i128,
        receiver: Address,
        owner: Address,
        max_shares: i128,
    ) -> i128 {
        let shares = Self::withdraw_ibt(env, caller, ibts, receiver, owner);
        if shares > max_shares {
            panic_with_error!(env, PrincipalTokenError::SlippageProtectionFailed);
        }
        shares
    }

    // -----------------------------------------------------------------------
    // Vault views — pure reads, no auth, no TTL bump.
    // -----------------------------------------------------------------------

    /// Underlying value of PT's IBT holdings (preview redeem on PT's IBT balance).
    fn total_assets(env: &Env) -> i128 {
        let ibt = storage::get_ibt(env);
        let pt = env.current_contract_address();
        let pt_ibt_balance = soroban_sdk::token::Client::new(env, &ibt).balance(&pt);
        FungibleVaultClient::new(env, &ibt).preview_redeem(&pt_ibt_balance)
    }

    /// Convert underlying assets to PT shares using current rates.
    fn convert_to_principal(env: &Env, underlying: i128) -> i128 {
        let ibts =
            FungibleVaultClient::new(env, &storage::get_ibt(env)).preview_deposit(&underlying);
        _convert_ibts_to_shares(env, ibts, Rounding::Floor)
    }

    /// Convert PT shares to underlying assets using current rates.
    fn convert_to_underlying(env: &Env, principal: i128) -> i128 {
        let ibts = _convert_shares_to_ibts(env, principal, Rounding::Floor);
        FungibleVaultClient::new(env, &storage::get_ibt(env)).preview_redeem(&ibts)
    }

    /// Preview deposit with the headline tokenization fee (no user discount).
    #[when_not_paused]
    fn preview_deposit(env: &Env, assets: i128) -> i128 {
        let ibts = FungibleVaultClient::new(env, &storage::get_ibt(env)).preview_deposit(&assets);
        Self::preview_deposit_ibt(env, ibts)
    }

    /// Preview IBT deposit with the headline tokenization fee (no user discount).
    #[when_not_paused]
    fn preview_deposit_ibt(env: &Env, ibts: i128) -> i128 {
        if env.ledger().timestamp() >= storage::get_expiry(env) {
            panic_with_error!(env, PrincipalTokenError::Expired);
        }
        let fee = compute_tokenization_fee_base(env, ibts);
        _convert_ibts_to_shares_preview(env, ibts - fee)
    }

    /// Preview deposit for a specific user with their fee reduction applied.
    #[when_not_paused]
    fn preview_deposit_for(env: &Env, assets: i128, user: Address) -> i128 {
        let ibts = FungibleVaultClient::new(env, &storage::get_ibt(env)).preview_deposit(&assets);
        Self::preview_deposit_ibt_for(env, ibts, user)
    }

    /// Preview IBT deposit for a specific user with their fee reduction applied.
    #[when_not_paused]
    fn preview_deposit_ibt_for(env: &Env, ibts: i128, user: Address) -> i128 {
        if env.ledger().timestamp() >= storage::get_expiry(env) {
            panic_with_error!(env, PrincipalTokenError::Expired);
        }
        let fee = compute_tokenization_fee(env, ibts, &user);
        _convert_ibts_to_shares_preview(env, ibts - fee)
    }

    /// Preview redeem of PT shares into underlying assets.
    #[when_not_paused]
    fn preview_redeem(env: &Env, shares: i128) -> i128 {
        let ibts = Self::preview_redeem_for_ibt(env, shares);
        FungibleVaultClient::new(env, &storage::get_ibt(env)).preview_redeem(&ibts)
    }

    /// Preview redeem of PT shares into IBT shares.
    #[when_not_paused]
    fn preview_redeem_for_ibt(env: &Env, shares: i128) -> i128 {
        _convert_shares_to_ibts(env, shares, Rounding::Floor)
    }

    /// Preview withdraw of underlying assets into required PT shares.
    #[when_not_paused]
    fn preview_withdraw(env: &Env, assets: i128) -> i128 {
        let ibts = FungibleVaultClient::new(env, &storage::get_ibt(env)).preview_withdraw(&assets);
        Self::preview_withdraw_ibt(env, ibts)
    }

    /// Preview withdraw of IBT shares into required PT shares.
    #[when_not_paused]
    fn preview_withdraw_ibt(env: &Env, ibts: i128) -> i128 {
        _convert_ibts_to_shares(env, ibts, Rounding::Ceil)
    }

    /// Max depositable assets for `receiver`. Returns 0 when paused or expired.
    fn max_deposit(env: &Env, _receiver: Address) -> i128 {
        if pausable::paused(env) || env.ledger().timestamp() >= storage::get_expiry(env) {
            0
        } else {
            i128::MAX
        }
    }

    /// Max redeemable shares for `owner`. Returns 0 when paused.
    fn max_redeem(env: &Env, owner: Address) -> i128 {
        if pausable::paused(env) {
            0
        } else {
            _max_burnable(env, &owner)
        }
    }

    /// Max withdrawable underlying for `owner`. Returns 0 when paused.
    fn max_withdraw(env: &Env, owner: Address) -> i128 {
        if pausable::paused(env) {
            0
        } else {
            Self::convert_to_underlying(env, _max_burnable(env, &owner))
        }
    }

    /// Max withdrawable IBT shares for `owner`. Returns 0 when paused.
    fn max_withdraw_ibt(env: &Env, owner: Address) -> i128 {
        if pausable::paused(env) {
            0
        } else {
            _convert_shares_to_ibts(env, _max_burnable(env, &owner), Rounding::Floor)
        }
    }

    /// Return the current tokenization fee rate from the registry.
    fn get_tokenization_fee(env: &Env) -> i128 {
        let registry_addr = storage::get_registry(env);
        spectra_common::registry::RegistryClient::new(env, &registry_addr).get_tokenization_fee()
    }

    /// Return current accumulated yield in IBT for `user`, net of the yield fee.
    fn get_current_yield_of_user_in_ibt(env: &Env, user: Address) -> i128 {
        let yt_balance = read_yt_actual_balance(env, &user);
        let data = storage::get_user_yield(env, &user);
        let (pt_rate, ibt_rate) = _get_pt_and_ibt_rates(env, false);
        if data.ibt_rate == 0 {
            return data.yield_in_ibt;
        }
        let gross = compute_yield(
            env,
            yt_balance,
            data.yield_in_ibt,
            data.ibt_rate,
            ibt_rate,
            data.pt_rate,
            pt_rate,
        );
        gross - compute_yield_fee(env, gross)
    }

    // -----------------------------------------------------------------------
    // Yield claim
    // -----------------------------------------------------------------------

    /// Claim accumulated yield as underlying. Settles yield, applies the yield fee,
    /// redeems IBT, and transfers underlying to `receiver`.
    #[when_not_paused]
    fn claim_yield(env: &Env, caller: Address, receiver: Address, min_assets: i128) -> i128 {
        caller.require_auth();
        let user_yt_balance = read_yt_actual_balance(env, &caller);
        let yield_in_ibt = _claim_yield(env, &caller, user_yt_balance);
        crate::events::YieldClaimed {
            user: caller,
            receiver: receiver.clone(),
            yield_in_ibt,
        }
        .publish(env);

        let yield_in_asset = if yield_in_ibt != 0 {
            let ibt_addr = storage::get_ibt(env);
            let pt = env.current_contract_address();
            FungibleVaultClient::new(env, &ibt_addr).redeem(&yield_in_ibt, &receiver, &pt, &pt)
        } else {
            0
        };
        if yield_in_asset < min_assets {
            panic_with_error!(env, PrincipalTokenError::SlippageProtectionFailed);
        }
        yield_in_asset
    }

    /// Claim accumulated yield as IBT shares. Settles yield, applies the yield fee,
    /// and transfers IBT to `receiver`.
    #[when_not_paused]
    fn claim_yield_in_ibt(env: &Env, caller: Address, receiver: Address, min_ibts: i128) -> i128 {
        caller.require_auth();
        let user_yt_balance = read_yt_actual_balance(env, &caller);
        let yield_in_ibt = _claim_yield(env, &caller, user_yt_balance);
        crate::events::YieldClaimed {
            user: caller,
            receiver: receiver.clone(),
            yield_in_ibt,
        }
        .publish(env);

        if yield_in_ibt < min_ibts {
            panic_with_error!(env, PrincipalTokenError::SlippageProtectionFailed);
        }
        if yield_in_ibt != 0 {
            let ibt_addr = storage::get_ibt(env);
            soroban_sdk::token::Client::new(env, &ibt_addr).transfer(
                &env.current_contract_address(),
                &receiver,
                &yield_in_ibt,
            );
        }
        yield_in_ibt
    }

    // ── Admin rotation (2-step, delegated to stellar-access) ──────────

    fn transfer_admin_role(env: &Env, new_admin: Address, live_until_ledger: u32) {
        access_control::transfer_admin_role(env, &new_admin, live_until_ledger);
    }

    fn accept_admin_transfer(env: &Env) {
        access_control::accept_admin_transfer(env);
    }
}

/// Read the paired YT balance without expiry gating.
fn read_yt_actual_balance(env: &Env, user: &Address) -> i128 {
    let yt_addr = storage::get_yt(env);
    YieldTokenClient::new(env, &yt_addr).actual_balance_of(user)
}

/// Settle and claim yield for a user, returning net IBT after fees.
fn _claim_yield(env: &Env, caller: &Address, user_yt_balance: i128) -> i128 {
    let (mut yield_in_ibt, _, _) = update_yield_inner(env, caller, user_yt_balance);
    if yield_in_ibt == 0 {
        return 0;
    }
    let mut data = storage::get_user_yield(env, caller);
    data.yield_in_ibt = 0;
    storage::set_user_yield(env, caller, &data);

    let yield_fee = compute_yield_fee(env, yield_in_ibt);
    _update_fees(env, yield_fee);
    yield_in_ibt -= yield_fee;
    yield_in_ibt
}

/// Settle yield for a user given their pre-op YT balance.
/// Settle yield for `user` and return `(yield_in_ibt, pt_rate, ibt_rate)`. The
/// rates are returned so callers that already triggered this settlement can
/// reuse them instead of re-reading the vault (see `_deposit_ibt`).
fn update_yield_inner(env: &Env, user: &Address, user_yt_balance: i128) -> (i128, i128, i128) {
    let (pt_rate, ibt_rate) = _update_pt_and_ibt_rates(env);
    let yield_in_ibt = settle_user_yield(env, user, user_yt_balance, pt_rate, ibt_rate);
    (yield_in_ibt, pt_rate, ibt_rate)
}

/// Settle yield for `user` against already-resolved `(pt_rate, ibt_rate)`.
/// Split out of `update_yield_inner` so a batched caller (`before_yt_transfer`)
/// resolves the rates once and settles both sides with them. The two
/// settlements run in the same transaction at the same ledger with no vault
/// mutation between them, so the rate is identical either way — but resolving
/// it once avoids a duplicate IBT-rate read, each of which instantiates the IBT
/// wrapper and its backing vault.
fn settle_user_yield(
    env: &Env,
    user: &Address,
    user_yt_balance: i128,
    pt_rate: i128,
    ibt_rate: i128,
) -> i128 {
    let mut data = storage::get_user_yield(env, user);
    let old_ibt_rate = data.ibt_rate;
    let old_pt_rate = data.pt_rate;

    if data.ibt_rate != ibt_rate {
        data.ibt_rate = ibt_rate;
    }
    if data.pt_rate != pt_rate {
        data.pt_rate = pt_rate;
    }

    if old_ibt_rate == 0 {
        // First interaction — seed only, no yield.
        storage::set_user_yield(env, user, &data);
        return 0;
    }

    let updated = compute_yield(
        env,
        user_yt_balance,
        data.yield_in_ibt,
        old_ibt_rate,
        ibt_rate,
        old_pt_rate,
        pt_rate,
    );
    data.yield_in_ibt = updated;
    storage::set_user_yield(env, user, &data);

    crate::events::YieldUpdated {
        user: user.clone(),
        yield_in_ibt: updated,
    }
    .publish(env);
    updated
}

// ---------------------------------------------------------------------------
// compute_yield
// ---------------------------------------------------------------------------

/// Compute yield changes between old and new PT/IBT rates.
fn compute_yield(
    env: &Env,
    user_yt_balance: i128,
    current_yield_in_ibt: i128,
    old_ibt_rate: i128,
    ibt_rate: i128,
    old_pt_rate: i128,
    pt_rate: i128,
) -> i128 {
    if user_yt_balance < 0 || current_yield_in_ibt < 0 {
        panic_with_error!(env, PrincipalTokenError::NegativeAmount);
    }
    if old_ibt_rate <= 0 || old_pt_rate <= 0 {
        panic_with_error!(env, PrincipalTokenError::RateError);
    }
    if old_pt_rate == pt_rate && ibt_rate == old_ibt_rate {
        return current_yield_in_ibt;
    }

    let yt_decimals = storage::get_ibt_decimals(env);
    let underlying_decimals = storage::get_underlying_decimals(env);
    let user_yt_balance_ray = ray_math::to_ray(user_yt_balance, yt_decimals);

    // ibt_of_pt_ray = user_yt_balance_ray * old_pt_rate / old_ibt_rate.
    // Single `mul_div` flow: the Ray
    // scale on `old_pt_rate` and `old_ibt_rate` cancels naturally — no need
    // to detour via RAY intermediate, which would introduce a second Floor
    // rounding step. `mul_div_wide` uses an I256 intermediate so the
    // un-divided product can overflow i128 safely.
    let ibt_of_pt_ray = mul_div_wide(
        env,
        user_yt_balance_ray,
        old_pt_rate,
        old_ibt_rate,
        Rounding::Floor,
    );

    let new_yield_in_ibt_ray;

    if old_pt_rate == pt_rate && ibt_rate > old_ibt_rate {
        // Case 1: only positive yield
        new_yield_in_ibt_ray = mul_div_wide(
            env,
            ibt_of_pt_ray,
            ibt_rate - old_ibt_rate,
            ibt_rate,
            Rounding::Floor,
        );
    } else if old_pt_rate > pt_rate {
        // Case 2: PT depeg
        let actual_negative_yield_in_asset_ray = mul_div_wide(
            env,
            user_yt_balance_ray,
            old_pt_rate - pt_rate,
            RAY,
            Rounding::Floor,
        );

        let mut yield_in_asset_ray;
        if ibt_rate >= old_ibt_rate {
            // Case 2a: IBT gained while PT depegged
            let ibt_gain_ray = mul_div_wide(
                env,
                ibt_of_pt_ray,
                ibt_rate - old_ibt_rate,
                RAY,
                Rounding::Floor,
            );
            yield_in_asset_ray = actual_negative_yield_in_asset_ray + ibt_gain_ray;
        } else {
            // Case 2b: IBT lost too — protocol-favouring rounding
            let expected_negative_yield_in_asset_ray = mul_div_wide(
                env,
                ibt_of_pt_ray,
                old_ibt_rate - ibt_rate,
                RAY,
                Rounding::Ceil,
            );
            yield_in_asset_ray =
                if expected_negative_yield_in_asset_ray > actual_negative_yield_in_asset_ray {
                    0
                } else {
                    actual_negative_yield_in_asset_ray - expected_negative_yield_in_asset_ray
                };
            // SAFETY BOUND: dust < SAFETY_BOUND in underlying token-decimals → 0
            if ray_math::from_ray(yield_in_asset_ray, underlying_decimals) < SAFETY_BOUND {
                yield_in_asset_ray = 0;
            }
        }

        // Guard zero rates the same way the other rate-using helpers do.
        if ibt_rate == 0 {
            panic_with_error!(env, PrincipalTokenError::RateError);
        }
        new_yield_in_ibt_ray =
            mul_div_wide(env, yield_in_asset_ray, RAY, ibt_rate, Rounding::Floor);
    } else {
        // Case 3: PT rate increased without depeg — invalid
        panic_with_error!(env, PrincipalTokenError::RateError);
    }

    current_yield_in_ibt + ray_math::from_ray(new_yield_in_ibt_ray, yt_decimals)
}

// ---------------------------------------------------------------------------
// Internal rate helpers
// ---------------------------------------------------------------------------

/// Compute current PT and IBT rates without mutating storage.
fn _get_current_pt_and_ibt_rates(env: &Env, round_up_pt_rate: bool) -> (i128, i128) {
    let ibt = storage::get_ibt(env);
    let ibt_client = FungibleVaultClient::new(env, &ibt);

    // Short-circuit by branch: `total_supply` is only consulted to confirm
    // the insolvency case (assets wiped while shares remain), and
    // `preview_redeem` is skipped entirely in that case. When the vault holds
    // assets — the hot path — only `total_assets` + `preview_redeem` are read;
    // when assets are gone, only `total_assets` + `total_supply`. Each is a
    // cross-contract (and Blend-touching) read, so the previous unconditional
    // trio meant one extra vault instantiation on every rate refresh.
    let total_assets = ibt_client.total_assets();
    let ibt_rate = if total_assets == 0 && FungibleTokenClient::new(env, &ibt).total_supply() != 0 {
        0
    } else {
        let ibt_unit = storage::get_ibt_unit(env);
        let underlying_decimals = storage::get_underlying_decimals(env);
        ray_math::to_ray(ibt_client.preview_redeem(&ibt_unit), underlying_decimals)
    };

    let stored_ibt_rate = storage::get_ibt_rate(env);
    let stored_pt_rate = storage::get_pt_rate(env);
    let pt_rate = if ibt_rate < stored_ibt_rate {
        let rounding = if round_up_pt_rate {
            Rounding::Ceil
        } else {
            Rounding::Floor
        };
        mul_div_wide(env, stored_pt_rate, ibt_rate, stored_ibt_rate, rounding)
    } else {
        stored_pt_rate
    };
    (pt_rate, ibt_rate)
}

/// Return frozen rates after expiry or live rates otherwise.
fn _get_pt_and_ibt_rates(env: &Env, round_up_pt_rate: bool) -> (i128, i128) {
    if storage::get_rates_at_expiry_stored(env) {
        (storage::get_pt_rate(env), storage::get_ibt_rate(env))
    } else {
        _get_current_pt_and_ibt_rates(env, round_up_pt_rate)
    }
}

/// Update and persist rates, freezing them when past expiry.
fn _update_pt_and_ibt_rates(env: &Env) -> (i128, i128) {
    let expiry = storage::get_expiry(env);
    let now = env.ledger().timestamp();
    if now >= expiry && !storage::get_rates_at_expiry_stored(env) {
        store_rates_at_expiry_inner(env);
    }
    let (pt_rate, ibt_rate) = _get_pt_and_ibt_rates(env, false);
    if now < expiry {
        let stored_ibt_rate = storage::get_ibt_rate(env);
        let stored_pt_rate = storage::get_pt_rate(env);
        if ibt_rate != stored_ibt_rate {
            storage::set_ibt_rate(env, ibt_rate);
        }
        if pt_rate != stored_pt_rate {
            storage::set_pt_rate(env, pt_rate);
        }
    }
    (pt_rate, ibt_rate)
}

/// Freeze rates at expiry and emit the rates-stored event.
fn store_rates_at_expiry_inner(env: &Env) {
    storage::set_rates_at_expiry_stored(env);
    let (pt_rate, ibt_rate) = _get_current_pt_and_ibt_rates(env, false);
    storage::set_pt_rate(env, pt_rate);
    storage::set_ibt_rate(env, ibt_rate);
    crate::events::RatesStoredAtExpiry { ibt_rate, pt_rate }.publish(env);
}

// ---------------------------------------------------------------------------
// Share/IBT conversion — Ray cancels because pt_rate and
// ibt_rate share the same Ray scale; result naturally falls in token decimals.
// ---------------------------------------------------------------------------

fn _convert_shares_to_ibts(env: &Env, shares: i128, rounding: Rounding) -> i128 {
    let (pt_rate, ibt_rate) = _get_pt_and_ibt_rates(env, false);
    _convert_shares_to_ibts_with(env, shares, rounding, pt_rate, ibt_rate)
}

/// `_convert_shares_to_ibts` with caller-supplied rates, to avoid re-reading the
/// vault when the caller already obtained the current rates (see `redeem`).
fn _convert_shares_to_ibts_with(
    env: &Env,
    shares: i128,
    rounding: Rounding,
    pt_rate: i128,
    ibt_rate: i128,
) -> i128 {
    if ibt_rate == 0 {
        panic_with_error!(env, PrincipalTokenError::RateError);
    }
    mul_div_wide(env, shares, pt_rate, ibt_rate, rounding)
}

fn _convert_ibts_to_shares(env: &Env, ibts: i128, rounding: Rounding) -> i128 {
    let (pt_rate, ibt_rate) = _get_pt_and_ibt_rates(env, false);
    _convert_ibts_to_shares_with(env, ibts, rounding, pt_rate, ibt_rate)
}

/// `_convert_ibts_to_shares` with caller-supplied rates, to avoid re-reading the
/// vault when the caller already obtained the current rates (see `_deposit_ibt`).
fn _convert_ibts_to_shares_with(
    env: &Env,
    ibts: i128,
    rounding: Rounding,
    pt_rate: i128,
    ibt_rate: i128,
) -> i128 {
    if pt_rate == 0 {
        panic_with_error!(env, PrincipalTokenError::RateError);
    }
    mul_div_wide(env, ibts, ibt_rate, pt_rate, rounding)
}

fn _convert_ibts_to_shares_preview(env: &Env, ibts: i128) -> i128 {
    // To round shares up, the PT rate must round down — `_get_pt_and_ibt_rates(true)`
    // returns a PT rate rounded UP (Ceil), so dividing by it rounds the share count DOWN.
    // Keep Floor on the final mulDiv for preview parity.
    let (pt_rate, ibt_rate) = _get_pt_and_ibt_rates(env, true);
    if pt_rate == 0 {
        panic_with_error!(env, PrincipalTokenError::RateError);
    }
    mul_div_wide(env, ibts, ibt_rate, pt_rate, Rounding::Floor)
}

// ---------------------------------------------------------------------------
// Tokenization fee — port of PrincipalTokenUtil.computeTokenizationFee.
// Inlined for the same reasons as compute_yield: no Soroban delegatecall, and
// avoiding a cross-contract call inside vault settlement.
// ---------------------------------------------------------------------------

/// Headline tokenization fee for `ibts`, with no per-user discount applied.
/// Used by preview paths (`preview_deposit*`) where the actual depositor isn't
/// a function parameter — see Refs #50 for the broader UX discussion. Real
/// deposits go through `compute_tokenization_fee` with the genuine caller.
fn compute_tokenization_fee_base(env: &Env, ibts: i128) -> i128 {
    let registry_addr = storage::get_registry(env);
    let tokenization_fee =
        spectra_common::registry::RegistryClient::new(env, &registry_addr).get_tokenization_fee();
    if tokenization_fee == 0 {
        return 0;
    }
    let fee_divisor: i128 = spectra_common::constants::FEE_DIVISOR;
    if tokenization_fee < 0 || tokenization_fee >= fee_divisor {
        panic_with_error!(env, PrincipalTokenError::InvalidTokenizationFee);
    }
    mul_div_wide(env, ibts, tokenization_fee, fee_divisor, Rounding::Ceil)
}

fn compute_tokenization_fee(env: &Env, ibts: i128, user: &Address) -> i128 {
    let base = compute_tokenization_fee_base(env, ibts);
    if base == 0 {
        return 0;
    }
    let registry_addr = storage::get_registry(env);
    let pt = env.current_contract_address();
    let fee_reduction = spectra_common::registry::RegistryClient::new(env, &registry_addr)
        .get_fee_reduction(&pt, user);
    if fee_reduction == 0 {
        return base;
    }
    let fee_divisor: i128 = spectra_common::constants::FEE_DIVISOR;
    if fee_reduction < 0 || fee_reduction > fee_divisor {
        panic_with_error!(env, PrincipalTokenError::InvalidFeeReduction);
    }
    mul_div_wide(
        env,
        base,
        fee_divisor - fee_reduction,
        fee_divisor,
        Rounding::Ceil,
    )
}

/// Compute the yield fee for a gross IBT yield amount.
fn compute_yield_fee(env: &Env, gross_yield_in_ibt: i128) -> i128 {
    let registry_addr = storage::get_registry(env);
    let yield_fee_rate =
        spectra_common::registry::RegistryClient::new(env, &registry_addr).get_yield_fee();
    if yield_fee_rate == 0 {
        return 0;
    }
    mul_div_wide(
        env,
        gross_yield_in_ibt,
        yield_fee_rate,
        spectra_common::constants::FEE_DIVISOR,
        Rounding::Ceil,
    )
}

/// Increment both fee accumulators by `fees_in_ibt`.
fn _update_fees(env: &Env, fees_in_ibt: i128) {
    if fees_in_ibt == 0 {
        return;
    }
    let unclaimed = storage::get_unclaimed_fees_ibt(env);
    storage::set_unclaimed_fees_ibt(env, unclaimed + fees_in_ibt);
    let total = storage::get_total_fees_ibt(env);
    storage::set_total_fees_ibt(env, total + fees_in_ibt);
}

// ---------------------------------------------------------------------------
// Vault internals
// ---------------------------------------------------------------------------

/// Return the max burnable PT shares for an owner (pre/post-expiry).
fn _max_burnable(env: &Env, owner: &Address) -> i128 {
    let pt_balance = Base::balance(env, owner);
    if env.ledger().timestamp() >= storage::get_expiry(env) {
        pt_balance
    } else {
        let yt_balance = read_yt_actual_balance(env, owner);
        if pt_balance < yt_balance {
            pt_balance
        } else {
            yt_balance
        }
    }
}

/// Pre-withdraw checks: settle yield and cap assets against max withdraw.
fn _before_withdraw(env: &Env, assets: i128, owner: &Address) {
    if env.ledger().timestamp() >= storage::get_expiry(env) {
        if !storage::get_rates_at_expiry_stored(env) {
            store_rates_at_expiry_inner(env);
        }
    } else {
        let yt_balance = read_yt_actual_balance(env, owner);
        update_yield_inner(env, owner, yt_balance);
    }
    let max_assets = {
        let ibts = _convert_shares_to_ibts(env, _max_burnable(env, owner), Rounding::Floor);
        FungibleVaultClient::new(env, &storage::get_ibt(env)).preview_redeem(&ibts)
    };
    if assets > max_assets {
        panic_with_error!(env, PrincipalTokenError::InsufficientBalance);
    }
}

/// Burn shares for a withdraw using pre-withdraw rate snapshots.
fn _burn_shares_for_withdraw(
    env: &Env,
    caller: &Address,
    ibts: i128,
    receiver: &Address,
    owner: &Address,
    pt_rate: i128,
    ibt_rate: i128,
) -> i128 {
    if pt_rate == 0 {
        panic_with_error!(env, PrincipalTokenError::RateError);
    }
    let shares = mul_div_wide(env, ibts, ibt_rate, pt_rate, Rounding::Ceil);
    if env.ledger().timestamp() < storage::get_expiry(env) {
        let yt_addr = storage::get_yt(env);
        YieldTokenClient::new(env, &yt_addr).burn_without_yield_update(owner, caller, &shares);
    }
    if caller == owner {
        Base::burn(env, owner, shares);
    } else {
        Base::burn_from(env, caller, owner, shares);
    }
    crate::events::Redeem {
        owner: owner.clone(),
        receiver: receiver.clone(),
        shares,
    }
    .publish(env);
    shares
}

/// Pre-redeem checks and burns, settling yield and enforcing max burnable.
/// Pre-redeem checks/burns; returns `(pt_rate, ibt_rate)` so the caller can
/// convert shares→IBT without re-reading the vault. Pre-expiry the rates come
/// from the `update_yield_inner` settlement; post-expiry they are the frozen
/// stored rates — in both cases identical to a fresh `_get_pt_and_ibt_rates`.
fn _before_redeem(env: &Env, caller: &Address, shares: i128, owner: &Address) -> (i128, i128) {
    if shares > _max_burnable(env, owner) {
        panic_with_error!(env, PrincipalTokenError::InsufficientBalance);
    }
    let rates = if env.ledger().timestamp() >= storage::get_expiry(env) {
        if !storage::get_rates_at_expiry_stored(env) {
            store_rates_at_expiry_inner(env);
        }
        (storage::get_pt_rate(env), storage::get_ibt_rate(env))
    } else {
        let yt_balance = read_yt_actual_balance(env, owner);
        let (_, pt_rate, ibt_rate) = update_yield_inner(env, owner, yt_balance);
        let yt_addr = storage::get_yt(env);
        YieldTokenClient::new(env, &yt_addr).burn_without_yield_update(owner, caller, &shares);
        (pt_rate, ibt_rate)
    };
    if caller == owner {
        Base::burn(env, owner, shares);
    } else {
        Base::burn_from(env, caller, owner, shares);
    }
    rates
}

/// Settle yield, apply tokenization fee, and mint PT/YT for an IBT deposit.
fn _deposit_ibt(
    env: &Env,
    caller: &Address,
    ibts: i128,
    pt_receiver: &Address,
    yt_receiver: &Address,
) -> i128 {
    if env.ledger().timestamp() >= storage::get_expiry(env) {
        panic_with_error!(env, PrincipalTokenError::Expired);
    }

    let yt_balance = read_yt_actual_balance(env, yt_receiver);
    // `update_yield_inner` computes and persists the current PT/IBT rates from
    // the vault. We are guaranteed pre-expiry here (checked above), so those
    // persisted rates are exactly what a fresh `_get_pt_and_ibt_rates` would
    // return — reuse them for the conversion instead of re-reading the vault
    // (3 cross-contract reads), which on a heavy IBT (e.g. a Blend pool or an
    // Upshift vault) is a dominant per-tx memory cost.
    let (_, pt_rate, ibt_rate) = update_yield_inner(env, yt_receiver, yt_balance);

    let fee = compute_tokenization_fee(env, ibts, pt_receiver);
    _update_fees(env, fee);
    let net_ibts = ibts - fee;
    let shares = _convert_ibts_to_shares_with(env, net_ibts, Rounding::Floor, pt_rate, ibt_rate);
    if shares == 0 {
        panic_with_error!(env, PrincipalTokenError::RateError);
    }

    Base::mint(env, pt_receiver, shares);
    crate::events::PtMinted {
        caller: caller.clone(),
        receiver: pt_receiver.clone(),
        shares,
    }
    .publish(env);
    let yt_addr = storage::get_yt(env);
    YieldTokenClient::new(env, &yt_addr).mint(yt_receiver, &shares);
    shares
}

// ---------------------------------------------------------------------------
// SEP-41 surface. PT is a fungible token whose shares represent
// a claim on the underlying at maturity. Mint and burn are internal to vault
// flows; transfers and approvals use this surface.
// ---------------------------------------------------------------------------
#[contractimpl]
impl FungibleToken for PrincipalTokenContract {
    type ContractType = Base;

    fn balance(e: &Env, account: Address) -> i128 {
        Base::balance(e, &account)
    }
    fn total_supply(e: &Env) -> i128 {
        Base::total_supply(e)
    }
    fn allowance(e: &Env, owner: Address, spender: Address) -> i128 {
        Base::allowance(e, &owner, &spender)
    }
    #[when_not_paused]
    fn transfer(e: &Env, from: Address, to: MuxedAddress, amount: i128) {
        Base::transfer(e, &from, &to, amount);
    }
    #[when_not_paused]
    fn transfer_from(e: &Env, spender: Address, from: Address, to: Address, amount: i128) {
        Base::transfer_from(e, &spender, &from, &to, amount);
    }
    fn approve(e: &Env, owner: Address, spender: Address, amount: i128, live_until_ledger: u32) {
        Base::approve(e, &owner, &spender, amount, live_until_ledger);
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
