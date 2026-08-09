// SPDX-License-Identifier: BUSL-1.1
use soroban_sdk::contracterror;

/// Codes start at 200: 1–3 is the parent wrapper, 100+ OZ fungible,
/// 400+ OZ vault, 1000+ pausable, 2000+ access control.
#[contracterror]
#[derive(Copy, Clone, Debug, Eq, PartialEq, PartialOrd, Ord)]
#[repr(u32)]
pub enum BlendWrapperError {
    AlreadyInitialized = 200,
    /// Blend bTokens are not transferable: mint/withdraw/wrap/unwrap have no implementation.
    NotSupported = 201,
    NegativeAmount = 202,
    /// bTokens credited by the pool diverge from the floor preview.
    BTokenMismatch = 203,
}
