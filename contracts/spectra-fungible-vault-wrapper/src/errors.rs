// SPDX-License-Identifier: BUSL-1.1
use soroban_sdk::contracterror;

/// Wrapper-specific errors. Token/vault math failures come from OZ
/// `FungibleTokenError` / `VaultTokenError`.
#[contracterror]
#[derive(Copy, Clone, Debug, Eq, PartialEq, PartialOrd, Ord)]
#[repr(u32)]
pub enum SpectraFungibleVaultError {
    AlreadyInitialized = 1,
    /// ERC-5143 slippage check: `wrap`/`unwrap` returned less than the caller's minimum.
    SlippageProtectionFailed = 2,
    /// `claim_rewards` called while no rewards proxy is set.
    NoRewardsProxy = 3,
    /// Proxy invocation in `claim_rewards` panicked or returned a host error.
    ClaimRewardsFailed = 4,
}
