# spectra-fungible-vault-wrapper

Soroban port of the Solidity `Spectra4626Wrapper` contract: the
**abstract base** wrapper that makes a non-vault interest-bearing token (the *vault share* / IBT) a
compliant **FungibleVault** (the Soroban analog of ERC-4626) so a Spectra Principal Token can be
deployed on top of it. Like the Solidity base, it is abstract — it ships **no deployable contract**;
a concrete instance supplies the rate conversions and 4626 flows. See `blend-fungible-vault-wrapper`
for a worked instance.

## Layout

- **`lib.rs` — the `SpectraFungibleVaultWrapper` trait.** Provides, as default methods, everything
  generic: the `wrap`/`unwrap` pair, previews, `total_assets`, the rewards proxy, and the admin
  methods (`pause`/`upgrade`/`*_role`). Plus `init_base`, a free fn (not a trait method, so it is
  *not* an on-chain entry point) holding the shared initializer body — the analog of Solidity's
  `internal __Spectra4626Wrapper_init`.
- **Abstract seams (required trait methods, no default body):**
  `convert_to_shares_with_rounding` / `convert_to_assets_with_rounding` (the rate, analog of
  Solidity's virtual `_convertToShares`/`_convertToAssets`) and the flows
  `deposit`/`mint`/`withdraw`/`redeem`. Every instance must implement these; the base routes all
  previews and `total_assets` through the conversion pair so overriding it retargets the quotes.

## Building an instance

Implement the `SpectraFungibleVaultWrapper` trait on your contract, supply the required seams, and
reuse `init_base` (or write a bespoke `initialize`, as Blend does, when the IBT is not a token whose
decimals can be read). Each instance also implements `FungibleToken` (`type ContractType = Vault`)
and is responsible for pause-gating its own methods — `transfer`/`transfer_from` via
`#[when_not_paused]` (Solidity's `_update` `whenNotPaused`), and its `deposit`/`mint`/`withdraw`/
`redeem`. The in-test `RefWrapper` in `test.rs` is a minimal instance used to exercise the base
scaffolding.

## Notes on the trait design

- The trait's supertrait is **`FungibleToken`, not `FungibleVault`** — so the vault methods are not
  inherited; each is restated to surface it in the ABI and to keep the override points
  (`convert_*`) on one trait, reachable by instances via `Self::`.
- **The 4626 flows are abstract here, unlike Solidity.** OZ `ERC4626Upgradeable` supplies a generic
  `deposit`/`mint`/`withdraw`/`redeem` that instances customize via `_deposit`/`_withdraw`; Soroban
  has no such shared layer, so each instance implements the flow outright.
- **File layout.** The workspace convention is `lib.rs - #[contract],
  #[contractimpl], public entry points`. Because this crate is abstract — no `#[contract]` lives
  here, only the trait + `init_base` — there are no contract entry points in `lib.rs`. Concrete
  instances (e.g. `blend-fungible-vault-wrapper`) follow the workspace convention themselves; this
  base crate intentionally does not.

## Solidity differences

- **Rewards proxy:** Soroban has no `delegatecall`, so the swappable-code proxy becomes a dynamic
  cross-contract call — `claim_rewards(caller, function, args)` invokes `function(args)` on the
  proxy. Two consequences vs. Solidity:
  - **Caller identity is not forwarded.** Solidity `delegatecall` runs the proxy code as the
    wrapper, so `msg.sender == harvester` inside the proxy. The Soroban port runs the proxy *as a
    separate contract* with the **wrapper** as the immediate caller — the harvester's identity is
    not propagated. A proxy that needs it must accept it explicitly in `args` (supplied by the
    off-chain caller).
  - **Proxy failures are structured.** A panic / host error inside the proxy is mapped to
    `SpectraFungibleVaultError::ClaimRewardsFailed` instead of surfacing as a raw host error,
    matching Solidity's `ClaimRewardsFailed()` revert reason.
- **`asset()` → `query_asset()`:** named per the canonical Soroban `FungibleVault` trait.
- **Auth/roles:** OZ `AccessManager` → `stellar_access` with explicit roles
  (`PAUSER`, `UPGRADE`, `REWARDS_PROXY_SETTER`, `REWARDS_HARVESTER`); all granted to `admin` at
  init, rotated post-init via `grant_role`/`revoke_role`. The admin role itself rotates via the
  `stellar_access` native 2-step (`transfer_admin_role` proposes, `accept_admin_transfer`
  finalizes), matching the rest of the workspace. `deposit`/`mint`/`withdraw`/`redeem` carry no
  `require_auth` — authorization is the caller's responsibility, matching FungibleVault.
- **Slippage:** `wrap`/`unwrap` add `*_with_min_*` variants (ERC-5143).

## TTL strategy

All wrapper state is **instance** storage: `VaultShare` and `RewardsProxy` (`storage.rs`), plus the
access-control admin/roles and the pause flag. Every state-changing entry point calls
`storage::extend_instance_ttl`, so the whole instance is bumped together; there is no persistent or
temporary state to manage separately.
