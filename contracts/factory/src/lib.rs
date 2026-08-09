// SPDX-License-Identifier: BUSL-1.1
#![no_std]

mod errors;
mod events;
pub mod storage;
#[cfg(test)]
mod test;

use errors::FactoryError;
use events::*;
use soroban_sdk::{Address, Bytes, BytesN, Env, Symbol, contract, contractimpl, panic_with_error};
use spectra_common::constants::roles;
use spectra_common::principal_token::PrincipalTokenInitClient;
use spectra_common::registry::RegistryClient;
use stellar_access::access_control;
use stellar_macros::only_role;

#[contract]
pub struct FactoryContract;

#[contractimpl]
impl FactoryContract {
    pub fn __constructor(env: &Env, admin: Address, registry: Address) {
        if registry == env.current_contract_address() {
            panic_with_error!(env, FactoryError::RegistryAddressInvalid);
        }

        access_control::set_admin(env, &admin);
        access_control::grant_role_no_auth(env, &admin, &roles::UPGRADE, &admin);

        storage::set_registry(env, &registry);
        storage::set_pt_deploy_counter(env, 0);

        FactoryInitialized { admin, registry }.publish(env);
    }

    pub fn get_registry(env: &Env) -> Address {
        Self::assert_initialized(env);
        storage::get_registry(env)
    }

    // -- Role management --
    //
    // Thin wrappers over OZ stellar-access. The library functions enforce
    // auth + admin-role checks themselves; we expose them as contract entry
    // points so off-chain tooling (deploy dispatcher, key rotation) can
    // rotate the FACTORY and UPGRADE roles after `initialize`.

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

    #[only_role(caller, "UPGRADE")]
    pub fn upgrade(env: &Env, caller: Address, new_wasm_hash: BytesN<32>) {
        FactoryUpgraded {
            caller,
            new_wasm_hash: new_wasm_hash.clone(),
        }
        .publish(env);
        env.deployer().update_current_contract_wasm(new_wasm_hash);
    }

    /// Deploys and registers a PT without creating AMM or pool. `caller` is
    /// recorded as the deployer in `PtDeployed`; it must sign for the call.
    /// The deployed PT's admin is the Factory's current admin (collapsed
    /// from the prior separate `authority` field — see PR body for the
    /// breaking-ABI note).
    pub fn deploy_pt(env: &Env, caller: Address, ibt: Address, duration: u64) -> Address {
        Self::assert_initialized(env);
        if duration == 0 {
            panic_with_error!(env, FactoryError::InvalidDuration);
        }
        caller.require_auth();

        let registry = storage::get_registry(env);
        let admin = access_control::get_admin(env)
            .unwrap_or_else(|| panic_with_error!(env, FactoryError::NotInitialized));
        let counter = storage::get_pt_deploy_counter(env);

        let reg_client = RegistryClient::new(env, &registry);
        let pt_wasm_hash = reg_client.get_pt_wasm_hash();

        let salt = Self::compute_salt(env, duration, counter);

        let pt = env
            .deployer()
            .with_current_contract(salt)
            .deploy_v2(pt_wasm_hash, ());

        PrincipalTokenInitClient::new(env, &pt).initialize(&ibt, &duration, &admin, &registry);

        reg_client.add_pt(&env.current_contract_address(), &pt);

        storage::set_pt_deploy_counter(env, counter + 1);

        PtDeployed {
            deployer: caller,
            pt: pt.clone(),
            ibt,
            duration,
        }
        .publish(env);

        pt
    }

    fn compute_salt(env: &Env, duration: u64, counter: u64) -> BytesN<32> {
        let mut buf = Bytes::new(env);
        buf.extend_from_array(&duration.to_be_bytes());
        buf.extend_from_array(&counter.to_be_bytes());
        env.crypto().sha256(&buf).into()
    }

    fn assert_initialized(env: &Env) {
        if !storage::is_initialized(env) {
            panic_with_error!(env, FactoryError::NotInitialized);
        }
    }

    // -- Admin rotation (2-step, delegated to stellar-access) --

    pub fn transfer_admin_role(env: &Env, new_admin: Address, live_until_ledger: u32) {
        access_control::transfer_admin_role(env, &new_admin, live_until_ledger);
    }

    pub fn accept_admin_transfer(env: &Env) {
        access_control::accept_admin_transfer(env);
    }
}
