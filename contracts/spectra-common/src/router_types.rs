// SPDX-License-Identifier: BUSL-1.1
//! Typed command grammar used by the Spectra Router.
//!
//! Replaces Solidity's `bytes _commands` + `bytes[] _inputs` calldata
//! pattern with a strongly-typed enum.

use soroban_sdk::{Address, BytesN, Vec, contracttype};

use crate::limit_order_engine::{FillOrderParams, Order};

// ─── StellarBroker types ────────────────────────────────────────────────────

/// LP protocol identifier, mirroring `stellar_broker::types::protocol::Protocol`.
#[contracttype]
#[repr(i8)]
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Protocol {
    AquaConstant = 0,
    AquaStable = 1,
    Soroswap = 2,
    Comet = 3,
    Phoenix = 4,
}

/// A single hop inside a swap route.
#[contracttype]
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PathStep {
    pub protocol: Protocol,
    pub asset: Address,
    pub pool: Address,
    pub si: u32,
    pub bi: u32,
}

/// Multi-hop swap route descriptor, matching `stellar_broker::types::route::Route`.
#[contracttype]
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Route {
    pub path: Vec<PathStep>,
    pub amount: i128,
    pub min: i128,
    pub estimated: i128,
}

/// Sentinel for the recipient slot of a command argument.
///
/// Resolved by the Router's dispatcher to a concrete `Address` at execute
/// time. `Caller` is the original `execute` caller, `Router` is the Router
/// contract itself, and `Address` is an explicit third-party address.
#[contracttype]
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Recipient {
    Caller,
    Router,
    Address(Address),
}

/// Sentinel for an amount slot of a command argument.
///
/// `Balance` instructs the dispatcher to use the Router's current balance
/// of the relevant token (Solidity's `CONTRACT_BALANCE` sentinel). `Exact`
/// is a plain `i128` value.
///
/// NOTE: `TransferFrom + Balance` is a special case — the owner is the
/// caller, so it resolves to `token.balance(&caller)` and attempts to pull
/// that entire balance via SEP-41 `transfer_from`. The caller must have
/// pre-approved the Router for at least that amount, otherwise the call
/// panics with `InsufficientAllowance`. All other commands resolve
/// `Balance` against the Router's own balance.
#[contracttype]
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Amount {
    Balance,
    Exact(i128),
}

#[contracttype]
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TransferFromArgs {
    pub token: Address,
    pub amount: Amount,
}

#[contracttype]
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TransferArgs {
    pub token: Address,
    pub recipient: Recipient,
    pub amount: Amount,
}

#[contracttype]
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AssertMinBalanceArgs {
    pub token: Address,
    pub owner: Recipient,
    pub min_value: i128,
}

#[contracttype]
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DepositAssetInIbtArgs {
    pub ibt: Address,
    pub assets: Amount,
    pub recipient: Recipient,
}

#[contracttype]
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DepositIbtInPtArgs {
    pub pt: Address,
    pub ibts: Amount,
    pub pt_recipient: Recipient,
    pub yt_recipient: Recipient,
    pub min_shares: i128,
}

#[contracttype]
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RedeemIbtForAssetArgs {
    pub ibt: Address,
    pub shares: Amount,
    pub recipient: Recipient,
}

#[contracttype]
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RedeemPtForIbtArgs {
    pub pt: Address,
    pub shares: Amount,
    pub recipient: Recipient,
    pub min_ibts: i128,
}

/// Register (create) resting maker orders on the LimitOrderEngine. Each order
/// carries its own `maker`, whose auth the engine requires — so a maker can
/// create orders through the router in the same `execute` that later fills or
/// stages them. Wraps the engine's `register_orders_batch`.
#[contracttype]
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RegisterOrdersArgs {
    pub orders: Vec<Order>,
}

/// Cancel resting maker orders on the LimitOrderEngine. Cancellation is
/// self-service: every order in the batch must share one maker, whose auth the
/// engine requires once over the whole batch. Wraps `cancel_orders_batch`.
#[contracttype]
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CancelOrdersArgs {
    pub orders: Vec<Order>,
}

#[contracttype]
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FillOrdersArgs {
    pub receiver: Recipient,
    pub params: Vec<FillOrderParams>,
    pub taker_input_token: Address,
    pub taker_input_amount: Amount,
}

#[contracttype]
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BridgeNativePtToEvmArgs {
    pub pt: Address,
    pub amount: Amount,
    pub target_chain_id: u32,
    pub evm_recipient: BytesN<20>,
    pub gas_token: Address,
    pub gas_amount: Amount,
    pub refund_address: Recipient,
    pub min_amount_out: i128,
}

#[contracttype]
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BridgeWrappedPtBackToEvmArgs {
    pub wrapped_pt: Address,
    pub amount: Amount,
    pub target_chain_id: u32,
    pub evm_recipient: BytesN<20>,
    pub gas_token: Address,
    pub gas_amount: Amount,
    pub refund_address: Recipient,
    pub min_amount_out: i128,
}

#[contracttype]
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BrokerSwapArgs {
    pub token_in: Address,
    pub token_out: Address,
    pub routes: Vec<Route>,
    pub min_amount_out: i128,
    pub fixed_fee_permille: u32,
    pub fee_path: Vec<PathStep>,
}

/// The Router's `execute(caller, commands, deadline)` entry point loops
/// over a `Vec<Command>` and dispatches each variant. Sentinel enums in
/// the recipient/amount slots are resolved per command.
///
/// Variants map to Solidity opcodes from `Commands.sol`:
/// - `TransferFrom`             → 0x00 TRANSFER_FROM
/// - `Transfer`                 → 0x02 TRANSFER
/// - `AssertMinBalance`         → 0x0f ASSERT_MIN_BALANCE
/// - `DepositAssetInIbt`        → 0x04 DEPOSIT_ASSET_IN_IBT
/// - `DepositIbtInPt`           → 0x06 DEPOSIT_IBT_IN_PT
/// - `RedeemIbtForAsset`        → 0x07 REDEEM_IBT_FOR_ASSET
/// - `RedeemPtForIbt`           → 0x09 REDEEM_PT_FOR_IBT
/// - `CancelOrders`             → (Stellar-only; no Solidity opcode)
/// - `FillOrders`               → (Stellar-only; no Solidity opcode)
/// - `BridgeNativePtToEvm`      → (Stellar-only; no Solidity opcode)
/// - `BridgeWrappedPtBackToEvm` → (Stellar-only; no Solidity opcode)
/// - `BrokerSwap`               → (Stellar-only; no Solidity opcode)
#[contracttype]
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Command {
    TransferFrom(TransferFromArgs),
    Transfer(TransferArgs),
    AssertMinBalance(AssertMinBalanceArgs),
    DepositAssetInIbt(DepositAssetInIbtArgs),
    DepositIbtInPt(DepositIbtInPtArgs),
    RedeemIbtForAsset(RedeemIbtForAssetArgs),
    RedeemPtForIbt(RedeemPtForIbtArgs),
    RegisterOrders(RegisterOrdersArgs),
    CancelOrders(CancelOrdersArgs),
    FillOrders(FillOrdersArgs),
    BridgeNativePtToEvm(BridgeNativePtToEvmArgs),
    BridgeWrappedPtBackToEvm(BridgeWrappedPtBackToEvmArgs),
    BrokerSwap(BrokerSwapArgs),
}
