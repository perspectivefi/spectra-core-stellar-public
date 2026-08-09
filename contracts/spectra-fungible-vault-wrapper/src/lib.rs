// SPDX-License-Identifier: BUSL-1.1
#![no_std]

//! `SpectraFungibleVaultWrapper`: the abstract base wrapper, the Soroban analog
//! of Solidity's `Spectra4626Wrapper`. It provides the wrap/unwrap path,
//! previews, `total_assets`, and the pausable/rewards/access scaffolding, and
//! leaves the rate conversions and 4626 flows (`deposit`/`mint`/`withdraw`/
//! `redeem`) as abstract seams for a concrete instance to implement. This crate
//! ships no deployable contract; see `blend-fungible-vault-wrapper` for an
//! example instance.

pub mod errors;
pub mod events;
pub mod storage;

#[cfg(test)]
mod test;

use soroban_sdk::{Address, BytesN, Env, Symbol, Val, Vec, contracttrait, panic_with_error, token};
use spectra_common::constants::roles;
use spectra_common::naming;
use spectra_math::mul_div::{Rounding, mul_div_wide};
use stellar_access::access_control;
use stellar_contract_utils::pausable;
use stellar_macros::{only_role, when_not_paused};
use stellar_tokens::fungible::{Base, FungibleToken};
use stellar_tokens::vault::Vault;

use crate::errors::SpectraFungibleVaultError;
use crate::events::{RewardsProxyUpdated, Unwrap, Upgraded, Wrap};

#[contracttrait]
pub trait SpectraFungibleVaultWrapper: FungibleToken<ContractType = Vault> {
    // ── conversions (override points) ──────────────────────────────────────

    fn convert_to_shares(e: &Env, assets: i128) -> i128 {
        Self::convert_to_shares_with_rounding(e, assets, Rounding::Floor)
    }

    fn convert_to_assets(e: &Env, shares: i128) -> i128 {
        Self::convert_to_assets_with_rounding(e, shares, Rounding::Floor)
    }

    /// Rounding-aware asset↔share conversions — the abstract seam, the Soroban
    /// analog of Solidity's virtual `_convertToShares`/`_convertToAssets`. Each
    /// instance defines its rate here (see the Blend adapter); every preview and
    /// `total_assets` routes through this pair.
    fn convert_to_shares_with_rounding(e: &Env, assets: i128, rounding: Rounding) -> i128;

    fn convert_to_assets_with_rounding(e: &Env, shares: i128, rounding: Rounding) -> i128;

    // ── views ──────────────────────────────────────────────────────────────

    /// The underlying asset. Defaults to the wrapped token (`vault_share`).
    fn query_asset(e: &Env) -> Address {
        Vault::query_asset(e)
    }

    fn total_assets(e: &Env) -> i128 {
        Self::convert_to_assets(e, Base::total_supply(e))
    }

    /// The wrapped vault-share token
    fn vault_share(e: &Env) -> Address {
        storage::get_vault_share(e)
    }

    fn total_vault_shares(e: &Env) -> i128 {
        vault_share_balance(e)
    }

    // ── max (0 when paused) ─────────────────────────────────────────────────

    fn max_deposit(e: &Env, receiver: Address) -> i128 {
        if pausable::paused(e) {
            return 0;
        }
        Vault::max_deposit(e, receiver)
    }

    fn max_mint(e: &Env, receiver: Address) -> i128 {
        if pausable::paused(e) {
            return 0;
        }
        Vault::max_mint(e, receiver)
    }

    fn max_withdraw(e: &Env, owner: Address) -> i128 {
        if pausable::paused(e) {
            return 0;
        }
        Self::convert_to_assets(e, Base::balance(e, &owner))
    }

    fn max_redeem(e: &Env, owner: Address) -> i128 {
        if pausable::paused(e) {
            return 0;
        }
        Vault::max_redeem(e, owner)
    }

    // ── preview ────────────────────────────────────

    fn preview_deposit(e: &Env, assets: i128) -> i128 {
        Self::convert_to_shares_with_rounding(e, assets, Rounding::Floor)
    }

    fn preview_mint(e: &Env, shares: i128) -> i128 {
        Self::convert_to_assets_with_rounding(e, shares, Rounding::Ceil)
    }

    fn preview_withdraw(e: &Env, assets: i128) -> i128 {
        Self::convert_to_shares_with_rounding(e, assets, Rounding::Ceil)
    }

    fn preview_redeem(e: &Env, shares: i128) -> i128 {
        Self::convert_to_assets_with_rounding(e, shares, Rounding::Floor)
    }

    fn preview_wrap(e: &Env, vault_shares: i128) -> i128 {
        preview_wrap_internal(e, vault_shares, Rounding::Floor)
    }

    fn preview_unwrap(e: &Env, shares: i128) -> i128 {
        preview_unwrap_internal(e, shares, Rounding::Floor)
    }

    fn deposit(e: &Env, assets: i128, receiver: Address, from: Address, operator: Address) -> i128;

    fn mint(e: &Env, shares: i128, receiver: Address, from: Address, operator: Address) -> i128;

    fn withdraw(
        e: &Env,
        assets: i128,
        receiver: Address,
        owner: Address,
        operator: Address,
    ) -> i128;

    fn redeem(e: &Env, shares: i128, receiver: Address, owner: Address, operator: Address) -> i128;

    // ── wrapping  ─────────────────────────

    #[when_not_paused]
    fn wrap(e: &Env, vault_shares: i128, receiver: Address, caller: Address) -> i128 {
        caller.require_auth();
        let shares = preview_wrap_internal(e, vault_shares, Rounding::Floor);
        token::Client::new(e, &storage::get_vault_share(e)).transfer(
            &caller,
            e.current_contract_address(),
            &vault_shares,
        );
        Base::update(e, None, Some(&receiver), shares);
        Wrap {
            caller,
            receiver,
            vault_shares,
            shares,
        }
        .publish(e);
        shares
    }

    #[when_not_paused]
    fn unwrap(e: &Env, shares: i128, receiver: Address, owner: Address, operator: Address) -> i128 {
        operator.require_auth();
        if operator != owner {
            Base::spend_allowance(e, &owner, &operator, shares);
        }
        let vault_shares = preview_unwrap_internal(e, shares, Rounding::Floor);
        Base::update(e, Some(&owner), None, shares);
        token::Client::new(e, &storage::get_vault_share(e)).transfer(
            &e.current_contract_address(),
            &receiver,
            &vault_shares,
        );
        Unwrap {
            caller: operator,
            receiver,
            owner,
            shares,
            vault_shares,
        }
        .publish(e);
        vault_shares
    }

    /// `wrap` with ERC-5143 slippage protection: reverts when the minted wrapper
    /// shares are below `min_shares`.
    fn wrap_with_min_shares(
        e: &Env,
        vault_shares: i128,
        receiver: Address,
        caller: Address,
        min_shares: i128,
    ) -> i128 {
        let shares = Self::wrap(e, vault_shares, receiver, caller);
        if shares < min_shares {
            panic_with_error!(e, SpectraFungibleVaultError::SlippageProtectionFailed);
        }
        shares
    }

    /// `unwrap` with ERC-5143 slippage protection: reverts when the returned
    /// vault shares are below `min_vault_shares`.
    fn unwrap_with_min_vault_shares(
        e: &Env,
        shares: i128,
        receiver: Address,
        owner: Address,
        operator: Address,
        min_vault_shares: i128,
    ) -> i128 {
        let vault_shares = Self::unwrap(e, shares, receiver, owner, operator);
        if vault_shares < min_vault_shares {
            panic_with_error!(e, SpectraFungibleVaultError::SlippageProtectionFailed);
        }
        vault_shares
    }

    // ── rewards proxy ─────────────────────
    // Soroban has no `delegatecall`, so the Solidity proxy pattern (swappable code
    // running in the wrapper's context) becomes a dynamic cross-contract call: the
    // caller names the proxy entry point (`function`) and passes its `args`.

    /// The rewards proxy, or `None` when unset. See {ISpectra4626Wrapper-rewardsProxy}.
    fn rewards_proxy(e: &Env) -> Option<Address> {
        storage::get_rewards_proxy(e)
    }

    #[only_role(caller, "RWD_PROX")]
    fn set_rewards_proxy(e: &Env, caller: Address, new_rewards_proxy: Option<Address>) {
        let old_rewards_proxy = storage::get_rewards_proxy(e);
        storage::set_rewards_proxy(e, new_rewards_proxy.clone());
        RewardsProxyUpdated {
            old_rewards_proxy,
            new_rewards_proxy,
        }
        .publish(e);
    }

    #[only_role(caller, "RWD_HARV")]
    fn claim_rewards(e: &Env, caller: Address, function: Symbol, args: Vec<Val>) -> Val {
        let proxy = storage::get_rewards_proxy(e)
            .unwrap_or_else(|| panic_with_error!(e, SpectraFungibleVaultError::NoRewardsProxy));
        match e.try_invoke_contract::<Val, soroban_sdk::Error>(&proxy, &function, args) {
            Ok(Ok(v)) => v,
            _ => panic_with_error!(e, SpectraFungibleVaultError::ClaimRewardsFailed),
        }
    }

    // ── pausable (PAUSER) ─────────────────

    /// Pause. Requires `PAUSER`.
    #[only_role(caller, "PAUSER")]
    fn pause(e: &Env, caller: Address) {
        pausable::pause(e);
    }

    /// Unpause. Requires `PAUSER`.
    #[only_role(caller, "PAUSER")]
    fn unpause(e: &Env, caller: Address) {
        pausable::unpause(e);
    }

    /// Whether paused.
    fn paused(e: &Env) -> bool {
        pausable::paused(e)
    }

    // ── upgrade (UPGRADE) ─────────────────

    /// Upgrade the contract Wasm. Requires `UPGRADE`.
    #[only_role(caller, "UPGRADE")]
    fn upgrade(e: &Env, caller: Address, new_wasm_hash: BytesN<32>) {
        e.deployer()
            .update_current_contract_wasm(new_wasm_hash.clone());
        Upgraded {
            caller,
            new_wasm_hash,
        }
        .publish(e);
    }

    // ── role administration ───────────────

    /// Grant `role` to `account`.
    fn grant_role(e: &Env, caller: Address, account: Address, role: Symbol) {
        access_control::grant_role(e, &account, &role, &caller);
        storage::extend_instance_ttl(e);
    }

    /// Revoke `role` from `account`.
    fn revoke_role(e: &Env, caller: Address, account: Address, role: Symbol) {
        access_control::revoke_role(e, &account, &role, &caller);
        storage::extend_instance_ttl(e);
    }

    /// Whether `account` holds `role`.
    fn has_role(e: &Env, account: Address, role: Symbol) -> bool {
        access_control::has_role(e, &account, &role).is_some()
    }

    // ── admin 2-step (stellar-access native) ──────────────────────────────

    /// The current admin (role administrator). `None` if uninitialized.
    fn admin(e: &Env) -> Option<Address> {
        access_control::get_admin(e)
    }

    /// Propose `new_admin` as the next admin. Two-step: the proposed admin
    /// must then call `accept_admin_transfer` before `live_until_ledger`.
    /// Requires the current admin's auth (enforced inside `stellar-access`).
    fn transfer_admin_role(e: &Env, new_admin: Address, live_until_ledger: u32) {
        access_control::transfer_admin_role(e, &new_admin, live_until_ledger);
        storage::extend_instance_ttl(e);
    }

    /// Accept a pending admin role. Requires the proposed admin's auth
    /// (enforced inside `stellar-access`).
    fn accept_admin_transfer(e: &Env) {
        access_control::accept_admin_transfer(e);
        storage::extend_instance_ttl(e);
    }
}

/// Wrap `vault_share` with `asset` as the FungibleVault underlying (pass the
/// same address for both when the wrapped token has no distinct underlying),
/// set the virtual `decimals_offset`, derive name/symbol, and set `admin` as
/// the role administrator holding every role. Rotate individual roles
/// post-init via `grant_role`/`revoke_role`.
pub fn init_base(
    e: &Env,
    asset: Address,
    vault_share: Address,
    decimals_offset: u32,
    admin: Address,
) {
    if storage::is_initialized(e) {
        panic_with_error!(e, SpectraFungibleVaultError::AlreadyInitialized);
    }

    storage::set_vault_share(e, &vault_share);
    Vault::set_asset(e, asset);
    Vault::set_decimals_offset(e, decimals_offset);

    let t = token::Client::new(e, &vault_share);
    let name = naming::gen_wrapper_name(e, &t.name());
    let symbol = naming::gen_wrapper_symbol(e, &t.symbol());
    // decimals follow the vault share, not the asset (Solidity line: decimals())
    Base::set_metadata(e, t.decimals() + decimals_offset, name, symbol);

    access_control::set_admin(e, &admin);
    access_control::grant_role_no_auth(e, &admin, &roles::PAUSER, &admin);
    access_control::grant_role_no_auth(e, &admin, &roles::UPGRADE, &admin);
    access_control::grant_role_no_auth(e, &admin, &roles::REWARDS_PROXY_SETTER, &admin);
    access_control::grant_role_no_auth(e, &admin, &roles::REWARDS_HARVESTER, &admin);
}

// ── internal ratio helpers — the literal Solidity `_previewWrap`/`_previewUnwrap` ──

/// Vault-share balance held by this contract (`IERC20(vaultShare).balanceOf(this)`).
fn vault_share_balance(e: &Env) -> i128 {
    token::Client::new(e, &storage::get_vault_share(e)).balance(&e.current_contract_address())
}

/// `vaultShares * (totalSupply + 10^offset) / (totalVaultShares + 1)`.
pub fn preview_wrap_internal(e: &Env, vault_shares: i128, rounding: Rounding) -> i128 {
    let pow = 10_i128.pow(Vault::get_decimals_offset(e));
    mul_div_wide(
        e,
        vault_shares,
        Base::total_supply(e) + pow,
        vault_share_balance(e) + 1,
        rounding,
    )
}

/// `shares * (totalVaultShares + 1) / (totalSupply + 10^offset)`.
pub fn preview_unwrap_internal(e: &Env, shares: i128, rounding: Rounding) -> i128 {
    let pow = 10_i128.pow(Vault::get_decimals_offset(e));
    mul_div_wide(
        e,
        shares,
        vault_share_balance(e) + 1,
        Base::total_supply(e) + pow,
        rounding,
    )
}
