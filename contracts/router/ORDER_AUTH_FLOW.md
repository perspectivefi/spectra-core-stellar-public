# Order registration & fill — authorization flow

How makers sign orders off-chain and how a relayer/taker submits them. This is
the production signing model; the stellar CLI's auto-signer only covers the
simplest (root) case and is not used here.

## Background: two signature layers

A Soroban transaction carries two independent things:

1. **Source signature** — whoever submits and pays fees.
2. **`SorobanAuthorizationEntry` list** — one per address whose `require_auth`
   fires *anywhere* in the call tree. Each entry is
   `{ address, nonce, signature_expiration_ledger, signature }`, where the
   signature covers the exact invocation `(contract, fn, args, sub_invocations)`
   + nonce + network passphrase.

Because each entry is signed for its exact invocation, it is valid regardless of
where that invocation sits in the tree (root or nested). The submitter need not
be the signer.

## Registration payloads

A maker signs an auth entry for a specific `(contract, fn, args)`:

| Path | Payload the maker signs |
|---|---|
| Single order (`register_order`) | `(engine, "register_order", [order])` |
| Batch — **direct engine or via router** | `(engine, "register_orders_batch", [order])` |

The batch payload is **path-agnostic**: the same signed entry works whether the
batch is submitted directly to the engine or through the router's
`RegisterOrders` command.

**Cancellation** is self-service — a maker cancels only their own orders, so a
batch cancel shares one maker, signed **once** over the whole batch:

| Path | Payload the maker signs |
|---|---|
| `cancel_order(order)` | `(engine, "cancel_order", [order])` |
| `cancel_orders_batch(orders)` (one maker, whole batch) | `(engine, "cancel_orders_batch", [orders])` |

`cancel_orders_batch` is best-effort (skips expired / already-terminal members)
and rejects any member whose maker differs from the batch maker. This payload is
also **path-agnostic**: the same signed entry works whether the batch is
submitted directly to the engine or through the router's `CancelOrders` command.

## Maker flow (off-chain, gasless)

1. Backend builds the maker's intended invocation: `engine.register_orders_batch`
   with args `[order]`.
2. Maker's wallet signs that **auth entry** — `signAuthEntry` (SEP-43 wallet
   API: Freighter, Lobstr, …) or, in code, `authorizeEntry(entry, signer,
   validUntilLedger, networkPassphrase)` from `@stellar/stellar-sdk`. No
   transaction, no fee.
3. Backend stores the signed entry (with its nonce + expiration ledger).

Smart-contract wallets (passkey, multisig) work identically — their
`__check_auth` validates the entry.

## Relayer / taker flow (submission)

1. Assemble the operation:
   - **Register only:** `engine.register_orders_batch(orders)`, or
   - **Register + fill atomically:** `router.execute([RegisterOrders{orders},
     FillOrders{...}])`.
2. Attach all makers' pre-signed auth entries. (Fill-side maker token pulls are
   allowance-based — makers pre-approve the engine once via SEP-41 — so filling
   needs no maker signature, only the taker's.)
3. Sign the **transaction source** with the taker/relayer key and submit.

The host matches each maker's entry to its `register_orders_batch(order)` node
and each fill auth to the taker — no maker needs to co-sign the transaction.

## Building it with `@stellar/stellar-sdk`

```
// 1. Simulate to discover required auth entries (recording mode).
const sim = await rpc.simulateTransaction(tx);

// 2. Sign each non-source entry with its owner (maker wallet / keypair).
const signed = await authorizeEntry(
  entry, makerSigner, validUntilLedger, networkPassphrase,
);

// 3. Rebuild the op with the signed entries, sign the source, submit.
```

## Notes

- **Nonce + expiry**: each signed entry is single-use and expires at
  `signature_expiration_ledger` — set an expiry aligned with the order's
  `expiry`.
- **No forging**: the signature covers the full order args; changing anything
  invalidates it.
- **CLI limitation**: `stellar contract invoke` auto-signs only root-level auth,
  so it can drive the *direct* engine batch but not the router path. Use the
  SDK (above) for the router/relayer flow. `scripts/core presign-register`
  (`--build-only`) is a starting point for extracting the entry to sign.
