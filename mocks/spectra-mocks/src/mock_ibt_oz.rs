// SPDX-License-Identifier: BUSL-1.1
//! Test-only mock IBT backed by OZ `FungibleVault` (full ERC-4626).
//!
//! Used by router tests where realistic 4626 semantics + Soroban auth
//! propagation matter. The original `mock_ibt` is a simpler toy mock kept
//! for PT yield-system tests that need exact-rate manipulation.

use soroban_sdk::{Address, Env, MuxedAddress, String, contract, contractimpl};
use stellar_tokens::fungible::{Base, ContractOverrides, FungibleToken};
use stellar_tokens::vault::{FungibleVault, Vault};

#[contract]
pub struct MockIbtOzContract;

#[contractimpl]
impl MockIbtOzContract {
    pub fn initialize(
        env: &Env,
        underlying: Address,
        name: String,
        symbol: String,
        decimals_offset: u32,
    ) {
        Vault::set_asset(env, underlying);
        Vault::set_decimals_offset(env, decimals_offset);
        let dec = Vault::decimals(env);
        Base::set_metadata(env, dec, name, symbol);
    }
}

#[contractimpl(contracttrait)]
impl FungibleToken for MockIbtOzContract {
    type ContractType = Vault;

    fn balance(e: &Env, account: Address) -> i128 {
        <Vault as ContractOverrides>::balance(e, &account)
    }
    fn total_supply(e: &Env) -> i128 {
        <Vault as ContractOverrides>::total_supply(e)
    }
    fn allowance(e: &Env, owner: Address, spender: Address) -> i128 {
        <Vault as ContractOverrides>::allowance(e, &owner, &spender)
    }
    fn transfer(e: &Env, from: Address, to: MuxedAddress, amount: i128) {
        <Vault as ContractOverrides>::transfer(e, &from, &to, amount);
    }
    fn transfer_from(e: &Env, spender: Address, from: Address, to: Address, amount: i128) {
        <Vault as ContractOverrides>::transfer_from(e, &spender, &from, &to, amount);
    }
    fn approve(e: &Env, owner: Address, spender: Address, amount: i128, live_until_ledger: u32) {
        <Vault as ContractOverrides>::approve(e, &owner, &spender, amount, live_until_ledger);
    }
    fn decimals(e: &Env) -> u32 {
        <Vault as ContractOverrides>::decimals(e)
    }
    fn name(e: &Env) -> String {
        <Vault as ContractOverrides>::name(e)
    }
    fn symbol(e: &Env) -> String {
        <Vault as ContractOverrides>::symbol(e)
    }
}

#[contractimpl(contracttrait)]
impl FungibleVault for MockIbtOzContract {}
