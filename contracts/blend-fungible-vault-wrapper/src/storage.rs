// SPDX-License-Identifier: BUSL-1.1
use soroban_sdk::{Address, Env, contracttype};
use spectra_common::ttl::extend_instance_ttl;

#[contracttype]
pub enum DataKey {
    Pool,
    /// `(ledger_timestamp, b_rate)` — a within-transaction cache of the Blend
    /// reserve's bToken→underlying index. `b_rate` only advances with elapsed
    /// time, so it is invariant across calls at the same timestamp; caching it
    /// lets repeated preview/convert calls in one fill avoid re-entering the
    /// pool (the dominant per-fill memory cost). Temporary storage: a miss
    /// (expired or stale timestamp) simply re-reads the pool.
    CachedBRate,
}

/// The cached `b_rate` iff it was recorded at the current ledger timestamp.
pub fn get_cached_b_rate(env: &Env) -> Option<i128> {
    let entry: Option<(u64, i128)> = env.storage().temporary().get(&DataKey::CachedBRate);
    match entry {
        Some((ts, rate)) if ts == env.ledger().timestamp() => Some(rate),
        _ => None,
    }
}

pub fn set_cached_b_rate(env: &Env, rate: i128) {
    let ts = env.ledger().timestamp();
    env.storage()
        .temporary()
        .set(&DataKey::CachedBRate, &(ts, rate));
}

/// The Blend pool holding this wrapper's bToken position.
pub fn get_pool(env: &Env) -> Address {
    let v = env.storage().instance().get(&DataKey::Pool).unwrap();
    extend_instance_ttl(env);
    v
}

pub fn set_pool(env: &Env, pool: &Address) {
    env.storage().instance().set(&DataKey::Pool, pool);
    extend_instance_ttl(env);
}
