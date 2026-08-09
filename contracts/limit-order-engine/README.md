# Limit Order Engine

Soroban port of Spectra's Solidity `LimitOrderEngine`.

## Solidity-to-Soroban differences

| Aspect | Solidity | Soroban | Reason |
|---|---|---|---|
| Off-chain signatures | EIP-712 + `NonceManager` | Soroban auth entries (`order.maker.require_auth()`) | Native auth model; no EIP-712 |
| Router gating | `onlyRouter` on `fillOrders` | Permissionless `fill_orders`; `caller` authenticates taker | Simpler; no Router dependency |
| Fee config | In Registry | On engine (`fee_rate`, `fee_collector`); settable by `FEE_SETTER` | No Registry dependency |
| Order registration | `preSignSingle` (on-chain) + inline EIP-712 fill | Single `register_order` replaces both | Soroban auth binds to specific call + args |
| Order existence sentinel | `remaining == 0` = unregistered; `> 0` = active | `remaining == 0` = never; `== 1` = filled/cancelled; `> 1` = actual = `remaining - 1` | Backend decoder compatibility (`-1` works cross-chain) |
| Order hash | `keccak256(abi.encode + EIP-712)` | `sha256(xdr(order))` | SHA-256 native on Soroban; XDR serialization |
| `Order` fields | `salt, expiry, nonce, orderType, pt, maker, receiver, makingAmount, impliedRate` (all `uint256`/`address`) | `salt(u64), expiry(u64), order_type, pt, maker, receiver, making_amount(i128), implied_rate(i128)` — no `nonce` | Narrower types match `spectra-math` i128 guarantees |
| Tokenization | `_handleTokenisation` | Ported — caller passes `taker_input_token` + `taker_input_amount` | Engine wraps IBT→PT+YT or redeems PT+YT→IBT |
| Funding-leg pull (`_convertToIbt`) | `safeIncreaseAllowance` + `deposit`/`wrap` (EVM ERC4626/wrapper pull via `transferFrom`, allowance-backed) | `authorize_pull` (`authorize_as_current_contract`) + `deposit`/`wrap` | Soroban wrapper pulls via `transfer(engine→wrapper)`, which reads no allowance; the on-behalf sub-auth must be declared (auto-auth covers only direct calls). Mirrors `PrincipalToken.deposit` |

## Auth & off-chain orders

**Why off-chain signing matters:** Makers post quotes for free — only filled orders hit chain, taker/solver pays. Without this, every quote is an on-chain tx, making limit-order book unviable.

Soroban has no inline EIP-712 verification. Maker signs a **Soroban auth entry** for `register_order(order)` offline — this plays the role of the EIP-712 signature.

**Maker flow:**
1. Construct `Order`, simulate a `register_order(order)` call to extract the `SorobanAuthorizationEntry`, sign it off-chain via SDK/SEP-43, hand to backend. **No broadcast, no tx fee.** (`stellar tx sign` does not produce a detachable auth entry — simulation + extraction is required.)
2. Backend stores both.
3. Taker matches → backend assembles **one transaction** with maker's pre-signed auth entry + `fill_orders` call. `register_order` runs first (consumes maker auth), then `fill_orders` settles. Atomic, paid by taker/backend.
4. Maker pre-approves engine (SEP-41 allowance) on `making_token` for `transfer_from` at fill time.

**Why one `register_order` not two paths?** EIP-712 binds to payload only — any taker combines signature with arbitrary fill args. Soroban auth entries bind to *specific* function call + args — maker can only pre-sign `register_order(order)`, not `fill_orders(...)` (fill args unknown at sign time). Hence: `register_order` is the single funnel.

**Batch fill and maker homogeneity.** `fill_orders` enforces same `pt` and `order_type` across the batch (`IncompatibleBatch`), but does **not** enforce same `maker` — expired, filled, and cancelled orders are skipped before any maker check. The maker-homogeneity guarantee therefore applies only to live, non-terminal orders in the batch.

**Taker:** `fill_orders(caller, ...)` requires `caller.require_auth()`; engine pulls `taker_input_token` from `caller` via SEP-41 `transfer`.

**Batch registration & the auth-node trap.** A batch runs in a single invocation frame. A plain per-order `require_auth()` inside the loop binds every maker to the *same* aggregate `register_orders_batch(orders)` node, so independently pre-signed orders can't be batched (and a self cross-contract call per order is impossible — Soroban forbids contract re-entry). `register_orders_batch` therefore authorizes each order with `order.maker.require_auth_for_args((order,))` — a per-order payload scoped to `(engine, "register_orders_batch", [order])` that each maker signs independently (and which also lets the *same* maker place several orders in one batch, unlike a repeated no-arg `require_auth`).

### Registration payload by submission path

A Soroban auth entry binds to a **specific `(contract, function, args)`**. There are two registration payloads — one for single orders, one for batches — and the **batch payload is the same whether submitted directly to the engine or through the router**:

| Submission path | Function actually invoked | Payload the maker signs |
|---|---|---|
| `register_order` (single) | `engine.register_order(order)` | `(engine, "register_order", [order])` |
| `register_orders_batch` (direct engine batch) | `engine.register_orders_batch(orders)`, each order via `require_auth_for_args((order,))` | `(engine, "register_orders_batch", [order])` |
| Router `RegisterOrders` command | forwards to `engine.register_orders_batch(orders)` (same as above) | `(engine, "register_orders_batch", [order])` |

Because `require_auth_for_args` decouples the authorized payload from the parent call, the router and the direct engine batch consume the **identical** per-order signature — so a relayer can submit makers' pre-signed batch orders either way. Only the single-order path (`register_order`) uses a different payload. The order still can't be forged on any path (the `args` — the full order — must match exactly).

**Cancellation is self-service (asymmetric with registration).** `cancel_orders_batch` is for the caller's *own* orders: every member must share one maker, that maker is authorized **once** (no `require_auth_for_args`), and each order is cancelled via a shared inner that verifies `order.maker == maker` (`CancelBatchMakerMismatch` otherwise). Signed payload: `(engine, "cancel_orders_batch", [orders])` — the whole batch, one signature. It is **best-effort**: expired and already-terminal (filled/cancelled) members are skipped so one dead order does not revert the rest. (Registration is multi-maker because a relayer populates the book with many makers' orders; cancellation is not, because you only cancel your own.)

## Fail-safe rate convention (`Order.fail_safe_rate`)

`fail_safe_rate` is the maker's slippage floor on the wrapper conversion the fill performs. **It is "raw output per `1e18` raw input"**: the minimum raw output-token amount accepted for every `1e18` raw units of input. `0` disables the floor.

The engine applies it as:

```text
floor = amount * fail_safe_rate / 1e18
```

against the realized output of `deposit` / `redeem` (Underlying path), or passes it natively as `min_shares` / `min_vault_shares` to `wrap_with_min_shares` / `unwrap_with_min_vault_shares` (VaultShare path). This mirrors Solidity `failSafeRate` and — like Solidity — performs **no on-chain decimal rescaling**.

### The convention: set it to `previewConversion(1e18)`

Because the value is raw-output-per-raw-input, it **already carries the output token's decimals** and therefore implicitly encodes the underlying↔IBT decimal gap. Set it to the wrapper's `previewConversion(1e18)` for the leg the order runs:

| Order leg | Conversion | Preview to use |
|---|---|---|
| Funding (`IbtForPt` / `IbtForYt`, Underlying) | underlying → IBT | `previewDeposit(1e18)` |
| Payout (`PtForIbt` / `YtForIbt`, Underlying) | IBT → underlying | `previewRedeem(1e18)` |
| VaultShare paths | vault-share ↔ IBT | corresponding wrap/unwrap preview of `1e18` |

Apply your slippage tolerance by scaling that preview down (e.g. `preview * 99 / 100` for 1%).

### It is NOT a decimal-agnostic `1e18` WAD ratio

Do **not** treat `fail_safe_rate` as "`1.0` = `1e18`". For a market with a decimal gap the correct value is large and market-specific. Example — a value-preserving vault with a **6-dec underlying** and an **18-dec IBT**: `1e18` raw underlying = `1e12` whole tokens, which value-preservingly convert to `1e12` whole IBT = **`~1e30` raw IBT**. So the correct `fail_safe_rate` there is `~1e30`, not `~1e18`. Passing a WAD-style `~0.99e18` would set a floor `10^12`× too small on the funding leg (silently ineffective) or too large on the payout leg (valid fills revert) — avoided by following the `previewConversion(1e18)` convention.

The cross-decimal test `fail_safe_rate_preview_convention_holds_cross_decimals` pins this: it sets the floor from `preview_deposit(1e18)` / `preview_redeem(1e18)` and asserts a healthy conversion clears it while a degraded one trips `FailSafeRateNotMet`, across the underlying/IBT decimal sweep (`underlying ≤ IBT`).

## Cross-chain identity

`hash_order` does **not** match Solidity `hashOrder` for same logical order:

| Concern | Solidity | Soroban |
|---|---|---|
| Hash function | `keccak256` | `sha256` |
| Serialization | ABI + EIP-712 typed-data | XDR via `ToXdr` |
| `Order` fields | Includes `nonce`; all 256-bit | No `nonce`; `u64`/`i128` |

**Implications:**
- Backends/indexers: `order_id` ≠ EVM `orderHash`. Treat Soroban hash as local identifier.
- Cross-chain order books: use `(chain_id, order_hash)` tuple as canonical ID.
- Salt space 2^64 (vs 2^256) — practically safe (1k orders/sec → birthday bound ~160 years), but salt not stable cross-chain ID.

**Why the divergence:**
- Soroban host exposes SHA-256 natively, not keccak256 (Rust keccak would inflate WASM)
- EIP-712 irrelevant — auth model uses native auth entries
- Narrower types match `spectra-math` i128 guarantees and codebase number model

## TTL strategy

| Data | Storage Tier | TTL | When Extended |
|---|---|---|---|
| Fee config, pause, roles | Instance | `max_ttl()` | Every state-changing entry point |
| `OrderStatus` | Persistent | Scoped to `order.expiry`: `(expiry - now) / secs_per_ledger`, capped at `max_ttl()` | Every write (`register_order`, `cancel_order`, `fill_order`) |
| `OrderStatus` reads | Persistent | — | **Never** (reads past expiry must not keep entry alive) |

## Resource limits & Blend-backed PTs

⚠️ **`fill_orders` against a Blend-backed PT currently exceeds the per-invocation
memory limit (40 MiB) and reverts on-chain.** Validated on Stellar **testnet
(protocol 26, module cache active)** against the live Blend `TestnetV2` pool:
both an IBT-tokenising fill (fails at `deposit_ibt`) and a canonical-token fill
(fails earlier, in pricing at `pt.preview_redeem`) revert with
`Error(Budget, ExceededLimit)` / `Memory(OutOfBoundsGrowth)`. Mainnet is the same
protocol/limit, so the same fill reverts there.

### Why

`mem_bytes` is dominated by **Wasm parse + instantiation** (`VmInstantiation` +
`ParseWasm*`), a large multiple of the ≤128 KiB on-disk bytecode, summed over
**every contract instantiated in the single fill invocation**: engine → PT → YT
→ Blend wrapper → **Blend pool** → SAC. The Blend pool alone dominates. The
**module cache does not help** — it amortises *re-parsing across* invocations,
but the per-invocation memory bound still counts the instantiation; the live node
confirms ~46 MiB > 40 MiB.

Crucially, the pool is instantiated during **pricing** —
`limit-order-engine/src/pricing.rs` reads `pt.preview_redeem` + `pt.get_ibt_rate`
on every fill — so **the taker token / denomination does not matter**. There is
no order-routing workaround: a standalone `pt.deposit_ibt` fits on its own, but
the full fill (engine + pricing + pool + settlement in one invocation) does not.

### What actually has to change (out of scope for this PR)

- **Engine/pricing:** let pricing take the IBT↔asset rate as a caller-supplied
  input instead of reading it cross-contract, so the fill never instantiates the
  PT/wrapper/pool. This is the only engine-level fix that removes the pool from
  the invocation.
- **Wrapper/PT layer:** shrink the Blend wrapper/pool instantiation footprint.
- **Network:** a higher per-invocation memory limit (not in our control).

Non-Blend (lightweight ERC-4626) IBTs are unaffected and fill normally.

`secs_per_ledger` from instance storage (default 4s — below ~5s mainnet target — overshoots wall-clock for headroom). Configurable via admin-gated entrypoint. All writes revert `InvalidOrder` on `expiry <= now`; past expiry entry archives naturally. Diverges from workspace `extend_persistent_ttl` (~120 days) — shared helper suits long-lived per-user data; wasteful for orders bounded by `expiry`. See issue #62.