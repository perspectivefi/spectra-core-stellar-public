// SPDX-License-Identifier: BUSL-1.1
use soroban_sdk::{Env, IntoVal, Symbol, Val, contracterror, panic_with_error, symbol_short};

use crate::constants::{PERSISTENT_TTL_EXTEND, PERSISTENT_TTL_THRESHOLD};

/// Errors raised by the shared TTL helpers.
#[contracterror]
#[derive(Copy, Clone, Debug, Eq, PartialEq, PartialOrd, Ord)]
#[repr(u32)]
pub enum TtlError {
    /// `set_secs_per_ledger` was called with a non-positive value.
    NonPositiveSecsPerLedger = 210,
}

/// Instance-storage key holding the configured ledger close time in seconds.
/// Stored per-contract so each consumer can tune it independently.
const SECS_PER_LEDGER_KEY: Symbol = symbol_short!("SECS_LDG");

/// Default ledger close time in seconds when no value has been configured.
/// Chosen below Stellar's ~5 s mainnet target so that ledger counts derived
/// from this constant overshoot wall-clock duration — TTL extensions keep
/// headroom even if the network speeds up.
pub const DEFAULT_SECS_PER_LEDGER: u64 = 4;

/// Bump every entry under `env.storage().instance()` to the host-max TTL.
/// Call from every state-changing entry point.
pub fn extend_instance_ttl(env: &Env) {
    let max = env.storage().max_ttl();
    env.storage().instance().extend_ttl(max, max);
}

/// Bump a single persistent entry's TTL using the workspace's standard
/// per-user thresholds. Call from write paths and from read-only views that
/// touch the entry (otherwise frequently-read but rarely-written entries
/// archive while their owning contract instance stays alive).
pub fn extend_persistent_ttl<K>(env: &Env, key: &K)
where
    K: IntoVal<Env, Val>,
{
    env.storage()
        .persistent()
        .extend_ttl(key, PERSISTENT_TTL_THRESHOLD, PERSISTENT_TTL_EXTEND);
}

/// Configure the seconds-per-ledger value used by TTL math in the caller.
/// Stored under instance storage; auth is the caller's responsibility.
pub fn set_secs_per_ledger(env: &Env, secs: u64) {
    if secs == 0 {
        panic_with_error!(env, TtlError::NonPositiveSecsPerLedger);
    }
    env.storage().instance().set(&SECS_PER_LEDGER_KEY, &secs);
}

/// Read the configured seconds-per-ledger, falling back to
/// `DEFAULT_SECS_PER_LEDGER` when the caller has never set one.
pub fn get_secs_per_ledger(env: &Env) -> u64 {
    env.storage()
        .instance()
        .get(&SECS_PER_LEDGER_KEY)
        .unwrap_or(DEFAULT_SECS_PER_LEDGER)
}

#[cfg(test)]
mod tests {
    use super::*;
    use soroban_sdk::{Address, contract, contractimpl};

    #[contract]
    struct TestHarness;

    #[contractimpl]
    impl TestHarness {
        pub fn set(env: Env, secs: u64) {
            set_secs_per_ledger(&env, secs);
        }
        pub fn get(env: Env) -> u64 {
            get_secs_per_ledger(&env)
        }
        pub fn extend_inst(env: Env) {
            extend_instance_ttl(&env);
        }
    }

    fn harness() -> (Env, Address) {
        let env = Env::default();
        let addr = env.register(TestHarness, ());
        (env, addr)
    }

    /// Default value returned when unset.
    #[test]
    fn test_default_secs_per_ledger_returned_when_unset() {
        let (env, addr) = harness();
        let v: u64 = env.as_contract(&addr, || get_secs_per_ledger(&env));
        assert_eq!(v, DEFAULT_SECS_PER_LEDGER);
    }

    /// Regression: pin the default value to 4 seconds.
    #[test]
    fn test_default_secs_per_ledger_is_4() {
        assert_eq!(DEFAULT_SECS_PER_LEDGER, 4);
    }

    /// set → get round trip.
    #[test]
    fn test_set_get_round_trip() {
        let (env, addr) = harness();
        env.as_contract(&addr, || set_secs_per_ledger(&env, 7));
        let v: u64 = env.as_contract(&addr, || get_secs_per_ledger(&env));
        assert_eq!(v, 7);
    }

    /// `set_secs_per_ledger(0)` panics with the structured `TtlError`.
    #[test]
    fn test_set_zero_panics() {
        let (env, addr) = harness();
        let h = TestHarnessClient::new(&env, &addr);
        let r = h.try_set(&0);
        assert_eq!(
            r,
            Err(Ok(soroban_sdk::Error::from_contract_error(
                TtlError::NonPositiveSecsPerLedger as u32
            )))
        );
    }

    /// `set_secs_per_ledger(u64::MAX)` succeeds — no upper bound.
    #[test]
    fn test_set_u64_max_succeeds() {
        let (env, addr) = harness();
        env.as_contract(&addr, || set_secs_per_ledger(&env, u64::MAX));
        let v: u64 = env.as_contract(&addr, || get_secs_per_ledger(&env));
        assert_eq!(v, u64::MAX);
    }

    /// Overwrite semantics: last write wins.
    #[test]
    fn test_overwrite_replaces_previous_value() {
        let (env, addr) = harness();
        env.as_contract(&addr, || set_secs_per_ledger(&env, 3));
        env.as_contract(&addr, || set_secs_per_ledger(&env, 10));
        let v: u64 = env.as_contract(&addr, || get_secs_per_ledger(&env));
        assert_eq!(v, 10);
    }

    /// `extend_instance_ttl` is callable in a contract context without
    /// panicking. Smoke test.
    #[test]
    fn test_extend_instance_ttl_smoke() {
        let (env, addr) = harness();
        env.as_contract(&addr, || extend_instance_ttl(&env));
    }

    /// `extend_persistent_ttl` smoke — instance writes a sample persistent
    /// entry first so there's something to extend.
    #[test]
    fn test_extend_persistent_ttl_smoke() {
        let (env, addr) = harness();
        let key = symbol_short!("TESTKEY");
        env.as_contract(&addr, || {
            env.storage().persistent().set(&key, &123i128);
            extend_persistent_ttl(&env, &key);
        });
    }
}
