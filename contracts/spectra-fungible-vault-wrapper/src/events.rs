// SPDX-License-Identifier: BUSL-1.1
use soroban_sdk::{Address, BytesN, contractevent};

/// Vault shares wrapped into wrapper shares.
#[contractevent]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Wrap {
    #[topic]
    pub caller: Address,
    #[topic]
    pub receiver: Address,
    pub vault_shares: i128,
    pub shares: i128,
}

/// Wrapper shares unwrapped back into vault shares.
#[contractevent]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Unwrap {
    #[topic]
    pub caller: Address,
    #[topic]
    pub receiver: Address,
    #[topic]
    pub owner: Address,
    pub shares: i128,
    pub vault_shares: i128,
}

/// Rewards proxy updated (`None` = disabled).
#[contractevent]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RewardsProxyUpdated {
    pub old_rewards_proxy: Option<Address>,
    pub new_rewards_proxy: Option<Address>,
}

/// Contract code upgraded.
#[contractevent]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Upgraded {
    #[topic]
    pub caller: Address,
    pub new_wasm_hash: BytesN<32>,
}
