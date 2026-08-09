// SPDX-License-Identifier: BUSL-1.1
//! Minimal OZ-FungibleVault-shaped IBT (Interest-Bearing Token) for vault
//! flow tests. NOT a real vault — exposes `set_rate` / `set_total_assets` so
//! tests can drive PT through depeg / appreciation scenarios
//! deterministically. Call shapes, auth model, and rounding mirror
//! `stellar_tokens::vault::FungibleVault`; share/asset math is rate-driven
//! instead of supply-driven. Real OZ vaults derive
//! `decimals = asset_decimals + offset`; here decimals are set freely.

use soroban_sdk::{Address, Env, MuxedAddress, String, contract, contractimpl, contracttype};
use spectra_math::mul_div::{Rounding, mul_div_wide};
use stellar_tokens::fungible::{Base, FungibleToken};

#[contract]
pub struct MockIbtContract;

#[contracttype]
enum IbtKey {
    Underlying,
    TotalAssets,
    Rate,
}

#[contractimpl]
impl MockIbtContract {
    pub fn initialize(env: &Env, underlying: Address, name: String, symbol: String, decimals: u32) {
        Base::set_metadata(env, decimals, name, symbol);
        let st = env.storage().instance();
        st.set(&IbtKey::Underlying, &underlying);
        st.set(&IbtKey::TotalAssets, &0i128);
        st.set(&IbtKey::Rate, &10_i128.pow(decimals));
    }

    pub fn total_assets(env: &Env) -> i128 {
        env.storage()
            .instance()
            .get(&IbtKey::TotalAssets)
            .unwrap_or(0)
    }

    pub fn query_asset(env: &Env) -> Address {
        env.storage().instance().get(&IbtKey::Underlying).unwrap()
    }

    /// `preview_deposit(assets) = assets * ibt_unit / rate` (Floor) — inverse of preview_redeem.
    pub fn preview_deposit(env: &Env, assets: i128) -> i128 {
        let dec = Base::decimals(env);
        let rate: i128 = env.storage().instance().get(&IbtKey::Rate).unwrap();
        mul_div_wide(env, assets, 10_i128.pow(dec), rate, Rounding::Floor)
    }

    /// `preview_redeem(shares) = shares * rate / ibt_unit` (Floor).
    pub fn preview_redeem(env: &Env, shares: i128) -> i128 {
        let dec = Base::decimals(env);
        let rate: i128 = env.storage().instance().get(&IbtKey::Rate).unwrap();
        mul_div_wide(env, shares, rate, 10_i128.pow(dec), Rounding::Floor)
    }

    /// `convert_to_shares(assets)` — idealized fee-free rate, Floor per OZ.
    pub fn convert_to_shares(env: &Env, assets: i128) -> i128 {
        Self::preview_deposit(env, assets)
    }

    /// `convert_to_assets(shares)` — idealized fee-free rate, Floor per OZ.
    pub fn convert_to_assets(env: &Env, shares: i128) -> i128 {
        Self::preview_redeem(env, shares)
    }

    /// `preview_withdraw(assets)` — inverse of preview_redeem, Ceil per OZ.
    pub fn preview_withdraw(env: &Env, assets: i128) -> i128 {
        let dec = Base::decimals(env);
        let rate: i128 = env.storage().instance().get(&IbtKey::Rate).unwrap();
        mul_div_wide(env, assets, 10_i128.pow(dec), rate, Rounding::Ceil)
    }

    /// `preview_mint(shares)` — assets needed to mint `shares`, Ceil per OZ.
    pub fn preview_mint(env: &Env, shares: i128) -> i128 {
        let dec = Base::decimals(env);
        let rate: i128 = env.storage().instance().get(&IbtKey::Rate).unwrap();
        mul_div_wide(env, shares, rate, 10_i128.pow(dec), Rounding::Ceil)
    }

    /// OZ FungibleVault shape: pulls underlying from `from` (`transfer` when
    /// `operator == from`, else `transfer_from` spending `from → operator`
    /// allowance), mints shares to `receiver`.
    pub fn deposit(
        env: &Env,
        assets: i128,
        receiver: Address,
        from: Address,
        operator: Address,
    ) -> i128 {
        operator.require_auth();
        let ibts = Self::preview_deposit(env, assets);
        let underlying: Address = env.storage().instance().get(&IbtKey::Underlying).unwrap();
        let vault = env.current_contract_address();
        let und = soroban_sdk::token::Client::new(env, &underlying);
        if operator == from {
            und.transfer(&from, &vault, &assets);
        } else {
            und.transfer_from(&operator, &from, &vault, &assets);
        }
        let st = env.storage().instance();
        let total: i128 = st.get(&IbtKey::TotalAssets).unwrap_or(0);
        st.set(&IbtKey::TotalAssets, &(total + assets));
        Base::mint(env, &receiver, ibts);
        ibts
    }

    /// OZ FungibleVault shape: burns shares from `owner` (spending the
    /// `owner → operator` share allowance when they differ), transfers
    /// underlying to `receiver`.
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
        let assets = Self::preview_redeem(env, shares);
        let st = env.storage().instance();
        let total: i128 = st.get(&IbtKey::TotalAssets).unwrap_or(0);
        st.set(&IbtKey::TotalAssets, &(total - assets));
        Base::update(env, Some(&owner), None, shares);
        let underlying: Address = st.get(&IbtKey::Underlying).unwrap();
        soroban_sdk::token::Client::new(env, &underlying).transfer(
            &env.current_contract_address(),
            &receiver,
            &assets,
        );
        assets
    }

    /// OZ FungibleVault shape: burns `preview_withdraw(assets)` shares (Ceil)
    /// from `owner`, transfers underlying to `receiver`. Returns shares.
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
        let total: i128 = st.get(&IbtKey::TotalAssets).unwrap_or(0);
        st.set(&IbtKey::TotalAssets, &(total - assets));
        let underlying: Address = st.get(&IbtKey::Underlying).unwrap();
        soroban_sdk::token::Client::new(env, &underlying).transfer(
            &env.current_contract_address(),
            &receiver,
            &assets,
        );
        shares
    }

    pub fn set_total_assets(env: &Env, amount: i128) {
        env.storage().instance().set(&IbtKey::TotalAssets, &amount);
    }

    pub fn set_rate(env: &Env, rate: i128) {
        env.storage().instance().set(&IbtKey::Rate, &rate);
    }
}

#[contractimpl]
impl FungibleToken for MockIbtContract {
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
