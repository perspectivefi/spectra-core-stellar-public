// SPDX-License-Identifier: BUSL-1.1
use soroban_sdk::contracterror;

#[contracterror]
#[derive(Copy, Clone, Debug, Eq, PartialEq)]
#[repr(u32)]
pub enum FactoryError {
    AlreadyInitialized = 1,
    NotInitialized = 2,
    RegistryAddressInvalid = 3,
    InvalidDuration = 4,
}
