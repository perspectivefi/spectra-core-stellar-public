// SPDX-License-Identifier: BUSL-1.1
use soroban_sdk::{Address, BytesN, contractevent};

#[contractevent]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LimitOrderEngineInitialized {
    #[topic]
    pub admin: Address,
    pub fee_collector: Address,
    pub fee_rate: i128,
    pub registry: Address,
}

#[contractevent]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct OrderRegistered {
    #[topic]
    pub maker: Address,
    #[topic]
    pub order_id: BytesN<32>,
    pub making_amount: i128,
}

#[contractevent]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct OrderCancelled {
    #[topic]
    pub maker: Address,
    #[topic]
    pub order_id: BytesN<32>,
}

#[contractevent]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct OrderFilled {
    #[topic]
    pub order_id: BytesN<32>,
    pub actual_making: i128,
}

#[contractevent]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LimitOrderFeeChange {
    pub previous: i128,
    #[topic]
    pub new: i128,
}

#[contractevent]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct FeeCollectorChange {
    pub previous: Address,
    #[topic]
    pub new: Address,
}

#[contractevent]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RegistryChange {
    pub previous: Address,
    #[topic]
    pub new: Address,
}
