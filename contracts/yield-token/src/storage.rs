// SPDX-License-Identifier: BUSL-1.1
use soroban_sdk::{Address, Env, contracttype};
use spectra_common::ttl::extend_instance_ttl as common_extend_instance_ttl;

#[contracttype]
#[derive(Clone)]
pub enum DataKey {
    // Instance (pt presence = initialized).
    // name/symbol/decimals/balances/allowances/total_supply are managed by
    // stellar-tokens `Base`. Pause flag is managed by
    // stellar_contract_utils::pausable.
    Pt,
    Maturity,
}

pub fn is_initialized(env: &Env) -> bool {
    env.storage().instance().has(&DataKey::Pt)
}

pub fn get_pt(env: &Env) -> Address {
    env.storage().instance().get(&DataKey::Pt).unwrap()
}

pub fn set_pt(env: &Env, pt: &Address) {
    env.storage().instance().set(&DataKey::Pt, pt);
}

pub fn get_maturity(env: &Env) -> u64 {
    env.storage().instance().get(&DataKey::Maturity).unwrap()
}

pub fn set_maturity(env: &Env, maturity: u64) {
    env.storage().instance().set(&DataKey::Maturity, &maturity);
}

pub fn extend_instance_ttl(env: &Env) {
    common_extend_instance_ttl(env);
}
