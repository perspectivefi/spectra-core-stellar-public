# spectra-core-stellar

Soroban implementation of the Spectra yield-tokenization protocol. The original
public release corresponds to the audited source revision described in the
[included Certora audit documentation](audit/README.md). That document also lists
the subsequent contract changes included in the current source tree.

## Build and run tests

```sh
make build   # builds all contracts to target/wasm32v1-none/release/*.wasm
make test    # `make build` + `cargo test --workspace`. Safe default.
```

**Always run `make build` (or `./scripts/build-wasm.sh`) before tests.** Several crates embed sibling-contract WASM via `include_bytes!` — e.g. `principal-token` and `router` tests load `yield_token.wasm`. A missing or stale artifact surfaces as a confusing `Error(Auth, InvalidAction)` inside the downstream contract, not as a build failure.

Plain `cargo test` (no prior build) also fails because `stellar-tokens 0.7.0` needs the `SOROBAN_SDK_BUILD_SYSTEM_SUPPORTS_SPEC_SHAKING_V2=1` env-var bypass that `scripts/build-wasm.sh` sets.

## Workspace layout

```
contracts/
  spectra-common/        Shared utilities: constants, naming, cross-contract traits
  spectra-math/          Fixed-point math: ray_math, mul_div, log_exp_math
  registry/              Protocol registry: PT WASM hash, fee config, fee collector
  yield-token/           SEP-41 YT, paired with PT
  principal-token/       PT contract (ERC-4626 vault)
  factory/               PT deployment factory
  router/                Batch executor (typed Command enum)
  limit-order-engine/    Off-chain signed order matching
mocks/
  spectra-mocks/         Test fixtures: underlying, IBT (toy), IBT (OZ vault), PT, rewards proxy
  mock-factory-pt-init/  Factory deploy-test mock
scripts/                 Audited contract build script
```

## Build chain note

`stellar-tokens 0.7.0` (transitive) enables `experimental_spec_shaking_v2` on
`soroban-sdk`. The audited build configuration sets
`SOROBAN_SDK_BUILD_SYSTEM_SUPPORTS_SPEC_SHAKING_V2=1` and uses a 64 KiB Wasm
linker stack. Build outputs depend on the pinned Rust toolchain, locked
dependencies, Stellar CLI, and this configuration; this repository does not
claim bit-for-bit reproducibility across other environments.

## Deployment: initializers are not auth-gated

Every contract's `initialize` is guarded only by an `is_initialized` check — it does **not** call `admin.require_auth()` (mirrors Solidity's `initialize(admin)`, which lets the deployer designate any admin). This means whoever calls `initialize` first wins and sets every parameter, including the admin and all role holders.

Deploy each contract **atomically** — deploy and `initialize` in the same transaction (factory / same-tx flow), or otherwise restrict who can reach the uninitialized instance. A contract deployed and left uninitialized can be front-run and taken over by any caller.

Each contract crate has its own `README.md` documenting Solidity-to-Soroban differences and TTL strategy.
