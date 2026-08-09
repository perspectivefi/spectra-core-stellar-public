// SPDX-License-Identifier: BUSL-1.1
use soroban_sdk::{Address, BytesN, contractevent};

/// Emitted at the end of `initialize` once the YT has been deployed.
#[contractevent]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct YtDeployed {
    pub address: Address,
}

/// Emitted when the fee collector claims accrued IBT fees.
#[contractevent]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct FeeClaimed {
    pub fee_collector: Address,
    pub ibts: i128,
}

/// Emitted at the end of every successful `update_yield`.
#[contractevent]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct YieldUpdated {
    pub user: Address,
    pub yield_in_ibt: i128,
}

/// Emitted at the start of `claim_yield` / `claim_yield_in_ibt`.
#[contractevent]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct YieldClaimed {
    pub user: Address,
    pub receiver: Address,
    pub yield_in_ibt: i128,
}

/// Emitted on every successful PT mint via `deposit*`. Renamed from `Mint`
/// to avoid colliding with the OZ `stellar-tokens::fungible::Mint(to, amount)`
/// event that `Base::mint` emits internally — both events now appear in the
/// contract spec under distinct names.
#[contractevent]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PtMinted {
    pub caller: Address,
    pub receiver: Address,
    pub shares: i128,
}

/// Emitted on every successful PT `redeem*` / `withdraw*`.
#[contractevent]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Redeem {
    pub owner: Address,
    pub receiver: Address,
    pub shares: i128,
}

/// Emitted by `store_rates_at_expiry` when the rates are frozen.
#[contractevent]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RatesStoredAtExpiry {
    pub ibt_rate: i128,
    pub pt_rate: i128,
}

/// Emitted on `set_rewards_proxy`.
#[contractevent]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RewardsProxyChange {
    pub old: Option<Address>,
    pub new: Option<Address>,
}

/// Emitted on `claim_rewards`.
#[contractevent]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RewardsClaimed {
    pub caller: Address,
}

/// Emitted on `upgrade` — off-chain monitor signal for WASM rotation.
#[contractevent]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PtUpgraded {
    #[topic]
    pub caller: Address,
    pub new_wasm_hash: BytesN<32>,
}
