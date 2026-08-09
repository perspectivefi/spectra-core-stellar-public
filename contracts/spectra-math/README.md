# spectra-math

Fixed-point math primitives for the Spectra Soroban port. Library crate only — no contract, no storage.

## Modules

| Module | Purpose | Solidity source |
|---|---|---|
| `ray_math` | 27-decimal ↔ token-decimal conversions (`to_ray`, `from_ray`, `from_ray_rounded`) | `RayMath.sol` |
| `mul_div` | Fixed-point `a * b / d` with `Floor` / `Ceil` rounding | OpenZeppelin `Math.mulDiv` |
| `log_exp_math` | `exp`, `ln`, `pow` on 18-decimal fixed-point using `I256`/`U256` | Balancer `LogExpMath.sol` |

`log_exp_math` is a strict port of Balancer — identical constants, domain bounds, intermediate precision (20-decimal for `exp`/`_ln`, 36-decimal for `_ln_36`). All functions take `&Env` because `I256`/`U256` routes through Soroban host.

## Overflow policy

`to_ray` and `mul_div` rely on workspace `overflow-checks = true` (both debug and release profiles in root `Cargo.toml`). Multiplication panics cleanly on overflow. **If that flag is ever disabled, these functions will silently wrap** — replace bare `*` with `checked_mul` first.

## Testing

| Type | Description |
|---|---|
| Unit tests | Per-module correctness (rounding, edge cases, exact values). `cargo test -p spectra-math` |
| Cross-implementation parity | `test_cross_validation.rs` loads `test-vectors/cross_validation.json` (~80 vectors for `exp`, `ln`, `pow` including adversarial inputs) and asserts bit-exact equality with Balancer's Solidity output (generated via Foundry) |
| Invariant sweep | `test_rounding_invariants` asserts `ceil - floor ∈ {0, 1}` and `floor * d ≤ a*b` across hand-picked inputs. Property-based fuzzing upgrade tracked separately. |

## Soroban notes

- No storage, no events, no contract wrapper — pure library
- `no_std` crate; tests use `extern crate std` for `serde_json` + formatting
- `I256`/`U256` requires `&Env` on every call, propagates through `log_exp_math` public API. Callers with `Env` pay no cost beyond passing it in.