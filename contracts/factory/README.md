# Factory

Soroban port of `core-v2/src/factory/FactorySNG.sol`. Deploys PT contracts (which in turn deploy their paired YT) and registers them on the Registry. AMM deployment is deferred to a future release; the factory exposes `deploy_pt`.

## Mandatory bootstrap step

After deploying the Factory, the `FACTORY` role on the Registry contract **must** be granted to the Factory's deployed address. Until this grant is in place, every `deploy_pt` call panics inside `registry.add_pt`. Operators must run, after deploying the Factory:

```text
registry.grant_role(factory_address, "FACTORY", admin)
```

(Equivalent direct call: `access_control::grant_role` from the admin account.)

`FACTORY` here is the role on the **Registry** contract that gates `add_pt`. It is the only `FACTORY` role the deployment cares about — the Factory contract itself does not grant or read a `FACTORY` role on its own storage.

This is the Soroban analogue of granting the Solidity Factory the `Roles.REGISTRY_ROLE` on the AccessManager — the Soroban role is renamed to `FACTORY` so the gate is named after the principal that holds it.

## Solidity → Soroban differences

| Concern | Solidity (FactorySNG.sol) | Soroban Factory |
|---|---|---|
| Logic-contract upgrade model | BeaconProxy + beacon swap | Per-PT `pt.upgrade(new_wasm_hash)` |
| WASM source for new PT | `IRegistry.getPTBeacon()` | `spectra_common::registry::RegistryClient::get_pt_wasm_hash()` |
| Access control | External `AccessManager` + `setTargetFunctionRole` | In-contract `stellar_access` per contract; Factory passes `authority` into PT.`initialize` |
| Registration | `IRegistry.addPT(pt)` | `RegistryClient::add_pt(self, pt)` — Factory needs the Registry-side `FACTORY` role |
| `msg.sender` in events | `poolCreator = msg.sender` in `PTDeployed` | `deploy_pt` takes a `caller: Address` arg with `require_auth()` and records it as `deployer` in `PtDeployed` |
| Initializer args | `(initialAuthority, curveFactory)` | `(admin, registry, authority)` — registry is now a deployment-time invariant |
| Address derivation | `CREATE` (nonce) | `env.deployer().with_current_contract(salt).deploy_v2(...)`; salt = `sha256(duration ‖ counter)` |
| Reentrancy | None used | None needed — no external user funds custodied by Factory in this PR |

Why cross-contract clients via `spectra_common::{registry, principal_token}` traits rather than direct path-deps: a workspace path-dep on a `#[contract]` crate causes the Soroban SDK spec-shaker to leak the dependency's `#[contractimpl]` exports into the consumer's WASM (e.g. PT WASM dropping `initialize` and gaining Registry's `add_pt`). The trait + `#[contractclient]` pattern generates a typed client without dragging in the implementation.

## TTL strategy

- **All Factory state is Instance tier.** Single keyset, infrequent writes, must persist for the contract's lifetime.
- `storage::extend_instance_ttl(env)` is called by every storage accessor on both read and write, matching the principal-token pattern. No explicit bumps are needed at entry-point exits.
- Read-only getters extend TTL through their storage accessors.
- No Persistent or Temporary state. PTs maintain their own TTL; Factory holds no per-PT records (the PT list lives in Registry).
