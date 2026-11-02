#!/usr/bin/env bash
#
# Verify that a built Kage.app carries a STABLE code signature, meaning a
# certificate-based designated requirement (DR) rather than a cdhash. Read-only.
# Run by .github/workflows/release.yml after `cargo tauri build` when signing is
# configured. Also handy locally:
#
#   scripts/macos/verify_signature.sh target/release/bundle/macos/Kage.app \
#       [--tarball target/release/bundle/macos/Kage.app.tar.gz] \
#       [--dmg target/release/bundle/dmg/Kage_<ver>_aarch64.dmg] \
#       [--identity <sha1>]
#
# Fails if:
#   * the bundle or any executable in Contents/MacOS has no valid signature
#     (codesign --verify --deep --strict),
#   * a DR is cdhash-based (ad-hoc, which TCC re-keys on every build), or is only
#     implicit,
#   * hardened runtime is off,
#   * --identity is given and the bundle's leaf certificate has another SHA-1,
#   * the .app inside --tarball (the updater artefact) has a different DR,
#   * --dmg is given and the disk image itself is unsigned.
#
# Gatekeeper's verdict (spctl) is printed but never fails the run: a self-signed
# build is expected to be "rejected" there.

set -uo pipefail

app=""
tarball=""
dmg=""
identity=""
while [[ $# -gt 0 ]]; do
    case "$1" in
        --tarball)  tarball="${2:?}"; shift 2 ;;
        --dmg)      dmg="${2:?}"; shift 2 ;;
        --identity) identity="$(printf '%s' "${2:?}" | tr 'A-F' 'a-f')"; shift 2 ;;
        -h|--help)  sed -n '2,/^set -uo pipefail/p' "$0" | sed -e '$d' -e 's/^# \{0,1\}//'; exit 0 ;;
        -*) echo "unknown argument: $1" >&2; exit 2 ;;
        *)  app="$1"; shift ;;
    esac
done
[[ -n "$app" && -d "$app" ]] || { echo "usage: $0 path/to/Kage.app [--tarball T] [--dmg D] [--identity SHA1]" >&2; exit 2; }

failures=0
bad() { printf '::error::%s\n' "$*"; failures=$((failures + 1)); }
section() { printf '\n==== %s ====\n' "$*"; }

# Prints the explicit DR of $1 (empty if none or implicit).
explicit_dr() { codesign -d -r- "$1" 2>&1 | sed -n 's/^designated => //p'; }

check_dr() {
    local path="$1" label="$2" raw dr
    raw="$(codesign -d -r- "$path" 2>&1 | grep 'designated =>' || true)"
    printf '%s: %s\n' "$label" "${raw:-<no designated requirement>}"
    dr="$(explicit_dr "$path")"
    if [[ -z "$dr" ]]; then
        bad "$label: no explicit designated requirement (unsigned or ad-hoc). TCC would key grants on the cdhash."
    elif [[ "$dr" == *cdhash* ]]; then
        bad "$label: designated requirement is cdhash-based. TCC grants would not survive the next build."
    elif [[ "$dr" != *certificate* && "$dr" != *anchor* ]]; then
        bad "$label: designated requirement does not pin a certificate: $dr"
    fi
}

section "codesign --verify --deep --strict $app"
codesign --verify --deep --strict --verbose=2 "$app" 2>&1 || bad "$app: signature does not verify"

section "signature details"
details="$(codesign -dvvv "$app" 2>&1)"
printf '%s\n' "$details" | grep -E '^(Identifier|Format|CodeDirectory|Authority|TeamIdentifier|Runtime Version|Sealed Resources|Info.plist|Timestamp|Signed Time)'
printf '%s\n' "$details" | grep -E '^CodeDirectory' | grep -q 'runtime' \
    || bad "$app: hardened runtime flag missing (bundle.macOS.hardenedRuntime should be true)"

if [[ -n "$identity" ]]; then
    # Read the leaf certificate out of the signature. This works for
    # self-signed DRs (certificate leaf = H"...") and for Developer ID DRs
    # (anchor apple generic ... leaf[subject.OU]) alike.
    certs="$(mktemp -d)"
    leaf_sha1=""
    if codesign -d --extract-certificates="$certs/cert" "$app" >/dev/null 2>&1 && [[ -f "$certs/cert0" ]]; then
        leaf_sha1="$(openssl x509 -inform DER -in "$certs/cert0" -noout -fingerprint -sha1 \
            | sed -e 's/^.*=//' -e 's/://g' | tr 'A-F' 'a-f')"
    fi
    rm -rf "$certs"
    echo "leaf certificate SHA-1: ${leaf_sha1:-<none>}"
    [[ "$leaf_sha1" == "$identity" ]] \
        || bad "$app: signed by certificate ${leaf_sha1:-<none>}, expected $identity"
fi

section "designated requirements"
check_dr "$app" "Kage.app"
app_dr="$(explicit_dr "$app")"
# Every Mach-O tauri put in Contents/MacOS: the main binary plus the
# externalBin sidecars (kage-computer-control-mcp, kage-calendar-helper).
for bin in "$app"/Contents/MacOS/*; do
    [[ -f "$bin" ]] || continue
    codesign --verify --strict "$bin" 2>/dev/null || bad "$(basename "$bin"): signature does not verify"
    check_dr "$bin" "  Contents/MacOS/$(basename "$bin")"
done

section "entitlements"
codesign -d --entitlements - --xml "$app" 2>/dev/null | plutil -p - 2>/dev/null || echo "(none)"

if [[ -n "$tarball" ]]; then
    section "updater artefact $tarball"
    tmp="$(mktemp -d)"
    if tar -xzf "$tarball" -C "$tmp"; then
        inner="$(find "$tmp" -maxdepth 2 -name '*.app' -type d | head -n 1)"
        if [[ -z "$inner" ]]; then
            bad "$tarball: no .app inside"
        else
            codesign --verify --deep --strict --verbose=2 "$inner" 2>&1 || bad "$tarball: contained app does not verify"
            inner_dr="$(explicit_dr "$inner")"
            printf 'tarball app DR: %s\n' "${inner_dr:-<none>}"
            [[ -n "$inner_dr" && "$inner_dr" == "$app_dr" ]] \
                || bad "$tarball: contained app's DR differs from the bundle's (the updater would ship an unsigned/different app)"
        fi
    else
        bad "$tarball: could not extract"
    fi
    rm -rf "$tmp"
fi

if [[ -n "$dmg" ]]; then
    section "disk image $dmg"
    codesign -dvv "$dmg" 2>&1 | grep -E '^(Identifier|Format|Authority)' || true
    codesign --verify --verbose=2 "$dmg" 2>&1 || bad "$dmg: disk image is not signed"
fi

section "Gatekeeper (informational; self-signed builds are expected to be rejected)"
spctl --assess --verbose=4 --type execute "$app" 2>&1 || true

echo
if [[ "$failures" -gt 0 ]]; then
    echo "verify_signature: $failures problem(s)"
    exit 1
fi
echo "verify_signature: OK, stable certificate-based signature"
