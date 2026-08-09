// SPDX-License-Identifier: BUSL-1.1
//! Minimal `SpectraFungibleVaultWrapper`-shaped wrapper for LOE multi-
//! denomination tests. Three-token layering:
//!
//!   underlying → [deposit at `deposit_rate`] → wrapper_share
//!   vault_share → [wrap   at `wrap_rate`]    → wrapper_share
//!
//! Rates are independent so tests can drive an Underlying-path floor
//! breach without affecting the VaultShare path (and vice versa). Real
//! SAC transfers happen in/out, so the engine's pre-/post-transfer
//! balance accounting in `convert_to_ibt` / `payout_from_ibt` is
//! exercised end-to-end. Underlying/vault-share decimals and the wrapper
//! share decimals are set independently at `initialize`, so tests can
//! exercise the underlying↔IBT decimal gap the real markets carry.

use soroban_sdk::{
    Address, Env, MuxedAddress, String, contract, contracterror, contractimpl, contracttype,
    panic_with_error,
};
use spectra_math::mul_div::{Rounding, mul_div_wide};
use stellar_tokens::fungible::{Base, FungibleToken};

/// 1.0 in 18-decimal fixed-point. Rates of this magnitude give 1:1 conversion.
const UNIT: i128 = 1_000_000_000_000_000_000;

#[contracterror]
#[derive(Copy, Clone, Debug, Eq, PartialEq, PartialOrd, Ord)]
#[repr(u32)]
pub enum MockSpectraWrapperError {
    AlreadyInitialized = 1,
    /// `wrap_with_min_shares` / `unwrap_with_min_vault_shares` realized output
    /// below the caller's minimum. Wire-compatible with the real wrapper's
    /// `SlippageProtectionFailed` so engine-level tests can match either.
    SlippageProtectionFailed = 2,
}

#[contract]
pub struct MockSpectraWrapperContract;

#[contracttype]
enum Key {
    Underlying,
    VaultShare,
    /// 18-decimal: 1 unit underlying → `DepositRate` wrapper shares (Floor).
    DepositRate,
    /// 18-decimal: 1 vault_share → `WrapRate` wrapper shares (Floor).
    WrapRate,
    /// Underlying-denominated total, mirroring `MockIbt`: a rate-independent
    /// counter so the wrapper satisfies the `FungibleVault` view surface the
    /// PrincipalToken reads at `initialize`/tokenisation. Seeded via
    /// `set_total_assets`, bumped on `deposit`, drawn down on `redeem`/`withdraw`.
    TotalAssets,
}

#[contractimpl]
impl MockSpectraWrapperContract {
    pub fn initialize(
        env: &Env,
        underlying: Address,
        vault_share: Address,
        share_decimals: u32,
        name: String,
        symbol: String,
    ) {
        if env.storage().instance().has(&Key::Underlying) {
            panic_with_error!(env, MockSpectraWrapperError::AlreadyInitialized);
        }
        Base::set_metadata(env, share_decimals, name, symbol);
        let st = env.storage().instance();
        st.set(&Key::Underlying, &underlying);
        st.set(&Key::VaultShare, &vault_share);
        st.set(&Key::DepositRate, &UNIT);
        st.set(&Key::WrapRate, &UNIT);
        st.set(&Key::TotalAssets, &0i128);
    }

    // ── FungibleVault view surface (PrincipalToken reads these) ─────────────

    pub fn total_assets(env: &Env) -> i128 {
        env.storage().instance().get(&Key::TotalAssets).unwrap_or(0)
    }

    /// `assets → shares` at `deposit_rate` (Floor) — matches `deposit`.
    pub fn preview_deposit(env: &Env, assets: i128) -> i128 {
        let rate: i128 = env.storage().instance().get(&Key::DepositRate).unwrap();
        mul_div_wide(env, assets, rate, UNIT, Rounding::Floor)
    }

    /// `shares → assets` at the inverse of `deposit_rate` (Floor) — matches `redeem`.
    pub fn preview_redeem(env: &Env, shares: i128) -> i128 {
        let rate: i128 = env.storage().instance().get(&Key::DepositRate).unwrap();
        mul_div_wide(env, shares, UNIT, rate, Rounding::Floor)
    }

    /// `assets → shares` to burn for an exact-asset withdraw (Ceil).
    pub fn preview_withdraw(env: &Env, assets: i128) -> i128 {
        let rate: i128 = env.storage().instance().get(&Key::DepositRate).unwrap();
        mul_div_wide(env, assets, rate, UNIT, Rounding::Ceil)
    }

    pub fn convert_to_shares(env: &Env, assets: i128) -> i128 {
        Self::preview_deposit(env, assets)
    }

    pub fn convert_to_assets(env: &Env, shares: i128) -> i128 {
        Self::preview_redeem(env, shares)
    }

    /// OZ FungibleVault shape: burn `preview_withdraw(assets)` shares (Ceil)
    /// from `owner`, transfer underlying to `receiver`. Returns shares burned.
    pub fn withdraw(
        env: &Env,
        assets: i128,
        receiver: Address,
        owner: Address,
        operator: Address,
    ) -> i128 {
        operator.require_auth();
        let shares = Self::preview_withdraw(env, assets);
        if operator != owner {
            Base::spend_allowance(env, &owner, &operator, shares);
        }
        Base::update(env, Some(&owner), None, shares);
        let st = env.storage().instance();
        let total: i128 = st.get(&Key::TotalAssets).unwrap_or(0);
        st.set(&Key::TotalAssets, &(total - assets));
        let underlying: Address = st.get(&Key::Underlying).unwrap();
        soroban_sdk::token::Client::new(env, &underlying).transfer(
            &env.current_contract_address(),
            &receiver,
            &assets,
        );
        shares
    }

    pub fn set_total_assets(env: &Env, amount: i128) {
        env.storage().instance().set(&Key::TotalAssets, &amount);
    }

    // ── Wrapper trait surface ──────────────────────────────────────────────

    pub fn query_asset(env: &Env) -> Address {
        env.storage().instance().get(&Key::Underlying).unwrap()
    }

    pub fn vault_share(env: &Env) -> Address {
        env.storage().instance().get(&Key::VaultShare).unwrap()
    }

    /// Pull `assets` of underlying from `from` (via `from`'s allowance to
    /// `operator` when they differ), mint wrapper shares to `receiver` at
    /// the configured `deposit_rate`. Returns shares minted.
    pub fn deposit(
        env: &Env,
        assets: i128,
        receiver: Address,
        from: Address,
        operator: Address,
    ) -> i128 {
        operator.require_auth();
        let rate: i128 = env.storage().instance().get(&Key::DepositRate).unwrap();
        let shares = mul_div_wide(env, assets, rate, UNIT, Rounding::Floor);

        let underlying: Address = env.storage().instance().get(&Key::Underlying).unwrap();
        let wrapper = env.current_contract_address();
        let und = soroban_sdk::token::Client::new(env, &underlying);
        if operator == from {
            und.transfer(&from, &wrapper, &assets);
        } else {
            und.transfer_from(&operator, &from, &wrapper, &assets);
        }
        let st = env.storage().instance();
        let total: i128 = st.get(&Key::TotalAssets).unwrap_or(0);
        st.set(&Key::TotalAssets, &(total + assets));
        Base::mint(env, &receiver, shares);
        shares
    }

    /// Burn `shares` from `owner` (spending allowance when `operator != owner`),
    /// transfer underlying to `receiver` at the inverse of `deposit_rate`.
    /// Returns underlying delivered.
    pub fn redeem(
        env: &Env,
        shares: i128,
        receiver: Address,
        owner: Address,
        operator: Address,
    ) -> i128 {
        operator.require_auth();
        if operator != owner {
            Base::spend_allowance(env, &owner, &operator, shares);
        }
        let rate: i128 = env.storage().instance().get(&Key::DepositRate).unwrap();
        let assets = mul_div_wide(env, shares, UNIT, rate, Rounding::Floor);
        Base::update(env, Some(&owner), None, shares);
        let st = env.storage().instance();
        let total: i128 = st.get(&Key::TotalAssets).unwrap_or(0);
        st.set(&Key::TotalAssets, &(total - assets));
        let underlying: Address = env.storage().instance().get(&Key::Underlying).unwrap();
        soroban_sdk::token::Client::new(env, &underlying).transfer(
            &env.current_contract_address(),
            &receiver,
            &assets,
        );
        assets
    }

    /// Pull `vault_shares` of `vault_share` token from `caller` via
    /// `transfer(caller → contract)` — matching the real wrapper's `wrap`,
    /// which pulls one frame below the caller and therefore needs the caller
    /// to pre-declare the sub-auth (`authorize_pull`); a plain allowance does
    /// not back a `transfer`. Mint wrapper shares to `receiver` at `wrap_rate`.
    /// Panics `SlippageProtectionFailed` if the realized mint is below `min_shares`.
    pub fn wrap_with_min_shares(
        env: &Env,
        vault_shares: i128,
        receiver: Address,
        caller: Address,
        min_shares: i128,
    ) -> i128 {
        caller.require_auth();
        let rate: i128 = env.storage().instance().get(&Key::WrapRate).unwrap();
        let shares = mul_div_wide(env, vault_shares, rate, UNIT, Rounding::Floor);
        if shares < min_shares {
            panic_with_error!(env, MockSpectraWrapperError::SlippageProtectionFailed);
        }
        let vs_addr: Address = env.storage().instance().get(&Key::VaultShare).unwrap();
        let wrapper = env.current_contract_address();
        soroban_sdk::token::Client::new(env, &vs_addr).transfer(&caller, &wrapper, &vault_shares);
        Base::mint(env, &receiver, shares);
        shares
    }

    /// Burn `shares` of wrapper share from `owner` (spending allowance when
    /// `operator != owner`), deliver `vault_share` to `receiver` at the
    /// inverse of `wrap_rate`. Panics `SlippageProtectionFailed` if the
    /// realized delivery falls below `min_vault_shares`.
    pub fn unwrap_with_min_vault_shares(
        env: &Env,
        shares: i128,
        receiver: Address,
        owner: Address,
        operator: Address,
        min_vault_shares: i128,
    ) -> i128 {
        operator.require_auth();
        if operator != owner {
            Base::spend_allowance(env, &owner, &operator, shares);
        }
        let rate: i128 = env.storage().instance().get(&Key::WrapRate).unwrap();
        let vault_shares = mul_div_wide(env, shares, UNIT, rate, Rounding::Floor);
        if vault_shares < min_vault_shares {
            panic_with_error!(env, MockSpectraWrapperError::SlippageProtectionFailed);
        }
        Base::update(env, Some(&owner), None, shares);
        let vs_addr: Address = env.storage().instance().get(&Key::VaultShare).unwrap();
        soroban_sdk::token::Client::new(env, &vs_addr).transfer(
            &env.current_contract_address(),
            &receiver,
            &vault_shares,
        );
        vault_shares
    }

    // ── Test knobs ─────────────────────────────────────────────────────────

    pub fn set_deposit_rate(env: &Env, rate: i128) {
        env.storage().instance().set(&Key::DepositRate, &rate);
    }

    pub fn set_wrap_rate(env: &Env, rate: i128) {
        env.storage().instance().set(&Key::WrapRate, &rate);
    }
}

#[contractimpl]
impl FungibleToken for MockSpectraWrapperContract {
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
    fn transfer(e: &Env, from: Address, to: MuxedAddress, amount: i128) {
        Base::transfer(e, &from, &to, amount);
    }
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
