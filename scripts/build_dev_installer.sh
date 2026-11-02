#!/usr/bin/env bash
#
# Build a Kage installer fast for dev iteration.
# macOS / Linux equivalent of build_dev_installer.ps1.
#
# Default: `cargo tauri build --debug` (debug profile). Smallest
# compile time, unoptimised runtime, and dependencies that key on
# `cfg(debug_assertions)` (notably tauri-plugin-aptabase) tag every
# event `isDebug=true` so Aptabase routes them to the Debug bucket
# and your prod dashboard stays clean.
#
# Pass --release for a release-profile build with relaxed LTO/
# codegen-units. Slower compile, faster runtime, and Aptabase events
# tagged `isDebug=false` (production). Use this when you specifically
# need to verify perf or repro a bug that only shows up under
# optimisation.
#
# Profile env vars are exported per-invocation only; Cargo.toml stays
# untouched, so CI and teammates running plain `cargo tauri build`
# still get the optimized config.
#
# Usage:
#   ./scripts/build_dev_installer.sh                # debug profile (default)
#   ./scripts/build_dev_installer.sh --no-bundle    # skip DMG/.app bundling
#   ./scripts/build_dev_installer.sh --release      # release profile
#   ./scripts/build_dev_installer.sh --replace      # kill running kage and
#                                                   # swap the installed exe
#                                                   # (implies --no-bundle)
#
# macOS: optional stable code signature (keeps TCC grants across rebuilds)
#   KAGE_MACOS_SIGNING_IDENTITY=<sha1 or exact cert name> ./scripts/build_dev_installer.sh
#
#   Unsigned builds are only linker-signed ad-hoc, so macOS TCC
#   (Accessibility, Input Monitoring, Screen Recording, ...) sees every
#   rebuild as a new app and silently stops honouring your grants. When
#   KAGE_MACOS_SIGNING_IDENTITY is set, it is passed to tauri as
#   APPLE_SIGNING_IDENTITY, so the bundle (sidecars, .app, .dmg) is signed
#   with that identity, hardened runtime and src-tauri/macos/entitlements.plist,
#   exactly like CI's signed releases. With --replace, the swapped binaries
#   and the installed .app are re-signed the same way, so the installed
#   app keeps its designated requirement. Make the identity with
#   scripts/macos/create_signing_cert.sh --import-keychain ... (see its
#   output for the one-time trust step). Unset means no signing, same as
#   before. Note: hardened runtime also blocks attaching lldb to these
#   bundles; use `cargo tauri dev` for debugger sessions.

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

if [[ "$release_profile" -eq 1 ]]; then
    echo "[build_dev_installer] Using fast release profile (lto=false, codegen-units=16)"
    export CARGO_PROFILE_RELEASE_LTO=false
    export CARGO_PROFILE_RELEASE_CODEGEN_UNITS=16
    profile_dir=release
else
    echo "[build_dev_installer] Using debug profile (fast compile; Aptabase classifies as Debug)"
    unset CARGO_PROFILE_RELEASE_LTO || true
    unset CARGO_PROFILE_RELEASE_CODEGEN_UNITS || true
    profile_dir=debug
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

macos_signing_identity=""
if [[ "$(uname -s)" == "Darwin" && -n "${KAGE_MACOS_SIGNING_IDENTITY:-}" ]]; then
    macos_signing_identity="$KAGE_MACOS_SIGNING_IDENTITY"
    echo "[build_dev_installer] macOS code signing with identity: $macos_signing_identity"
    # tauri-cli reads APPLE_SIGNING_IDENTITY (overrides bundle.macOS.signingIdentity).
    export APPLE_SIGNING_IDENTITY="$macos_signing_identity"
    if [[ "$no_bundle" -eq 1 && "$replace" -eq 0 ]]; then
        echo "[build_dev_installer] note: --no-bundle skips tauri's bundling step, which is where signing happens; target/$profile_dir/kage stays unsigned"
    fi
fi

# Re-sign one path with the same flags tauri-bundler uses
# (codesign --force -s <id> --options runtime --entitlements <plist>).
macos_resign() {
    codesign --force -s "$macos_signing_identity" --options runtime \
        --entitlements "$repo_root/src-tauri/macos/entitlements.plist" "$1"
}

cargo_args=(tauri build)
if [[ "$release_profile" -eq 0 ]]; then
    cargo_args+=(--debug)
fi
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

    source_exe="$repo_root/target/$profile_dir/kage"
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

    # Also copy the MCP binary so it stays in sync with the main binary.
    mcp_source="$repo_root/target/$profile_dir/kage-computer-control-mcp"
    if [[ -x "$mcp_source" ]]; then
        mcp_target="$(dirname "$target_exe")/kage-computer-control-mcp"
        echo "[build_dev_installer] --replace: $mcp_source -> $mcp_target"
        cp -f "$mcp_source" "$mcp_target"
    fi

    # The copied binaries are only linker-signed, which breaks the installed
    # bundle's signature (and with it the TCC grants). Re-sign inside-out like
    # tauri-bundler: the sidecar first, then the .app. Signing the .app re-signs
    # the main executable and reseals Contents/.
    if [[ -n "$macos_signing_identity" && "$target_exe" == */Contents/MacOS/* ]]; then
        app_bundle="${target_exe%/Contents/MacOS/*}"
        echo "[build_dev_installer] --replace: re-signing $app_bundle"
        if [[ -n "${mcp_target:-}" && -f "$mcp_target" ]]; then
            macos_resign "$mcp_target"
        fi
        macos_resign "$app_bundle"
        codesign --verify --deep --strict "$app_bundle"
        codesign -d -r- "$app_bundle" 2>&1 | grep 'designated =>' || true
    fi

    echo "[build_dev_installer] --replace: done. Launch via Spotlight / Dock to test."
fi

exit "$status"
