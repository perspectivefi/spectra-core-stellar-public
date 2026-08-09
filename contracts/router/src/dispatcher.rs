// SPDX-License-Identifier: BUSL-1.1
// Bridge trait + mock impls both exceed clippy's default 7-arg threshold;
// the bridge's ABI requires this surface area.
#![allow(clippy::too_many_arguments)]

use crate::errors::RouterError;
use crate::storage;
use bridge_traits::PtBridgeClient;
use soroban_sdk::auth::InvokerContractAuthEntry;
use soroban_sdk::{Address, Env, Vec as SVec, contractclient, panic_with_error, token, vec};
use spectra_common::auth::{authorize_pull, transfer_entry};
use spectra_common::limit_order_engine::LimitOrderEngineClient;
use spectra_common::principal_token::PrincipalTokenClient;
use spectra_common::router_types::{
    Amount, BridgeNativePtToEvmArgs, BridgeWrappedPtBackToEvmArgs, BrokerSwapArgs, Command,
    Recipient,
};
use stellar_tokens::vault::FungibleVaultClient;

#[contractclient(name = "BrokerClient")]
pub trait StellarBroker {
    fn swap(
        env: Env,
        selling: Address,
        routes: soroban_sdk::Vec<spectra_common::router_types::Route>,
        trader: Address,
        vfee: u32,
        ffee: u32,
        fpath: soroban_sdk::Vec<spectra_common::router_types::PathStep>,
    ) -> soroban_sdk::Vec<i128>;
}

pub(crate) fn dispatch(env: &Env, caller: &Address, cmd: &Command) {
    let router = env.current_contract_address();
    match cmd {
        Command::TransferFrom(args) => {
            let amt = resolve_amount(env, &args.token, caller, &args.amount);
            token::Client::new(env, &args.token).transfer_from(&router, caller, &router, &amt);
        }
        Command::Transfer(args) => {
            let amt = resolve_amount(env, &args.token, &router, &args.amount);
            let to = resolve_recipient(caller, &router, &args.recipient);
            token::Client::new(env, &args.token).transfer(&router, &to, &amt);
        }
        Command::AssertMinBalance(args) => {
            if args.min_value < 0 {
                panic_with_error!(env, RouterError::NegativeMinValue);
            }
            let owner = resolve_recipient(caller, &router, &args.owner);
            let bal = token::Client::new(env, &args.token).balance(&owner);
            if bal < args.min_value {
                panic_with_error!(env, RouterError::AssertMinBalanceFailed);
            }
        }
        Command::DepositAssetInIbt(args) => {
            let ibt_client = FungibleVaultClient::new(env, &args.ibt);
            let underlying = ibt_client.query_asset();
            let assets = resolve_amount(env, &underlying, &router, &args.assets);
            let to = resolve_recipient(caller, &router, &args.recipient);
            // Soroban auth: router is two hops from the eventual
            // `token.transfer(router, ibt, assets)` inside the vault, so the
            // host's auto-self-auth doesn't propagate. Pre-declare the nested
            // transfer as authorized-by-router.
            authorize_pull(env, &underlying, &router, &args.ibt, assets);
            ibt_client.deposit(&assets, &to, &router, &router);
        }
        Command::RedeemIbtForAsset(args) => {
            let ibt_client = FungibleVaultClient::new(env, &args.ibt);
            let shares = resolve_amount(env, &args.ibt, &router, &args.shares);
            let to = resolve_recipient(caller, &router, &args.recipient);
            // owner == operator == router. Burn is local to the vault;
            // outbound token transfer is from vault (auto-self-authorized).
            ibt_client.redeem(&shares, &to, &router, &router);
        }
        Command::DepositIbtInPt(args) => {
            // `min_shares` is a signed slippage floor; reject negatives before
            // dispatch, consistent with the bridge/broker guards.
            if args.min_shares < 0 {
                panic_with_error!(env, RouterError::NegativeAmount);
            }
            let pt_client = PrincipalTokenClient::new(env, &args.pt);
            let ibt = pt_client.get_ibt();
            let ibts = resolve_amount(env, &ibt, &router, &args.ibts);
            let pt_to = resolve_recipient(caller, &router, &args.pt_recipient);
            let yt_to = resolve_recipient(caller, &router, &args.yt_recipient);
            // PT pulls IBT via SEP-41 `transfer_from(spender=pt, from=router,
            // to=pt, ibts)`, which requires a stored allowance — auth signals
            // alone aren't enough. Grant a one-shot allowance.
            // Consumed by the very next call in this same transaction, so the
            // smallest valid window suffices; matches the engine's one-shot
            // approvals.
            let exp = env.ledger().sequence();
            token::Client::new(env, &ibt).approve(&router, &args.pt, &ibts, &exp);
            pt_client.deposit_ibt_with_min_shares(&router, &ibts, &pt_to, &yt_to, &args.min_shares);
        }
        Command::RedeemPtForIbt(args) => {
            // `min_ibts` is a signed slippage floor; reject negatives before
            // dispatch, consistent with the bridge/broker guards.
            if args.min_ibts < 0 {
                panic_with_error!(env, RouterError::NegativeAmount);
            }
            let pt_client = PrincipalTokenClient::new(env, &args.pt);
            let mut shares = resolve_amount(env, &args.pt, &router, &args.shares);
            // Clamp to min(YT balance, shares) so pre-maturity redeem can't revert on insufficient YT.
            // WARNING: clamping leaves the unredeemed PT in the Router's shared custody — callers must
            // append a sweep to reclaim it (see README "Pre-maturity RedeemPtForIbt residual").
            if env.ledger().timestamp() < pt_client.maturity() {
                let yt_balance = token::Client::new(env, &pt_client.get_yt()).balance(&router);
                if shares > yt_balance {
                    shares = yt_balance;
                }
            }
            let to = resolve_recipient(caller, &router, &args.recipient);
            // owner == caller == router. PT internally burns PT+YT shares and
            // transfers IBT out from itself (auto-self-authorized for that leg).
            pt_client.redeem_for_ibt_with_min_ibts(&router, &shares, &to, &router, &args.min_ibts);
        }
        Command::RegisterOrders(args) => {
            // No token movement; the maker's per-order auth payload is path-agnostic.
            let engine = storage::get_engine(env)
                .unwrap_or_else(|| panic_with_error!(env, RouterError::EngineNotSet));
            LimitOrderEngineClient::new(env, &engine).register_orders_batch(&args.orders);
        }
        Command::CancelOrders(args) => {
            // No token movement; the maker's batch auth payload is identical direct-vs-router.
            let engine = storage::get_engine(env)
                .unwrap_or_else(|| panic_with_error!(env, RouterError::EngineNotSet));
            LimitOrderEngineClient::new(env, &engine).cancel_orders_batch(&args.orders);
        }
        Command::FillOrders(args) => {
            let engine = storage::get_engine(env)
                .unwrap_or_else(|| panic_with_error!(env, RouterError::EngineNotSet));
            let amt = resolve_amount(
                env,
                &args.taker_input_token,
                &router,
                &args.taker_input_amount,
            );
            let to = resolve_recipient(caller, &router, &args.receiver);

            authorize_pull(env, &args.taker_input_token, &router, &engine, amt);
            LimitOrderEngineClient::new(env, &engine).fill_orders(
                &router,
                &to,
                &args.params,
                &args.taker_input_token,
                &amt,
            );
        }
        Command::BridgeNativePtToEvm(args) => {
            dispatch_bridge_native(env, caller, &router, args);
        }
        Command::BridgeWrappedPtBackToEvm(args) => {
            dispatch_bridge_back(env, caller, &router, args);
        }
        Command::BrokerSwap(args) => {
            dispatch_broker_swap(env, caller, &router, args);
        }
    }
}

fn dispatch_bridge_native(
    env: &Env,
    caller: &Address,
    router: &Address,
    args: &BridgeNativePtToEvmArgs,
) {
    let lock_amount = resolve_amount(env, &args.pt, router, &args.amount);
    let gas_amount = resolve_amount(env, &args.gas_token, router, &args.gas_amount);

    if lock_amount < 0 || gas_amount < 0 {
        panic_with_error!(env, RouterError::NegativeAmount);
    }
    if args.min_amount_out < 0 {
        panic_with_error!(env, RouterError::NegativeMinAmountOut);
    }
    // Gas below the admin floor is rejected here (negatives caught above).
    if gas_amount < storage::get_min_gas_floor(env, args.target_chain_id, &args.gas_token) {
        panic_with_error!(env, RouterError::InsufficientGas);
    }

    let bridge = storage::get_bridge(env)
        .unwrap_or_else(|| panic_with_error!(env, RouterError::BridgeNotSet));
    let bridge_client = PtBridgeClient::new(env, &bridge);
    let messenger = bridge_client.get_messenger();
    let refund = resolve_recipient(caller, router, &args.refund_address);

    let mut entries: SVec<InvokerContractAuthEntry> = vec![env];
    entries.push_back(transfer_entry(env, &args.pt, router, &bridge, lock_amount));
    if gas_amount > 0 {
        entries.push_back(transfer_entry(
            env,
            &args.gas_token,
            router,
            &messenger,
            gas_amount,
        ));
    }
    env.authorize_as_current_contract(entries);

    bridge_client.bridge_native_pt_to_evm(
        router,
        &args.pt,
        &lock_amount,
        &args.target_chain_id,
        &args.evm_recipient,
        &gas_amount,
        &Some(refund),
        &args.min_amount_out,
    );
}

fn dispatch_bridge_back(
    env: &Env,
    caller: &Address,
    router: &Address,
    args: &BridgeWrappedPtBackToEvmArgs,
) {
    let burn_total = resolve_amount(env, &args.wrapped_pt, router, &args.amount);
    let gas_amount = resolve_amount(env, &args.gas_token, router, &args.gas_amount);

    if burn_total < 0 || gas_amount < 0 {
        panic_with_error!(env, RouterError::NegativeAmount);
    }
    if args.min_amount_out < 0 {
        panic_with_error!(env, RouterError::NegativeMinAmountOut);
    }
    // Gas below the admin floor is rejected here (negatives caught above).
    if gas_amount < storage::get_min_gas_floor(env, args.target_chain_id, &args.gas_token) {
        panic_with_error!(env, RouterError::InsufficientGas);
    }

    let bridge = storage::get_bridge(env)
        .unwrap_or_else(|| panic_with_error!(env, RouterError::BridgeNotSet));
    let bridge_client = PtBridgeClient::new(env, &bridge);
    let messenger = bridge_client.get_messenger();
    let fee_bps = bridge_client.get_bridge_back_fee_bps();
    let refund = resolve_recipient(caller, router, &args.refund_address);

    let fee_amount: i128 = (burn_total * fee_bps as i128) / 10_000;

    let mut entries: SVec<InvokerContractAuthEntry> = vec![env];
    if fee_amount > 0 {
        entries.push_back(transfer_entry(
            env,
            &args.wrapped_pt,
            router,
            &bridge,
            fee_amount,
        ));
    }
    if gas_amount > 0 {
        entries.push_back(transfer_entry(
            env,
            &args.gas_token,
            router,
            &messenger,
            gas_amount,
        ));
    }
    if !entries.is_empty() {
        env.authorize_as_current_contract(entries);
    }

    bridge_client.bridge_back_to_evm(
        router,
        &args.wrapped_pt,
        &burn_total,
        &args.target_chain_id,
        &args.evm_recipient,
        &gas_amount,
        &Some(refund),
        &args.min_amount_out,
    );
}

fn dispatch_broker_swap(env: &Env, _caller: &Address, router: &Address, args: &BrokerSwapArgs) {
    let broker = storage::get_broker(env)
        .unwrap_or_else(|| panic_with_error!(env, RouterError::BrokerNotSet));

    if args.min_amount_out < 0 {
        panic_with_error!(env, RouterError::NegativeMinAmountOut);
    }
    // Same-token routes break the balance-delta measurement: the outgoing
    // input reduces the same balance used to measure incoming output.
    if args.token_in == args.token_out {
        panic_with_error!(env, RouterError::InvalidRoutes);
    }

    // The broker pulls exactly the sum of per-route amounts from the trader
    // in a single transfer, and invoker auth entries match on exact args —
    // so authorize precisely that sum, derived from the routes themselves.
    if args.routes.is_empty() {
        panic_with_error!(env, RouterError::InvalidRoutes);
    }
    let mut amount_in: i128 = 0;
    for route in args.routes.iter() {
        if route.amount <= 0 {
            panic_with_error!(env, RouterError::InvalidRoutes);
        }
        amount_in += route.amount;
    }

    let token_out = token::Client::new(env, &args.token_out);
    let balance_before = token_out.balance(router);
    // Note: any pre-existing token_out balance on the Router is included in
    // the delta. Not exploitable in the pull-then-swap flow (the broker only
    // receives token_in), but worth being aware of.

    // The selling pull is the broker's only trader-side transfer: fees are
    // withheld from the bought amount and converted on the broker's own
    // balance, so no token_out authorization is needed.
    authorize_pull(env, &args.token_in, router, &broker, amount_in);

    // Router is trader (auto-self-auth); vfee=0 since the variable fee is unused here.
    BrokerClient::new(env, &broker).swap(
        &args.token_in,
        &args.routes,
        router,
        &0u32,
        &args.fixed_fee_permille,
        &args.fee_path,
    );

    // Trust the measured balance delta, not the broker's self-reported output.
    let received = token_out.balance(router) - balance_before;
    if received < args.min_amount_out {
        panic_with_error!(env, RouterError::AssertMinBalanceFailed);
    }
}

fn resolve_recipient(caller: &Address, router: &Address, r: &Recipient) -> Address {
    match r {
        Recipient::Caller => caller.clone(),
        Recipient::Router => router.clone(),
        Recipient::Address(a) => a.clone(),
    }
}

fn resolve_amount(env: &Env, t: &Address, owner: &Address, a: &Amount) -> i128 {
    match a {
        Amount::Exact(x) => *x,
        Amount::Balance => token::Client::new(env, t).balance(owner),
    }
}
