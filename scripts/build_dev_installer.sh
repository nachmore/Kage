#!/usr/bin/env bash
#
# Build a Kage installer with relaxed release-profile settings for faster
# dev iteration. macOS / Linux equivalent of build_dev_installer.ps1.
#
# `cargo tauri build` defaults to the project's `[profile.release]` config,
# which sets `lto = true` + `codegen-units = 1`. That's right for ship
# builds but very slow to iterate on. This wrapper overrides those via
# CARGO_PROFILE_RELEASE_* env vars (which Cargo honors per-invocation) so
# the env-var overrides flow into both the kage-computer-control-mcp build
# done by scripts/build_mcp.py and the main `kage` build Tauri kicks off.
#
# Cargo.toml stays untouched, so CI and teammates running plain
# `cargo tauri build` still get the optimized config.
#
# Usage:
#   ./scripts/build_dev_installer.sh                # fast iteration build
#   ./scripts/build_dev_installer.sh --no-bundle    # skip DMG/.app bundling
#   ./scripts/build_dev_installer.sh --release      # use full Cargo.toml profile

set -euo pipefail

repo_root="$(cd "$(dirname "$0")/.." && pwd)"
cd "$repo_root"

no_bundle=0
release_profile=0
for arg in "$@"; do
    case "$arg" in
        --no-bundle) no_bundle=1 ;;
        --release)   release_profile=1 ;;
        *) echo "[build_dev_installer] Unknown arg: $arg" >&2; exit 2 ;;
    esac
done

if [[ "$release_profile" -eq 0 ]]; then
    echo "[build_dev_installer] Using fast release profile (lto=false, codegen-units=16)"
    export CARGO_PROFILE_RELEASE_LTO=false
    export CARGO_PROFILE_RELEASE_CODEGEN_UNITS=16
else
    echo "[build_dev_installer] Using full release profile (Cargo.toml defaults)"
    unset CARGO_PROFILE_RELEASE_LTO || true
    unset CARGO_PROFILE_RELEASE_CODEGEN_UNITS || true
fi

# Bumping rustc's stack avoids a STATUS_STACK_BUFFER_OVERRUN we hit on
# Windows during heavy generic-monomorphization in type analysis. Harmless
# on Unix where the default stack is already large; setting it
# unconditionally keeps the script symmetric.
export RUST_MIN_STACK="${RUST_MIN_STACK:-16777216}"

cargo_args=(tauri build)
if [[ "$no_bundle" -eq 1 ]]; then
    cargo_args+=(--no-bundle)
fi

start=$(date +%s)
echo "[build_dev_installer] Running: cargo ${cargo_args[*]}"
cargo "${cargo_args[@]}"
status=$?
elapsed=$(( $(date +%s) - start ))

echo
echo "[build_dev_installer] cargo exit code: $status (elapsed: ${elapsed}s)"
exit "$status"
