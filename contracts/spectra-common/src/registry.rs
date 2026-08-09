// SPDX-License-Identifier: BUSL-1.1
//! Registry cross-contract surface.
//!
//! Mirrors Solidity's `IRegistry`. Declares only the methods other
//! protocol contracts (PT, Factory) reach across to. The real
//! `RegistryContract` provides the bodies and exposes more methods
//! (admin setters, PT removal) that aren't relevant cross-contract.
//!
//! Why a trait + `#[contractclient]` instead of a workspace path-dep
//! on the `registry` crate: a path-dep on a `#[contract]` crate leaks
//! the dependency's `#[contractimpl]` exports into the consumer's
//! WASM and drops some of the consumer's own exports (observed: PT
//! WASM losing `initialize` while gaining Registry's `add_pt`,
//! `set_*` exports). Using a trait generates a typed client without
//! dragging in the contract.

use soroban_sdk::{Address, BytesN, Env, contractclient};

#[contractclient(name = "RegistryClient")]
pub trait Registry {
    /// PT WASM hash used by the Factory to deploy new PTs.
    /// Solidity: `getPTBeacon()`.
    fn get_pt_wasm_hash(env: &Env) -> BytesN<32>;

    /// YT WASM hash used by the PT to deploy its paired YT.
    /// Solidity has no direct equivalent — YT is part of the PT proxy.
    fn get_yt_wasm_hash(env: &Env) -> BytesN<32>;

    /// Protocol fee collector address. PT sends collected fees here.
    /// Solidity: `getFeeCollector()`.
    fn get_fee_collector(env: &Env) -> Address;

    /// Same as `get_fee_collector` but returns `None` instead of panicking
    /// when the registry has never set a fee collector. Lets callers
    /// (notably PT's `claim_fees`) surface a structured error rather than
    /// the unstructured host panic from `Option::unwrap` on `None`.
    ///
    /// Named `get_fee_collector_opt` (not `try_get_fee_collector`) because
    /// the `#[contractclient]` macro auto-generates a `try_*` variant for
    /// every entry — a method literally named `try_*` collides with the
    /// generated one.
    fn get_fee_collector_opt(env: &Env) -> Option<Address>;

    /// Registry-level tokenization fee rate (unreduced). Per-PT
    /// `fee_reduction` is applied separately at the call site.
    /// Solidity: `getTokenizationFee()`.
    fn get_tokenization_fee(env: &Env) -> i128;

    /// Yield fee rate skimmed from user yield on claim/settle.
    /// Solidity: `getYieldFee()`.
    fn get_yield_fee(env: &Env) -> i128;

    /// Per-`(pt, user)` fee reduction applied on top of the registry's
    /// tokenization fee. Solidity: `getFeeReduction(pt, user)`.
    fn get_fee_reduction(env: &Env, pt: Address, user: Address) -> i128;

    /// Append a new PT to the registered set. Only callable by an account
    /// holding the `REGISTRY` role on this contract (in practice: the
    /// Factory). Solidity: `addPT(address)`.
    fn add_pt(env: &Env, caller: Address, pt: Address);

    /// Whether `pt` is in the registered set. Read-only gate used by the
    /// LimitOrderEngine to reject orders against unregistered PTs.
    /// Solidity: `isRegisteredPT(address)`.
    fn is_registered_pt(env: &Env, pt: Address) -> bool;
}
