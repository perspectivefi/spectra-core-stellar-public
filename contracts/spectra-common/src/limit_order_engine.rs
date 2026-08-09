// SPDX-License-Identifier: BUSL-1.1
//! LimitOrderEngine
//! Off-chain signed orders translate to Soroban auth entries: maker calls
//! `register_order(order)` either directly or via a backend that submits the
//! maker's pre-signed auth entry. The contract validates with `maker.require_auth()`
//! and the host handles signature verification + nonce/expiration replay protection.
//!
//! ## Auth-entry expiry vs `order.expiry`
//!
//! The maker's pre-signed Soroban auth entry carries its own
//! `signature_expiration_ledger`, independent of `order.expiry` (the on-chain
//! field validated by `is_order_valid`). If a backend sets the auth-entry
//! expiration ledger below the wall-clock equivalent of `order.expiry`, the
//! pre-signed entry rots before the order itself does, and the order becomes
//! unfillable even while it is still logically valid. Backends pre-signing
//! `register_order` should set the auth-entry expiration ledger to at least
//! the `order.expiry` wall-clock time.
//!

use soroban_sdk::{Address, BytesN, Env, Vec, contractclient, contracttype};

// ---------------------------------------------------------------------------
// Order types
// ---------------------------------------------------------------------------

/// Order direction. Solidity `OrderType` enum (ILimitOrderEngine.sol).
/// `#[contracttype]` unit-variant enums serialize as the variant name
/// (`Symbol`), so off-chain consumers see `"IbtForPt"` on the wire — not a
/// numeric discriminant. Cross-chain consumers must therefore match on the
/// variant name, not on a numeric value shared with the Solidity enum.
#[contracttype]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum OrderType {
    /// maker sells IBT, taker pays PT
    IbtForPt,
    /// maker sells PT, taker pays IBT
    PtForIbt,
    /// maker sells IBT, taker pays YT
    IbtForYt,
    /// maker sells YT, taker pays IBT
    YtForIbt,
}

/// Denomination the maker funds (or receives, on payout-side) the IBT leg
/// in. Mirrors Solidity `MakerTokenType` from PR #22. `Ibt` is the no-conversion
/// path and equals `pt.get_ibt()` (whether or not that IBT is a Spectra wrapper).
#[contracttype]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MakerTokenType {
    Ibt,
    VaultShare,
    Underlying,
}

#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Order {
    /// Disambiguates otherwise-identical orders (same maker/pt/amount/rate)
    pub salt: u64,
    /// Order expiry — unix seconds. Must satisfy `now < expiry <= pt.maturity`.
    pub expiry: u64,
    pub order_type: OrderType,
    /// PrincipalToken contract for which this order is placed.
    pub pt: Address,
    pub maker: Address,
    /// Where the maker receives the taker's payment.
    pub receiver: Address,
    /// Total amount of the making token offered (token-decimals of the making side).
    pub making_amount: i128,
    /// Implied APY, 18-decimal fixed-point (1e18 = 100%). Used by the pricing
    /// library to compute spot + fee-adjusted prices.
    pub implied_rate: i128,
    /// Denomination of the IBT leg. For IBT-funding order types (`IbtForPt`,
    /// `IbtForYt`) governs what the maker deposits. For IBT-payout types
    /// (`PtForIbt`, `YtForIbt`) governs what the maker receives. `Ibt` is the
    /// passthrough; `Underlying` and `VaultShare` route through the IBT's
    /// `SpectraFungibleVaultWrapper` surface and require `pt.get_ibt()` to
    /// host that wrapper.
    pub maker_token_type: MakerTokenType,
    /// Slippage floor for the wrapper conversions, expressed as **output per
    /// input**: the minimum raw output-token amount per `1e18` raw input-token
    /// units. `0` disables the floor.
    ///
    /// The engine applies it as `floor = amount * fail_safe_rate / 1e18` against
    /// the realized output of `deposit` / `redeem` (Underlying path), or passes
    /// it natively as `min_shares` / `min_vault_shares` to `wrap_with_min_shares`
    /// / `unwrap_with_min_vault_shares` (VaultShare path).
    ///
    /// Being raw-output-per-raw-input it carries the output token's decimals and
    /// so implicitly encodes the underlying↔IBT decimal gap — set it to
    /// `previewConversion(1e18)` of the order's conversion (`previewDeposit`
    /// funding, `previewRedeem` payout, wrap/unwrap preview for VaultShare). It
    /// is *not* a decimal-agnostic value ratio: a gapped market holds a large
    /// market-specific value (e.g. ~`1e30` for a value-preserving
    /// 6-dec-underlying / 18-dec-IBT deposit), not `~1e18`, and no decimal
    /// rescaling is applied on-chain.
    pub fail_safe_rate: i128,
}

/// Recorded state of a registered order. Existence in persistent storage
/// implies "registered"; `remaining == 0` post-registration means cancelled
/// or fully filled.
/// **Raw encoding (Solidity-compatible sentinel)**: `remaining == 0` ⇒ never
/// registered (Solidity `_ORDER_DOES_NOT_EXIST`); `remaining == 1` ⇒ cancelled
/// or fully filled (Solidity `_ORDER_FILLED`); `remaining > 1` ⇒
/// `actual_remaining = remaining - 1`. Returned as-is by `order_status_raw`.
/// **Decoded encoding**: `order_status` / `order_statuses` apply `- 1` so
/// `remaining` is already the actual figure (0 = filled or cancelled).
#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct OrderStatus {
    pub remaining: i128,
    pub filled_amount: i128,
}

#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct FillOrderParams {
    pub order: Order,
    /// Amount of the making side the caller wants this order to provide.
    /// Engine fills `min(making_amount, status.remaining)`.
    pub making_amount: i128,
    pub max_taking: i128,
}

/// Per-order fill outcome. `actual_taking` excludes the fee
#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct OrderResult {
    pub actual_taking: i128,
    pub actual_making: i128,
}

// ---------------------------------------------------------------------------
// Cross-contract client
// ---------------------------------------------------------------------------

#[contractclient(name = "LimitOrderEngineClient")]
pub trait LimitOrderEngine {
    // ── Maker-driven lifecycle ────────────────────────────────────────────

    /// Register `order` so it can be filled. `order.maker.require_auth()` —
    /// covers both on-chain calls (maker submits) and off-chain signed orders
    /// (backend submits with the maker's auth entry attached). Panics
    /// `OrderAlreadyRegistered` if the order is already on-chain.
    fn register_order(env: &Env, order: Order);

    /// Batch version. Each order independently auth'd by its `maker`.
    fn register_orders_batch(env: &Env, orders: Vec<Order>);

    /// Cancel a registered order. `order.maker.require_auth()`. Cancelling a
    /// never-registered `Order` is allowed and writes the `ORDER_FILLED`
    /// sentinel into a fresh slot, permanently blocking future registration
    /// of that order hash — mirrors Solidity `cancelSingle`. Panics
    /// `AlreadyFilled` if the order is already cancelled or fully consumed.
    fn cancel_order(env: &Env, order: Order);
    fn cancel_orders_batch(env: &Env, orders: Vec<Order>);

    /// Fill a batch of orders. `caller` is the taker and must authorize this
    /// call (`caller.require_auth()`); the engine then pulls
    /// `taker_input_amount` of `taker_input_token` from `caller` via a direct
    /// SEP-41 `transfer`. No allowance to the engine is required or consumed —
    /// a direct caller just needs to hold the balance and sign the call. For
    /// routed fills the Router authorizes its own pull (`authorize_pull`)
    /// before invoking the engine.
    ///
    /// `taker_input_token` is what the caller pays in. For a "natural" fill
    /// it equals the canonical taker token derived from `order_type`
    /// for a cross-chain it can be IBT (engine wraps to PT+YT) or PT/YT
    /// (engine redeems to IBT) `_handleTokenisation`
    /// All orders must share the same `pt` and `order_type`.
    fn fill_orders(
        env: &Env,
        caller: Address,
        receiver: Address,
        params: Vec<FillOrderParams>,
        taker_input_token: Address,
        taker_input_amount: i128,
    ) -> Vec<OrderResult>;

    // ── Views ─────────────────────────────────────────────────────────────

    fn hash_order(env: &Env, order: Order) -> BytesN<32>;
    /// Panics `OrderNotExisting` if the order was never registered.
    fn order_status(env: &Env, order: Order) -> OrderStatus;
    /// Returns `None` for never-registered orders. Mirrors Solidity
    /// `orderStatusesRaw` (no panic).
    fn order_status_raw(env: &Env, order_id: BytesN<32>) -> Option<OrderStatus>;
    /// Batch variant of `order_status_raw`. Returns one `Option<OrderStatus>`
    /// per `order_id`; `None` indicates the order was never registered.
    /// Mirrors Solidity `orderStatusesRaw(bytes32[])`.
    fn order_statuses_raw(env: &Env, order_ids: Vec<BytesN<32>>) -> Vec<Option<OrderStatus>>;
    fn order_statuses(env: &Env, orders: Vec<Order>) -> Vec<OrderStatus>;

    // ── Fee admin ─────────────────────────────────────────────────────────

    fn set_fee_rate(env: &Env, caller: Address, fee_rate: i128);
    fn set_fee_collector(env: &Env, caller: Address, fee_collector: Address);
    fn get_fee_rate(env: &Env) -> i128;
    fn get_fee_collector(env: &Env) -> Address;

    // ── Registry ──────────────────────────────────────────────────────────

    fn get_registry(env: &Env) -> Address;

    /// `caller` must hold the `REGISTRY` role. Swapping the registry
    /// retroactively re-gates already-registered orders.
    fn set_registry(env: &Env, caller: Address, registry: Address);

    // ── Admin ─────────────────────────────────────────────────────────────

    fn pause(env: &Env, caller: Address);
    fn unpause(env: &Env, caller: Address);
    fn upgrade(env: &Env, caller: Address, new_wasm_hash: BytesN<32>);
    fn paused(env: &Env) -> bool;

    // ── Admin rotation (2-step, delegated to stellar-access) ──────────────

    fn transfer_admin_role(env: &Env, new_admin: Address, live_until_ledger: u32);
    fn accept_admin_transfer(env: &Env);

    // ── Network parameters ────────────────────────────────────────────────

    /// Set the seconds-per-ledger value used to derive ledger counts from
    /// wall-clock durations (e.g. `OrderStatus` TTL extension). Defaults to
    /// `spectra_common::ttl::DEFAULT_SECS_PER_LEDGER` when never set.
    fn set_secs_per_ledger(env: &Env, caller: Address, secs: u64);
    fn get_secs_per_ledger(env: &Env) -> u64;
}
