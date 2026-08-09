# LimitOrderEngine — per-fill memory limits

Per-`fill_orders` memory against the Stellar mainnet per-transaction memory cap:
what each path costs, why, and how much margin it keeps. Every number below is
measured on a **mainnet fork** with our contracts deployed as **WASM**
(mainnet-accurate metering — native execution under-counts memory by ~1.5–2×).

**All numbers here are for contracts built with the 64 KiB Wasm stack**
(`-C link-arg=-zstack-size=65536`, shipped in `scripts/build-wasm.sh` and CI).
Soroban meters the Wasm linker stack reservation as linear memory on every
contract invocation, so the default ~1 MiB reservation dominates per-tx
`mem_bytes` once a fill enters dozens of contract frames. Shrinking it to 64 KiB
cuts every path ~2.6× and is the single highest-leverage lever — see §2 and
https://github.com/stellar/rs-soroban-env/pull/1691. **At 64 KiB every measured
cell fits; at the 1 MiB default most minting/redeeming fills did not.**

The cap: **`tx_memory_limit` = 41,943,040 bytes = 40 MiB** per invocation.
Instructions (`tx_max_instructions` = 400M) are never the binding limit here —
the heaviest fill uses ~41M. **Memory is the sole binding dimension.**

> Convention: "MB" below means bytes ÷ 1,000,000 (matches the raw `mem_bytes`
> the tests print). The cap is 41.94 MB in those units.

---

## 1. Results

### What each order does

Read each row as "maker has … and wants …, taker has … and wants …, so the
engine converts … → …". The order has **three** independent inputs:
`order_type`, `maker_token_type` (how the maker funds/receives its IBT leg —
Underlying or Ibt), and the **taker token** (what the taker pays). The taker
token is the biggest cost lever — it decides whether the engine **mints**,
**redeems**, or just **routes** tokens.

| Order | Plain English (taker pays IBT, the default) |
|---|---|
| **IbtForPt** | Maker has Underlying (XLM) and wants **PT**; taker has IBT and ends up with YT. The engine converts the maker's **Underlying → IBT**, then **mints IBT → PT + YT**, giving the PT to the maker and the leftover YT to the taker. |
| **IbtForYt** | Maker has IBT and wants **YT**; taker has IBT and ends up with PT. The engine **mints IBT → PT + YT**, giving the YT to the maker and the leftover PT to the taker. |
| **PtForIbt** | Maker has **PT** and wants IBT (paid out as Underlying); taker has IBT and wants the PT. The engine hands the maker's **PT → taker** and **redeems IBT → Underlying** to pay the maker. |
| **YtForIbt** | Maker has **YT** and wants IBT (paid out in IBT); taker has IBT and wants the YT. The engine hands the maker's **YT → taker** and transfers **IBT → maker**. |

If the taker instead pays the **complementary** share, the engine does the
opposite tokenisation: funding-side orders skip the mint (taker hands their PT/YT
straight to the maker — cheap), while payout-side orders must **redeem** the
taker's share into IBT (the single most expensive op).

### The full cube — every IBT backend

The complete `order_type × maker_denom × taker_token` matrix (16 cells each), for
all three IBT backends. Numbers are MB (÷1e6), cap = 41.94. The **taker = IBT**
column is the realistic default (engine mints/redeems on the fly); the **taker =
PT/YT** column is the taker paying the other share (funding-side: a cheap direct
hand-off; payout-side: an expensive redeem). Measured by `fork_blend_memory_matrix`,
`fork_upshift_vault_memory_matrix`, `fork_upshift_wrapper_memory_matrix` against
the 64 KiB-stack build. **Every cell fits** — the columns below show the residual
cost structure and the margin each keeps.

**Blend v2 "Fixed" pool, shipped `b_rate`-cached wrapper (production):**

| Order / maker denom | taker = IBT | taker = PT/YT |
|---|:--:|:--:|
| IbtForPt / Underlying | 22.6 ✅ mint | 13.8 ✅ PT direct |
| IbtForPt / Ibt | 18.2 ✅ mint | 9.4 ✅ PT direct |
| IbtForYt / Underlying | 26.3 ✅ mint | 24.6 ✅ YT direct |
| IbtForYt / Ibt | 22.0 ✅ mint | 20.1 ✅ YT direct |
| PtForIbt / Underlying | 12.4 ✅ | 25.3 ✅ redeem |
| PtForIbt / Ibt | 9.6 ✅ | 22.6 ✅ redeem |
| YtForIbt / Underlying | 19.8 ✅ | 22.0 ✅ redeem |
| YtForIbt / Ibt | 17.0 ✅ | 19.3 ✅ redeem |

**Upshift "August" vault — direct (PT tokenizes the vault itself):**

| Order / maker denom | taker = IBT | taker = PT/YT |
|---|:--:|:--:|
| IbtForPt / Underlying | 24.7 ✅ mint | 14.1 ✅ PT direct |
| IbtForPt / Ibt | 23.3 ✅ mint | 12.7 ✅ PT direct |
| IbtForYt / Underlying | 27.9 ✅ mint | 23.2 ✅ YT direct |
| IbtForYt / Ibt | 26.6 ✅ mint | 21.8 ✅ YT direct |
| PtForIbt / Underlying | 15.0 ✅ | 26.2 ✅ redeem |
| PtForIbt / Ibt | 15.0 ✅ | 26.2 ✅ redeem |
| YtForIbt / Underlying | 21.3 ✅ | 23.5 ✅ redeem |
| YtForIbt / Ibt | 21.3 ✅ | 23.5 ✅ redeem |

**Upshift vault behind a delegating wrapper (the faithful Blend-wrapper analog):**

| Order / maker denom | taker = IBT | taker = PT/YT |
|---|:--:|:--:|
| IbtForPt / Underlying | 17.5 ✅ mint | 10.6 ✅ PT direct |
| IbtForPt / Ibt | 15.9 ✅ mint | 9.0 ✅ PT direct |
| IbtForYt / Underlying | 20.8 ✅ mint | 19.9 ✅ YT direct |
| IbtForYt / Ibt | 19.3 ✅ mint | 18.3 ✅ YT direct |
| PtForIbt / Underlying | 10.1 ✅ | 21.6 ✅ redeem |
| PtForIbt / Ibt | 8.8 ✅ | 20.2 ✅ redeem |
| YtForIbt / Underlying | 16.6 ✅ | 18.8 ✅ redeem |
| YtForIbt / Ibt | 15.2 ✅ | 17.5 ✅ redeem |

What the cube shows:
- **Everything fits with ample margin.** The global worst case is **27.9 MB**
  (direct Upshift, `IbtForYt / Underlying / taker-IBT`) — ~14 MB under the
  41.94 MB cap. Backend maxima: Blend 26.3, direct Upshift 27.9, wrapper 21.6.
- **Taker token is still the top lever.** The IBT-vs-share choice moves the
  result by ~9–13 MB: mint ≈ +9–11 MB, **redeem ≈ +11–13 MB (the worst op)** —
  but at 64 KiB neither crosses the cap.
- **YT in bulk stays heaviest** even with no mint/redeem — `IbtForYt` "direct"
  and `YtForIbt` direct stay high because a YT-denominated leg moves a large YT
  quantity, and every YT move forces a vault valuation.
- **Maker denom is a minor lever, bigger on Blend** (~4.4 MB Underlying vs Ibt,
  vs ~1.4 MB on Upshift) — because the maker's Underlying funding does a Blend
  **pool submit**, heavier than an Upshift vault deposit.
- **The ranking is the same on every backend; the backend only sets the scale.**
  The relative order of paths is unchanged from the 1 MiB build — the 64 KiB
  stack shifted every cell down by a near-constant ~2.6× and pulled the whole
  matrix under the cap.

---

## 1b. On-chain confirmation (Stellar testnet)

An earlier full stack was deployed on **Stellar testnet** over a real Blend
TestnetV2 wrapper to confirm the network actually enforces the 40 MiB cap by
reverting with `Budget,ExceededLimit`. That deployment used the **default 1 MiB
Wasm stack** (pre-64 KiB build), so the over-cap cells below reflect the
default-stack build, not the 64 KiB production build documented above — under
64 KiB these same cells model well under the cap and are expected to fill.

Deployment (testnet, 1 MiB stack): wrapper `CBSPQPT5…` (XLM / Blend TestnetV2
pool), PT `CDMYKQ4V…`, YT `CDJQJNEG…`, registry `CBQAFUQ3…`, engine `CCJCOVPQ…`.

| Cell | 1 MiB-stack fork model | on-chain result (1 MiB build) |
|---|---|---|
| `PtForIbt / Ibt / taker-IBT` | 38.9 ✅ | **filled** (maker received IBT) |
| `PtForIbt / Underlying / taker-YT` | 108.8 ❌ | `Budget,ExceededLimit` |
| `PtForIbt / Ibt / taker-YT` | 106.3 ❌ | `Budget,ExceededLimit` |
| `YtForIbt / Underlying / taker-IBT` | 80.7 ❌ | `Budget,ExceededLimit` |
| `YtForIbt / Ibt / taker-IBT` | 78.1 ❌ | `Budget,ExceededLimit` |
| `IbtForYt / Ibt / taker-YT` | 96.8 ❌ | `Budget,ExceededLimit` |

The fitting cell fills; every tested over-cap cell reverts on `Budget` —
confirming the fork model and that the cap is enforced on a live network. (A
re-deploy of the 64 KiB build to confirm the now-fitting cells fill on-chain is
future work; the modelled 64 KiB numbers in §1 all sit under the cap.)

---

## 2. The model (what actually costs memory)

Two components stack:

1. **The Wasm stack reservation (dominant at the default).** Each cross-contract
   call re-instantiates the callee module's linear memory, and Soroban meters the
   linker stack reservation as part of it. At the ~1 MiB rustc default a fill that
   enters dozens of contract frames pays ~1 MiB per frame — this, not our Rust
   allocations or DeFi work, was the bulk of `mem_bytes`. Building with
   `-zstack-size=65536` (64 KiB) removes almost all of it, cutting every path
   ~2.6× and pulling the whole matrix under the cap. This is a build/deploy flag,
   not a code change.

2. **IBT NAV valuations (what's left at 64 KiB).** With the stack term gone, the
   residual cost is dominated by **full IBT NAV valuations** — the PT helper
   `_get_current_pt_and_ibt_rates` (`principal-token/src/lib.rs:1041`), which
   reads `ibt.preview_redeem` + `ibt.total_assets` + `ibt.total_supply`.

Fitting the 64 KiB direct-Upshift rows gives a clean rule of thumb:

```
per-fill memory  ≈  12 MB base  +  ~3.2 MB × (number of IBT NAV valuations)
```

(At the 1 MiB default the same fit was ~23 MB base + ~9 MB/valuation — both terms
shrank once the stack reservation was removed.)

A NAV valuation happens in exactly three places:

| Where | Valuations | Triggered by |
|---|:--:|---|
| **Pricing** (`pricing::get_price` → `pt.preview_redeem`) | **1** | every order, every type (the floor) |
| **Minting** (`pt.deposit_ibt`) | **2** | funding-side orders *when the taker pays IBT* — it re-reads rates once in `update_yield_inner` and once in `_convert_ibts_to_shares` |
| **Redeeming** (`pt.redeem_for_ibt`) | **3+** | payout-side orders *when the taker pays the complementary share* — burns PT **and YT** (the YT burn calls `PT.update_yield`) and converts shares→IBT; the heaviest op in the cube (~+11–13 MB) |
| **Each YT movement** (`YieldToken.transfer`/`transfer_from`/burn) | **1** | YT calls back into `PT.update_yield` (`yield-token/src/lib.rs:207,221`), which re-reads rates |

The silent multiplier is the last one: **a PT transfer is free; a YT transfer
forces a vault NAV valuation.** This is why the **taker token** is the top lever
(it picks mint vs redeem vs route), the order type is second, and the maker
denomination is a distant third.

Things that are **NOT** significant levers (measured): order size (amount), the
maker denomination (`Underlying` vs `Ibt` ≈ 1.4 MB on Upshift, ~4.4 MB on Blend),
and a single vault `deposit`/`redeem` on the direct Upshift vault.

---

## 3. The four paths + execution trees

Legend: `⊕` = a full IBT NAV valuation (~3.2 MB each on direct Upshift at 64 KiB);
`·` = cheap (storage read, XLM SAC transfer, PT transfer, plain SEP-41).

In every fork test **the taker pays IBT**, which is what forces the engine to
mint or redeem on the fly. `get_taker_token` (`lib.rs:802`) defines the
*canonical* token instead; see §5.

### IbtForPt — maker buys PT with IBT   (direct 24.7 ✅ / wrapper 17.5 ✅ / Blend 22.5 ✅)
```
fill_orders
├─ transfer taker IBT → engine                              ·
├─ pull_and_price
│   ├─ transfer_from(maker funding) → engine                ·   (XLM SAC, or IBT)
│   ├─ convert_to_ibt → vault.deposit   (only if maker=Underlying)  (+1.3 MB)
│   └─ get_price → pt.preview_redeem                         ⊕   (1 valuation)
├─ handle_tokenisation → pt.deposit_ibt   (taker paid IBT → MINT PT+YT)
│   ├─ update_yield_inner → rate refresh                    ⊕
│   ├─ _convert_ibts_to_shares → rate refresh               ⊕
│   ├─ mint PT, YT.mint                                      ·
├─ settle_makers: transfer PT → maker                        ·
└─ settle_taker: sweep surplus YT → receiver                ⊕   (YT move)
                                                    total ≈ 4 valuations → ~24.7 MB
```

### IbtForYt — maker buys YT with IBT   (direct 26.1 ✅ / wrapper 18.8 ✅ / Blend 21.5 ✅)
```
fill_orders
├─ transfer taker IBT → engine                              ·
├─ pull_and_price
│   ├─ transfer_from(IBT) maker → engine                    ·   (maker funds Ibt → no deposit)
│   └─ get_price → pt.preview_redeem                        ⊕
├─ handle_tokenisation → pt.deposit_ibt   (MINT PT+YT)
│   ├─ update_yield_inner                                   ⊕
│   ├─ _convert_ibts_to_shares                              ⊕
│   └─ YT.mint                                               ·
├─ settle_makers: transfer YT → maker                       ⊕   (YT move)
└─ settle_taker: sweep surplus PT → receiver                ·
                                          total ≈ 4 valuations, but measured the
                                          heaviest per valuation-count — YT is
                                          cheap per move, so this order trades a
                                          20× larger IBT/YT quantity, inflating
                                          the mint/YT work.
```

### PtForIbt — maker sells PT for IBT   (direct 14.6 ✅ / wrapper 9.7 ✅ / Blend 12.0 ✅)
```
fill_orders
├─ transfer taker IBT → engine                              ·
├─ pull_and_price
│   ├─ transfer_from(PT) maker → engine                     ·   (PT transfer free)
│   └─ get_price → pt.preview_redeem                        ⊕
├─ handle_tokenisation = NO-OP   (taker IBT == what maker wants → no mint)
├─ settle_makers: payout_from_ibt → vault.redeem (Underlying) ·  (cheap on direct)
└─ settle_taker: sweep PT → receiver                        ·
                                                    total ≈ 1 valuation → ~14.6 MB
```

### YtForIbt — maker sells YT for IBT   (direct 20.5 ✅ / wrapper 14.4 ✅ / Blend 16.3 ✅)
```
fill_orders
├─ transfer taker IBT → engine                              ·
├─ pull_and_price
│   ├─ transfer_from(YT) maker → engine → update_yield      ⊕   (YT move!)
│   └─ get_price → pt.preview_redeem                        ⊕
├─ handle_tokenisation = NO-OP   (no mint)
├─ settle_makers: payout IBT → transfer                     ·   (maker funds Ibt → free)
└─ settle_taker: sweep YT → receiver → update_yield         ⊕   (YT move!)
                                                    total ≈ 3 valuations → ~20.5 MB
```

`YtForIbt` does **no mint and no redeem**, yet costs ~6 MB more than
`PtForIbt` — purely because the YT pull-in and YT sweep-out each force a NAV
valuation, while PtForIbt's PT movements are free. **This is the cleanest proof
that YT movements, not work done, drive the cost.**

---

## 4. Decomposition experiment (empirical)

Three fills of the *same* order (IbtForPt), peeling off one lever at a time
(direct Upshift, 64 KiB stack):

| Variant | mem_bytes | MB | Δ | what it isolates |
|---|---:|---:|---:|---|
| maker Underlying, taker pays IBT | 24,681,459 | 24.7 | — | baseline (floor + mint + convert) |
| maker **Ibt**, taker pays IBT | 23,377,095 | 23.4 | −1.3 | the `convert_to_ibt` deposit |
| maker Ibt, **taker pays canonical PT** | 12,699,610 | 12.7 | **−10.7** | the `deposit_ibt` **mint** |

- The **mint is the biggest single lever: ~10.7 MB.**
- The maker denomination is **~1.3 MB** — negligible.
- Removing the mint lands IbtForPt at **12.7 MB** (in line with PtForIbt's
  no-mint ~15 MB — both are now single-valuation paths).

---

## 5. Why the taker token is the biggest lever

`handle_tokenisation` (`lib.rs:623`) only mints/redeems based on what the taker
pays:

- Taker pays **IBT** + funding-side order (IbtForPt/IbtForYt) → engine
  **mints** a fresh PT+YT pair via `deposit_ibt` (the +~10 MB).
- Taker pays the **canonical token** (`get_taker_token`: PT for IbtForPt, YT for
  IbtForYt) → `handle_tokenisation` is a **no-op**; the taker hands their
  existing PT/YT straight to the maker. No mint.

So the identical order costs 24.7 MB or 12.7 MB depending only on what the taker
pays. The tests deliberately exercise the worst case (taker pays IBT, engine
mints) — and even that fits under the cap at 64 KiB.

---

## 6. Pass/fail summary and mitigations

**Fits on every backend and every cell** once contracts are built with the
64 KiB Wasm stack (`scripts/build-wasm.sh`, CI). Worst case 27.9 MB of 41.94 MB
— ~14 MB of headroom. At the 1 MiB default stack, by contrast, every minting or
redeeming fill exceeded the cap and only the no-mint/no-redeem paths fit.

**Levers, ranked by measured impact** (now headroom-management, not fit-critical):

1. **64 KiB Wasm stack — the load-bearing lever, already shipped.** Cuts every
   path ~2.6× and is what pulls the whole matrix under the cap. A build/deploy
   flag, no code change. Everything below is secondary headroom.
2. **Route takers to pay the canonical PT/YT** for funding-side orders so the
   engine never mints. Proven: drops IbtForPt from 24.7 → 12.7 on direct Upshift.
3. **Keep the heavy backend out of pricing** — supply the IBT rate to
   `get_price` (caller/oracle) instead of `pt.preview_redeem`. Removes the one
   guaranteed valuation every path pays.
4. **Delegating wrapper + `b_rate` cache** (shipped for Blend) — a near-constant
   cut vs direct (wrapper max 21.6 vs direct 27.9).
5. **Cut a NAV valuation out of `deposit_ibt`** — it currently re-reads rates
   twice (`update_yield_inner` + `_convert_ibts_to_shares`). Collapsing to one
   would save ~3 MB on every minting fill.

---

## 7. Reproduce

Numbers depend on the 64 KiB-stack Wasm fixtures the fork tests `include_bytes!`
from `target/wasm32v1-none/release/`. **Build them first** (this applies the
`-zstack-size=65536` flag):

```bash
./scripts/build-wasm.sh
```

Then run the fork suites (each prints every fill's modelled `mem_bytes` vs cap):

```bash
# Per-fill lifecycle (§3 headline numbers, + §4 decomposition, salt 11/12):
cargo test -p limit-order-engine fork_upshift_vault_full_lifecycle    -- --ignored --nocapture
cargo test -p limit-order-engine fork_upshift_wrapper_full_lifecycle  -- --ignored --nocapture
cargo test -p limit-order-engine fork_blend_wrapper_full_lifecycle    -- --ignored --nocapture

# Full 16-cell cubes (§1 tables):
cargo test -p limit-order-engine fork_blend_memory_matrix          -- --ignored --nocapture
cargo test -p limit-order-engine fork_upshift_vault_memory_matrix  -- --ignored --nocapture
cargo test -p limit-order-engine fork_upshift_wrapper_memory_matrix -- --ignored --nocapture
```

All are `#[ignore]`, pinned to fork ledger `62_980_314`, and run offline from the
cached fork state in `tests/fork_cache_*.json` (first run needs mainnet RPC).

> Note: rebuilding the wasm at the default 1 MiB stack (a plain `cargo build`
> without `scripts/build-wasm.sh`) reproduces the pre-64 KiB numbers, where most
> minting/redeeming cells exceed the cap.
