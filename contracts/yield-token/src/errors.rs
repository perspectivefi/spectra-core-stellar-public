// SPDX-License-Identifier: BUSL-1.1
use soroban_sdk::contracterror;

// YT-specific errors. Balance/allowance failures use
// `stellar_tokens::fungible::FungibleTokenError` (owned by OZ Base).
#[contracterror]
#[derive(Copy, Clone, Debug, Eq, PartialEq)]
#[repr(u32)]
pub enum YieldTokenError {
    AlreadyInitialized = 1,
}
