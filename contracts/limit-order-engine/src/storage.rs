// SPDX-License-Identifier: BUSL-1.1
use soroban_sdk::{Address, BytesN, Env, contracttype};
use spectra_common::limit_order_engine::OrderStatus;
use spectra_common::ttl::{extend_instance_ttl as common_extend_instance_ttl, get_secs_per_ledger};
use stellar_access::access_control;

// ---------------------------------------------------------------------------
// Storage keys
// ---------------------------------------------------------------------------

#[contracttype]
#[derive(Clone)]
pub enum DataKey {
    // Instance — fee config (admin/roles managed by stellar-access;
    // admin presence = initialized)
    FeeRate,
    FeeCollector,
    /// Instance — Registry address. Set at `initialize`, rotatable via the
    /// `REGISTRY`-gated `set_registry`.
    Registry,
    // Persistent — order book. Existence ⇔ registered.
    OrderStatus(BytesN<32>),
}

// ---------------------------------------------------------------------------
// Initialization sentinel
// ---------------------------------------------------------------------------

pub fn is_initialized(env: &Env) -> bool {
    access_control::get_admin(env).is_some()
}

pub fn extend_instance_ttl(env: &Env) {
    common_extend_instance_ttl(env);
}

// ---------------------------------------------------------------------------
// Fee config (instance)
// ---------------------------------------------------------------------------

pub fn get_fee_rate(env: &Env) -> i128 {
    env.storage().instance().get(&DataKey::FeeRate).unwrap()
}

pub fn set_fee_rate(env: &Env, fee_rate: i128) {
    env.storage().instance().set(&DataKey::FeeRate, &fee_rate);
}

pub fn get_fee_collector(env: &Env) -> Address {
    env.storage()
        .instance()
        .get(&DataKey::FeeCollector)
        .unwrap()
}

pub fn set_fee_collector(env: &Env, fee_collector: &Address) {
    env.storage()
        .instance()
        .set(&DataKey::FeeCollector, fee_collector);
}

pub fn get_registry(env: &Env) -> Address {
    env.storage().instance().get(&DataKey::Registry).unwrap()
}

pub fn set_registry(env: &Env, registry: &Address) {
    env.storage().instance().set(&DataKey::Registry, registry);
}

// ---------------------------------------------------------------------------
// Order status (persistent)
//
// TTL is scoped to `order.expiry` rather than the workspace default
// `PERSISTENT_TTL_EXTEND` (~120 days). Past expiry the entry is useless —
// `is_order_valid` rejects fills and `cancel_order` already tolerates
// `get_order_status` returning `None` — so we let the entry archive
// naturally instead of paying rent for the standard 120-day window.
// ---------------------------------------------------------------------------

pub fn get_order_status(env: &Env, order_id: &BytesN<32>) -> Option<OrderStatus> {
    env.storage()
        .persistent()
        .get(&DataKey::OrderStatus(order_id.clone()))
}

pub fn set_order_status(env: &Env, order_id: &BytesN<32>, expiry: u64, status: &OrderStatus) {
    let key = DataKey::OrderStatus(order_id.clone());
    env.storage().persistent().set(&key, status);
    extend_order_status_ttl(env, &key, expiry);
}

pub fn has_order_status(env: &Env, order_id: &BytesN<32>) -> bool {
    env.storage()
        .persistent()
        .has(&DataKey::OrderStatus(order_id.clone()))
}

/// Bump the `OrderStatus` entry's TTL just far enough to cover `expiry`.
/// Caller must pass a non-expired `expiry`; every write path
/// (`register_order`, `cancel_order`, `fill_order`) reverts on `expiry <= now`
/// before reaching this helper.
fn extend_order_status_ttl(env: &Env, key: &DataKey, expiry: u64) {
    let now = env.ledger().timestamp();
    let ledgers_left = u32::try_from((expiry - now) / get_secs_per_ledger(env)).unwrap_or(u32::MAX);
    let extend_to = ledgers_left.min(env.storage().max_ttl());
    let threshold = extend_to / 2;
    env.storage()
        .persistent()
        .extend_ttl(key, threshold, extend_to);
}
