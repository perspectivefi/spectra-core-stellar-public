// SPDX-License-Identifier: BUSL-1.1
use soroban_sdk::{Address, BytesN, contractevent};

#[contractevent]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct FactoryInitialized {
    pub admin: Address,
    pub registry: Address,
}

#[contractevent]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PtDeployed {
    pub deployer: Address,
    pub pt: Address,
    pub ibt: Address,
    pub duration: u64,
}

#[contractevent]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct FactoryUpgraded {
    #[topic]
    pub caller: Address,
    pub new_wasm_hash: BytesN<32>,
}
