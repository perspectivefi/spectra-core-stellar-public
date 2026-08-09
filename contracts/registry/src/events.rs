// SPDX-License-Identifier: BUSL-1.1
use soroban_sdk::{Address, BytesN, contractevent};

#[contractevent]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct FactoryChange {
    pub old: Address,
    pub new: Address,
}

#[contractevent]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RouterChange {
    pub old: Address,
    pub new: Address,
}

#[contractevent]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PtWasmHashChange {
    pub old: BytesN<32>,
    pub new: BytesN<32>,
}

#[contractevent]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct YtWasmHashChange {
    pub old: BytesN<32>,
    pub new: BytesN<32>,
}

#[contractevent]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TokenizationFeeChange {
    pub old: i128,
    pub new: i128,
}

#[contractevent]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct YieldFeeChange {
    pub old: i128,
    pub new: i128,
}

#[contractevent]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PtFlashLoanFeeChange {
    pub old: i128,
    pub new: i128,
}

#[contractevent]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct FeeCollectorChange {
    pub old: Address,
    pub new: Address,
}

#[contractevent]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BridgeChange {
    pub old: Address,
    pub new: Address,
}

#[contractevent]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LimitOrderEngineChange {
    pub old: Address,
    pub new: Address,
}

#[contractevent]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct FeeReduced {
    #[topic]
    pub pt: Address,
    #[topic]
    pub user: Address,
    pub reduction: i128,
}

#[contractevent]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PtAdded {
    pub pt: Address,
}

#[contractevent]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PtRemoved {
    pub pt: Address,
}

#[contractevent]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RegistryUpgraded {
    #[topic]
    pub caller: Address,
    pub new_wasm_hash: BytesN<32>,
}
