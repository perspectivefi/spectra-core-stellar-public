// SPDX-License-Identifier: BUSL-1.1
use soroban_sdk::contracterror;

#[contracterror]
#[derive(Copy, Clone, Debug, Eq, PartialEq, PartialOrd, Ord)]
#[repr(u32)]
pub enum RouterError {
    AlreadyInitialized = 1,
    DeadlineExpired = 4,
    EmptyCommands = 5,
    AssertMinBalanceFailed = 6,
    EngineNotSet = 7,
    BridgeNotSet = 8,
    BrokerNotSet = 9,
    InvalidRoutes = 10,
    NegativeMinAmountOut = 11,
    NegativeMinValue = 12,
    NegativeAmount = 13,
    InsufficientGas = 14,
}
