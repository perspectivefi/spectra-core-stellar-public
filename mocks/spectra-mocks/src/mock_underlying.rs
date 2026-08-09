// SPDX-License-Identifier: BUSL-1.1
//! Minimal SEP-41 token used as the underlying asset (e.g., USDC) in PT/YT
//! integration tests. Permissionless `mint` for setup convenience.

use soroban_sdk::{Address, Env, MuxedAddress, String, contract, contractimpl};
use stellar_tokens::fungible::{Base, FungibleToken};

#[contract]
pub struct MockUnderlyingContract;

#[contractimpl]
impl MockUnderlyingContract {
    pub fn initialize(env: &Env, name: String, symbol: String, decimals: u32) {
        Base::set_metadata(env, decimals, name, symbol);
    }

    pub fn mint(env: &Env, to: Address, amount: i128) {
        Base::mint(env, &to, amount);
    }
}

#[contractimpl]
impl FungibleToken for MockUnderlyingContract {
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
