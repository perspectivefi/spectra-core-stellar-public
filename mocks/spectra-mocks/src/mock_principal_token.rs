// SPDX-License-Identifier: BUSL-1.1
//! Recording PT stub for YT (and downstream) tests.
//!
//! Implements the full `PrincipalToken` trait but only the YT-callback
//! surface (`before_yt_transfer`, `update_yield`, `paused`, `maturity`)
//! has real bodies — every other method is `unimplemented!()`. Tests that
//! exercise the vault/yield/admin surface must use the real
//! `PrincipalTokenContract` instead.
//!
//! `before_yt_transfer` / `update_yield` record their arguments under
//! `MockKey::*` so tests can assert YT made the expected call back into PT.

use soroban_sdk::{Address, BytesN, Env, Symbol, Val, Vec, contract, contractimpl, contracttype};
use spectra_common::principal_token::PrincipalToken;
use spectra_common::yield_token::YieldTokenClient;

#[contract]
pub struct MockPrincipalTokenContract;

#[contracttype]
#[derive(Clone)]
pub enum MockKey {
    LastBeforeYtTransfer,
    LastUpdateYield,
    Maturity,
}

#[contracttype]
#[derive(Clone)]
pub struct LastBeforeYtTransfer {
    pub from: Address,
    pub to: Address,
    pub from_balance: i128,
    pub to_balance: i128,
}

#[contracttype]
#[derive(Clone)]
pub struct LastUpdateYield {
    pub user: Address,
    pub user_yt_balance: i128,
}

#[contractimpl]
impl PrincipalToken for MockPrincipalTokenContract {
    fn maturity(env: &Env) -> u64 {
        env.storage()
            .instance()
            .get(&MockKey::Maturity)
            .unwrap_or(10_000_000_000)
    }

    fn paused(_env: &Env) -> bool {
        false
    }

    fn before_yt_transfer(
        env: &Env,
        from: Address,
        to: Address,
        from_balance: i128,
        to_balance: i128,
    ) {
        env.storage().instance().set(
            &MockKey::LastBeforeYtTransfer,
            &LastBeforeYtTransfer {
                from,
                to,
                from_balance,
                to_balance,
            },
        );
    }

    fn update_yield(env: &Env, user: Address, user_yt_balance: i128) -> i128 {
        env.storage().instance().set(
            &MockKey::LastUpdateYield,
            &LastUpdateYield {
                user,
                user_yt_balance,
            },
        );
        0
    }

    // Trait-completion stubs. Tests that need real behavior should swap in
    // the production `PrincipalTokenContract`.

    fn pause(_env: &Env, _caller: Address) {}
    fn unpause(_env: &Env, _caller: Address) {}
    fn transfer_admin_role(_env: &Env, _new_admin: Address, _live_until_ledger: u32) {
        unimplemented!()
    }
    fn accept_admin_transfer(_env: &Env) {
        unimplemented!()
    }
    fn claim_fees(_env: &Env, _caller: Address) -> i128 {
        0
    }
    fn store_rates_at_expiry(_env: &Env) {}

    fn get_ibt(env: &Env) -> Address {
        env.current_contract_address()
    }
    fn get_yt(env: &Env) -> Address {
        env.current_contract_address()
    }
    fn get_pt_rate(_env: &Env) -> i128 {
        0
    }
    fn get_ibt_rate(_env: &Env) -> i128 {
        0
    }
    fn get_ibt_unit(_env: &Env) -> i128 {
        0
    }
    fn get_duration(_env: &Env) -> u64 {
        0
    }
    fn get_unclaimed_fees_in_ibt(_env: &Env) -> i128 {
        0
    }
    fn get_total_fees_in_ibt(_env: &Env) -> i128 {
        0
    }

    fn deposit(
        _env: &Env,
        _caller: Address,
        _assets: i128,
        _pt_receiver: Address,
        _yt_receiver: Address,
    ) -> i128 {
        unimplemented!()
    }
    fn deposit_with_min_shares(
        _env: &Env,
        _caller: Address,
        _assets: i128,
        _pt_receiver: Address,
        _yt_receiver: Address,
        _min_shares: i128,
    ) -> i128 {
        unimplemented!()
    }
    fn deposit_ibt(
        _env: &Env,
        _caller: Address,
        _ibts: i128,
        _pt_receiver: Address,
        _yt_receiver: Address,
    ) -> i128 {
        unimplemented!()
    }
    fn deposit_ibt_with_min_shares(
        _env: &Env,
        _caller: Address,
        _ibts: i128,
        _pt_receiver: Address,
        _yt_receiver: Address,
        _min_shares: i128,
    ) -> i128 {
        unimplemented!()
    }

    fn redeem(
        _env: &Env,
        _caller: Address,
        _shares: i128,
        _receiver: Address,
        _owner: Address,
    ) -> i128 {
        unimplemented!()
    }
    fn redeem_with_min_assets(
        _env: &Env,
        _caller: Address,
        _shares: i128,
        _receiver: Address,
        _owner: Address,
        _min_assets: i128,
    ) -> i128 {
        unimplemented!()
    }
    fn redeem_for_ibt(
        _env: &Env,
        _caller: Address,
        _shares: i128,
        _receiver: Address,
        _owner: Address,
    ) -> i128 {
        unimplemented!()
    }
    fn redeem_for_ibt_with_min_ibts(
        _env: &Env,
        _caller: Address,
        _shares: i128,
        _receiver: Address,
        _owner: Address,
        _min_ibts: i128,
    ) -> i128 {
        unimplemented!()
    }

    fn withdraw(
        _env: &Env,
        _caller: Address,
        _assets: i128,
        _receiver: Address,
        _owner: Address,
    ) -> i128 {
        unimplemented!()
    }
    fn withdraw_with_max_shares(
        _env: &Env,
        _caller: Address,
        _assets: i128,
        _receiver: Address,
        _owner: Address,
        _max_shares: i128,
    ) -> i128 {
        unimplemented!()
    }
    fn withdraw_ibt(
        _env: &Env,
        _caller: Address,
        _ibts: i128,
        _receiver: Address,
        _owner: Address,
    ) -> i128 {
        unimplemented!()
    }
    fn withdraw_ibt_with_max_shares(
        _env: &Env,
        _caller: Address,
        _ibts: i128,
        _receiver: Address,
        _owner: Address,
        _max_shares: i128,
    ) -> i128 {
        unimplemented!()
    }

    fn claim_yield(_env: &Env, _caller: Address, _receiver: Address, _min_assets: i128) -> i128 {
        unimplemented!()
    }
    fn claim_yield_in_ibt(
        _env: &Env,
        _caller: Address,
        _receiver: Address,
        _min_ibts: i128,
    ) -> i128 {
        unimplemented!()
    }

    fn upgrade(_env: &Env, _caller: Address, _new_wasm_hash: BytesN<32>) {
        unimplemented!()
    }
    fn upgrade_yt(_env: &Env, _caller: Address, _new_wasm_hash: BytesN<32>) {
        unimplemented!()
    }
    fn rewards_proxy(_env: &Env) -> Option<Address> {
        unimplemented!()
    }
    fn set_rewards_proxy(_env: &Env, _caller: Address, _proxy: Option<Address>) {
        unimplemented!()
    }
    fn claim_rewards(_env: &Env, _caller: Address, _function: Symbol, _args: Vec<Val>) -> Val {
        unimplemented!()
    }

    fn total_assets(_env: &Env) -> i128 {
        unimplemented!()
    }
    fn convert_to_principal(_env: &Env, _underlying_amount: i128) -> i128 {
        unimplemented!()
    }
    fn convert_to_underlying(_env: &Env, _principal_amount: i128) -> i128 {
        unimplemented!()
    }
    fn underlying(_env: &Env) -> Address {
        unimplemented!()
    }

    fn preview_deposit(_env: &Env, _assets: i128) -> i128 {
        unimplemented!()
    }
    fn preview_deposit_ibt(_env: &Env, _ibts: i128) -> i128 {
        unimplemented!()
    }
    fn preview_deposit_for(_env: &Env, _assets: i128, _user: Address) -> i128 {
        unimplemented!()
    }
    fn preview_deposit_ibt_for(_env: &Env, _ibts: i128, _user: Address) -> i128 {
        unimplemented!()
    }
    fn preview_redeem(_env: &Env, _shares: i128) -> i128 {
        unimplemented!()
    }
    fn preview_redeem_for_ibt(_env: &Env, _shares: i128) -> i128 {
        unimplemented!()
    }
    fn preview_withdraw(_env: &Env, _assets: i128) -> i128 {
        unimplemented!()
    }
    fn preview_withdraw_ibt(_env: &Env, _ibts: i128) -> i128 {
        unimplemented!()
    }

    fn max_deposit(_env: &Env, _receiver: Address) -> i128 {
        unimplemented!()
    }
    fn max_redeem(_env: &Env, _owner: Address) -> i128 {
        unimplemented!()
    }
    fn max_withdraw(_env: &Env, _owner: Address) -> i128 {
        unimplemented!()
    }
    fn max_withdraw_ibt(_env: &Env, _owner: Address) -> i128 {
        unimplemented!()
    }

    fn get_tokenization_fee(_env: &Env) -> i128 {
        unimplemented!()
    }
    fn get_current_yield_of_user_in_ibt(_env: &Env, _user: Address) -> i128 {
        unimplemented!()
    }
}

/// Test-only helpers — not part of the `PrincipalToken` trait.
#[contractimpl]
impl MockPrincipalTokenContract {
    /// Invokes `yt.pause(self_addr)` — used to test pause propagation from PT
    /// to YT through the OZ Pausable setters without spinning up real PT.
    pub fn forward_pause(env: &Env, yt: Address) {
        let self_addr = env.current_contract_address();
        YieldTokenClient::new(env, &yt).pause(&self_addr);
    }

    pub fn forward_unpause(env: &Env, yt: Address) {
        let self_addr = env.current_contract_address();
        YieldTokenClient::new(env, &yt).unpause(&self_addr);
    }
}
