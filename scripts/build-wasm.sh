#!/usr/bin/env bash
set -euo pipefail

# Build all (or specified) contract crates to wasm32v1-none with wasm-opt
# optimization via `stellar contract build --optimize`.
#
# `SOROBAN_SDK_BUILD_SYSTEM_SUPPORTS_SPEC_SHAKING_V2` is passed for
# compatibility with stellar-cli < v25.2; newer versions set it automatically.
#
# Required by tests that use `include_bytes!` to load fixture wasms (e.g.
# principal-token tests deploying real YT via env.deployer().upload_contract_wasm).
#
# Usage:
#   ./scripts/build-wasm.sh             # build all contracts
#   ./scripts/build-wasm.sh yield-token # build a single crate

# Shrink the Wasm linker stack from the ~1 MiB default to 64 KiB. Soroban meters
# the stack reservation as linear memory on every contract invocation, so the
# default dominates per-tx `mem_bytes` once a call enters many contract frames
# (a LimitOrderEngine fill enters dozens). 64 KiB cuts a Blend minting fill from
# ~80 MiB to ~33 MiB — under the 40 MiB mainnet cap — while keeping ample stack
# headroom (the full test suite + mainnet-fork lifecycles pass with no overflow).
# See https://github.com/stellar/rs-soroban-env/pull/1691 (core-dev confirmed the
# 1 MiB default is excessive). A smaller stack only ever traps (reverts) on
# overflow — Wasm is memory-safe — so this cannot corrupt state; validate the
# deepest-stack path before lowering further.
export CARGO_BUILD_RUSTFLAGS="${CARGO_BUILD_RUSTFLAGS:-} -C link-arg=-zstack-size=65536"

CRATES=("$@")

if [ ${#CRATES[@]} -eq 0 ]; then
    echo "=== Building all contracts (wasm32v1-none, optimized) ==="
    SOROBAN_SDK_BUILD_SYSTEM_SUPPORTS_SPEC_SHAKING_V2=1 \
        stellar contract build --optimize
else
    echo "=== Building: ${CRATES[*]} ==="
    for crate in "${CRATES[@]}"; do
        echo "--- $crate ---"
        SOROBAN_SDK_BUILD_SYSTEM_SUPPORTS_SPEC_SHAKING_V2=1 \
            stellar contract build --package "$crate" --optimize
    done
fi

echo "=== Wasm artifacts at target/wasm32v1-none/release/*.wasm ==="
