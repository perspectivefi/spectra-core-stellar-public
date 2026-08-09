# Router

Soroban port of Solidity `Router` / `Dispatcher`. Batch executor: takes an ordered `Vec<Command>` and runs each variant atomically.

## Key differences vs Solidity

**Typed `Command` enum** replaces `bytes commands` + `bytes[] inputs`. Variants in `spectra_common::router_types`.

**Sentinels are enums**:
- `CONTRACT_BALANCE` → `Amount::Balance`
- `MSG_SENDER` → `Recipient::Caller`
- `ADDRESS_THIS` → `Recipient::Router`

**No `msgSender` slot** — caller is passed explicitly; No flashloans in this version (and even when implemented the desing will be differetn due to soroban callback gurad), so no need to store the sender.

**Auth**: `caller.require_auth()` once at the top. Beyond that the dispatcher uses one of four patterns depending on how the downstream contract pulls funds:

| Pattern | When | Why |
|---|---|---|
| Direct `transfer_from(spender=router, from=caller, …)` | `Command::TransferFrom` (the caller has already approved the router) | Allowance check happens at the token; no extra plumbing. |
| `authorize_pull` = `env.authorize_as_current_contract([transfer_entry])` | The callee will itself invoke `token.transfer(from=router, …)` two hops away (e.g. `IBT.deposit`, `LimitOrderEngine.fill_orders`, bridge `transfer` entries) | Soroban auto-self-auth only covers the *direct* call out of the contract. Nested invocations need the router to pre-declare them as authorized. |
| `token.approve(router, callee, amount, exp)` followed by the call | The callee will invoke `token.transfer_from(spender=callee, from=router, …)` (e.g. `PT.deposit_ibt_with_min_shares` — the PT pulls IBT from the router) | SEP-41 `transfer_from` checks a *stored* allowance, not auth signals. `exp = current_sequence` — the smallest valid window (allowance is consumed in the same transaction), matching the engine's one-shot approvals. |
| `env.authorize_as_current_contract([entry1, entry2, …])` with a `vec![]` of explicit `transfer_entry`s | Bridge commands (`BridgeNativePtToEvm`, `BridgeWrappedPtBackToEvm`) — the bridge will pull both PT/wrapped-PT and the gas token | Same reason as the second row, but multiple distinct transfers need to be pre-declared in one call. |

Rule of thumb: **`approve` when the callee uses `transfer_from`; `authorize_pull` when the callee uses `transfer`.**

`authorize_pull` / `transfer_entry` live in `spectra_common::auth` (shared with PT, which needs the same pre-auth for OZ `FungibleVault` deposits).

## Order-engine commands

Two variants front the `LimitOrderEngine` (address in instance storage under `DataKey::Engine`; admin setter `set_engine`, both panic `EngineNotSet` if unset):

- `RegisterOrders { orders }` — creates resting maker orders via the engine's `register_orders_batch`. Moves no tokens; the engine authorizes each order with `require_auth_for_args((order,))`, so each maker signs only its own order (payload `(engine, "register_orders_batch", [order])`) — a relayer/taker can submit others' pre-signed orders. That payload is identical whether the batch is submitted directly to the engine or through this command, and one `execute` can stage `RegisterOrders` then `FillOrders` atomically.
- `CancelOrders { orders }` — cancels resting orders via the engine's `cancel_orders_batch`. Moves no tokens. Cancellation is self-service, so every order in the batch must share one maker, who authorizes the batch once (payload `(engine, "cancel_orders_batch", [orders])`); a member belonging to another maker reverts with `CancelBatchMakerMismatch`. Best-effort: expired and already-terminal members are skipped.
- `FillOrders { receiver, params, taker_input_token, taker_input_amount }` — fills a batch via `fill_orders`; the router pre-stages the taker payment and `authorize_pull`s the engine's pull of it.

Because registering is the maker's action and filling the taker's, the two are separate commands; a single `execute` can chain them only when the same signer is both (self-fill).

## Balance semantics: sweeps and `AssertMinBalance`

Both `Amount::Balance` and `AssertMinBalance` operate on the router's **absolute** token balance, not on a per-command delta. This is efficient and matches the Solidity `CONTRACT_BALANCE` sentinel, but integrators must understand two consequences:

**`Amount::Balance` is a full sweep.** A command with `amount: Amount::Balance` resolves to the router's *entire* current balance of that token (`token.balance(router)` in `resolve_amount`), then moves it. It does not track "what this batch produced" — it sweeps whatever the router holds at that moment, including any pre-existing residual or funds a prior command left behind. Order commands so a sweep runs only when the router's balance is exactly what you intend to move. Note also that transferring `0` does not revert, so a sweep of an empty balance silently succeeds.

**`AssertMinBalance` checks an absolute balance, not receipt.** The command asserts `token.balance(owner) >= min_value`. It does **not** measure how much a preceding command delivered. If the router (or the owner) was pre-funded — holds a residual balance of `token` before the batch — that residual counts toward the threshold and can mask an under-delivery: a command that should have delivered `N` but delivered less still passes `AssertMinBalance` as long as the pre-existing balance covers the gap. `AssertMinBalance` is therefore a floor check on final holdings, not a delivery receipt.

**Guidance for verifying receipt.** When you need to verify that a specific command actually delivered its output (e.g. an external swap), do not rely on `AssertMinBalance` against a possibly pre-funded balance. Either (a) start the batch from a known-zero router balance for that token so the absolute check equals the delta, or (b) use a command that verifies the balance delta internally. `BrokerSwap` does the latter: it snapshots `token_out` before the swap and asserts `balance_after - balance_before >= min_amount_out` inside `dispatch_broker_swap`, so its output verification does not depend on the caller appending an `AssertMinBalance`.

## Pre-maturity RedeemPtForIbt residual

Pre-maturity, `PT.redeem_for_ibt` burns equal amounts of PT and YT. If a `RedeemPtForIbt` command requests more shares than the Router's YT balance, the dispatcher clamps the redeem to that YT balance (mirroring the Solidity `PrincipalTokenModule.redeemPTForIBT` `min(shares, YT.balanceOf(this))` clamp) so the batch does not revert.

The clamp means the unredeemed PT (`requested - redeemed`) stays in the Router. The Router keeps no per-user accounting and its `Transfer(Amount::Balance)` command is public, so any residual left in the Router is in shared custody and can be swept by a later caller.

**Callers must reclaim the residual themselves** — append a `Transfer` command (`Amount::Balance` of the PT to `Recipient::Caller`) at the end of the batch, or size `shares` to the known YT balance so no residual is produced. This matches the Solidity Router, which also leaves the residual in `address(this)` and relies on an explicit sweep.

## Bridge commands

Two variants, `BridgeNativePtToEvm` and `BridgeWrappedPtBackToEvm`, sit on top of the external `PtBridge` contract from
[`spectra-bridge-stellar`](https://github.com/perspectivefi/spectra-bridge-stellar). They have **no Solidity counterpart**:
the EVM router does not bridge — the EVM-side Bridge contract is called directly.

**Bridge address** lives in the router's instance storage under `DataKey::Bridge`. Admin-only setter `set_bridge`
mirrors `set_registry`. `BridgeNativePtToEvm` and `BridgeWrappedPtBackToEvm` panic with `BridgeNotSet` if the slot is
unset.

**Auth**:
- The router calls `PtBridge.{bridge_native_pt_to_evm, bridge_back_to_evm}` with `sender = router`. Auto-self-auth covers
  `sender.require_auth()` inside the bridge.
- The bridge will then pull tokens via SEP-41 `transfer(sender, …)`. Since the router is *two hops* from the token, the
  dispatcher pre-declares these invocations via `env.authorize_as_current_contract` (`transfer_entry` helper):
  - For Type 3: `pt.transfer(router, bridge, lock_amount)` and `gas_token.transfer(router, messenger, gas_amount)`.
  - For Type 2: `wrapped_pt.transfer(router, bridge, fee_amount)` (only when fee > 0) and `gas_token.transfer(router, messenger, gas_amount)`.
- The Type 2 fee amount is computed in the dispatcher by reading `bridge.get_bridge_back_fee_bps()` and applying it to
  the resolved amount. Same transaction → no race.

## Broker swap output verification

`BrokerSwap` hands a swap to the external `StellarBroker` aggregator (Stellar-only, no Solidity counterpart). The broker
is trusted to route, but **not** trusted to self-report its output: `min_amount_out` is verified against the Router's
own `token_out` balance delta (snapshotted before the swap, re-read after), not the `bought` value the broker returns.
`BrokerSwapArgs` therefore carries an explicit `token_out`. This makes slippage protection obligatory inside the command
rather than relying on the caller to append a follow-up `AssertMinBalance`: a broker that takes the input,
reports a satisfying `bought`, and delivers less (or zero) is rejected with `AssertMinBalanceFailed`.

**Minimum gas floor**: both bridge arms reject a resolved `gas_amount` below the floor configured for the command's `(target_chain_id, gas_token)` with `InsufficientGas`. The bridge itself accepts any `gas_amount` (including 0 — the user forgoes the relay guarantee), so the floor is a router-level policy the admin (`RTR_MGR`) tunes per chain/token via `set_min_gas_floor(target_chain_id, gas_token, floor)`. It is keyed rather than global because a sane floor depends on the destination chain's relay cost and the gas token's decimals/value. Each key defaults to `0`, preserving the bridge's permissive behavior until an admin sets one. The check runs after `resolve_amount`, so it also rejects negative resolved amounts — no separate negative guard is needed here.

## TTL

`Bridge` and the per-`(target_chain_id, gas_token)` `MinGasFloor` entries are stored under instance TTL alongside `Registry`. `extend_instance_ttl` already fires whenever any
mutator or `execute` runs. The floor keyspace is bounded by the admin-configured set of supported chains/gas tokens, so instance storage is appropriate.

## Bridge interface provenance

`spectra-common::bridge_traits` is a verbatim copy of `bridge-traits/src/lib.rs` from
[`spectra-bridge-stellar`](https://github.com/perspectivefi/spectra-bridge-stellar) at commit `a8a8c8e`. Re-vendor when the upstream
interface changes.

## Role management (post-init)

`initialize` grants the deploying admin the **PAUSER**, **UPGRADE**, and **ROUTER_MANAGER** (`RTR_MGR`) roles via `access_control::grant_role_no_auth`. To rotate role holders after deployment (deploy dispatcher, key rotation), Router exposes three thin wrappers over `stellar_access::access_control`:

- `grant_role(caller, account, role)` — grant `role` to `account`. `caller` must be the contract admin or hold the admin-role of `role`.
- `revoke_role(caller, account, role)` — revoke; same auth model.
- `has_role(account, role) -> bool` — view.

Auth + admin-role checks are enforced by the library; the wrappers exist only to expose the surface to off-chain callers. Mutators extend Instance TTL on success.
