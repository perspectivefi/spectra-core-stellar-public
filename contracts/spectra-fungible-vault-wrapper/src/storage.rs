// SPDX-License-Identifier: BUSL-1.1
use soroban_sdk::{Address, Env, contracttype};
use spectra_common::ttl::extend_instance_ttl as common_extend_instance_ttl;
use stellar_access::access_control;

/// Wrapper-instance storage keys.
///
/// **Reserved variant names — must NOT be reused.**
///
/// A contract has one shared storage namespace. `#[contracttype]` turns each
/// enum variant into a key identified only by the variant's *name* (encoded as
/// a `Symbol`) — the enum type is not part of the key. So `DataKey::Admin` here
/// and `AccessControlStorageKey::Admin` in the `stellar_access` dependency
/// produce the byte-identical key `"Admin"` and read/write the same storage
/// slot. The OZ modules below already own these names; reusing one here would
/// silently alias its storage (e.g. our admin would overwrite the
/// access-control admin) with no compile-time error. (A unit variant encodes as
/// `Symbol("Name")` and a tuple variant as `Vec([Symbol("Name"), args…])`, so
/// an exact byte-collision needs the same *shape* too — but treat the name
/// alone as reserved; matching a name with a different shape is still a latent
/// hazard.) Do not name a variant any of the following (grouped by the module
/// that already uses it):
///
/// - `stellar_tokens::vault`            → `AssetAddress`, `VirtualDecimalsOffset`
/// - `stellar_tokens::fungible` (Base)  → `Meta`, `TotalSupply`, `Balance`, `Allowance`
/// - `stellar_contract_utils::pausable` → `Paused`
/// - `stellar_access::access_control`   → `ExistingRoles`, `RoleAccounts`, `HasRole`,
///                                        `RoleAccountsCount`, `RoleAdmin`, `Admin`,
///                                        `PendingAdmin`
#[contracttype]
pub enum DataKey {
    VaultShare,
    RewardsProxy,
}

/// The wrapped vault-share token.
pub fn get_vault_share(env: &Env) -> Address {
    let v = env.storage().instance().get(&DataKey::VaultShare).unwrap();
    common_extend_instance_ttl(env);
    v
}

pub fn set_vault_share(env: &Env, vault_share: &Address) {
    env.storage()
        .instance()
        .set(&DataKey::VaultShare, vault_share);
    common_extend_instance_ttl(env);
}

/// The rewards proxy, or `None` when unset (Solidity's address-zero = disabled).
pub fn get_rewards_proxy(env: &Env) -> Option<Address> {
    let v = env.storage().instance().get(&DataKey::RewardsProxy);
    common_extend_instance_ttl(env);
    v
}

pub fn set_rewards_proxy(env: &Env, proxy: Option<Address>) {
    match proxy {
        Some(addr) => env.storage().instance().set(&DataKey::RewardsProxy, &addr),
        None => {
            env.storage().instance().remove(&DataKey::RewardsProxy);
        }
    }
    common_extend_instance_ttl(env);
}

pub fn is_initialized(env: &Env) -> bool {
    access_control::get_admin(env).is_some()
}

pub fn extend_instance_ttl(env: &Env) {
    common_extend_instance_ttl(env);
}
