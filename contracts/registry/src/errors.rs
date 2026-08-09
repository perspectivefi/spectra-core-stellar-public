// SPDX-License-Identifier: BUSL-1.1
use soroban_sdk::contracterror;

#[contracterror]
#[derive(Copy, Clone, Debug, Eq, PartialEq)]
#[repr(u32)]
pub enum RegistryError {
    AlreadyInitialized = 1,
    // 2 reserved (was AddressError — dead code, removed; do not re-use).
    FeeGreaterThanMaxValue = 3,
    ReductionTooBig = 4,
    PtListUpdateFailed = 5,
    NegativeFee = 6,
    UnauthorizedCaller = 7,
}
