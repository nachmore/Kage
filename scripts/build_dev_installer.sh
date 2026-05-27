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
#   ./scripts/build_dev_installer.sh --replace      # kill running kage and
#                                                   # swap the installed exe
#                                                   # (implies --no-bundle)

set -euo pipefail

repo_root="$(cd "$(dirname "$0")/.." && pwd)"
cd "$repo_root"

no_bundle=0
release_profile=0
replace=0
for arg in "$@"; do
    case "$arg" in
        --no-bundle) no_bundle=1 ;;
        --release)   release_profile=1 ;;
        --replace)   replace=1; no_bundle=1 ;;
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

# Tag the binary as a local dev build so init_logger() opts it in
# to trace-level logging. CI's release pipeline doesn't set this,
# so beta/stable channels still cap at Debug.
export KAGE_LOCAL_DEV_BUILD=1

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

if [[ "$status" -eq 0 && "$replace" -eq 1 ]]; then
    # macOS install: target is the binary inside the installed .app
    # bundle. Default to /Applications/Kage.app/Contents/MacOS/kage,
    # but prefer wherever a running Kage is actually launched from
    # so non-standard installs (~/Applications, dev-only directory)
    # work without flags.
    case "$(uname -s)" in
        Darwin)
            running_path="$(pgrep -lf '/Kage\.app/Contents/MacOS/kage' | awk '{print $2}' | head -n1 || true)"
            if [[ -n "$running_path" && -x "$running_path" ]]; then
                target_exe="$running_path"
            else
                target_exe="/Applications/Kage.app/Contents/MacOS/kage"
            fi
            ;;
        Linux)
            # Linux installs aren't shipped today; if you're hand-running
            # a release build the user knows where they put it.
            running_path="$(pgrep -af 'kage' | awk '/[/]kage($| )/ {print $2; exit}' || true)"
            target_exe="$running_path"
            ;;
        *)
            target_exe=""
            ;;
    esac

    source_exe="$repo_root/target/release/kage"
    if [[ -z "$target_exe" || ! -e "$target_exe" ]]; then
        echo "[build_dev_installer] --replace: no running Kage and no default target — skipping copy"
        exit "$status"
    fi
    if [[ ! -x "$source_exe" ]]; then
        echo "[build_dev_installer] --replace: source missing at $source_exe — skipping copy"
        exit "$status"
    fi

    echo "[build_dev_installer] --replace: stopping running kage processes…"
    pkill -f '[/]kage($| )' 2>/dev/null || true
    pkill -f 'kage-computer-control-mcp' 2>/dev/null || true
    sleep 1

    echo "[build_dev_installer] --replace: $source_exe -> $target_exe"
    cp -f "$source_exe" "$target_exe"
    echo "[build_dev_installer] --replace: done. Launch via Spotlight / Dock to test."
fi

exit "$status"
