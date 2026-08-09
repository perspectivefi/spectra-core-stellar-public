# Registry

Protocol registry storing Factory/Router/Bridge/LOE addresses, PT/YT WASM hashes, fee config, fee collector, and the registered PT set.

## Solidity-to-Soroban differences

| Aspect | Solidity (`src/Registry.sol`) | Soroban | Reason |
|---|---|---|---|
| Upgrade model | BeaconProxy — beacon swap upgrades all instances | Per-contract `update_current_contract_wasm()` via stored `BytesN<32>` WASM hashes | No beacon pattern on Soroban; each PT/YT upgrades individually |
| PT Set | `EnumerableSet.AddressSet` (dense indices via swap-and-pop) | Indexed pattern: `IsPt(Address)->u32`, `PtAt(u32)->Address`, monotonic `PtCount` | Soroban storage model; avoids reindexing on removal |
| Access Control | External `AccessManager` with `uint64` role constants | In-contract OZ `stellar-access` with `Symbol` roles (`REGISTRY`, `FEE_SET`, `UPGRADE`, `FACTORY`) | No separate contract needed; roles granted at init |
| Bridge / LOE addresses | Not present | Stored in instance storage; setters gated to contract admin only | Cross-chain peripherals new in Soroban port (PR #90) |
| Error handling | Custom errors → `revert` | `panic_with_error!` with `#[contracterror]` enum | Both roll back atomically |

## TTL strategy

| Data | Storage Tier | TTL | When Extended |
|---|---|---|---|
| All config (factory, router, bridge, LOE, fees, wasm hashes, fee collector) | Instance | `max_ttl()` | Every accessor (read or write) |
| `IsPt(Address)` — PT registration | Persistent | 6 months | Every accessor when entry exists |
| `PtAt(u32)` — PT enumeration index | Persistent | 6 months | Every accessor when entry exists |
| `PtCount` — next PT index | Persistent | 6 months | Every accessor when entry exists |
| `FeeReduction(pt, user)` — per-user fee discounts | Persistent | 6 months | Every accessor when entry exists |

Storage accessors in `storage.rs` bump TTL on both reads and writes.