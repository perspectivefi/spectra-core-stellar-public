// SPDX-License-Identifier: BUSL-1.1
#![no_std]

//! Blend v2 adapter for [`SpectraFungibleVaultWrapper`]: the wrapper holds a
//! non-transferable bToken position in a Blend pool and emits a transferable
//! share pegged 1:1 to the bToken. `deposit`/`withdraw`/`redeem` are
//! supported; mint/wrap/unwrap panic `NotSupported`.

pub mod errors;
pub mod storage;

#[cfg(test)]
mod test;

#[cfg(test)]
mod fork_test;

use blend_types::{
    BlendPoolClient, REQUEST_SUPPLY, REQUEST_WITHDRAW, Request, Reserve, to_asset_from_b_token,
    to_asset_from_b_token_up, to_b_token_down, to_b_token_up,
};
use soroban_sdk::{
    Address, BytesN, Env, MuxedAddress, String, Symbol, Val, Vec, contract, contractimpl,
    panic_with_error, token, vec,
};
use spectra_common::auth::authorize_pull;
use spectra_common::constants::roles;
use spectra_common::naming;
use spectra_fungible_vault_wrapper::SpectraFungibleVaultWrapper;
use spectra_math::mul_div::Rounding;
use stellar_access::access_control;
use stellar_contract_utils::pausable;
use stellar_macros::when_not_paused;
use stellar_tokens::fungible::{Base, ContractOverrides, FungibleToken};
use stellar_tokens::vault::{Vault, VaultTokenError, emit_deposit, emit_withdraw};

use crate::errors::BlendWrapperError;

#[contract]
pub struct BlendFungibleVaultWrapperContract;

#[contractimpl]
impl BlendFungibleVaultWrapperContract {
    /// Wrap a Blend position in `pool` on the reserve `asset`. Metadata is
    /// derived from the underlying `asset`; `admin` receives every role.
    pub fn __constructor(e: &Env, pool: Address, asset: Address, admin: Address) {
        // panics inside the pool when `asset` is not a reserve
        BlendPoolClient::new(e, &pool).get_reserve(&asset);
        storage::set_pool(e, &pool);
        Vault::set_asset(e, asset.clone());

        let t = token::Client::new(e, &asset);
        let name = naming::gen_wrapper_name(e, &t.name());
        let symbol = naming::gen_wrapper_symbol(e, &t.symbol());
        Base::set_metadata(e, Vault::decimals(e), name, symbol);

        access_control::set_admin(e, &admin);
        access_control::grant_role_no_auth(e, &admin, &roles::PAUSER, &admin);
        access_control::grant_role_no_auth(e, &admin, &roles::UPGRADE, &admin);
        access_control::grant_role_no_auth(e, &admin, &roles::REWARDS_PROXY_SETTER, &admin);
        access_control::grant_role_no_auth(e, &admin, &roles::REWARDS_HARVESTER, &admin);
    }

    /// The Blend pool this wrapper supplies into.
    pub fn pool(e: &Env) -> Address {
        storage::get_pool(e)
    }
}

#[contractimpl(contracttrait)]
impl SpectraFungibleVaultWrapper for BlendFungibleVaultWrapperContract {
    /// 1 share = 1 bToken: the pool's `to_b_token_down`/`to_b_token_up`.
    fn convert_to_shares_with_rounding(e: &Env, assets: i128, rounding: Rounding) -> i128 {
        if assets < 0 {
            panic_with_error!(e, BlendWrapperError::NegativeAmount);
        }
        let b_rate = cached_b_rate(e);
        match rounding {
            Rounding::Floor => to_b_token_down(e, assets, b_rate),
            Rounding::Ceil => to_b_token_up(e, assets, b_rate),
        }
    }

    /// The pool's `to_asset_from_b_token` (and its ceil counterpart).
    fn convert_to_assets_with_rounding(e: &Env, shares: i128, rounding: Rounding) -> i128 {
        if shares < 0 {
            panic_with_error!(e, BlendWrapperError::NegativeAmount);
        }
        let b_rate = cached_b_rate(e);
        match rounding {
            Rounding::Floor => to_asset_from_b_token(e, shares, b_rate),
            Rounding::Ceil => to_asset_from_b_token_up(e, shares, b_rate),
        }
    }

    fn total_assets(e: &Env) -> i128 {
        Self::convert_to_assets(e, Self::total_vault_shares(e))
    }

    /// Blend has no bToken contract; the pool address is exposed via `pool()`.
    fn vault_share(e: &Env) -> Address {
        panic_with_error!(e, BlendWrapperError::NotSupported)
    }

    /// The wrapper's bToken position in the pool.
    fn total_vault_shares(e: &Env) -> i128 {
        let client = BlendPoolClient::new(e, &storage::get_pool(e));
        let idx = client.get_reserve(&Vault::query_asset(e)).config.index;
        client
            .get_positions(&e.current_contract_address())
            .supply
            .get(idx)
            .unwrap_or(0)
    }

    fn max_mint(_e: &Env, _receiver: Address) -> i128 {
        0
    }

    /// Supply-cap headroom in underlying, or `0` when paused/disabled.
    fn max_deposit(e: &Env, _receiver: Address) -> i128 {
        if pausable::paused(e) {
            return 0;
        }
        let r = load_reserve(e);
        if !r.config.enabled {
            return 0;
        }
        if r.data.b_rate <= 0 {
            return 0;
        }
        let supplied = to_asset_from_b_token(e, r.data.b_supply, r.data.b_rate);
        (r.config.supply_cap - supplied).max(0)
    }

    /// Owner's shares clamped to serviceable liquidity.
    fn max_redeem(e: &Env, owner: Address) -> i128 {
        if pausable::paused(e) {
            return 0;
        }
        let r = load_reserve(e);
        Base::balance(e, &owner).min(redeemable_btokens(e, &r))
    }

    /// Owner's position clamped to serviceable underlying liquidity.
    fn max_withdraw(e: &Env, owner: Address) -> i128 {
        if pausable::paused(e) {
            return 0;
        }
        let r = load_reserve(e);
        let owner_underlying = to_asset_from_b_token(e, Base::balance(e, &owner), r.data.b_rate);
        let available = to_asset_from_b_token(e, redeemable_btokens(e, &r), r.data.b_rate);
        owner_underlying.min(available)
    }

    fn preview_mint(e: &Env, _shares: i128) -> i128 {
        panic_with_error!(e, BlendWrapperError::NotSupported)
    }

    fn preview_wrap(e: &Env, _vault_shares: i128) -> i128 {
        panic_with_error!(e, BlendWrapperError::NotSupported)
    }

    fn preview_unwrap(e: &Env, _shares: i128) -> i128 {
        panic_with_error!(e, BlendWrapperError::NotSupported)
    }

    /// Pull underlying from `from`, supply it to the pool, mint the bToken
    /// position delta as shares to `receiver`.
    #[when_not_paused]
    fn deposit(e: &Env, assets: i128, receiver: Address, from: Address, operator: Address) -> i128 {
        operator.require_auth();
        if assets < 0 {
            panic_with_error!(e, BlendWrapperError::NegativeAmount);
        }
        let wrapper = e.current_contract_address();
        let asset = Vault::query_asset(e);
        let pool = storage::get_pool(e);
        let client = BlendPoolClient::new(e, &pool);
        // Single reserve read for both share conversion and cache priming.
        let reserve = client.get_reserve(&asset);
        storage::set_cached_b_rate(e, reserve.data.b_rate);
        let shares = to_b_token_down(e, assets, reserve.data.b_rate);
        let idx = reserve.config.index;

        let t = token::Client::new(e, &asset);
        if operator == from {
            t.transfer(&from, &wrapper, &assets);
        } else {
            t.transfer_from(&operator, &from, &wrapper, &assets);
        }

        let before = client.get_positions(&wrapper).supply.get(idx).unwrap_or(0);
        // the pool pulls via transfer(wrapper -> pool) two frames below us
        authorize_pull(e, &asset, &wrapper, &pool, assets);
        let after = client
            .submit(
                &wrapper,
                &wrapper,
                &wrapper,
                &vec![
                    e,
                    Request {
                        request_type: REQUEST_SUPPLY,
                        address: asset.clone(),
                        amount: assets,
                    },
                ],
            )
            .supply
            .get(idx)
            .unwrap_or(0);
        let minted = after - before;
        if minted != shares {
            panic_with_error!(e, BlendWrapperError::BTokenMismatch);
        }

        Base::update(e, None, Some(&receiver), minted);
        emit_deposit(e, &operator, &from, &receiver, assets, minted);
        minted
    }

    /// Burn `shares`, withdraw their floor underlying value from the pool
    /// straight to `receiver`. The pool burns at most `shares` bTokens.
    #[when_not_paused]
    fn redeem(e: &Env, shares: i128, receiver: Address, owner: Address, operator: Address) -> i128 {
        operator.require_auth();
        if shares < 0 {
            panic_with_error!(e, BlendWrapperError::NegativeAmount);
        }
        if shares > Self::max_redeem(e, owner.clone()) {
            panic_with_error!(e, VaultTokenError::VaultExceededMaxRedeem);
        }
        if operator != owner {
            Base::spend_allowance(e, &owner, &operator, shares);
        }

        refresh_b_rate(e);
        let assets = Self::convert_to_assets(e, shares);
        Base::update(e, Some(&owner), None, shares);

        let wrapper = e.current_contract_address();
        BlendPoolClient::new(e, &storage::get_pool(e)).submit(
            &wrapper,
            &wrapper,
            &receiver,
            &vec![
                e,
                Request {
                    request_type: REQUEST_WITHDRAW,
                    address: Vault::query_asset(e),
                    amount: assets,
                },
            ],
        );

        emit_withdraw(e, &operator, &receiver, &owner, assets, shares);
        assets
    }

    fn mint(
        e: &Env,
        _shares: i128,
        _receiver: Address,
        _from: Address,
        _operator: Address,
    ) -> i128 {
        panic_with_error!(e, BlendWrapperError::NotSupported)
    }

    /// Burn `ceil` shares to pay exactly `assets` to `receiver` — Blend's
    /// `Withdraw` request is natively asset-denominated.
    #[when_not_paused]
    fn withdraw(
        e: &Env,
        assets: i128,
        receiver: Address,
        owner: Address,
        operator: Address,
    ) -> i128 {
        operator.require_auth();
        refresh_b_rate(e);
        let shares = Self::preview_withdraw(e, assets);
        if assets > Self::max_withdraw(e, owner.clone()) {
            panic_with_error!(e, VaultTokenError::VaultExceededMaxWithdraw);
        }
        if operator != owner {
            Base::spend_allowance(e, &owner, &operator, shares);
        }
        Base::update(e, Some(&owner), None, shares);

        let wrapper = e.current_contract_address();
        BlendPoolClient::new(e, &storage::get_pool(e)).submit(
            &wrapper,
            &wrapper,
            &receiver,
            &vec![
                e,
                Request {
                    request_type: REQUEST_WITHDRAW,
                    address: Vault::query_asset(e),
                    amount: assets,
                },
            ],
        );

        emit_withdraw(e, &operator, &receiver, &owner, assets, shares);
        shares
    }

    fn wrap(e: &Env, _vault_shares: i128, _receiver: Address, _caller: Address) -> i128 {
        panic_with_error!(e, BlendWrapperError::NotSupported)
    }

    fn unwrap(
        e: &Env,
        _shares: i128,
        _receiver: Address,
        _owner: Address,
        _operator: Address,
    ) -> i128 {
        panic_with_error!(e, BlendWrapperError::NotSupported)
    }
}

/// Ceiling-rounded total borrowed underlying.
fn borrowed_underlying(e: &Env, r: &Reserve) -> i128 {
    to_asset_from_b_token_up(e, r.data.d_supply, r.data.d_rate)
}

/// bTokens the pool can service for an exit while keeping utilization strictly
/// below 100%. The buffer lives in bToken space: Blend measures utilization as
/// floor(remaining_b_supply × b_rate) against ceil-rounded liabilities, so a
/// fixed 1-share/1-unit buffer is insufficient once b_rate < 1. To leave at
/// least `liabilities + 1` underlying supplied, keep at least
/// `ceil((liabilities + 1) × 1e12 / b_rate)` bTokens.
fn redeemable_btokens(e: &Env, r: &Reserve) -> i128 {
    // Blend clamps b_rate to 0 after bad debt (pool/src/pool/user.rs); a zero or
    // negative rate would divide by zero in the bToken conversion below.
    if r.data.b_rate <= 0 {
        return 0;
    }
    let liabilities = borrowed_underlying(e, r);
    if liabilities == 0 {
        return r.data.b_supply;
    }
    let min_remaining = to_b_token_up(e, liabilities + 1, r.data.b_rate);
    (r.data.b_supply - min_remaining).max(0)
}

fn load_reserve(e: &Env) -> Reserve {
    BlendPoolClient::new(e, &storage::get_pool(e)).get_reserve(&Vault::query_asset(e))
}

/// `b_rate` from within-transaction cache; pool read on miss.
fn cached_b_rate(e: &Env) -> i128 {
    if let Some(r) = storage::get_cached_b_rate(e) {
        return r;
    }
    let r = load_reserve(e).data.b_rate;
    storage::set_cached_b_rate(e, r);
    r
}

/// Force a fresh pool read of `b_rate`, overwriting the per-timestamp cache.
fn refresh_b_rate(e: &Env) {
    storage::set_cached_b_rate(e, load_reserve(e).data.b_rate);
}

/// SEP-41 via OZ `Vault`; only the transfer pair is overridden, to pause-gate it.
#[contractimpl(contracttrait)]
impl FungibleToken for BlendFungibleVaultWrapperContract {
    type ContractType = Vault;

    /// Share decimals as stored at construction. Blend positions have no
    /// separate share token, so the stored value equals the asset decimals;
    /// the override keeps every wrapper on the same rule.
    fn decimals(e: &Env) -> u32 {
        spectra_fungible_vault_wrapper::share_decimals(e)
    }

    #[when_not_paused]
    fn transfer(e: &Env, from: Address, to: MuxedAddress, amount: i128) {
        <Vault as ContractOverrides>::transfer(e, &from, &to, amount);
    }
    #[when_not_paused]
    fn transfer_from(e: &Env, spender: Address, from: Address, to: Address, amount: i128) {
        <Vault as ContractOverrides>::transfer_from(e, &spender, &from, &to, amount);
    }
}
