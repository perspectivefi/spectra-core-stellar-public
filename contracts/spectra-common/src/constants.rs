// SPDX-License-Identifier: BUSL-1.1
use soroban_sdk::{Symbol, symbol_short};

// ---------------------------------------------------------------------------
// Fixed-point constants
// ---------------------------------------------------------------------------

/// 1e18 — standard 18-decimal fixed-point unit
pub const ONE: i128 = 1_000_000_000_000_000_000;

/// Fee divisor (1e18)
pub const FEE_DIVISOR: i128 = ONE;

/// Max tokenization fee: 10% (1e17). Diverges from Solidity (`Registry.sol`)
/// which caps at 1% (1e16).
pub const MAX_TOKENIZATION_FEE: i128 = 100_000_000_000_000_000;

/// Max yield fee: 50% (5e17)
pub const MAX_YIELD_FEE: i128 = 500_000_000_000_000_000;

/// Max PT flash loan fee: 100% (1e18)
pub const MAX_PT_FLASH_LOAN_FEE: i128 = 1_000_000_000_000_000_000;

// ---------------------------------------------------------------------------
// TTL constants
// ---------------------------------------------------------------------------

pub const DAY_IN_LEDGERS: u32 = 17_280; // 86400s / 5s per ledger

/// Per-user persistent storage TTL extension. Aligns with Blend Capital's
/// `LEDGER_BUMP_USER` (~120 days) — established convention for per-user
/// lifecycle data on Soroban. OZ `stellar_tokens::Base` uses 30 days for
/// balances, which assumes near-monthly user activity; PT positions are
/// typically held longer (6mo+ to maturity), so we extend further.
pub const PERSISTENT_TTL_EXTEND: u32 = 120 * DAY_IN_LEDGERS;
pub const PERSISTENT_TTL_THRESHOLD: u32 = PERSISTENT_TTL_EXTEND - 20 * DAY_IN_LEDGERS;

// ---------------------------------------------------------------------------
// Role constants — maps Solidity Roles.sol to Symbol values
//
// Solidity uses uint64 constants checked by AccessManager.
// Soroban uses Symbol values checked by OZ stellar-access #[only_role] macro.
// ---------------------------------------------------------------------------

pub mod roles {
    use super::*;

    /// Solidity: UPGRADE_ROLE = 1
    pub const UPGRADE: Symbol = symbol_short!("UPGRADE");
    /// Solidity: PAUSER_ROLE = 2
    pub const PAUSER: Symbol = symbol_short!("PAUSER");
    /// Solidity: FEE_SETTER_ROLE = 3
    pub const FEE_SETTER: Symbol = symbol_short!("FEE_SET");
    /// Solidity: REGISTRY_ROLE = 4
    pub const REGISTRY: Symbol = symbol_short!("REGISTRY");
    /// Solidity: REWARDS_HARVESTER_ROLE = 5
    pub const REWARDS_HARVESTER: Symbol = symbol_short!("RWD_HARV");
    /// Solidity: REWARDS_PROXY_SETTER_ROLE = 6
    pub const REWARDS_PROXY_SETTER: Symbol = symbol_short!("RWD_PROX");
    /// Solidity: RATE_ADJUSTMENT_ORACLE_SETTER_ROLE = 14
    pub const RATE_ORACLE_SETTER: Symbol = symbol_short!("RATE_SET");
    /// Solidity: ROUTER_MANAGER_ROLE = 15
    pub const ROUTER_MANAGER: Symbol = symbol_short!("RTR_MGR");
    /// Soroban-only: gates Factory setters (set_authority).
    /// No direct Solidity equivalent — Solidity Factory uses AccessManager
    /// `restricted` modifier instead.
    pub const FACTORY: Symbol = symbol_short!("FACTORY");
}

#[cfg(test)]
mod tests {
    #![allow(clippy::assertions_on_constants)]
    use super::*;

    // Regression: pin every published constant value. If any of these
    // change unintentionally, the test fails loudly.

    #[test]
    fn test_one_constant_is_1e18() {
        assert_eq!(ONE, 1_000_000_000_000_000_000);
    }

    #[test]
    fn test_fee_divisor_equals_one() {
        assert_eq!(FEE_DIVISOR, ONE);
    }

    #[test]
    fn test_max_tokenization_fee_is_10pct_of_one() {
        // 10 % of 1e18
        assert_eq!(MAX_TOKENIZATION_FEE, 100_000_000_000_000_000);
        assert_eq!(MAX_TOKENIZATION_FEE * 10, ONE);
    }

    #[test]
    fn test_max_yield_fee_is_50pct_of_one() {
        assert_eq!(MAX_YIELD_FEE, 500_000_000_000_000_000);
        assert_eq!(MAX_YIELD_FEE * 2, ONE);
    }

    #[test]
    fn test_max_pt_flash_loan_fee_is_100pct_of_one() {
        assert_eq!(MAX_PT_FLASH_LOAN_FEE, ONE);
    }

    // Invariant: every fee max must be ≤ FEE_DIVISOR.
    #[test]
    fn test_no_fee_max_exceeds_fee_divisor() {
        assert!(MAX_TOKENIZATION_FEE <= FEE_DIVISOR);
        assert!(MAX_YIELD_FEE <= FEE_DIVISOR);
        assert!(MAX_PT_FLASH_LOAN_FEE <= FEE_DIVISOR);
    }

    // Ordering invariant: tokenization ≤ yield ≤ flash-loan (current spec).
    // Pins that yield isn't accidentally lowered below tokenization.
    #[test]
    fn test_fee_max_ordering() {
        assert!(MAX_TOKENIZATION_FEE <= MAX_YIELD_FEE);
        assert!(MAX_YIELD_FEE <= MAX_PT_FLASH_LOAN_FEE);
    }

    #[test]
    fn test_day_in_ledgers_assumes_5s_close() {
        // 86_400s / 5s = 17_280
        assert_eq!(DAY_IN_LEDGERS, 17_280);
        assert_eq!(DAY_IN_LEDGERS * 5, 86_400);
    }

    #[test]
    fn test_persistent_ttl_extend_is_120_days() {
        assert_eq!(PERSISTENT_TTL_EXTEND, 120 * DAY_IN_LEDGERS);
    }

    #[test]
    fn test_persistent_ttl_threshold_is_extend_minus_20_days() {
        assert_eq!(
            PERSISTENT_TTL_THRESHOLD,
            PERSISTENT_TTL_EXTEND - 20 * DAY_IN_LEDGERS
        );
    }

    /// Invariant: threshold MUST be strictly less than extend, otherwise a
    /// no-op extend call could underflow / mis-bump.
    #[test]
    fn test_persistent_ttl_threshold_lt_extend() {
        assert!(PERSISTENT_TTL_THRESHOLD < PERSISTENT_TTL_EXTEND);
    }

    /// Sanity: 20 days of headroom between threshold and extend.
    /// (Threshold = extend - 20 days; the trigger-window for a bump.)
    #[test]
    fn test_persistent_ttl_headroom_is_20_days() {
        assert_eq!(
            PERSISTENT_TTL_EXTEND - PERSISTENT_TTL_THRESHOLD,
            20 * DAY_IN_LEDGERS
        );
    }

    /// Pin the role symbols by their short-string identity.
    #[test]
    fn test_role_symbols_match_documented_names() {
        use roles::*;
        assert_eq!(UPGRADE, symbol_short!("UPGRADE"));
        assert_eq!(PAUSER, symbol_short!("PAUSER"));
        assert_eq!(FEE_SETTER, symbol_short!("FEE_SET"));
        assert_eq!(REGISTRY, symbol_short!("REGISTRY"));
        assert_eq!(REWARDS_HARVESTER, symbol_short!("RWD_HARV"));
        assert_eq!(REWARDS_PROXY_SETTER, symbol_short!("RWD_PROX"));
        assert_eq!(RATE_ORACLE_SETTER, symbol_short!("RATE_SET"));
        assert_eq!(ROUTER_MANAGER, symbol_short!("RTR_MGR"));
        assert_eq!(FACTORY, symbol_short!("FACTORY"));
    }
}
