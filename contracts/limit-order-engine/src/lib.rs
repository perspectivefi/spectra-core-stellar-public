// SPDX-License-Identifier: BUSL-1.1
#![no_std]

//! LimitOrderEngine.

pub mod errors;
pub mod events;
pub mod pricing;
pub mod storage;

#[cfg(test)]
mod test;

#[cfg(test)]
mod test_pricing_vectors;

#[cfg(test)]
mod fork_test;

#[cfg(test)]
mod fork_test_upshift;

#[cfg(test)]
mod fork_test_upshift_wrapper;

use soroban_sdk::{
    Address, BytesN, Env, IntoVal, Symbol, Val, Vec, contract, contractimpl, panic_with_error,
    xdr::ToXdr,
};
use spectra_common::limit_order_engine::{
    FillOrderParams, LimitOrderEngine, MakerTokenType, Order, OrderResult, OrderStatus, OrderType,
};
use spectra_common::principal_token::PrincipalTokenClient;
use spectra_common::registry::RegistryClient;
use spectra_fungible_vault_wrapper::SpectraFungibleVaultWrapperClient;
use stellar_access::access_control;
use stellar_macros::{only_role, when_not_paused};

use crate::errors::LimitOrderEngineError;
use crate::events::{
    FeeCollectorChange, LimitOrderEngineInitialized, LimitOrderFeeChange, OrderCancelled,
    OrderFilled, OrderRegistered, RegistryChange,
};

/// Fee-rate cap. 10% in 18-decimal fixed-point.
const MAX_FEE_RATE: i128 = 100_000_000_000_000_000;

const ORDER_FILLED: i128 = 1;

#[contract]
pub struct LimitOrderEngineContract;

#[contractimpl]
impl LimitOrderEngineContract {
    /// `fee_rate` is 18-decimal (1e16 = 1%). Admin holds PAUSER + FEE_SETTER +
    /// UPGRADE + REGISTRY. `registry` gates `register_order` / `fill_orders` on
    /// `is_registered_pt`. The `fee_rate` range check is retained here.
    pub fn __constructor(
        env: &Env,
        admin: Address,
        fee_collector: Address,
        fee_rate: i128,
        registry: Address,
    ) {
        if !(0..=MAX_FEE_RATE).contains(&fee_rate) {
            panic_with_error!(env, LimitOrderEngineError::FeeRateTooHigh);
        }

        access_control::set_admin(env, &admin);
        access_control::grant_role_no_auth(
            env,
            &admin,
            &spectra_common::constants::roles::PAUSER,
            &admin,
        );
        access_control::grant_role_no_auth(
            env,
            &admin,
            &spectra_common::constants::roles::FEE_SETTER,
            &admin,
        );
        access_control::grant_role_no_auth(
            env,
            &admin,
            &spectra_common::constants::roles::UPGRADE,
            &admin,
        );
        access_control::grant_role_no_auth(
            env,
            &admin,
            &spectra_common::constants::roles::REGISTRY,
            &admin,
        );

        storage::set_fee_rate(env, fee_rate);
        storage::set_fee_collector(env, &fee_collector);
        storage::set_registry(env, &registry);
        storage::extend_instance_ttl(env);

        LimitOrderEngineInitialized {
            admin,
            fee_collector,
            fee_rate,
            registry,
        }
        .publish(env);
    }

    // -- Role management --
    //
    // Thin wrappers over OZ stellar-access. The library functions enforce
    // auth + admin-role checks themselves; we expose them as contract entry
    // points so off-chain tooling (deploy dispatcher, key rotation) can
    // rotate the PAUSER / FEE_SETTER / UPGRADE roles after `initialize`.

    /// Grant `role` to `account`. `caller` must be the contract admin or hold
    /// the admin-role of `role`. Auth + admin checks are enforced by
    /// `access_control::grant_role`.
    pub fn grant_role(env: &Env, caller: Address, account: Address, role: Symbol) {
        access_control::grant_role(env, &account, &role, &caller);
        storage::extend_instance_ttl(env);
    }

    /// Revoke `role` from `account`. Same auth model as `grant_role`.
    pub fn revoke_role(env: &Env, caller: Address, account: Address, role: Symbol) {
        access_control::revoke_role(env, &account, &role, &caller);
        storage::extend_instance_ttl(env);
    }

    /// View: whether `account` currently holds `role`.
    pub fn has_role(env: &Env, account: Address, role: Symbol) -> bool {
        access_control::has_role(env, &account, &role).is_some()
    }
}

#[contractimpl]
impl LimitOrderEngine for LimitOrderEngineContract {
    // ── Maker lifecycle ───────────────────────────────────────────────────

    #[when_not_paused]
    fn register_order(env: &Env, order: Order) {
        order.maker.require_auth();
        register_order_inner(env, &order);
    }

    #[when_not_paused]
    fn register_orders_batch(env: &Env, orders: Vec<Order>) {
        // Per-order auth so each maker signs only their own order.
        for order in orders.iter() {
            let args: Vec<Val> = (order.clone(),).into_val(env);
            order.maker.require_auth_for_args(args);
            register_order_inner(env, &order);
        }
    }

    fn cancel_order(env: &Env, order: Order) {
        order.maker.require_auth();
        if order.expiry <= env.ledger().timestamp() {
            panic_with_error!(env, LimitOrderEngineError::InvalidOrder);
        }
        let order_id = hash_order_internal(env, &order);
        let (prior_filled, was_filled) = match storage::get_order_status(env, &order_id) {
            None => (0i128, false),
            Some(s) => (s.filled_amount, s.remaining == ORDER_FILLED),
        };
        if was_filled {
            panic_with_error!(env, LimitOrderEngineError::AlreadyFilled);
        }
        cancel_order_inner(env, &order, &order.maker, &order_id, prior_filled);
    }

    /// Batch-cancel orders for a single maker. All orders must share the same
    /// maker; the maker authorises once. See README for design rationale.
    fn cancel_orders_batch(env: &Env, orders: Vec<Order>) {
        let Some(first) = orders.get(0) else {
            return;
        };
        let maker = first.maker.clone();
        maker.require_auth();
        let now = env.ledger().timestamp();
        for order in orders.iter() {
            if order.expiry <= now {
                continue;
            }
            let order_id = hash_order_internal(env, &order);
            let (prior_filled, was_filled) = match storage::get_order_status(env, &order_id) {
                None => (0i128, false),
                Some(s) => (s.filled_amount, s.remaining == ORDER_FILLED),
            };
            if was_filled {
                continue;
            }
            cancel_order_inner(env, &order, &maker, &order_id, prior_filled);
        }
    }

    // ── Fill ──────────────────────────────────────────────────────────────
    ///     1. validate + update statuses (no pricing, no max_taking)
    ///     2. pull taker_input_token (caller pre-stages)
    ///     3. pull maker funds + convert to IBT + price (fused single loop —
    ///        pricing uses *realized* IBT from the conversion, not a preview)
    ///     4. taker-side tokenisation (IBT↔PT+YT) if needed
    ///     5. settle makers (denomination-aware on payout-side), pay fees,
    ///        sweep residuals to receiver
    #[when_not_paused]
    fn fill_orders(
        env: &Env,
        caller: Address,
        receiver: Address,
        params: Vec<FillOrderParams>,
        taker_input_token: Address,
        taker_input_amount: i128,
    ) -> Vec<OrderResult> {
        caller.require_auth();

        if taker_input_amount < 0 {
            panic_with_error!(env, LimitOrderEngineError::InvalidBatchParams);
        }
        if params.is_empty() {
            panic_with_error!(env, LimitOrderEngineError::NoValidOrder);
        }

        // Resolve (ibt, yt) once at the top and thread through helpers —
        // every batch shares the same pt by the IncompatibleBatch invariant.
        let sample = params.get(0).unwrap().order.clone();
        let pt_client = PrincipalTokenClient::new(env, &sample.pt);
        let ibt = pt_client.get_ibt();
        let yt = pt_client.get_yt();

        let (mut results, taker_token) = validate_orders_and_update_status(env, &params, &ibt, &yt);

        let engine = env.current_contract_address();

        // Only the canonical taker token or the cross-order conversion token
        // (ibt/pt/yt) is accepted; anything else would be pulled but never
        // converted or swept, stranding it in the engine.
        let supported_input = taker_input_token == taker_token
            || (taker_input_token == ibt
                && matches!(sample.order_type, OrderType::IbtForPt | OrderType::IbtForYt))
            || (taker_input_token == sample.pt && sample.order_type == OrderType::YtForIbt)
            || (taker_input_token == yt && sample.order_type == OrderType::PtForIbt);
        if !supported_input {
            panic_with_error!(env, LimitOrderEngineError::UnsupportedTakerInputToken);
        }

        if taker_input_amount > 0 {
            soroban_sdk::token::Client::new(env, &taker_input_token).transfer(
                &caller,
                &engine,
                &taker_input_amount,
            );
        }

        let (total_taking, total_fees) = pull_and_price(env, &params, &mut results, &ibt, &yt);

        handle_tokenisation(env, &sample, &taker_input_token, &ibt, &yt);

        transfer_post_validation(
            env,
            &receiver,
            &params,
            &results,
            &taker_token,
            total_taking,
            total_fees,
            &ibt,
            &yt,
        );
        results
    }

    // ── Views ─────────────────────────────────────────────────────────────

    fn hash_order(env: &Env, order: Order) -> BytesN<32> {
        hash_order_internal(env, &order)
    }

    /// Decoded view of an order's status.
    fn order_status(env: &Env, order: Order) -> OrderStatus {
        let order_id = hash_order_internal(env, &order);
        let raw = storage::get_order_status(env, &order_id)
            .unwrap_or_else(|| panic_with_error!(env, LimitOrderEngineError::OrderNotExisting));
        OrderStatus {
            remaining: raw.remaining - 1,
            filled_amount: raw.filled_amount,
        }
    }

    fn order_status_raw(env: &Env, order_id: BytesN<32>) -> Option<OrderStatus> {
        storage::get_order_status(env, &order_id)
    }

    fn order_statuses_raw(env: &Env, order_ids: Vec<BytesN<32>>) -> Vec<Option<OrderStatus>> {
        let mut out: Vec<Option<OrderStatus>> = Vec::new(env);
        for id in order_ids.iter() {
            out.push_back(storage::get_order_status(env, &id));
        }
        out
    }

    fn order_statuses(env: &Env, orders: Vec<Order>) -> Vec<OrderStatus> {
        let mut out: Vec<OrderStatus> = Vec::new(env);
        for order in orders.iter() {
            let order_id = hash_order_internal(env, &order);
            match storage::get_order_status(env, &order_id) {
                Some(raw) => out.push_back(OrderStatus {
                    remaining: raw.remaining - 1,
                    filled_amount: raw.filled_amount,
                }),
                None => panic_with_error!(env, LimitOrderEngineError::OrderNotExisting),
            }
        }
        out
    }

    // ── Fee admin ─────────────────────────────────────────────────────────

    #[only_role(caller, "FEE_SET")]
    fn set_fee_rate(env: &Env, caller: Address, fee_rate: i128) {
        if !(0..=MAX_FEE_RATE).contains(&fee_rate) {
            panic_with_error!(env, LimitOrderEngineError::FeeRateTooHigh);
        }
        let previous = storage::get_fee_rate(env);
        storage::set_fee_rate(env, fee_rate);
        LimitOrderFeeChange {
            previous,
            new: fee_rate,
        }
        .publish(env);
        storage::extend_instance_ttl(env);
    }

    #[only_role(caller, "FEE_SET")]
    fn set_fee_collector(env: &Env, caller: Address, fee_collector: Address) {
        let previous = storage::get_fee_collector(env);
        storage::set_fee_collector(env, &fee_collector);
        FeeCollectorChange {
            previous,
            new: fee_collector,
        }
        .publish(env);
        storage::extend_instance_ttl(env);
    }

    fn get_fee_rate(env: &Env) -> i128 {
        storage::get_fee_rate(env)
    }

    fn get_fee_collector(env: &Env) -> Address {
        storage::get_fee_collector(env)
    }

    // ── Registry ──────────────────────────────────────────────────────────

    fn get_registry(env: &Env) -> Address {
        storage::get_registry(env)
    }

    #[only_role(caller, "REGISTRY")]
    fn set_registry(env: &Env, caller: Address, registry: Address) {
        let previous = storage::get_registry(env);
        storage::set_registry(env, &registry);
        RegistryChange {
            previous,
            new: registry,
        }
        .publish(env);
        storage::extend_instance_ttl(env);
    }

    // ── Admin ─────────────────────────────────────────────────────────────

    #[only_role(caller, "PAUSER")]
    fn pause(env: &Env, caller: Address) {
        stellar_contract_utils::pausable::pause(env);
    }

    #[only_role(caller, "PAUSER")]
    fn unpause(env: &Env, caller: Address) {
        stellar_contract_utils::pausable::unpause(env);
    }

    fn paused(env: &Env) -> bool {
        stellar_contract_utils::pausable::paused(env)
    }

    #[only_role(caller, "UPGRADE")]
    fn upgrade(env: &Env, caller: Address, new_wasm_hash: BytesN<32>) {
        env.deployer().update_current_contract_wasm(new_wasm_hash);
    }

    // ── Admin rotation (2-step, delegated to stellar-access) ──────────────

    fn transfer_admin_role(env: &Env, new_admin: Address, live_until_ledger: u32) {
        access_control::transfer_admin_role(env, &new_admin, live_until_ledger);
    }

    fn accept_admin_transfer(env: &Env) {
        access_control::accept_admin_transfer(env);
    }

    // ── Network parameters ────────────────────────────────────────────────

    #[only_role(caller, "UPGRADE")]
    fn set_secs_per_ledger(env: &Env, caller: Address, secs: u64) {
        spectra_common::ttl::set_secs_per_ledger(env, secs);
        storage::extend_instance_ttl(env);
    }

    fn get_secs_per_ledger(env: &Env) -> u64 {
        spectra_common::ttl::get_secs_per_ledger(env)
    }
}

// ===========================================================================
// Internal helpers
// ===========================================================================

/// Validates each order and updates its status. Pricing and max_taking
/// enforcement live in `pull_and_price` so they see the *realized* IBT
/// resulting from the maker's funding-side conversion.
/// Returns `(results, taker_token)`. `results[i].actual_making` is set to
/// `net_making` (in the maker's chosen denomination); `actual_taking` is
/// left at 0 until `pull_and_price` runs.
fn validate_orders_and_update_status(
    env: &Env,
    params: &Vec<FillOrderParams>,
    ibt: &Address,
    yt: &Address,
) -> (Vec<OrderResult>, Address) {
    let initial_pt = params.get(0).unwrap().order.pt.clone();
    let initial_type = params.get(0).unwrap().order.order_type;

    let mut results: Vec<OrderResult> = Vec::new(env);
    let mut skipped: u32 = 0;
    let mut taker_token: Option<Address> = None;

    for p in params.iter() {
        let order = p.order.clone();

        if p.max_taking <= 0 {
            panic_with_error!(env, LimitOrderEngineError::NonPositiveMaxTaking);
        }

        // Batch consistency.
        if order.pt != initial_pt || order.order_type != initial_type {
            panic_with_error!(env, LimitOrderEngineError::IncompatibleBatch);
        }

        if !is_order_valid(env, &order) {
            skipped += 1;
            results.push_back(zero_result());
            continue;
        }

        let order_id = hash_order_internal(env, &order);
        let Some(status) = storage::get_order_status(env, &order_id) else {
            skipped += 1;
            results.push_back(zero_result());
            continue;
        };
        if status.remaining == ORDER_FILLED {
            skipped += 1;
            results.push_back(zero_result());
            continue;
        }
        let actual_remaining = status.remaining - 1;

        let net_making = p.making_amount.min(actual_remaining);
        if net_making <= 0 {
            skipped += 1;
            results.push_back(zero_result());
            continue;
        }

        let new_actual = actual_remaining - net_making;
        let new_stored = if new_actual == 0 {
            ORDER_FILLED
        } else {
            new_actual + 1
        };
        storage::set_order_status(
            env,
            &order_id,
            order.expiry,
            &OrderStatus {
                remaining: new_stored,
                filled_amount: status.filled_amount + net_making,
            },
        );

        results.push_back(OrderResult {
            actual_taking: 0,
            actual_making: net_making,
        });
        if taker_token.is_none() {
            taker_token = Some(get_taker_token(&order, ibt, yt));
        }
    }

    if skipped == params.len() {
        panic_with_error!(env, LimitOrderEngineError::NoValidOrder);
    }

    (results, taker_token.unwrap())
}

/// Phases 3 + 4 fused: for each filled order, transfer maker funds in, convert
/// to IBT if the order is funding-side, then price against the realized IBT
/// and enforce `max_taking`. Accumulates `(total_taking, total_fees)`.
/// Fused vs split: a single per-order loop avoids the intermediate
/// `Vec<i128>` of pricing inputs and one extra `params`/`results` walk.
fn pull_and_price(
    env: &Env,
    params: &Vec<FillOrderParams>,
    results: &mut Vec<OrderResult>,
    ibt: &Address,
    yt: &Address,
) -> (i128, i128) {
    let engine = env.current_contract_address();
    let fee_rate = storage::get_fee_rate(env);
    let mut total_taking: i128 = 0;
    let mut total_fees: i128 = 0;

    for i in 0..params.len() {
        let r = results.get(i).unwrap();
        if r.actual_making == 0 {
            continue;
        }
        let p = params.get(i).unwrap();
        let order = p.order.clone();
        let net_making = r.actual_making;

        let maker_token = get_making_token(env, &order, ibt, yt);
        soroban_sdk::token::Client::new(env, &maker_token).transfer_from(
            &engine,
            &order.maker,
            &engine,
            &net_making,
        );

        let ibt_for_pricing = match order.order_type {
            OrderType::IbtForPt | OrderType::IbtForYt => {
                convert_to_ibt(env, ibt, &maker_token, &order, net_making)
            }
            OrderType::PtForIbt | OrderType::YtForIbt => net_making,
        };

        let (spot_price, price_with_fee) = pricing::get_price(
            env,
            order.order_type,
            &order.pt,
            order.implied_rate,
            fee_rate,
        );
        let fee = mul_div_floor(
            env,
            price_with_fee - spot_price,
            ibt_for_pricing,
            pricing::UNIT,
        );
        let actual_taking = mul_div_floor(env, spot_price, ibt_for_pricing, pricing::UNIT);

        // A positive fill must pay the maker something; flooring can round the
        // proceeds to zero for dust-sized fills or near-zero YT prices.
        if net_making > 0 && actual_taking == 0 {
            panic_with_error!(env, LimitOrderEngineError::ZeroTaking);
        }

        if actual_taking + fee > p.max_taking {
            panic_with_error!(env, LimitOrderEngineError::MaxTakingExceeded);
        }

        results.set(
            i,
            OrderResult {
                actual_taking,
                actual_making: net_making,
            },
        );
        total_taking += actual_taking + fee;
        total_fees += fee;
    }

    (total_taking, total_fees)
}

/// Checks balance, settles makers, pays fees, sweeps surplus to receiver.
#[allow(clippy::too_many_arguments)]
fn transfer_post_validation(
    env: &Env,
    receiver: &Address,
    params: &Vec<FillOrderParams>,
    results: &Vec<OrderResult>,
    taker_token: &Address,
    total_taking: i128,
    total_fees: i128,
    ibt: &Address,
    yt: &Address,
) {
    let engine = env.current_contract_address();

    // Engine must already hold `total_taking` of `taker_token` before settling.
    let engine_taker_bal = soroban_sdk::token::Client::new(env, taker_token).balance(&engine);
    if engine_taker_bal < total_taking {
        panic_with_error!(env, LimitOrderEngineError::InsufficientFunds);
    }

    settle_makers(env, params, results, taker_token, ibt);

    // Pay fees.
    if total_fees > 0 {
        let fee_collector = storage::get_fee_collector(env);
        soroban_sdk::token::Client::new(env, taker_token).transfer(
            &engine,
            &fee_collector,
            &total_fees,
        );
    }

    // Sweep aggregated PT/YT/IBT held by the engine to the receiver.
    settle_taker(env, receiver, &params.get(0).unwrap().order, ibt, yt);
}

/// Two supported conversions:
///   - `taker_input_token == IBT` AND order_type ∈ {IbtForPt, IbtForYt}:
///     deposit all IBT held by engine into PT to mint PT+YT pairs. Engine
///     then has PT+YT to pay makers; surplus YT swept to receiver.
///   - `taker_input_token == PT` AND order_type == YtForIbt, OR
///     `taker_input_token == YT` AND order_type == PtForIbt:
///     redeem `min(PT, YT)` shares for IBT, giving engine the IBT to pay
///     makers; surplus PT/YT swept to receiver.
///   - Everything else: no-op. The natural `taker_token` path requires no
///     conversion because the taker is already paying in what makers need.
fn handle_tokenisation(
    env: &Env,
    sample_order: &Order,
    taker_input_token: &Address,
    ibt: &Address,
    yt: &Address,
) {
    let pt = sample_order.pt.clone();
    let order_type = sample_order.order_type;
    let engine = env.current_contract_address();
    let pt_client = PrincipalTokenClient::new(env, &pt);

    // Approval is consumed by the very next call in this same transaction
    // (`deposit_ibt` / `redeem_for_ibt`), so the minimum TTL is enough:
    // `live_until == sequence()` is the smallest value the SEP-41 token
    // contract accepts (must be ≥ current sequence) and expires the
    // allowance at the end of this ledger.
    let live_until = env.ledger().sequence();

    let pays_ibt_wants_pt_or_yt =
        taker_input_token == ibt && matches!(order_type, OrderType::IbtForPt | OrderType::IbtForYt);
    let pays_pt_or_yt_wants_ibt = (*taker_input_token == pt && order_type == OrderType::YtForIbt)
        || (taker_input_token == yt && order_type == OrderType::PtForIbt);

    if pays_ibt_wants_pt_or_yt {
        let ibt_balance = soroban_sdk::token::Client::new(env, ibt).balance(&engine);
        if ibt_balance > 0 {
            soroban_sdk::token::Client::new(env, ibt).approve(
                &engine,
                &pt,
                &ibt_balance,
                &live_until,
            );
            pt_client.deposit_ibt(&engine, &ibt_balance, &engine, &engine);
        }
    } else if pays_pt_or_yt_wants_ibt {
        let pt_balance = soroban_sdk::token::Client::new(env, &pt).balance(&engine);
        let yt_balance = soroban_sdk::token::Client::new(env, yt).balance(&engine);
        let redeem_amount = pt_balance.min(yt_balance);
        if redeem_amount > 0 {
            // No YT approval needed.
            pt_client.redeem_for_ibt(&engine, &redeem_amount, &engine, &engine);
        }
    }
    // else: taker pays canonical taker_token directly, no conversion needed.
}

/// Sends each maker their actualTaking and emits OrderFilled. Funding-side
/// orders (IbtForPt, IbtForYt) get the taker token (PT or YT) directly.
/// Payout-side orders (PtForIbt, YtForIbt) route through `payout_from_ibt`
/// so the IBT the engine just acquired is converted into the maker's
/// chosen denomination (Ibt / Underlying / VaultShare) before being sent.
fn settle_makers(
    env: &Env,
    params: &Vec<FillOrderParams>,
    results: &Vec<OrderResult>,
    taker_token: &Address,
    ibt: &Address,
) {
    let engine = env.current_contract_address();
    for (p, r) in params.iter().zip(results.iter()) {
        if r.actual_making == 0 {
            continue;
        }
        let order = p.order;
        match order.order_type {
            OrderType::IbtForPt | OrderType::IbtForYt => {
                soroban_sdk::token::Client::new(env, taker_token).transfer(
                    &engine,
                    &order.receiver,
                    &r.actual_taking,
                );
            }
            OrderType::PtForIbt | OrderType::YtForIbt => {
                payout_from_ibt(env, ibt, &order, r.actual_taking, &order.receiver);
            }
        }
        let order_id = hash_order_internal(env, &order);
        OrderFilled {
            order_id,
            actual_making: r.actual_making,
        }
        .publish(env);
    }
}

/// Sends the entire IBT/PT/YT balance held by the engine to the receiver.
fn settle_taker(env: &Env, receiver: &Address, sample_order: &Order, ibt: &Address, yt: &Address) {
    let engine = env.current_contract_address();

    // IBT → PT → YT mirrors Solidity _settleTaker order.
    for token in [ibt.clone(), sample_order.pt.clone(), yt.clone()] {
        let bal = soroban_sdk::token::Client::new(env, &token).balance(&engine);
        if bal > 0 {
            soroban_sdk::token::Client::new(env, &token).transfer(&engine, receiver, &bal);
        }
    }
}

// ---------------------------------------------------------------------------
// Pure helpers
// ---------------------------------------------------------------------------

/// `sha256(xdr(order))`. Deterministic across SDK versions for the same
/// contracttype layout.
fn hash_order_internal(env: &Env, order: &Order) -> BytesN<32> {
    let xdr = order.clone().to_xdr(env);
    env.crypto().sha256(&xdr).into()
}

/// Lenient validation — returns false on any check failure.
/// Used by fill validation, which silently skips invalid orders.
fn is_order_valid(env: &Env, order: &Order) -> bool {
    if !is_pt_registered(env, &order.pt) {
        return false;
    }
    if order.making_amount <= 0 {
        return false;
    }
    let now = env.ledger().timestamp();
    if order.expiry <= now {
        return false;
    }
    let pt_client = PrincipalTokenClient::new(env, &order.pt);
    if order.maker_token_type == MakerTokenType::VaultShare
        && try_vault_share(env, &pt_client.get_ibt()).is_none()
    {
        return false;
    }
    let maturity = pt_client.maturity();
    if maturity <= now {
        return false;
    }
    if order.expiry > maturity {
        return false;
    }
    true
}

/// Shared registration body; auth is enforced by the caller.
fn register_order_inner(env: &Env, order: &Order) {
    validate_order_for_registration(env, order);
    let order_id = hash_order_internal(env, order);
    if storage::has_order_status(env, &order_id) {
        panic_with_error!(env, LimitOrderEngineError::OrderAlreadyRegistered);
    }
    storage::set_order_status(
        env,
        &order_id,
        order.expiry,
        &OrderStatus {
            remaining: order.making_amount + 1,
            filled_amount: 0,
        },
    );
    OrderRegistered {
        maker: order.maker.clone(),
        order_id,
        making_amount: order.making_amount,
    }
    .publish(env);
}

/// Shared cancellation body; caller enforces auth, this verifies `order.maker == maker`.
fn cancel_order_inner(
    env: &Env,
    order: &Order,
    maker: &Address,
    order_id: &BytesN<32>,
    prior_filled: i128,
) {
    if order.maker != *maker {
        panic_with_error!(env, LimitOrderEngineError::CancelBatchMakerMismatch);
    }
    storage::set_order_status(
        env,
        order_id,
        order.expiry,
        &OrderStatus {
            remaining: ORDER_FILLED,
            filled_amount: prior_filled,
        },
    );
    OrderCancelled {
        maker: order.maker.clone(),
        order_id: order_id.clone(),
    }
    .publish(env);
}

fn validate_order_for_registration(env: &Env, order: &Order) {
    if !is_pt_registered(env, &order.pt) {
        panic_with_error!(env, LimitOrderEngineError::PtNotRegistered);
    }
    if order.making_amount <= 0 {
        panic_with_error!(env, LimitOrderEngineError::InvalidOrder);
    }
    // Reject a non-positive implied rate: an IbtForYt fill would divide by zero.
    if order.implied_rate <= 0 {
        panic_with_error!(env, LimitOrderEngineError::InvalidOrder);
    }
    if order.fail_safe_rate < 0 {
        panic_with_error!(env, LimitOrderEngineError::InvalidOrder);
    }
    let now = env.ledger().timestamp();
    if order.expiry <= now {
        panic_with_error!(env, LimitOrderEngineError::InvalidOrder);
    }
    let pt_client = PrincipalTokenClient::new(env, &order.pt);
    if order.maker_token_type == MakerTokenType::VaultShare
        && try_vault_share(env, &pt_client.get_ibt()).is_none()
    {
        panic_with_error!(env, LimitOrderEngineError::UnsupportedMakerTokenType);
    }
    let maturity = pt_client.maturity();
    if maturity <= now {
        panic_with_error!(env, LimitOrderEngineError::PtMatured);
    }
    if order.expiry > maturity {
        panic_with_error!(env, LimitOrderEngineError::OrderExpiryAfterMaturity);
    }
}

/// Token the taker pays at fill time. `ibt`/`yt` are threaded from the top
/// of `fill_orders` to avoid a second pair of `PrincipalTokenClient` calls.
fn get_taker_token(order: &Order, ibt: &Address, yt: &Address) -> Address {
    match order.order_type {
        OrderType::IbtForPt => order.pt.clone(),
        OrderType::PtForIbt => ibt.clone(),
        OrderType::IbtForYt => yt.clone(),
        OrderType::YtForIbt => ibt.clone(),
    }
}

/// Token the maker funds the order with on the funding side, or receives
/// on the payout side. Funding-side (IbtForPt, IbtForYt) routes through
/// `resolve_ibt_leg_token` so the maker can fund in IBT, Underlying, or
/// VaultShare; payout-side keeps the canonical PT / YT.
fn get_making_token(env: &Env, order: &Order, ibt: &Address, yt: &Address) -> Address {
    match order.order_type {
        OrderType::IbtForPt | OrderType::IbtForYt => {
            resolve_ibt_leg_token(env, ibt, order.maker_token_type)
        }
        OrderType::PtForIbt => order.pt.clone(),
        OrderType::YtForIbt => yt.clone(),
    }
}

/// Maps `maker_token_type` to the actual on-chain Address. For
/// `VaultShare`, raw `try_invoke_contract` is used (not the typed wrapper
/// client) because a wrapper-less IBT does not host the `vault_share`
/// symbol — a typed call would trap with the host's generic error and we
/// would lose the `UnsupportedMakerTokenType` semantic.
/// The `"vault_share"` literal exceeds `symbol_short!`'s 9-char ceiling,
/// hence `Symbol::new`.
fn resolve_ibt_leg_token(env: &Env, ibt: &Address, kind: MakerTokenType) -> Address {
    match kind {
        MakerTokenType::Ibt => ibt.clone(),
        MakerTokenType::Underlying => {
            SpectraFungibleVaultWrapperClient::new(env, ibt).query_asset()
        }
        MakerTokenType::VaultShare => try_vault_share(env, ibt).unwrap_or_else(|| {
            panic_with_error!(env, LimitOrderEngineError::UnsupportedMakerTokenType)
        }),
    }
}

/// Probe whether `ibt` hosts the Spectra wrapper `vault_share` entry point,
/// returning its address if so. Raw `try_invoke_contract` (not the typed
/// client) so a wrapper-less IBT — which doesn't host the symbol at all —
/// yields `None` instead of trapping with the host's generic error.
fn try_vault_share(env: &Env, ibt: &Address) -> Option<Address> {
    match env.try_invoke_contract::<Address, soroban_sdk::Error>(
        ibt,
        &Symbol::new(env, "vault_share"),
        Vec::new(env),
    ) {
        Ok(Ok(addr)) => Some(addr),
        _ => None,
    }
}

/// Whether `order.pt` is in the pinned Registry's PT set.
fn is_pt_registered(env: &Env, pt: &Address) -> bool {
    let registry = storage::get_registry(env);
    RegistryClient::new(env, &registry).is_registered_pt(pt)
}

/// Funding-side: turn `amount` of `order.maker_token_type` (held by engine)
/// into IBT. `Ibt` is identity. `Underlying` deposits through the wrapper.
/// `VaultShare` wraps through `wrap_with_min_shares`. The manual floor
/// check on the Underlying path mirrors Solidity (the wrapper's `deposit`
/// has no built-in min-shares); the VaultShare path relies on the
/// wrapper's native slippage protection. Both surface as
/// `FailSafeRateNotMet` at the engine boundary (the wrapper-native panic
/// may propagate as its own `SlippageProtectionFailed` — same condition).
fn convert_to_ibt(
    env: &Env,
    ibt: &Address,
    leg_token: &Address,
    order: &Order,
    amount: i128,
) -> i128 {
    let engine = env.current_contract_address();
    match order.maker_token_type {
        MakerTokenType::Ibt => amount,
        MakerTokenType::Underlying => {
            let client = SpectraFungibleVaultWrapperClient::new(env, ibt);
            // `deposit` pulls the leg token via `transfer(engine -> wrapper)`
            // one frame down: on the engine's behalf but outside its
            // direct-invoker auto-auth, and `transfer` reads no allowance, so
            // the pull must be pre-declared. Mirrors PT.deposit.
            spectra_common::auth::authorize_pull(env, leg_token, &engine, ibt, amount);
            let out = client.deposit(&amount, &engine, &engine, &engine);
            let floor = mul_div_floor(env, amount, order.fail_safe_rate, pricing::UNIT);
            if out < floor {
                panic_with_error!(env, LimitOrderEngineError::FailSafeRateNotMet);
            }
            out
        }
        MakerTokenType::VaultShare => {
            let client = SpectraFungibleVaultWrapperClient::new(env, ibt);
            // As in the Underlying branch: `wrap` pulls the vault-share via
            // `transfer(engine -> wrapper)` one frame down, so pre-declare it.
            spectra_common::auth::authorize_pull(env, leg_token, &engine, ibt, amount);
            let floor = mul_div_floor(env, amount, order.fail_safe_rate, pricing::UNIT);
            client.wrap_with_min_shares(&amount, &engine, &engine, &floor)
        }
    }
}

/// Payout-side: deliver `ibt_amount` of `order.maker_token_type` to `to`.
/// `Ibt` transfers ibt directly. `Underlying` redeems through the wrapper
/// (manual floor check). `VaultShare` unwraps via
/// `unwrap_with_min_vault_shares` (wrapper-native floor). Same engine-
/// level meaning as the funding-side floor: `FailSafeRateNotMet`.
fn payout_from_ibt(env: &Env, ibt: &Address, order: &Order, ibt_amount: i128, to: &Address) {
    let engine = env.current_contract_address();
    match order.maker_token_type {
        MakerTokenType::Ibt => {
            soroban_sdk::token::Client::new(env, ibt).transfer(&engine, to, &ibt_amount);
        }
        MakerTokenType::Underlying => {
            let client = SpectraFungibleVaultWrapperClient::new(env, ibt);
            let out = client.redeem(&ibt_amount, to, &engine, &engine);
            let floor = mul_div_floor(env, ibt_amount, order.fail_safe_rate, pricing::UNIT);
            if out < floor {
                panic_with_error!(env, LimitOrderEngineError::FailSafeRateNotMet);
            }
        }
        MakerTokenType::VaultShare => {
            let client = SpectraFungibleVaultWrapperClient::new(env, ibt);
            let floor = mul_div_floor(env, ibt_amount, order.fail_safe_rate, pricing::UNIT);
            client.unwrap_with_min_vault_shares(&ibt_amount, to, &engine, &engine, &floor);
        }
    }
}

#[inline(always)]
fn zero_result() -> OrderResult {
    OrderResult {
        actual_taking: 0,
        actual_making: 0,
    }
}

/// (a * b) / c with floor rounding, via I256 to avoid i128 overflow on `a*b`.
fn mul_div_floor(env: &Env, a: i128, b: i128, c: i128) -> i128 {
    use soroban_sdk::I256;
    I256::from_i128(env, a)
        .mul(&I256::from_i128(env, b))
        .div(&I256::from_i128(env, c))
        .to_i128()
        .unwrap_or_else(|| panic_with_error!(env, LimitOrderEngineError::PriceOverflow))
}
