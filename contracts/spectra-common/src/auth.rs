// SPDX-License-Identifier: BUSL-1.1
//! Invoker pre-authorization helpers for nested token pulls.
//!
//! Soroban only auto-self-authorizes the *direct* caller of a contract. When
//! the current contract calls an intermediary (e.g. an OZ `FungibleVault`)
//! that then pulls tokens via SEP-41 `transfer`, that transfer frame is two
//! hops away and must be pre-declared with `authorize_as_current_contract`.

use soroban_sdk::auth::{ContractContext, InvokerContractAuthEntry, SubContractInvocation};
use soroban_sdk::{Address, Env, IntoVal, Vec, symbol_short, vec};

/// Build an auth entry for a nested `token.transfer(from, to, amount)`
/// invocation, for use with `env.authorize_as_current_contract`.
pub fn transfer_entry(
    env: &Env,
    token_addr: &Address,
    from: &Address,
    to: &Address,
    amount: i128,
) -> InvokerContractAuthEntry {
    let args: Vec<soroban_sdk::Val> = (from.clone(), to.clone(), amount).into_val(env);
    InvokerContractAuthEntry::Contract(SubContractInvocation {
        context: ContractContext {
            contract: token_addr.clone(),
            fn_name: symbol_short!("transfer"),
            args,
        },
        sub_invocations: vec![env],
    })
}

/// Pre-authorize a nested `token.transfer(from, to, amount)` invocation on
/// behalf of the current contract.
pub fn authorize_pull(env: &Env, token_addr: &Address, from: &Address, to: &Address, amount: i128) {
    env.authorize_as_current_contract(vec![env, transfer_entry(env, token_addr, from, to, amount)]);
}

#[cfg(test)]
mod tests {
    use super::*;
    use soroban_sdk::testutils::Address as _;

    /// `transfer_entry` produces a `Contract` variant of `InvokerContractAuthEntry`
    /// targeting `token_addr` with fn_name `transfer` and exactly 3 args.
    #[test]
    fn test_transfer_entry_shape_targets_token_with_transfer_fn() {
        let env = Env::default();
        let token = Address::generate(&env);
        let from = Address::generate(&env);
        let to = Address::generate(&env);
        let amount: i128 = 1_000;

        let entry = transfer_entry(&env, &token, &from, &to, amount);
        match entry {
            InvokerContractAuthEntry::Contract(sub) => {
                assert_eq!(sub.context.contract, token);
                assert_eq!(sub.context.fn_name, symbol_short!("transfer"));
                assert_eq!(sub.context.args.len(), 3);
                assert!(sub.sub_invocations.is_empty());
            }
            _ => panic!("expected Contract variant"),
        }
    }

    /// `transfer_entry` for amount=0 still produces a valid entry shape.
    /// Pins that the helper is amount-agnostic at construction time.
    #[test]
    fn test_transfer_entry_zero_amount_still_constructs() {
        let env = Env::default();
        let token = Address::generate(&env);
        let from = Address::generate(&env);
        let to = Address::generate(&env);

        let entry = transfer_entry(&env, &token, &from, &to, 0);
        if let InvokerContractAuthEntry::Contract(sub) = entry {
            assert_eq!(sub.context.args.len(), 3);
        } else {
            panic!("expected Contract variant");
        }
    }

    /// Negative amounts are not validated at construction time — they
    /// propagate down to the eventual SEP-41 `transfer` call which will
    /// reject them. Pin this responsibility-shift.
    #[test]
    fn test_transfer_entry_negative_amount_constructs_validation_deferred_to_token() {
        let env = Env::default();
        let token = Address::generate(&env);
        let from = Address::generate(&env);
        let to = Address::generate(&env);

        let entry = transfer_entry(&env, &token, &from, &to, -1);
        if let InvokerContractAuthEntry::Contract(sub) = entry {
            assert_eq!(sub.context.args.len(), 3);
        } else {
            panic!("expected Contract variant");
        }
    }
}
