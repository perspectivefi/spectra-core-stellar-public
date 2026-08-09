# Principal Token

Soroban port of the Spectra V2 Solidity `PrincipalToken`.

## Solidity-to-Soroban differences

### IBT interface: OZ `FungibleVault`, not EVM ERC-4626

Stellar has no ERC-4626 standard. PT assumes the IBT implements OZ `stellar_tokens::vault::FungibleVault` (stellar-tokens 0.7.0) — the de-facto vault standard on Stellar — and calls it through the clients OZ's own `#[contracttrait]` macro generates (`stellar_tokens::vault::FungibleVaultClient` / `stellar_tokens::fungible::FungibleTokenClient`), so the call surface cannot drift from the canonical trait. Differences vs the Solidity ERC-4626 assumption:

- The underlying-asset accessor is `query_asset()`, not `asset()`.
- Mutating calls carry explicit auth args: `deposit(assets, receiver, from, operator)`, `redeem(shares, receiver, owner, operator)`, `withdraw(assets, receiver, owner, operator)`; the vault enforces `operator.require_auth()`. PT always passes `from = owner = operator = PT` (it holds the funds it wraps/unwraps), so the vault's auth check is satisfied by PT's direct-invoker auto-auth.
- The deposit asset pull is auth-based, not allowance-based: when `operator == from` the vault pulls via `transfer(from, vault)`. PT pre-authorizes that nested transfer with `spectra_common::auth::authorize_pull` (`env.authorize_as_current_contract`) instead of the Solidity-era ERC-20 approve — the transfer frame is two hops from PT, out of reach of direct-invoker auto-auth.
- IBT `preview_withdraw` rounds **Ceil** (OZ); `preview_deposit` / `preview_redeem` round Floor.
- Vault `decimals() = asset_decimals + decimals_offset` (OZ virtual-offset inflation-attack mitigation). PT just reads the value; the rate math is unaffected.
- If PT's IBT balance cannot cover a withdraw/redeem, the panic comes from the vault's `max_withdraw` / `max_redeem` check with the **vault's** error code, not a PT error. Behavior-equivalent (both trap).

### Slippage sentinels

The Solidity overloads without slippage params (`deposit(assets, receiver)`, `redeem(shares, receiver, owner)`, `withdraw(assets, receiver, owner)`) skip the check entirely. On Soroban the slippage variants reproduce "no protection" via sentinels:

| Operation | Sentinel for "no protection" | Why it works |
|---|---|---|
| `deposit_with_min_shares` / `deposit_ibt_with_min_shares` | `min_shares = 0` | `if shares < 0 panic` is unreachable for non-negative amounts. Matches Solidity's `uint256` semantics. |
| `redeem_with_min_assets` / `redeem_for_ibt_with_min_ibts` | `min_assets = 0` (or `min_ibts = 0`) | Same: `if assets < 0 panic` is unreachable. |
| `withdraw_with_max_shares` / `withdraw_ibt_with_max_shares` | `max_shares = i128::MAX` | `if shares > i128::MAX panic` is impossible. Matches Solidity's `type(uint256).max` sentinel. |

### Receiver conventions

`deposit` takes both `pt_receiver` and `yt_receiver` for the rare case where PT and YT should be minted to different addresses; Solidity's 2-arg overload corresponds to passing the same address twice.

### `caller` semantics

Every mutating PT entry takes an explicit `caller: Address` — Soroban's explicit version of Solidity's `msg.sender`. It carries no privileged role; deposit/redeem/withdraw are permissionless w.r.t. `owner`'s authority. Admin only matters for `pause` / `unpause` / `upgrade` / fee-setter calls.

Concrete redeem flows:

| Flow | `owner` | `caller` | What happens |
|---|---|---|---|
| Alice redeems her own PT | Alice | Alice | owner auth enforced through the PT/YT burn, no allowance spend |
| Router redeems on Alice's behalf (Alice approved Router for `shares` PT) | Alice | Router | Router's auth, then `spend_allowance(Alice, Router, shares)` on PT's allowance map. **Pre-expiry, this also spends a YT allowance**: PT burns the paired YT via `burn_without_yield_update`, which on `owner != spender` requires an allowance from `owner` on the **YT** token too — grant both the PT and YT allowances, else the YT `burn_from` panics `InsufficientAllowance` before the PT burn. Post-expiry no YT is burned. |
| Admin tries to redeem someone else's PT | victim | Admin | Same path as Router: needs a PT allowance (and, pre-expiry, a YT allowance) from victim. Admin role grants nothing here — without an allowance, the call panics on `spend_allowance`. |

### Interface surface

PT intentionally exposes only ERC-5143 naming for conversion/asset access. The ERC-4626/FungibleVault aliases (`convert_to_shares`, `convert_to_assets`, `query_asset`, `max_mint`, `mint`) are omitted to reduce audit surface and avoid redundant entrypoints.

`get_ibt()` stays separate — it returns the IBT vault address, distinct from the underlying. Solidity has both `underlying()` and `ibt()` for the same reason; we keep both as `underlying()` and `get_ibt()`.

### `mint` / `preview_mint`

ERC-4626 defines a `mint(shares, ...)` flow ("mint exact shares, deposit whatever assets needed"). Solidity Spectra does not implement this, and PT does not expose `mint` or `preview_mint`.

### Initialization

`initialize(env, ibt, duration, admin, registry)` replaces Solidity's `initialize(_ibt, _duration, _initialAuthority)` (lines 146–187). Four observable differences:

**Param shape.** Solidity treats `registry` as an `immutable` set by the factory's constructor. Soroban has no constructors — `registry` is a required init parameter and lives in instance storage. `admin` is a direct grantee, replacing Solidity's `_initialAuthority` (an `AccessManager` contract address).

**No zero-address validation.** Solidity reverts with `AddressError` if `_ibt == address(0) || _initialAuthority == address(0)`. Soroban drops the check entirely. There is no zero-`Address` sentinel, and any bogus address is caught at the first cross-contract call: an invalid IBT panics on `ibt.total_assets()` (which would also be `RateError == 0`), and an invalid registry panics on `registry.get_yt_wasm_hash()`. The `AddressError` variant is removed from the error enum.

**Access control: direct grant, not AccessManager.** Solidity's `__AccessManaged_init(_initialAuthority)` points the contract at an external `AccessManager`. Soroban uses in-contract OZ `stellar-access` state and grants `admin` the default admin role directly during initialization (matching `registry`).

**YT deployment: native `env.deployer()` instead of `BeaconProxy`.** Solidity calls `_deployYT` (lines 772–784), which reads a `getYTBeacon()` address from the registry and instantiates a `BeaconProxy`. Soroban reads `registry.get_yt_wasm_hash()` and calls `env.deployer().with_current_contract(salt).deploy(yt_wasm_hash)`, where `salt = sha256(ibt, duration, expiry)` for determinism. The `BeaconNotSet` error variant is removed — `get_yt_wasm_hash()` panics naturally if unset. PT passes `ibt_decimals` to `yt.initialize` explicitly so YT never calls back into PT during init (host reentrancy guard rejects re-entry while PT is on the stack).

### Naming utilities ported

`NamingUtil` (Solidity `src/libraries/NamingUtil.sol`) is ported to `spectra-common::naming`. PT and YT names/symbols embed the maturity date as `YYYY/MM/DD` exactly as in Solidity:

| Token | Name | Symbol |
|---|---|---|
| PT | `Principal Token: {ibt_sym}({underlying_sym}) {YYYY/MM/DD}` | `PT-{ibt_sym}({underlying_sym})-{YYYY/MM/DD}` |
| YT | `Yield Token: {ibt_sym}({underlying_sym}) {YYYY/MM/DD}` | `YT-{ibt_sym}({underlying_sym})-{YYYY/MM/DD}` |

The date math is a port of BokkyPooBahsDateTimeLibrary (`timestamp_to_date`).

**Type divergence — `u64` instead of Solidity's `int`.** Solidity's `_daysToDate` casts the input `uint _days` to `int` and performs every intermediate calculation in signed arithmetic, inherited from the original BokkyPooBahs C/Python source. The Soroban port uses `u64` throughout because every intermediate (`L`, `N`, `year_raw`, `month_raw`, `day_raw`) stays non-negative for any valid Unix timestamp — verified by tracing the algorithm. The output values are bit-identical to the Solidity version; only the intermediate types differ. The divergence is deliberate: it keeps types honest about the value range and avoids silencing the `cast_sign_loss` lint we apply across the workspace.

### Yield computation

Solidity isolates `computeYield` in a `PrincipalTokenUtil` library and calls it via `delegatecall`. Soroban inlines `compute_yield` as a private function inside PT — there is no Soroban equivalent of EVM library `delegatecall` that preserves storage context, and a real cross-contract call from PT to a util contract during yield settlement would land on the host reentrancy guard (PT is on the call stack). The math is bit-exact identical: 4 branches (positive only / case 2 PT depeg with subcases 2a IBT gain and 2b IBT loss / case 3 illegal PT increase), all in 27-decimal Ray, with `SAFETY_BOUND = 100` (in underlying token-decimals) zeroing dust in case 2b.

Solidity's `Math.mulDiv` uses a 512-bit intermediate to avoid overflow on Ray-scaled multiplies. `spectra-math::mul_div` uses bare `i128` and overflows for any realistic balance × Ray product, so PT calls `spectra_math::mul_div::mul_div_wide` — the same shape as `mul_div` but with a Soroban `I256` intermediate. The wide intermediate is necessary because `a*b` can reach ~10^62 even though the quotient `a*b/denom` ≈ 10^35 fits cleanly back in `i128`; the cast at the return is justified by the Ray-balance bounds documented on `mul_div_wide`.

### Per-user yield storage

Solidity stores three separate mappings (`ibtRateOfUser`, `ptRateOfUser`, `yieldOfUserInIBT`) keyed by user. Soroban compacts these into a single persistent entry: `DataKey::UserYield(Address) -> UserYieldData { ibt_rate, pt_rate, yield_in_ibt }`. `update_yield_inner` always reads/writes all three fields together, so compacting is pure upside — one storage operation instead of three, lower rent per user.

### `update_yield` and the YT-balance plumbing

Solidity exposes `updateYield(user)` and reads `yt.actualBalanceOf(user)` from inside the call. Soroban keeps the public entry but always passes `user_yt_balance` in: the trait method is `update_yield(env, user, balance) -> i128`. Reasons:

- **YT-originated calls** (e.g. `yt.burn`) pass the pre-burn balance directly. PT cannot read YT inside that call without hitting the host reentrancy guard.
- **PT-originated calls** read the balance via `read_yt_actual_balance(env, user)` (a private helper that does the cross-contract call) and pass it explicitly.

Public PT entry points that internally settle yield (`claim_yield`, `claim_yield_in_ibt`) follow the second path: they read YT balance once at the top, then call `update_yield_inner` with that balance. `before_yt_transfer` follows the first: YT passes both the sender and receiver pre-transfer balances; PT settles each side without ever calling back into YT.

## TTL strategy

| Data | Tier | TTL policy |
|---|---|---|
| `Initialized`, `Ibt`, `Underlying`, `Yt`, `Registry`, `IbtUnit`, `IbtDecimals`, `UnderlyingDecimals`, `Duration`, `Expiry`, `IbtRate`, `PtRate`, `UnclaimedFeesIbt`, `TotalFeesIbt`, `RatesAtExpiryStored`, `RewardsProxy`, OZ `Base` metadata, OZ `pausable` flag | **Instance** | Bumped to `env.storage().max_ttl()` on every storage read **and** write — the accessor helpers in `storage.rs` (the `addr_field!` / `scalar_field!` macros and the bespoke `get_rates_at_expiry_stored` / `set_rates_at_expiry_stored` / `try_get_rewards_proxy` / `set_rewards_proxy` / `remove_rewards_proxy`) all call `common_extend_instance_ttl` after touching the slot. Entry-point code does not bump explicitly; any state-changing path naturally hits at least one helper. |
| Per-user yield data (`UserYield(Address) -> UserYieldData`) | **Persistent** | Bumped to `PERSISTENT_TTL_EXTEND` (120 days, matching Blend's `LEDGER_BUMP_USER`) on every read **and** write via `storage::get_user_yield` / `storage::set_user_yield`. Threshold 100 days (Blend's `LEDGER_THRESHOLD_USER`). Active users refresh organically through any yield-touching entry (`update_yield` / `claim_yield` / `before_yt_transfer` / deposit / redeem / withdraw); inactive holders past the 120-day bump risk eviction (would silently zero their yield base on next read), so positions held to maturities >120 days should arrange a periodic Router-driven refresh. OZ `stellar_tokens::Base` uses 30 days for balances by comparison; we extend further because PT durations are typically 6mo+. |
| OZ `Base::Balance(account)` | **Persistent** | Managed by OZ. `Base::balance()` auto-extends to **30 days** (`BALANCE_EXTEND_AMOUNT`) when remaining TTL ≤29 days. Every transfer/mint/burn funnels through `balance()` so balances re-extend on each user op. PT does not layer its own extension on top — the 30-day OZ default is accepted because user activity refreshes it organically. Surface arrives with the vault PR. |
| Allowances (OZ `Base`) | **Temporary** | Managed by OZ. `Base::set_allowance()` extends to `live_until_ledger - current_ledger` on every approve. Not exercised in this PR — PT's transfer surface arrives with the vault PR. |

OZ never extends **instance** TTL itself (`fungible/mod.rs:63-67` in `stellar-tokens-0.7.0` is explicit: "instance TTL management is left to the implementor"), so PT must do it itself. Centralising the bump in the `storage.rs` accessor helpers (rather than at entry-point ends) means every read or write — including views like `get_current_yield_of_user_in_ibt` and the rate-snapshot reads inside `_get_current_pt_and_ibt_rates` — keeps the instance entry alive. Same pattern as YT (see `contracts/yield-token/README.md`). OZ `Base` *does* manage persistent (`Balance`) and temporary (`Allowance`) TTLs as noted above.

## Vault, rates, rewards (issue #37)

### Vault flow surface

Solidity exposes the ERC-4626 quartet plus Spectra-specific overloads (`deposit`/`depositIBT`/`redeem`/`redeemForIBT`/`withdraw`/`withdrawIBT`, each with optional `min_*`/`max_*` slippage variants). Solidity overloads on parameter count; Soroban can't, so each variant gets a distinct name:

| Solidity | Soroban |
|---|---|
| `deposit(assets, receiver)` | `deposit(caller, assets, pt_recv, yt_recv)` (single-receiver = pass same address twice) |
| `deposit(assets, ptR, ytR, minShares)` | `deposit_with_min_shares(caller, assets, pt_recv, yt_recv, min_shares)` |
| `depositIBT(...)` | `deposit_ibt`, `deposit_ibt_with_min_shares` |
| `redeem(shares, receiver, owner)` | `redeem(caller, shares, receiver, owner)` |
| `redeem(..., minAssets)` | `redeem_with_min_assets` |
| `redeemForIBT(...)` | `redeem_for_ibt`, `redeem_for_ibt_with_min_ibts` |
| `withdraw(assets, receiver, owner)` | `withdraw(caller, assets, receiver, owner)` |
| `withdraw(..., maxShares)` | `withdraw_with_max_shares` |
| `withdrawIBT(...)` | `withdraw_ibt`, `withdraw_ibt_with_max_shares` |

Solidity `mint`/`previewMint` have no Soroban equivalent — pulling an unknown amount of underlying for a precise share target is incompatible with the SEP-41 allowance flow (caller must commit the exact spend up front). `mint` is not exposed.

### Auth flow during deposit

Solidity uses `msg.sender` and `safeTransferFrom`, which propagate authorization implicitly. Soroban has no implicit caller — every authority is an explicit `Address` arg with `require_auth()`. The deposit flow:

1. PT's `deposit(caller, assets, pt_recv, yt_recv)` — `caller.require_auth()` at the top.
2. PT calls `underlying.transfer_from(spender=PT, from=caller, to=PT, assets)` — caller's allowance to PT must cover `assets`.
3. PT pre-authorizes the nested pull: `authorize_pull(env, underlying, PT, IBT, assets)` (`env.authorize_as_current_contract`) — the vault's `transfer(PT → IBT)` runs two hops below PT, where direct-invoker auto-auth doesn't reach.
4. PT calls `IBT.deposit(assets, receiver=PT, from=PT, operator=PT)` — the vault pulls `assets` from PT via `transfer`, mints IBT shares to PT.
5. `_deposit_ibt(caller, ibts, pt_recv, yt_recv)` runs the rest: settles yield on `yt_recv`, deducts tokenization fee, mints PT shares to `pt_recv` and YT shares to `yt_recv`.

`redeem` and `withdraw` deliberately **omit** the top-level `caller.require_auth()`. The YT burn (`Base::burn` for self, `Base::burn_from` for behalf-of) and the subsequent PT-side `Base::burn` already enforce owner / spender auth. Adding an extra root `require_auth` triggered Soroban's `Auth(ExistingValue)` (the auth tree forbids duplicate root entries).

### Zero-amount short-circuit

`redeem`, `redeem_for_ibt`, `withdraw`, and `withdraw_ibt` return `0` immediately when the input amount is zero. Without this guard, a zero-amount call still reaches `_before_redeem` / `_before_withdraw`, which settles the owner's yield checkpoint — and because `redeem` / `withdraw` have no top-level `caller.require_auth()`, anyone can force that settlement on the owner's behalf. The short-circuit makes the call a pure no-op, preventing unwanted yield checkpointing.

### `_deposit_ibt` / `_before_redeem` / `_before_withdraw`

Direct port of Solidity `_depositIBT` / `_beforeRedeem` / `_beforeWithdraw`. Two notable adaptations:

- `caller` is an explicit param (no `msg.sender`); used both as the `Mint` event's caller and as the YT-burn spender (so an allowance is consumed if `caller != owner`).
- `_max_burnable(owner)`: **pre-expiry** = `min(PT_balance, actual YT_balance)`; **post-expiry** = just `PT_balance` (YT becomes worthless). Calls `YT.actual_balance_of` (not `balance`) to bypass YT's expiry-check round trip — see "Avoiding YT→PT reentry" below.

### Tokenization fee

Solidity isolates `_computeTokenizationFee` in `PrincipalTokenUtil` and calls it via library-internal call. Soroban inlines `compute_tokenization_fee` as a private fn (same reasons as `compute_yield`: no `delegatecall`, host reentrancy guard). The composition is identical: `amount.mulDiv(tokenization_fee, FEE_DIVISOR, Ceil).mulDiv(FEE_DIVISOR - fee_reduction, FEE_DIVISOR, Ceil)`. `_update_fees(env, fees_in_ibt)` increments both `unclaimed_fees_ibt` and `total_fees_ibt`.

### Preview functions and per-user fee discount

Solidity discounted users get a faithful preview by spoofing `from = user_with_discount` on `eth_call` — `previewDeposit` reads `msg.sender` implicitly through the fee-reduction lookup. Soroban view simulation has no equivalent caller-spoof: a preview function only sees its explicit parameters.

To preserve ERC-4626 surface conformance and still give discounted users / SDKs an accurate quote, PT exposes two flavours per preview:

| Function | Fee applied | Use case |
|---|---|---|
| `preview_deposit(env, assets)` | Headline (unreduced) | ERC-4626-conformant generic preview |
| `preview_deposit_ibt(env, ibts)` | Headline (unreduced) | Same, IBT-side |
| `preview_deposit_for(env, assets, user)` | Per-user `fee_reduction` applied | Discounted users, SDKs needing faithful quote |
| `preview_deposit_ibt_for(env, ibts, user)` | Per-user `fee_reduction` applied | Same, IBT-side |

The bare 4626 forms route through `compute_tokenization_fee_base` (no registry `get_fee_reduction` lookup). The `_for` variants route through `compute_tokenization_fee` with the explicit `user`. Discounted users calling the bare forms see a *worse* preview than they will receive — funds-safe but UX-inaccurate. Tracked as issue #50.

Other preview functions (`preview_redeem`, `preview_redeem_for_ibt`, `preview_withdraw`, `preview_withdraw_ibt`) don't take a fee, so the discount is irrelevant — they have no `_for` variant.

### Fee reduction in deposit paths

`_deposit_ibt` computes the tokenization fee using `pt_receiver` (not `caller`) for the `get_fee_reduction` lookup, so the discount is applied to whoever receives the PT shares. When the Router calls `deposit_ibt`, `caller` is the Router and `pt_receiver` is the end user — the discount is correctly resolved against the user's PT balance.

Tokenization fees are currently set to zero across all registered PTs, so the discount has no practical effect today. To benefit from a nonzero fee reduction in the future, users must call the PT contract directly (bypassing the Router) so that their address is used for the `get_fee_reduction` lookup.

### Avoiding YT→PT reentry

`YT.balance` calls `PT.maturity` to check expiry (and returns 0 post-expiry). When PT calls `YT.balance` during a vault operation, Soroban's host reentrancy guard rejects the YT→PT round trip because PT is still on the call stack. Resolution: PT calls `YT.actual_balance_of` (already a public YT entry, returns `Base::balance` without the expiry check). The deposit/redeem/withdraw flows always need the *real* stored balance for yield settlement — never the expiry-zeroed view — so this is correct on its own merits.

### `store_rates_at_expiry` + frozen-rate dispatch

Solidity exposes `storeRatesAtExpiry()` (public, gated by `afterExpiry whenNotPaused`); Soroban mirrors it as `store_rates_at_expiry(env)`. Once the flag flips, all subsequent rate reads come from storage instead of the live IBT rate. `_update_pt_and_ibt_rates` auto-freezes when called past expiry, matching Solidity's implicit storeRatesAtExpiry path.

### Rewards proxy

Solidity uses `delegatecall` to a swappable proxy so reward harvest logic can change without redeploying PT. Soroban has no `delegatecall` that preserves storage context, so `claim_rewards(caller, function, args)` makes a dynamic cross-contract call — the caller names the proxy entry point and passes its args (same surface as the wrapper's rewards proxy). The proxy receives any context (e.g. the caller) explicitly through `args` — there is no implicit `msg.sender`. `set_rewards_proxy` is gated by `RWD_PROX`; `claim_rewards` by `RWD_HARV` (both granted to admin at init).

### Upgrade

Solidity uses a beacon proxy pattern. Soroban exposes `env.deployer().update_current_contract_wasm(new_wasm_hash)` directly, which is the canonical Soroban upgrade mechanism. PT's `upgrade(caller, new_wasm_hash)` is gated on the `UPGRADE` role (granted to admin at init).

### Storage additions for vault

| Key | Tier | TTL policy |
|---|---|---|
| `RatesAtExpiryStored: bool` | **Instance** | Set to `true` exactly once; flipped only by `store_rates_at_expiry_inner`. |
| `RewardsProxy: Address` | **Instance** | Set/replaced by `set_rewards_proxy` (`RWD_PROX`); emits `RewardsProxyChange{old, new}`. |


## Role management (post-init)

`initialize` grants the deploying admin the **PAUSER**, **UPGRADE**, **REWARDS_HARVESTER**, and **REWARDS_PROXY_SETTER** roles via `access_control::grant_role_no_auth`. To rotate role holders after deployment (deploy dispatcher, key rotation), PT exposes three thin wrappers over `stellar_access::access_control`:

- `grant_role(caller, account, role)` — grant `role` to `account`. `caller` must be the contract admin or hold the admin-role of `role`.
- `revoke_role(caller, account, role)` — revoke; same auth model.
- `has_role(account, role) -> bool` — view.

These wrappers live on the bare `impl PrincipalTokenContract` block, not the `PrincipalToken` trait, because they are contract-side infra rather than part of the cross-contract trait surface. Auth + admin-role checks are enforced by the library; mutators extend Instance TTL on success.
