// SPDX-License-Identifier: BUSL-1.1
use soroban_sdk::contracterror;

#[contracterror]
#[derive(Copy, Clone, Debug, Eq, PartialEq, PartialOrd, Ord)]
#[repr(u32)]
pub enum PrincipalTokenError {
    AlreadyInitialized = 1,
    RateError = 2,
    InvalidDecimals = 3,
    UnauthorizedFeeCollector = 4,
    /// `claim_yield` / `claim_yield_in_ibt`: actual amount below caller's `min_*` floor.
    SlippageProtectionFailed = 5,
    /// `before_yt_transfer`: caller is not the paired YT.
    UnauthorizedCaller = 6,
    /// `store_rates_at_expiry`: PT not yet at maturity.
    NotExpired = 7,
    /// `store_rates_at_expiry`: rates already frozen.
    RatesAtExpiryAlreadyStored = 8,
    /// `_before_redeem` / `_before_withdraw`: shares > _max_burnable.
    InsufficientBalance = 9,
    /// `deposit` / `deposit_ibt`: PT already at or past maturity.
    /// Mirrors Solidity `PTExpired()`.
    Expired = 10,
    /// `claim_fees`: no unclaimed fees to claim.
    NoFeesToClaim = 11,
    /// `compute_tokenization_fee_base`: registry returned a `tokenization_fee`
    /// outside `[0, FEE_DIVISOR)`. Defense-in-depth against a compromised
    /// registry that bypasses its own `MAX_TOKENIZATION_FEE` cap.
    InvalidTokenizationFee = 12,
    /// `compute_tokenization_fee`: registry returned a per-user `fee_reduction`
    /// outside `[0, FEE_DIVISOR]`. A `fee_reduction > FEE_DIVISOR` would let the
    /// user mint more shares than IBT deposited; negative values are rejected
    /// per the Solidity unsigned-port rule.
    InvalidFeeReduction = 13,
    /// `claim_rewards`: no rewards proxy installed.
    NoRewardsProxy = 14,
    /// `compute_yield`: a signed input that maps to a Solidity `uint*` field
    /// (`user_yt_balance`, `current_yield_in_ibt`) was passed as negative.
    NegativeAmount = 16,
    /// `claim_fees`: registry has not configured a fee_collector.
    /// Surfaces the `try_get_fee_collector` empty case as a structured
    /// error rather than the unstructured host panic from a `.unwrap()` on
    /// `None`.
    FeeCollectorNotSet = 15,
}
