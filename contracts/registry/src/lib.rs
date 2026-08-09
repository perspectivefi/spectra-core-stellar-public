// SPDX-License-Identifier: BUSL-1.1
#![no_std]

mod errors;
mod events;
pub mod storage;
#[cfg(test)]
mod test;

use errors::RegistryError;
use events::*;
use soroban_sdk::{Address, BytesN, Env, Symbol, contract, contractimpl, panic_with_error};
use spectra_common::constants::roles;
use stellar_access::access_control;
use stellar_macros::only_role;

#[contract]
pub struct RegistryContract;

#[contractimpl]
impl RegistryContract {
    pub fn __constructor(env: &Env, admin: Address) {
        access_control::set_admin(env, &admin);

        access_control::grant_role_no_auth(env, &admin, &roles::REGISTRY, &admin);
        access_control::grant_role_no_auth(env, &admin, &roles::FEE_SETTER, &admin);
        access_control::grant_role_no_auth(env, &admin, &roles::UPGRADE, &admin);
        access_control::grant_role_no_auth(env, &admin, &roles::FACTORY, &admin);

        storage::extend_instance_ttl(env);
    }

    // -- Getters --

    pub fn get_factory(env: &Env) -> Address {
        storage::get_factory(env)
    }

    pub fn get_router(env: &Env) -> Address {
        storage::get_router(env)
    }

    pub fn get_pt_wasm_hash(env: &Env) -> BytesN<32> {
        storage::get_pt_wasm_hash(env)
    }

    pub fn get_yt_wasm_hash(env: &Env) -> BytesN<32> {
        storage::get_yt_wasm_hash(env)
    }

    pub fn get_tokenization_fee(env: &Env) -> i128 {
        storage::get_tokenization_fee(env)
    }

    pub fn get_yield_fee(env: &Env) -> i128 {
        storage::get_yield_fee(env)
    }

    pub fn get_pt_flash_loan_fee(env: &Env) -> i128 {
        storage::get_pt_flash_loan_fee(env)
    }

    pub fn get_fee_collector(env: &Env) -> Address {
        storage::get_fee_collector(env)
    }

    /// Non-panicking variant of `get_fee_collector`. Returns `None` when
    /// the registry has never set a fee collector. Used by PT's
    /// `claim_fees` to surface a structured `FeeCollectorNotSet` error.
    pub fn get_fee_collector_opt(env: &Env) -> Option<Address> {
        storage::try_get_fee_collector(env)
    }

    pub fn get_bridge(env: &Env) -> Address {
        storage::get_bridge(env)
    }

    pub fn get_limit_order_engine(env: &Env) -> Address {
        storage::get_limit_order_engine(env)
    }

    pub fn get_fee_reduction(env: &Env, pt: Address, user: Address) -> i128 {
        storage::get_fee_reduction(env, &pt, &user)
    }

    pub fn is_registered_pt(env: &Env, pt: Address) -> bool {
        storage::is_registered_pt(env, &pt)
    }

    pub fn get_pt_count(env: &Env) -> u32 {
        storage::get_pt_count(env)
    }

    pub fn get_pt_at(env: &Env, index: u32) -> Option<Address> {
        storage::get_pt_at(env, index)
    }

    // -- Address setters (REGISTRY_ROLE) --

    #[only_role(caller, "REGISTRY")]
    pub fn set_factory(env: &Env, caller: Address, new_factory: Address) {
        let old = storage::try_get_factory(env);
        storage::set_factory(env, &new_factory);
        FactoryChange {
            old: old.unwrap_or(new_factory.clone()),
            new: new_factory,
        }
        .publish(env);
    }

    #[only_role(caller, "REGISTRY")]
    pub fn set_router(env: &Env, caller: Address, new_router: Address) {
        let old = storage::try_get_router(env);
        storage::set_router(env, &new_router);
        RouterChange {
            old: old.unwrap_or(new_router.clone()),
            new: new_router,
        }
        .publish(env);
    }

    #[only_role(caller, "REGISTRY")]
    pub fn set_pt_wasm_hash(env: &Env, caller: Address, hash: BytesN<32>) {
        let old = storage::try_get_pt_wasm_hash(env);
        storage::set_pt_wasm_hash(env, &hash);
        PtWasmHashChange {
            old: old.unwrap_or(hash.clone()),
            new: hash,
        }
        .publish(env);
    }

    #[only_role(caller, "REGISTRY")]
    pub fn set_yt_wasm_hash(env: &Env, caller: Address, hash: BytesN<32>) {
        let old = storage::try_get_yt_wasm_hash(env);
        storage::set_yt_wasm_hash(env, &hash);
        YtWasmHashChange {
            old: old.unwrap_or(hash.clone()),
            new: hash,
        }
        .publish(env);
    }

    #[only_role(caller, "REGISTRY")]
    pub fn set_fee_collector(env: &Env, caller: Address, new_collector: Address) {
        let old = storage::try_get_fee_collector(env);
        storage::set_fee_collector(env, &new_collector);
        FeeCollectorChange {
            old: old.unwrap_or(new_collector.clone()),
            new: new_collector,
        }
        .publish(env);
    }

    pub fn set_bridge(env: &Env, caller: Address, new_bridge: Address) {
        if Some(caller.clone()) != access_control::get_admin(env) {
            panic_with_error!(env, RegistryError::UnauthorizedCaller);
        }
        caller.require_auth();
        let old = storage::try_get_bridge(env);
        storage::set_bridge(env, &new_bridge);
        BridgeChange {
            old: old.unwrap_or(new_bridge.clone()),
            new: new_bridge,
        }
        .publish(env);
    }

    pub fn set_limit_order_engine(env: &Env, caller: Address, new_loe: Address) {
        if Some(caller.clone()) != access_control::get_admin(env) {
            panic_with_error!(env, RegistryError::UnauthorizedCaller);
        }
        caller.require_auth();
        let old = storage::try_get_limit_order_engine(env);
        storage::set_limit_order_engine(env, &new_loe);
        LimitOrderEngineChange {
            old: old.unwrap_or(new_loe.clone()),
            new: new_loe,
        }
        .publish(env);
    }

    // -- Fee setters (FEE_SETTER_ROLE) --

    #[only_role(caller, "FEE_SET")]
    pub fn set_tokenization_fee(env: &Env, caller: Address, fee: i128) {
        if fee < 0 {
            panic_with_error!(env, RegistryError::NegativeFee);
        }
        if fee > spectra_common::constants::MAX_TOKENIZATION_FEE {
            panic_with_error!(env, RegistryError::FeeGreaterThanMaxValue);
        }
        let old = storage::get_tokenization_fee(env);
        storage::set_tokenization_fee(env, fee);
        TokenizationFeeChange { old, new: fee }.publish(env);
    }

    #[only_role(caller, "FEE_SET")]
    pub fn set_yield_fee(env: &Env, caller: Address, fee: i128) {
        if fee < 0 {
            panic_with_error!(env, RegistryError::NegativeFee);
        }
        if fee > spectra_common::constants::MAX_YIELD_FEE {
            panic_with_error!(env, RegistryError::FeeGreaterThanMaxValue);
        }
        let old = storage::get_yield_fee(env);
        storage::set_yield_fee(env, fee);
        YieldFeeChange { old, new: fee }.publish(env);
    }

    #[only_role(caller, "FEE_SET")]
    pub fn set_pt_flash_loan_fee(env: &Env, caller: Address, fee: i128) {
        if fee < 0 {
            panic_with_error!(env, RegistryError::NegativeFee);
        }
        if fee > spectra_common::constants::MAX_PT_FLASH_LOAN_FEE {
            panic_with_error!(env, RegistryError::FeeGreaterThanMaxValue);
        }
        let old = storage::get_pt_flash_loan_fee(env);
        storage::set_pt_flash_loan_fee(env, fee);
        PtFlashLoanFeeChange { old, new: fee }.publish(env);
    }

    #[only_role(caller, "FEE_SET")]
    pub fn reduce_fee(env: &Env, caller: Address, pt: Address, user: Address, reduction: i128) {
        if reduction < 0 {
            panic_with_error!(env, RegistryError::NegativeFee);
        }
        if reduction > spectra_common::constants::FEE_DIVISOR {
            panic_with_error!(env, RegistryError::ReductionTooBig);
        }
        storage::set_fee_reduction(env, &pt, &user, reduction);
        FeeReduced {
            pt,
            user,
            reduction,
        }
        .publish(env);
    }

    // -- PT registration (FACTORY_ROLE) --

    #[only_role(caller, "FACTORY")]
    pub fn add_pt(env: &Env, caller: Address, pt: Address) {
        if storage::is_registered_pt(env, &pt) {
            panic_with_error!(env, RegistryError::PtListUpdateFailed);
        }
        storage::set_pt_registered(env, &pt);
        PtAdded { pt }.publish(env);
    }

    #[only_role(caller, "FACTORY")]
    pub fn remove_pt(env: &Env, caller: Address, pt: Address) {
        if !storage::is_registered_pt(env, &pt) {
            panic_with_error!(env, RegistryError::PtListUpdateFailed);
        }
        storage::remove_pt_registered(env, &pt);
        PtRemoved { pt }.publish(env);
    }

    // -- Role management --
    //
    // Thin wrappers over OZ stellar-access. The library functions enforce
    // auth + admin-role checks themselves; we expose them as contract entry
    // points so off-chain tooling (the deploy dispatcher) can grant the
    // factory the REGISTRY role after wiring `set_factory`.

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

    // -- Upgrade (UPGRADE_ROLE) --

    #[only_role(caller, "UPGRADE")]
    pub fn upgrade(env: &Env, caller: Address, new_wasm_hash: BytesN<32>) {
        RegistryUpgraded {
            caller,
            new_wasm_hash: new_wasm_hash.clone(),
        }
        .publish(env);
        env.deployer().update_current_contract_wasm(new_wasm_hash);
    }

    // -- Admin rotation (2-step, delegated to stellar-access) --

    pub fn transfer_admin_role(env: &Env, new_admin: Address, live_until_ledger: u32) {
        access_control::transfer_admin_role(env, &new_admin, live_until_ledger);
    }

    pub fn accept_admin_transfer(env: &Env) {
        access_control::accept_admin_transfer(env);
    }
}
