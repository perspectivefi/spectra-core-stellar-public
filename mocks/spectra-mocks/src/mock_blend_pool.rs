// SPDX-License-Identifier: BUSL-1.1
//! Minimal Blend v2 pool for wrapper tests. NOT a real pool — exposes
//! `set_b_rate` so tests drive accrual deterministically. Auth model,
//! request handling, and floor/ceil rounding mirror `blend-contracts-v2`
//! `execute_submit` for the Supply/Withdraw request types; interest,
//! collateral, and liabilities are out of scope.

use blend_types::{
    Positions, REQUEST_SUPPLY, REQUEST_WITHDRAW, Request, Reserve, ReserveConfig, ReserveData,
    SCALAR_12, to_asset_from_b_token, to_b_token_down, to_b_token_up,
};
use soroban_sdk::{Address, Env, Map, Vec, contract, contractimpl, contracttype, token};

#[contract]
pub struct MockBlendPoolContract;

#[contracttype]
enum PoolKey {
    ReserveIdx(Address),
    BRate(Address),
    BSupply(Address),
    SupplyCap(Address),
    Enabled(Address),
    DSupply(Address),
    DRate(Address),
    ReserveCount,
    Positions(Address),
    ClaimToken,
    ClaimAmount,
    LastClaim,
}

#[contracttype]
#[derive(Clone)]
pub struct LastClaim {
    pub from: Address,
    pub reserve_token_ids: Vec<u32>,
    pub to: Address,
}

#[contractimpl]
impl MockBlendPoolContract {
    /// Register `asset` as the next reserve with the given starting `b_rate`.
    pub fn add_reserve(env: &Env, asset: Address, b_rate: i128) {
        assert!(b_rate > 0);
        let st = env.storage().instance();
        let count: u32 = st.get(&PoolKey::ReserveCount).unwrap_or(0);
        st.set(&PoolKey::ReserveIdx(asset.clone()), &count);
        st.set(&PoolKey::BRate(asset.clone()), &b_rate);
        st.set(&PoolKey::BSupply(asset), &0i128);
        st.set(&PoolKey::ReserveCount, &(count + 1));
    }

    /// Simulate accrual (or loss) by repricing the bToken.
    pub fn set_b_rate(env: &Env, asset: Address, b_rate: i128) {
        assert!(b_rate > 0);
        let st = env.storage().instance();
        assert!(st.has(&PoolKey::ReserveIdx(asset.clone())));
        st.set(&PoolKey::BRate(asset), &b_rate);
    }

    /// Force `b_rate` to any value, including `0`, to simulate Blend clamping
    /// the rate to zero after bad debt (`pool/src/pool/user.rs`).
    pub fn force_b_rate(env: &Env, asset: Address, b_rate: i128) {
        let st = env.storage().instance();
        assert!(st.has(&PoolKey::ReserveIdx(asset.clone())));
        st.set(&PoolKey::BRate(asset), &b_rate);
    }

    /// Fixture: the reserve's supply cap (underlying-denominated, as Blend).
    pub fn set_supply_cap(env: &Env, asset: Address, supply_cap: i128) {
        env.storage()
            .instance()
            .set(&PoolKey::SupplyCap(asset), &supply_cap);
    }

    /// Fixture: toggle the reserve's `enabled` flag.
    pub fn set_enabled(env: &Env, asset: Address, enabled: bool) {
        env.storage()
            .instance()
            .set(&PoolKey::Enabled(asset), &enabled);
    }

    /// Fixture: borrowed liability, so available liquidity
    /// (`supplied - borrowed`) can be driven for the max_withdraw/redeem views.
    pub fn set_debt(env: &Env, asset: Address, d_supply: i128, d_rate: i128) {
        assert!(d_rate > 0);
        let st = env.storage().instance();
        st.set(&PoolKey::DSupply(asset.clone()), &d_supply);
        st.set(&PoolKey::DRate(asset), &d_rate);
    }

    /// Fixture for `claim`: the token and amount the next claim pays out.
    pub fn set_claim(env: &Env, token: Address, amount: i128) {
        let st = env.storage().instance();
        st.set(&PoolKey::ClaimToken, &token);
        st.set(&PoolKey::ClaimAmount, &amount);
    }

    pub fn submit(
        env: &Env,
        from: Address,
        spender: Address,
        to: Address,
        requests: Vec<Request>,
    ) -> Positions {
        spender.require_auth();
        if from != spender {
            from.require_auth();
        }
        let pool = env.current_contract_address();
        for r in requests.iter() {
            assert!(r.amount >= 0);
            let idx = reserve_idx(env, &r.address);
            let b_rate = b_rate(env, &r.address);
            let mut positions = positions(env, &from);
            match r.request_type {
                REQUEST_SUPPLY => {
                    let b = to_b_token_down(env, r.amount, b_rate);
                    positions
                        .supply
                        .set(idx, positions.supply.get(idx).unwrap_or(0) + b);
                    add_b_supply(env, &r.address, b);
                    token::Client::new(env, &r.address).transfer(&spender, &pool, &r.amount);
                }
                REQUEST_WITHDRAW => {
                    let cur = positions.supply.get(idx).unwrap_or(0);
                    // burn ceil, capped at the position like the real pool
                    let mut burn = to_b_token_up(env, r.amount, b_rate);
                    let mut out = r.amount;
                    if burn > cur {
                        burn = cur;
                        out = to_asset_from_b_token(env, cur, b_rate);
                    }
                    positions.supply.set(idx, cur - burn);
                    add_b_supply(env, &r.address, -burn);
                    token::Client::new(env, &r.address).transfer(&pool, &to, &out);
                }
                _ => panic!("unsupported request type"),
            }
            env.storage()
                .instance()
                .set(&PoolKey::Positions(from.clone()), &positions);
        }
        positions(env, &from)
    }

    pub fn get_reserve(env: &Env, asset: Address) -> Reserve {
        let idx = reserve_idx(env, &asset);
        let decimals = token::Client::new(env, &asset).decimals();
        let st = env.storage().instance();
        Reserve {
            asset: asset.clone(),
            config: ReserveConfig {
                index: idx,
                decimals,
                c_factor: 0,
                l_factor: 0,
                util: 0,
                max_util: 0,
                r_base: 0,
                r_one: 0,
                r_two: 0,
                r_three: 0,
                reactivity: 0,
                supply_cap: st
                    .get(&PoolKey::SupplyCap(asset.clone()))
                    .unwrap_or(i128::MAX),
                enabled: st.get(&PoolKey::Enabled(asset.clone())).unwrap_or(true),
            },
            data: ReserveData {
                d_rate: st.get(&PoolKey::DRate(asset.clone())).unwrap_or(SCALAR_12),
                b_rate: st.get(&PoolKey::BRate(asset.clone())).unwrap(),
                ir_mod: 0,
                b_supply: st.get(&PoolKey::BSupply(asset.clone())).unwrap_or(0),
                d_supply: st.get(&PoolKey::DSupply(asset)).unwrap_or(0),
                backstop_credit: 0,
                last_time: env.ledger().timestamp(),
            },
            scalar: 10i128.pow(decimals),
        }
    }

    pub fn get_positions(env: &Env, address: Address) -> Positions {
        positions(env, &address)
    }

    pub fn claim(env: &Env, from: Address, reserve_token_ids: Vec<u32>, to: Address) -> i128 {
        from.require_auth();
        let st = env.storage().instance();
        st.set(
            &PoolKey::LastClaim,
            &LastClaim {
                from,
                reserve_token_ids,
                to: to.clone(),
            },
        );
        let amount: i128 = st.get(&PoolKey::ClaimAmount).unwrap_or(0);
        if amount > 0 {
            let claim_token: Address = st.get(&PoolKey::ClaimToken).unwrap();
            token::Client::new(env, &claim_token).transfer(
                &env.current_contract_address(),
                &to,
                &amount,
            );
        }
        amount
    }

    pub fn last_claim(env: &Env) -> Option<LastClaim> {
        env.storage().instance().get(&PoolKey::LastClaim)
    }
}

fn reserve_idx(env: &Env, asset: &Address) -> u32 {
    env.storage()
        .instance()
        .get(&PoolKey::ReserveIdx(asset.clone()))
        .unwrap_or_else(|| panic!("asset is not a reserve"))
}

fn b_rate(env: &Env, asset: &Address) -> i128 {
    env.storage()
        .instance()
        .get(&PoolKey::BRate(asset.clone()))
        .unwrap()
}

fn positions(env: &Env, address: &Address) -> Positions {
    env.storage()
        .instance()
        .get(&PoolKey::Positions(address.clone()))
        .unwrap_or(Positions {
            liabilities: Map::new(env),
            collateral: Map::new(env),
            supply: Map::new(env),
        })
}

fn add_b_supply(env: &Env, asset: &Address, delta: i128) {
    let st = env.storage().instance();
    let cur: i128 = st.get(&PoolKey::BSupply(asset.clone())).unwrap_or(0);
    st.set(&PoolKey::BSupply(asset.clone()), &(cur + delta));
}
