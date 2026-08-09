// SPDX-License-Identifier: BUSL-1.1
use soroban_sdk::{Address, Env, contracttype};
use spectra_common::ttl::extend_instance_ttl as common_extend_instance_ttl;
use stellar_access::access_control;

#[contracttype]
pub enum DataKey {
    Registry,
    Engine,
    Bridge,
    Broker,
    MinGasFloor(u32, Address),
}

/// Admin presence is the canonical "already initialized" beacon. Matches PT
/// and registry.
pub fn is_initialized(env: &Env) -> bool {
    access_control::get_admin(env).is_some()
}

pub fn extend_instance_ttl(env: &Env) {
    common_extend_instance_ttl(env);
}

pub fn get_registry(env: &Env) -> Address {
    let v = env.storage().instance().get(&DataKey::Registry).unwrap();
    common_extend_instance_ttl(env);
    v
}

pub fn set_registry(env: &Env, v: &Address) {
    env.storage().instance().set(&DataKey::Registry, v);
    common_extend_instance_ttl(env);
}

pub fn get_engine(env: &Env) -> Option<Address> {
    let v = env.storage().instance().get(&DataKey::Engine);
    common_extend_instance_ttl(env);
    v
}

pub fn set_engine(env: &Env, v: &Address) {
    env.storage().instance().set(&DataKey::Engine, v);
    common_extend_instance_ttl(env);
}

pub fn get_bridge(env: &Env) -> Option<Address> {
    let v = env.storage().instance().get(&DataKey::Bridge);
    common_extend_instance_ttl(env);
    v
}

pub fn set_bridge(env: &Env, v: &Address) {
    env.storage().instance().set(&DataKey::Bridge, v);
    common_extend_instance_ttl(env);
}

pub fn get_broker(env: &Env) -> Option<Address> {
    let v = env.storage().instance().get(&DataKey::Broker);
    common_extend_instance_ttl(env);
    v
}

pub fn set_broker(env: &Env, v: &Address) {
    env.storage().instance().set(&DataKey::Broker, v);
    common_extend_instance_ttl(env);
}

pub fn get_min_gas_floor(env: &Env, chain_id: u32, gas_token: &Address) -> i128 {
    let v = env
        .storage()
        .instance()
        .get(&DataKey::MinGasFloor(chain_id, gas_token.clone()))
        .unwrap_or(0);
    common_extend_instance_ttl(env);
    v
}

pub fn set_min_gas_floor(env: &Env, chain_id: u32, gas_token: &Address, v: i128) {
    env.storage()
        .instance()
        .set(&DataKey::MinGasFloor(chain_id, gas_token.clone()), &v);
    common_extend_instance_ttl(env);
}
