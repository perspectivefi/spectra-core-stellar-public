// SPDX-License-Identifier: BUSL-1.1
use soroban_sdk::{Address, BytesN, Env, contracttype, panic_with_error};
use spectra_common::constants::{PERSISTENT_TTL_EXTEND, PERSISTENT_TTL_THRESHOLD};
use stellar_access::access_control;

use crate::errors::RegistryError;

// ---------------------------------------------------------------------------
// Storage keys
// ---------------------------------------------------------------------------

#[contracttype]
#[derive(Clone)]
pub enum DataKey {
    // Instance storage — protocol addresses and config
    // (admin/roles managed by stellar-access; admin presence = initialized)
    Factory,
    Router,
    PtWasmHash,
    YtWasmHash,
    TokenizationFee,
    YieldFee,
    PtFlashLoanFee,
    FeeCollector,
    Bridge,
    LimitOrderEngine,

    // Persistent storage — per-entity data
    IsPt(Address),                  // pt -> index (existence = registered)
    PtAt(u32),                      // index -> pt address (write-once, archivable)
    PtCount,                        // next index to assign
    FeeReduction(Address, Address), // (pt, user)
}

// ---------------------------------------------------------------------------
// Instance storage helpers
// ---------------------------------------------------------------------------

pub fn is_initialized(env: &Env) -> bool {
    access_control::get_admin(env).is_some()
}

pub fn get_factory(env: &Env) -> Address {
    let val: Address = env.storage().instance().get(&DataKey::Factory).unwrap();
    extend_instance_ttl(env);
    val
}

pub fn try_get_factory(env: &Env) -> Option<Address> {
    let val: Option<Address> = env.storage().instance().get(&DataKey::Factory);
    extend_instance_ttl(env);
    val
}

pub fn set_factory(env: &Env, factory: &Address) {
    env.storage().instance().set(&DataKey::Factory, factory);
    extend_instance_ttl(env);
}

pub fn get_router(env: &Env) -> Address {
    let val: Address = env.storage().instance().get(&DataKey::Router).unwrap();
    extend_instance_ttl(env);
    val
}

pub fn try_get_router(env: &Env) -> Option<Address> {
    let val: Option<Address> = env.storage().instance().get(&DataKey::Router);
    extend_instance_ttl(env);
    val
}

pub fn set_router(env: &Env, router: &Address) {
    env.storage().instance().set(&DataKey::Router, router);
    extend_instance_ttl(env);
}

pub fn get_pt_wasm_hash(env: &Env) -> BytesN<32> {
    let val: BytesN<32> = env.storage().instance().get(&DataKey::PtWasmHash).unwrap();
    extend_instance_ttl(env);
    val
}

pub fn try_get_pt_wasm_hash(env: &Env) -> Option<BytesN<32>> {
    let val: Option<BytesN<32>> = env.storage().instance().get(&DataKey::PtWasmHash);
    extend_instance_ttl(env);
    val
}

pub fn set_pt_wasm_hash(env: &Env, hash: &BytesN<32>) {
    env.storage().instance().set(&DataKey::PtWasmHash, hash);
    extend_instance_ttl(env);
}

pub fn get_yt_wasm_hash(env: &Env) -> BytesN<32> {
    let val: BytesN<32> = env.storage().instance().get(&DataKey::YtWasmHash).unwrap();
    extend_instance_ttl(env);
    val
}

pub fn try_get_yt_wasm_hash(env: &Env) -> Option<BytesN<32>> {
    let val: Option<BytesN<32>> = env.storage().instance().get(&DataKey::YtWasmHash);
    extend_instance_ttl(env);
    val
}

pub fn set_yt_wasm_hash(env: &Env, hash: &BytesN<32>) {
    env.storage().instance().set(&DataKey::YtWasmHash, hash);
    extend_instance_ttl(env);
}

pub fn get_tokenization_fee(env: &Env) -> i128 {
    let val: i128 = env
        .storage()
        .instance()
        .get(&DataKey::TokenizationFee)
        .unwrap_or(0);
    extend_instance_ttl(env);
    val
}

pub fn set_tokenization_fee(env: &Env, fee: i128) {
    env.storage()
        .instance()
        .set(&DataKey::TokenizationFee, &fee);
    extend_instance_ttl(env);
}

pub fn get_yield_fee(env: &Env) -> i128 {
    let val: i128 = env
        .storage()
        .instance()
        .get(&DataKey::YieldFee)
        .unwrap_or(0);
    extend_instance_ttl(env);
    val
}

pub fn set_yield_fee(env: &Env, fee: i128) {
    env.storage().instance().set(&DataKey::YieldFee, &fee);
    extend_instance_ttl(env);
}

pub fn get_pt_flash_loan_fee(env: &Env) -> i128 {
    let val: i128 = env
        .storage()
        .instance()
        .get(&DataKey::PtFlashLoanFee)
        .unwrap_or(0);
    extend_instance_ttl(env);
    val
}

pub fn set_pt_flash_loan_fee(env: &Env, fee: i128) {
    env.storage().instance().set(&DataKey::PtFlashLoanFee, &fee);
    extend_instance_ttl(env);
}

pub fn get_fee_collector(env: &Env) -> Address {
    let val: Address = env
        .storage()
        .instance()
        .get(&DataKey::FeeCollector)
        .unwrap();
    extend_instance_ttl(env);
    val
}

pub fn try_get_fee_collector(env: &Env) -> Option<Address> {
    let val: Option<Address> = env.storage().instance().get(&DataKey::FeeCollector);
    extend_instance_ttl(env);
    val
}

pub fn set_fee_collector(env: &Env, collector: &Address) {
    env.storage()
        .instance()
        .set(&DataKey::FeeCollector, collector);
    extend_instance_ttl(env);
}

pub fn get_bridge(env: &Env) -> Address {
    let val: Address = env.storage().instance().get(&DataKey::Bridge).unwrap();
    extend_instance_ttl(env);
    val
}

pub fn try_get_bridge(env: &Env) -> Option<Address> {
    let val: Option<Address> = env.storage().instance().get(&DataKey::Bridge);
    extend_instance_ttl(env);
    val
}

pub fn set_bridge(env: &Env, bridge: &Address) {
    env.storage().instance().set(&DataKey::Bridge, bridge);
    extend_instance_ttl(env);
}

pub fn get_limit_order_engine(env: &Env) -> Address {
    let val: Address = env
        .storage()
        .instance()
        .get(&DataKey::LimitOrderEngine)
        .unwrap();
    extend_instance_ttl(env);
    val
}

pub fn try_get_limit_order_engine(env: &Env) -> Option<Address> {
    let val: Option<Address> = env.storage().instance().get(&DataKey::LimitOrderEngine);
    extend_instance_ttl(env);
    val
}

pub fn set_limit_order_engine(env: &Env, loe: &Address) {
    env.storage()
        .instance()
        .set(&DataKey::LimitOrderEngine, loe);
    extend_instance_ttl(env);
}

// ---------------------------------------------------------------------------
// Persistent storage helpers — PT registration
// ---------------------------------------------------------------------------

pub fn is_registered_pt(env: &Env, pt: &Address) -> bool {
    let key = DataKey::IsPt(pt.clone());
    let exists = env.storage().persistent().has(&key);
    if exists {
        extend_persistent_ttl(env, &key);
    }
    exists
}

pub fn set_pt_registered(env: &Env, pt: &Address) {
    let count = get_pt_count(env);
    let is_pt_key = DataKey::IsPt(pt.clone());
    let pt_at_key = DataKey::PtAt(count);

    env.storage().persistent().set(&is_pt_key, &count);
    env.storage().persistent().set(&pt_at_key, pt);
    let new_count = match count.checked_add(1) {
        Some(v) => v,
        None => panic_with_error!(env, RegistryError::PtListUpdateFailed),
    };
    env.storage()
        .persistent()
        .set(&DataKey::PtCount, &new_count);

    extend_persistent_ttl(env, &is_pt_key);
    extend_persistent_ttl(env, &pt_at_key);
    extend_persistent_ttl(env, &DataKey::PtCount);
}

pub fn remove_pt_registered(env: &Env, pt: &Address) {
    let is_pt_key = DataKey::IsPt(pt.clone());
    if let Some(idx) = env.storage().persistent().get::<_, u32>(&is_pt_key) {
        env.storage().persistent().remove(&DataKey::PtAt(idx));
    }
    env.storage().persistent().remove(&is_pt_key);
}

pub fn get_pt_count(env: &Env) -> u32 {
    let val: u32 = env
        .storage()
        .persistent()
        .get(&DataKey::PtCount)
        .unwrap_or(0);
    if env.storage().persistent().has(&DataKey::PtCount) {
        extend_persistent_ttl(env, &DataKey::PtCount);
    }
    val
}

pub fn get_pt_at(env: &Env, index: u32) -> Option<Address> {
    let key = DataKey::PtAt(index);
    let val: Option<Address> = env.storage().persistent().get(&key);
    if val.is_some() {
        extend_persistent_ttl(env, &key);
    }
    val
}

// ---------------------------------------------------------------------------
// Persistent storage helpers — Fee reductions
// ---------------------------------------------------------------------------

pub fn get_fee_reduction(env: &Env, pt: &Address, user: &Address) -> i128 {
    let key = DataKey::FeeReduction(pt.clone(), user.clone());
    let val: i128 = env.storage().persistent().get(&key).unwrap_or(0);
    if env.storage().persistent().has(&key) {
        extend_persistent_ttl(env, &key);
    }
    val
}

pub fn set_fee_reduction(env: &Env, pt: &Address, user: &Address, reduction: i128) {
    let key = DataKey::FeeReduction(pt.clone(), user.clone());
    env.storage().persistent().set(&key, &reduction);
    extend_persistent_ttl(env, &key);
}

// ---------------------------------------------------------------------------
// TTL management
// ---------------------------------------------------------------------------

pub fn extend_instance_ttl(env: &Env) {
    let max = env.storage().max_ttl();
    env.storage().instance().extend_ttl(max, max);
}

fn extend_persistent_ttl(env: &Env, key: &DataKey) {
    env.storage()
        .persistent()
        .extend_ttl(key, PERSISTENT_TTL_THRESHOLD, PERSISTENT_TTL_EXTEND);
}
