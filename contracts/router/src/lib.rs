// SPDX-License-Identifier: BUSL-1.1
#![no_std]

pub mod dispatcher;
pub mod errors;
pub mod events;
#[cfg(test)]
mod fork_test;
pub mod storage;
#[cfg(test)]
mod test;

use crate::errors::RouterError;
use crate::events::{
    BridgeChanged, BrokerChanged, EngineChanged, ExecuteCompleted, MinGasFloorChanged, PausedSet,
    RegistryChanged,
};
use soroban_sdk::{Address, BytesN, Env, Symbol, Vec, contract, contractimpl, panic_with_error};
use spectra_common::constants::roles;
use spectra_common::router_types::Command;
use stellar_access::access_control;
use stellar_contract_utils::pausable;
use stellar_macros::{only_role, when_not_paused};

#[contract]
pub struct MainRouterContract;

#[contractimpl]
impl MainRouterContract {
    pub fn __constructor(env: &Env, admin: Address, registry: Address) {
        access_control::set_admin(env, &admin);
        access_control::grant_role_no_auth(env, &admin, &roles::PAUSER, &admin);
        access_control::grant_role_no_auth(env, &admin, &roles::UPGRADE, &admin);
        access_control::grant_role_no_auth(env, &admin, &roles::ROUTER_MANAGER, &admin);
        storage::set_registry(env, &registry);
    }

    #[when_not_paused]
    pub fn execute(env: &Env, caller: Address, commands: Vec<Command>, deadline: u64) {
        caller.require_auth();
        if env.ledger().timestamp() > deadline {
            panic_with_error!(env, RouterError::DeadlineExpired);
        }
        if commands.is_empty() {
            panic_with_error!(env, RouterError::EmptyCommands);
        }
        for cmd in commands.iter() {
            dispatcher::dispatch(env, &caller, &cmd);
        }
        ExecuteCompleted {
            caller,
            command_count: commands.len(),
        }
        .publish(env);
        storage::extend_instance_ttl(env);
    }

    #[only_role(caller, "PAUSER")]
    pub fn pause(env: &Env, caller: Address) {
        pausable::pause(env);
        PausedSet { paused: true }.publish(env);
        storage::extend_instance_ttl(env);
    }

    #[only_role(caller, "PAUSER")]
    pub fn unpause(env: &Env, caller: Address) {
        pausable::unpause(env);
        PausedSet { paused: false }.publish(env);
        storage::extend_instance_ttl(env);
    }

    #[only_role(caller, "RTR_MGR")]
    pub fn set_registry(env: &Env, caller: Address, new_registry: Address) {
        let old = storage::get_registry(env);
        storage::set_registry(env, &new_registry);
        RegistryChanged {
            old,
            new: new_registry,
        }
        .publish(env);
        storage::extend_instance_ttl(env);
    }

    pub fn get_registry(env: &Env) -> Address {
        storage::get_registry(env)
    }

    #[only_role(caller, "RTR_MGR")]
    pub fn set_engine(env: &Env, caller: Address, new_engine: Address) {
        let old = storage::get_engine(env);
        storage::set_engine(env, &new_engine);
        EngineChanged {
            old,
            new: new_engine,
        }
        .publish(env);
    }

    pub fn get_engine(env: &Env) -> Address {
        storage::get_engine(env)
            .unwrap_or_else(|| panic_with_error!(env, RouterError::EngineNotSet))
    }

    #[only_role(caller, "RTR_MGR")]
    pub fn set_bridge(env: &Env, caller: Address, new_bridge: Address) {
        let old = storage::get_bridge(env);
        storage::set_bridge(env, &new_bridge);
        BridgeChanged {
            old,
            new: new_bridge,
        }
        .publish(env);
    }

    pub fn get_bridge(env: &Env) -> Address {
        storage::get_bridge(env)
            .unwrap_or_else(|| panic_with_error!(env, RouterError::BridgeNotSet))
    }

    #[only_role(caller, "RTR_MGR")]
    pub fn set_broker(env: &Env, caller: Address, new_broker: Address) {
        let old = storage::get_broker(env);
        storage::set_broker(env, &new_broker);
        BrokerChanged {
            old,
            new: new_broker,
        }
        .publish(env);
    }

    pub fn get_broker(env: &Env) -> Address {
        storage::get_broker(env)
            .unwrap_or_else(|| panic_with_error!(env, RouterError::BrokerNotSet))
    }

    /// Minimum gas a bridge-out command must attach for a given
    /// `(target_chain_id, gas_token)`. Default 0 (disabled).
    #[only_role(caller, "RTR_MGR")]
    pub fn set_min_gas_floor(
        env: &Env,
        caller: Address,
        target_chain_id: u32,
        gas_token: Address,
        new_floor: i128,
    ) {
        if new_floor < 0 {
            panic_with_error!(env, RouterError::InsufficientGas);
        }
        let old = storage::get_min_gas_floor(env, target_chain_id, &gas_token);
        storage::set_min_gas_floor(env, target_chain_id, &gas_token, new_floor);
        MinGasFloorChanged {
            target_chain_id,
            gas_token,
            old,
            new: new_floor,
        }
        .publish(env);
    }

    pub fn get_min_gas_floor(env: &Env, target_chain_id: u32, gas_token: Address) -> i128 {
        storage::get_min_gas_floor(env, target_chain_id, &gas_token)
    }

    pub fn paused(env: &Env) -> bool {
        pausable::paused(env)
    }

    // -- Role management --
    //
    // Thin wrappers over OZ stellar-access. The library functions enforce
    // auth + admin-role checks themselves; we expose them as contract entry
    // points so off-chain tooling (deploy dispatcher, key rotation) can
    // rotate the PAUSER, RTR_MGR, and UPGRADE roles after `initialize`.

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
        env.deployer().update_current_contract_wasm(new_wasm_hash);
    }

    // ── Admin rotation (2-step, delegated to stellar-access) ──────────

    pub fn transfer_admin_role(env: &Env, new_admin: Address, live_until_ledger: u32) {
        access_control::transfer_admin_role(env, &new_admin, live_until_ledger);
    }

    pub fn accept_admin_transfer(env: &Env) {
        access_control::accept_admin_transfer(env);
    }
}
