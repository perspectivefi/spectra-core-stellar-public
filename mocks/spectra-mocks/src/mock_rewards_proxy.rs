// SPDX-License-Identifier: BUSL-1.1
//! Records `claim_rewards` invocations so tests can assert PT correctly
//! delegated to the configured proxy.

use soroban_sdk::{Address, Bytes, Env, contract, contractimpl, contracttype};

#[contract]
pub struct MockRewardsProxyContract;

#[contracttype]
enum ProxyKey {
    LastCaller,
    LastData,
    Calls,
}

#[contractimpl]
impl MockRewardsProxyContract {
    pub fn claim_rewards(env: &Env, caller: Address, data: Bytes) {
        let st = env.storage().instance();
        st.set(&ProxyKey::LastCaller, &caller);
        st.set(&ProxyKey::LastData, &data);
        let calls: u32 = st.get(&ProxyKey::Calls).unwrap_or(0);
        st.set(&ProxyKey::Calls, &(calls + 1));
    }

    pub fn calls(env: &Env) -> u32 {
        env.storage().instance().get(&ProxyKey::Calls).unwrap_or(0)
    }

    pub fn last_caller(env: &Env) -> Option<Address> {
        env.storage().instance().get(&ProxyKey::LastCaller)
    }
}
