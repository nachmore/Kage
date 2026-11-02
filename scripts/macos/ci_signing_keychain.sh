#!/usr/bin/env bash
#
# CI-only: set up (and later tear down) the macOS code-signing identity for
# tauri's bundler in .github/workflows/release.yml.
#
#   ci_signing_keychain.sh import    # before `cargo tauri build`
#   ci_signing_keychain.sh cleanup   # always(), after the build
#
# Inputs (step env; every one is optional):
#   MACOS_CERTIFICATE           base64 of a .p12 holding ONE code-signing identity:
#                               a "Developer ID Application: ..." certificate, or a
#                               self-signed one from scripts/macos/create_signing_cert.sh.
#   MACOS_CERTIFICATE_PASSWORD  password of that .p12.
#   MACOS_SIGNING_IDENTITY      pick an identity by SHA-1 or exact common name
#                               (default: the only one in the .p12).
#   APPLE_API_KEY, APPLE_API_ISSUER, APPLE_API_KEY_P8 (contents of AuthKey_<id>.p8)
#     or APPLE_ID, APPLE_PASSWORD (app-specific password), APPLE_TEAM_ID
#                               notarization credentials. Used only for a
#                               Developer ID identity.
#
# Outputs (appended to $GITHUB_ENV, ONLY when they have a real value):
#   APPLE_SIGNING_IDENTITY      read by tauri-cli; makes tauri-bundler codesign the
#                               sidecars, the .app and the .dmg.
#   APPLE_API_KEY / APPLE_API_ISSUER / APPLE_API_KEY_PATH, or
#   APPLE_ID / APPLE_PASSWORD / APPLE_TEAM_ID
#                               read by tauri-bundler to notarize and staple the .app.
#   KAGE_MACOS_SIGNING_KIND     "developer-id" or "self-signed" (for the verify step).
#   KAGE_MACOS_SIGNING_KEYCHAIN, KAGE_MACOS_SIGNING_TRUSTED_CERT  (for cleanup).
#
# Why this script imports the identity instead of handing tauri APPLE_CERTIFICATE.
# Behaviour below is from the tauri-bundler 2.9.2 / tauri-macos-sign 2.3.4 sources
# that tauri-cli 2.11.2 locks:
#   * tauri's own APPLE_CERTIFICATE import only finds Apple-issued identities. It
#     searches for the CN prefixes "Developer ID Application:", "Apple Development:"
#     and so on, and requires an OU (team id). A self-signed certificate fails with
#     "failed to resolve signing identity".
#   * tauri reads every APPLE_* variable with var_os(), so an EMPTY variable counts
#     as set. `${{ secrets.X }}` expands to "" for an unset secret, so putting these
#     in a step's env: would break secret-less builds: tauri would try to import an
#     empty .p12, run `codesign -s ""`, or try to notarize with blank credentials.
#     So this script writes only the non-empty values to $GITHUB_ENV.
#   * Given APPLE_SIGNING_IDENTITY, tauri runs `codesign --force -s <identity>
#     [--options runtime] --entitlements <plist> <path>` with no --keychain, so
#     codesign only searches keychains on the user search list. codesign also
#     refuses an identity whose certificate is not trusted for code signing ("no
#     identity found"). A Developer ID certificate chains to Apple's roots. A
#     self-signed one does not, so it gets an admin-domain trust setting limited
#     to the codeSign policy, on this throwaway runner only.
#
# Refuses to run outside GitHub Actions: it changes the keychain search list and,
# for a self-signed certificate, the admin trust settings and the
# com.apple.trust-settings.admin authorization right.

set -euo pipefail

log()  { printf '[macos-signing] %s\n' "$*"; }
warn() { printf '::warning::%s\n' "$*"; }
fail() { printf '::error::%s\n' "$*"; exit 1; }

require_ci() {
    [[ "${GITHUB_ACTIONS:-}" == "true" ]] \
        || { echo "ci_signing_keychain.sh only runs on GitHub Actions (it edits the keychain search list and trust settings)." >&2; exit 2; }
    [[ "$(uname -s)" == "Darwin" ]] || fail "ci_signing_keychain.sh is macOS-only"
    [[ -n "${RUNNER_TEMP:-}" && -n "${GITHUB_ENV:-}" ]] || fail "RUNNER_TEMP / GITHUB_ENV not set"
}

# Append NAME=VALUE to $GITHUB_ENV. Values must be single-line. Empty values are
# never written, because an empty APPLE_* var is "set" as far as tauri is concerned.
export_env() {
    local name="$1" value="$2"
    [[ -n "$value" ]] || return 0
    [[ "$value" != *$'\n'* && "$value" != *$'\r'* ]] || fail "$name must be a single line"
    printf '%s=%s\n' "$name" "$value" >> "$GITHUB_ENV"
}

state_dir() { printf '%s/kage-macos-signing' "$RUNNER_TEMP"; }

# Unquoted entries of the user keychain search list, one per line.
user_search_list() {
    security list-keychains -d user | sed -e 's/^[[:space:]]*"//' -e 's/"[[:space:]]*$//'
}

with_trust_settings_right() {
    # Since Big Sur, changing admin trust settings requires an authorization the
    # runner can't answer interactively ("The authorization was denied since no
    # user interaction was possible"), even as root. Open the right for the one
    # call, then restore the saved rule.
    local backup
    backup="$(state_dir)/trust-settings-admin.right.plist"
    if security authorizationdb read com.apple.trust-settings.admin > "$backup" 2>/dev/null; then
        sudo security authorizationdb write com.apple.trust-settings.admin allow >/dev/null
        local rc=0
        "$@" || rc=$?
        # The backup is ours (runner user), so the unprivileged shell opening it
        # for sudo's stdin is intended.
        # shellcheck disable=SC2024
        sudo security authorizationdb write com.apple.trust-settings.admin < "$backup" >/dev/null \
            || warn "could not restore the com.apple.trust-settings.admin right (ephemeral runner, continuing)"
        return "$rc"
    fi
    warn "could not read com.apple.trust-settings.admin; trying without changing it"
    "$@"
}

cmd_import() {
    if [[ -z "${MACOS_CERTIFICATE:-}" ]]; then
        log "MACOS_CERTIFICATE is not set: macOS bundles stay unsigned (linker-signed ad-hoc), same as before."
        return 0
    fi
    [[ -n "${MACOS_CERTIFICATE_PASSWORD:-}" ]] \
        || fail "MACOS_CERTIFICATE is set but MACOS_CERTIFICATE_PASSWORD is empty. Refusing to ship an unsigned build when signing was configured."

    local dir keychain kc_password p12
    dir="$(state_dir)"
    rm -rf "$dir"
    mkdir -p "$dir"
    chmod 700 "$dir"
    keychain="$dir/kage-signing.keychain-db"
    p12="$dir/identity.p12"
    kc_password="$(openssl rand -hex 24)"
    echo "::add-mask::$kc_password"

    printf '%s' "$MACOS_CERTIFICATE" | tr -d '[:space:]' | base64 --decode > "$p12" \
        || fail "MACOS_CERTIFICATE is not valid base64"

    security create-keychain -p "$kc_password" "$keychain"
    # Record the keychain before anything else can fail, so cleanup always finds it.
    export_env KAGE_MACOS_SIGNING_KEYCHAIN "$keychain"
    security set-keychain-settings -lut 21600 "$keychain"
    security unlock-keychain -p "$kc_password" "$keychain"
    security import "$p12" -k "$keychain" -f pkcs12 -P "$MACOS_CERTIFICATE_PASSWORD" \
        -T /usr/bin/codesign >/dev/null \
        || fail "security import failed. Wrong MACOS_CERTIFICATE_PASSWORD, or a .p12 that macOS cannot read (export with 3DES/SHA-1, as create_signing_cert.sh does)."
    rm -f "$p12"
    # Without a partition list, codesign's first use of the key waits for a
    # GUI "allow" click that never comes.
    security set-key-partition-list -S apple-tool:,apple:,codesign: -s -k "$kc_password" "$keychain" >/dev/null

    # tauri calls codesign without --keychain, so the keychain must be on the search list.
    local entries=("$keychain") line
    while IFS= read -r line; do
        [[ -n "$line" && "$line" != "$keychain" ]] && entries+=("$line")
    done < <(user_search_list)
    security list-keychains -d user -s "${entries[@]}"

    # --- pick the identity -------------------------------------------------
    local listing matches sha1 name
    listing="$(security find-identity -p codesigning "$keychain")"
    # Lines like:   1) 0123...ABCD "Common Name" (CSSMERR_TP_NOT_TRUSTED)
    matches="$(printf '%s\n' "$listing" \
        | awk '/Matching identities/{m=1; next} /Valid identities only/{m=0} m' \
        | grep -E '^[[:space:]]*[0-9]+\) [0-9A-F]{40} "' || true)"
    [[ -n "$matches" ]] || { printf '%s\n' "$listing"; fail "no code-signing identity in MACOS_CERTIFICATE"; }

    if [[ -n "${MACOS_SIGNING_IDENTITY:-}" ]]; then
        if [[ "$MACOS_SIGNING_IDENTITY" =~ ^[0-9A-Fa-f]{40}$ ]]; then
            local want
            want="$(printf '%s' "$MACOS_SIGNING_IDENTITY" | tr 'a-f' 'A-F')"
            matches="$(printf '%s\n' "$matches" | grep -E "\) ${want} \"" || true)"
        else
            matches="$(printf '%s\n' "$matches" | grep -F "\"${MACOS_SIGNING_IDENTITY}\"" || true)"
        fi
        [[ -n "$matches" ]] || { printf '%s\n' "$listing"; fail "MACOS_SIGNING_IDENTITY does not match any identity in MACOS_CERTIFICATE"; }
    fi
    [[ "$(printf '%s\n' "$matches" | wc -l | tr -d ' ')" -eq 1 ]] \
        || { printf '%s\n' "$listing"; fail "MACOS_CERTIFICATE holds several code-signing identities; set MACOS_SIGNING_IDENTITY to pick one"; }

    sha1="$(printf '%s\n' "$matches" | sed -E 's/^[[:space:]]*[0-9]+\) ([0-9A-F]{40}) ".*$/\1/')"
    name="$(printf '%s\n' "$matches" | sed -E 's/^[[:space:]]*[0-9]+\) [0-9A-F]{40} "(.*)"( \([^)]*\))?$/\1/')"

    # --- make codesign willing to use it -----------------------------------
    local kind
    if security find-identity -v -p codesigning "$keychain" | grep -q " ${sha1} "; then
        kind="developer-id"
        [[ "$name" == "Developer ID Application:"* ]] || kind="apple-issued"
        log "Identity \"$name\" ($sha1) is already trusted for code signing."
    else
        kind="self-signed"
        log "Identity \"$name\" ($sha1) is not trusted (self-signed). Trusting it for the codeSign policy on this runner."
        local cert_pem="$dir/signing-cert.pem"
        security find-certificate -a -Z -p "$keychain" \
            | awk -v want="$sha1" '
                /^SHA-1 hash:/ { grab = ($3 == want) }
                grab && /-----BEGIN CERTIFICATE-----/ { out = 1 }
                out { print }
                out && /-----END CERTIFICATE-----/ { exit }' > "$cert_pem"
        [[ -s "$cert_pem" ]] || fail "could not export the certificate for $sha1"
        with_trust_settings_right sudo security add-trusted-cert -d -r trustRoot -p codeSign "$cert_pem" \
            || fail "could not trust the self-signed certificate for code signing"
        export_env KAGE_MACOS_SIGNING_TRUSTED_CERT "$cert_pem"
        security find-identity -v -p codesigning "$keychain" | grep -q " ${sha1} " \
            || { security find-identity -p codesigning "$keychain"; fail "certificate still not a valid code-signing identity after add-trusted-cert"; }
    fi

    export_env APPLE_SIGNING_IDENTITY "$sha1"
    export_env KAGE_MACOS_SIGNING_KIND "$kind"
    log "tauri will sign with \"$name\" ($sha1), kind=$kind."

    # --- notarization (Developer ID only) ----------------------------------
    local has_api=0 has_appleid=0
    [[ -n "${APPLE_API_KEY:-}" && -n "${APPLE_API_ISSUER:-}" && -n "${APPLE_API_KEY_P8:-}" ]] && has_api=1
    [[ -n "${APPLE_ID:-}" && -n "${APPLE_PASSWORD:-}" && -n "${APPLE_TEAM_ID:-}" ]] && has_appleid=1
    if [[ "$kind" != "developer-id" ]]; then
        if [[ -n "${APPLE_API_KEY:-}${APPLE_ID:-}" ]]; then
            warn "Notarization credentials ignored: \"$name\" is not a Developer ID Application certificate, and Apple only notarizes Developer ID-signed code."
        fi
        return 0
    fi
    if [[ "$has_api" -eq 1 ]]; then
        [[ "$APPLE_API_KEY" =~ ^[A-Za-z0-9]+$ ]] || fail "APPLE_API_KEY must be the bare key id"
        local p8="$dir/AuthKey_${APPLE_API_KEY}.p8"
        printf '%s\n' "$APPLE_API_KEY_P8" > "$p8"
        export_env APPLE_API_KEY "$APPLE_API_KEY"
        export_env APPLE_API_ISSUER "$APPLE_API_ISSUER"
        export_env APPLE_API_KEY_PATH "$p8"
        log "Notarization: App Store Connect API key $APPLE_API_KEY."
    elif [[ "$has_appleid" -eq 1 ]]; then
        export_env APPLE_ID "$APPLE_ID"
        export_env APPLE_PASSWORD "$APPLE_PASSWORD"
        export_env APPLE_TEAM_ID "$APPLE_TEAM_ID"
        log "Notarization: Apple ID (team $APPLE_TEAM_ID)."
    else
        warn "Developer ID identity without complete notarization credentials. The app is signed but NOT notarized, so Gatekeeper still blocks a downloaded dmg."
    fi
}

cmd_cleanup() {
    local cert="${KAGE_MACOS_SIGNING_TRUSTED_CERT:-}" keychain="${KAGE_MACOS_SIGNING_KEYCHAIN:-}"
    if [[ -n "$cert" && -f "$cert" ]]; then
        with_trust_settings_right sudo security remove-trusted-cert -d "$cert" \
            || warn "could not remove the admin trust setting for the signing certificate"
    fi
    if [[ -n "$keychain" && -e "$keychain" ]]; then
        # Also drops it from the search list.
        security delete-keychain "$keychain" || warn "could not delete $keychain"
    fi
    rm -rf "$(state_dir)"
    log "signing keychain cleaned up"
}

require_ci
case "${1:-}" in
    import)  cmd_import ;;
    cleanup) cmd_cleanup ;;
    *) echo "usage: $0 import|cleanup" >&2; exit 2 ;;
esac
