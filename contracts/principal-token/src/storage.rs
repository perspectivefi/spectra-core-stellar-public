// SPDX-License-Identifier: BUSL-1.1
use soroban_sdk::{Address, Env, contracttype};
use spectra_common::ttl::{
    extend_instance_ttl as common_extend_instance_ttl, extend_persistent_ttl,
};
use stellar_access::access_control;

/// Per-user yield state — single persistent entry replaces Solidity's three
/// separate mappings (`ibtRateOfUser`, `ptRateOfUser`, `yieldOfUserInIBT`).
/// Spec-04 §2.4 / §17.
#[contracttype]
#[derive(Clone, Debug, Default)]
pub struct UserYieldData {
    /// User's last-seen IBT rate (Ray).
    pub ibt_rate: i128,
    /// User's last-seen PT rate (Ray).
    pub pt_rate: i128,
    /// Accumulated yield in IBT token-decimals (NOT Ray).
    pub yield_in_ibt: i128,
}

#[contracttype]
pub enum DataKey {
    Ibt,
    Underlying,
    IbtUnit,
    IbtRate,
    PtRate,
    IbtDecimals,
    UnderlyingDecimals,
    Duration,
    Expiry,
    Yt,
    Registry,
    UnclaimedFeesIbt,
    TotalFeesIbt,
    UserYield(Address),
    RatesAtExpiryStored,
    RewardsProxy,
}

/// Admin presence is the canonical "already initialized" beacon. `set_admin`
/// is called exactly once from `PrincipalTokenContract::initialize`; the OZ
/// stellar-access default-admin slot is then non-empty for the rest of the
/// contract's lifetime. Mirrors the registry pattern (`registry::storage`).
pub fn is_initialized(env: &Env) -> bool {
    access_control::get_admin(env).is_some()
}

pub fn extend_instance_ttl(env: &Env) {
    common_extend_instance_ttl(env);
}

macro_rules! addr_field {
    ($get:ident, $set:ident, $key:ident) => {
        pub fn $get(env: &Env) -> Address {
            let v = env.storage().instance().get(&DataKey::$key).unwrap();
            common_extend_instance_ttl(env);
            v
        }
        pub fn $set(env: &Env, v: &Address) {
            env.storage().instance().set(&DataKey::$key, v);
            common_extend_instance_ttl(env);
        }
    };
}

macro_rules! scalar_field {
    ($get:ident, $set:ident, $key:ident, $ty:ty) => {
        pub fn $get(env: &Env) -> $ty {
            let v = env.storage().instance().get(&DataKey::$key).unwrap();
            common_extend_instance_ttl(env);
            v
        }
        pub fn $set(env: &Env, v: $ty) {
            env.storage().instance().set(&DataKey::$key, &v);
            common_extend_instance_ttl(env);
        }
    };
}

addr_field!(get_ibt, set_ibt, Ibt);
addr_field!(get_underlying, set_underlying, Underlying);
addr_field!(get_yt, set_yt, Yt);
addr_field!(get_registry, set_registry, Registry);

scalar_field!(get_ibt_unit, set_ibt_unit, IbtUnit, i128);
scalar_field!(get_ibt_rate, set_ibt_rate, IbtRate, i128);
scalar_field!(get_pt_rate, set_pt_rate, PtRate, i128);
scalar_field!(get_ibt_decimals, set_ibt_decimals, IbtDecimals, u32);
scalar_field!(
    get_underlying_decimals,
    set_underlying_decimals,
    UnderlyingDecimals,
    u32
);
scalar_field!(get_duration, set_duration, Duration, u64);
scalar_field!(get_expiry, set_expiry, Expiry, u64);
scalar_field!(
    get_unclaimed_fees_ibt,
    set_unclaimed_fees_ibt,
    UnclaimedFeesIbt,
    i128
);
scalar_field!(get_total_fees_ibt, set_total_fees_ibt, TotalFeesIbt, i128);

pub fn get_rates_at_expiry_stored(env: &Env) -> bool {
    let v = env
        .storage()
        .instance()
        .get(&DataKey::RatesAtExpiryStored)
        .unwrap_or(false);
    common_extend_instance_ttl(env);
    v
}

pub fn set_rates_at_expiry_stored(env: &Env) {
    env.storage()
        .instance()
        .set(&DataKey::RatesAtExpiryStored, &true);
    common_extend_instance_ttl(env);
}

pub fn try_get_rewards_proxy(env: &Env) -> Option<Address> {
    let v = env.storage().instance().get(&DataKey::RewardsProxy);
    common_extend_instance_ttl(env);
    v
}

pub fn set_rewards_proxy(env: &Env, proxy: &Address) {
    env.storage().instance().set(&DataKey::RewardsProxy, proxy);
    common_extend_instance_ttl(env);
}

pub fn remove_rewards_proxy(env: &Env) {
    env.storage().instance().remove(&DataKey::RewardsProxy);
    common_extend_instance_ttl(env);
}

pub fn get_user_yield(env: &Env, user: &Address) -> UserYieldData {
    let key = DataKey::UserYield(user.clone());
    if env.storage().persistent().has(&key) {
        extend_persistent_ttl(env, &key);
    }
    env.storage().persistent().get(&key).unwrap_or_default()
}

pub fn set_user_yield(env: &Env, user: &Address, data: &UserYieldData) {
    let key = DataKey::UserYield(user.clone());
    env.storage().persistent().set(&key, data);
    extend_persistent_ttl(env, &key);
}
