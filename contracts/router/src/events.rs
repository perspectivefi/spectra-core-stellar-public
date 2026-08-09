// SPDX-License-Identifier: BUSL-1.1
use soroban_sdk::{Address, contractevent};

#[contractevent]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ExecuteCompleted {
    pub caller: Address,
    pub command_count: u32,
}

#[contractevent]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PausedSet {
    pub paused: bool,
}

#[contractevent]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RegistryChanged {
    pub old: Address,
    pub new: Address,
}

#[contractevent]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct EngineChanged {
    pub old: Option<Address>,
    pub new: Address,
}

#[contractevent]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BridgeChanged {
    pub old: Option<Address>,
    pub new: Address,
}

#[contractevent]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BrokerChanged {
    pub old: Option<Address>,
    pub new: Address,
}

#[contractevent]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MinGasFloorChanged {
    pub target_chain_id: u32,
    pub gas_token: Address,
    pub old: i128,
    pub new: i128,
}
