// SPDX-License-Identifier: BUSL-1.1
//! Vendored from `spectra-bridge-stellar/contracts/stellar/contracts/bridge-traits/src/lib.rs`
//! at the commit listed in the README. Re-vendor when the upstream interface changes.
//! Do not edit the trait shapes — divergence will silently break cross-contract calls.

use soroban_sdk::{Address, Bytes, BytesN, Env, String, contractclient, contracterror};

/// Error type for messenger operations
#[contracterror]
#[derive(Copy, Clone, Debug, Eq, PartialEq, PartialOrd, Ord)]
#[repr(u32)]
pub enum MessengerError {
    /// Required by Axelar's AxelarExecutable derive macro
    NotApproved = 0,
    NotAuthorized = 1,
    SendFailed = 2,
    InvalidDestination = 3,
    ChainNotSupported = 4,
}

/// The messenger interface that all bridge adapters must implement.
/// Mirrors the EVM `IBridgeMessenger` pattern.
///
/// Each adapter (Axelar, LayerZero, etc.) deploys as a separate Soroban contract
/// implementing these entry points. The core bridge calls them via cross-contract invocation.
#[contractclient(name = "MessengerClient")]
pub trait BridgeMessenger {
    /// Send a cross-chain message to the destination chain/address.
    /// The adapter handles protocol-specific gas payment and gateway interaction.
    ///
    /// # Arguments
    /// * `sender` - The address initiating the send (must authorize)
    /// * `destination_chain` - Target chain identifier (e.g., "ethereum", "polygon")
    /// * `destination_address` - Target contract address on the destination chain
    /// * `payload` - The encoded bridge message
    /// * `gas_amount` - Amount of gas token to pay for cross-chain delivery
    /// * `refund_address` - Address that should receive any excess gas refund.
    ///   Without this, refunds would accumulate in the messenger contract with
    ///   no way to withdraw them, stranding user gas tokens.
    fn send_message(
        env: Env,
        sender: Address,
        destination_chain: String,
        destination_address: String,
        payload: Bytes,
        gas_amount: i128,
        refund_address: Address,
    ) -> Result<(), MessengerError>;

    /// Returns the protocol name (e.g., "axelar", "layerzero")
    fn protocol_name(env: Env) -> String;
}

/// The interface that the core bridge exposes for adapters to deliver inbound messages.
/// The adapter calls `receive_message` on the bridge after validating with its protocol.
#[contractclient(name = "BridgeReceiverClient")]
pub trait BridgeReceiver {
    /// Called by the messenger adapter to deliver a validated inbound message.
    ///
    /// # Arguments
    /// * `caller` - The messenger address (must match current or previous messenger)
    /// * `source_chain` - The chain the message originated from
    /// * `message_id` - Protocol-specific unique message identifier
    /// * `source_address` - The sender address on the source chain
    /// * `payload` - The encoded bridge message
    fn receive_message(
        env: Env,
        caller: Address,
        source_chain: String,
        message_id: String,
        source_address: String,
        payload: Bytes,
    );
}

/// The interface that Stellar Principal Token contracts must implement
/// to be bridgeable. Mirrors the EVM `IPrincipalToken` pattern.
///
/// Any native Stellar PT can be bridged without prior registration
/// as long as it implements these entry points.
#[contractclient(name = "PrincipalTokenClient")]
pub trait PrincipalToken {
    /// Returns the token decimals
    fn decimals(env: Env) -> u32;

    /// Returns the token symbol
    fn symbol(env: Env) -> String;

    /// Returns the PT maturity timestamp
    fn maturity(env: Env) -> u64;
}

/// The interface that bridge-deployed wrapped PT contracts must implement.
/// Used by the bridge to query origin info when bridging back to EVM.
#[contractclient(name = "WrappedPtClient")]
pub trait WrappedPt {
    /// Returns the origin EVM PT address (32 bytes, zero-padded)
    fn origin_address(env: Env) -> BytesN<32>;

    /// Returns the origin EVM chain ID
    fn origin_chain_id(env: Env) -> u32;

    /// Mints wrapped PT tokens to a recipient (bridge only)
    fn mint(env: Env, to: Address, amount: i128);

    /// Burns wrapped PT tokens from an address (bridge only)
    fn burn_by_bridge(env: Env, from: Address, amount: i128);
}
