// SPDX-License-Identifier: BUSL-1.1
use soroban_sdk::contracterror;

#[contracterror]
#[derive(Copy, Clone, Debug, Eq, PartialEq, PartialOrd, Ord)]
#[repr(u32)]
pub enum LimitOrderEngineError {
    AlreadyInitialized = 1,
    /// Order expiry is in the past, making_amount ≤ 0, etc.
    InvalidOrder = 2,
    /// Order expiry is after PT maturity.
    OrderExpiryAfterMaturity = 3,
    /// Order was never registered.
    OrderNotExisting = 4,
    /// Order is already registered.
    OrderAlreadyRegistered = 5,
    /// Order was cancelled or fully filled.
    AlreadyFilled = 6,
    /// `fill_orders` skipped every params entry.
    NoValidOrder = 8,
    /// Mixed `pt` or `order_type` within a single fill batch.
    IncompatibleBatch = 9,
    /// `actual_taking + fee > params.max_taking`.
    MaxTakingExceeded = 11,
    /// Engine balance below required `total_taking`.
    InsufficientFunds = 12,
    InvalidBatchParams = 13,
    /// PT has already reached maturity.
    PtMatured = 14,
    /// `set_fee_rate` value exceeds the cap.
    FeeRateTooHigh = 15,
    /// Negative fee or invalid collector.
    InvalidFeeConfig = 16,
    /// Pricing helper received a negative `implied_rate.
    /// PT_FOR_IBT / IBT_FOR_PT fill path; the registration path
    /// already rejects this earlier.
    NegativeImpliedRate = 17,
    /// I256 -> i128 conversion overflowed in pricing math. Indicates extreme
    /// implied_rate / future_pt_value combinations.
    PriceOverflow = 18,
    /// `FillOrderParams.max_taking` is not strictly positive.
    NonPositiveMaxTaking = 19,
    /// Wrapper conversion (deposit/redeem on Underlying path, or wrap/unwrap
    /// on VaultShare path) produced less than the floor implied by
    /// `order.fail_safe_rate`. The wrapper-native paths may surface this as
    /// the wrapper's own `SlippageProtectionFailed`; both indicate the same
    /// engine-level condition.
    FailSafeRateNotMet = 20,
    /// `MakerTokenType::VaultShare` was used against an IBT that does not
    /// host the `SpectraFungibleVaultWrapper` surface (the `vault_share`
    /// symbol probe failed).
    UnsupportedMakerTokenType = 21,
    /// `order.pt` is not in the configured Registry's PT set.
    PtNotRegistered = 22,
    /// A `cancel_orders_batch` member's `maker` differs from the batch maker.
    /// Batch cancellation is self-service: every order must be the caller's.
    CancelBatchMakerMismatch = 23,
    /// A positive partial fill priced to zero maker proceeds
    /// (`actual_making > 0` but `actual_taking == 0`).
    ZeroTaking = 24,
    /// An inverted-price branch (`IbtForPt` / `IbtForYt`) computed a zero
    /// price divisor — the order cannot be priced and must not divide by zero.
    ZeroPriceDivisor = 25,
    /// `taker_input_token` is neither the canonical taker token nor a
    /// convertible ibt/pt/yt for the batch's order type.
    UnsupportedTakerInputToken = 26,
}
