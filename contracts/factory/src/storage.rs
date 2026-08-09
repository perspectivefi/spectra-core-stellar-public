// SPDX-License-Identifier: BUSL-1.1
use soroban_sdk::{Address, Env, contracttype};
use stellar_access::access_control;

#[contracttype]
#[derive(Clone, Debug)]
pub enum DataKey {
    Registry,
    PtDeployCounter,
}

pub fn is_initialized(env: &Env) -> bool {
    access_control::get_admin(env).is_some()
}

pub fn extend_instance_ttl(env: &Env) {
    let max = env.storage().max_ttl();
    env.storage().instance().extend_ttl(max, max);
}

// --- Registry ---

pub fn set_registry(env: &Env, value: &Address) {
    env.storage().instance().set(&DataKey::Registry, value);
    extend_instance_ttl(env);
}

pub fn get_registry(env: &Env) -> Address {
    let val: Address = env.storage().instance().get(&DataKey::Registry).unwrap();
    extend_instance_ttl(env);
    val
}

// --- PtDeployCounter ---

pub fn get_pt_deploy_counter(env: &Env) -> u64 {
    let val: u64 = env
        .storage()
        .instance()
        .get(&DataKey::PtDeployCounter)
        .unwrap_or(0u64);
    extend_instance_ttl(env);
    val
}

pub fn set_pt_deploy_counter(env: &Env, value: u64) {
    env.storage()
        .instance()
        .set(&DataKey::PtDeployCounter, &value);
    extend_instance_ttl(env);
}
