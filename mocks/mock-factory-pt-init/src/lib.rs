// SPDX-License-Identifier: BUSL-1.1
#![no_std]

use soroban_sdk::{Address, Env, contract, contractimpl};

/// Minimal mock used by `factory` deploy_pt tests. Exposes only the
/// `initialize(ibt, duration, authority, registry)` signature the factory
/// invokes after `deploy_v2`, with a no-op body. Decouples factory tests
/// from the real principal-token WASM (which exceeds the 131 KB
/// ContractCode size limit). Real-PT behaviour is exercised by
/// principal-token's own test suite.
#[contract]
pub struct MockFactoryPtInitContract;

#[contractimpl]
impl MockFactoryPtInitContract {
    pub fn initialize(
        _env: &Env,
        _ibt: Address,
        _duration: u64,
        _authority: Address,
        _registry: Address,
    ) {
    }
}
