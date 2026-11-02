#!/usr/bin/env bash
#
# Generate a self-signed code-signing certificate for Kage's macOS builds.
#
# Why this exists
# ---------------
# macOS TCC (Accessibility, Input Monitoring, Screen Recording, Automation,
# Calendars, Microphone) records each grant together with the app's
# *designated requirement* (DR). A build signed only ad-hoc (what the
# linker does on Apple Silicon when nobody runs codesign) has a DR that is
# just its cdhash, so every update and every local rebuild looks like a
# different app to TCC. The grants stop applying, while System Settings
# still shows "Kage" switched on for the old binary.
#
# If every build is signed with the SAME certificate, codesign writes the DR
#     identifier "com.kage.launcher" and certificate leaf = H"<cert sha1>"
# which stays the same across rebuilds, so grants survive updates. A
# self-signed certificate is enough for that. It does NOT satisfy
# Gatekeeper; that needs a Developer ID certificate plus notarization.
# See docs/RELEASE.md, "macOS code signing".
#
# codesign only uses an identity whose certificate is trusted for code signing.
# For a self-signed certificate that means an explicit trust setting. CI adds
# one on its throwaway runner (scripts/macos/ci_signing_keychain.sh). For local
# signing, this script prints the command for you to run.
#
# What it does
# ------------
#   1. openssl: makes an RSA key and a self-signed X.509 certificate
#      (CN "Kage Self-Signed Code Signing", keyUsage=digitalSignature,
#      extendedKeyUsage=codeSigning, valid for 10 years by default).
#   2. Exports a password-protected .p12. It uses 3DES/SHA-1 PBE, the
#      PKCS#12 format that `security import` reads on every macOS
#      version. OpenSSL 3's default (AES/PBKDF2) is rejected by older
#      releases with "MAC verification failed".
#   3. Prints the base64 of the .p12 and the exact `gh secret set` commands
#      for the release workflow. It NEVER runs them.
#   4. Optional --import-keychain PATH: imports the identity into a keychain
#      so local builds can be signed too (scripts/build_dev_installer.sh
#      with KAGE_MACOS_SIGNING_IDENTITY).
#
# The script does not change your login keychain, keychain search list or
# trust settings unless you point --import-keychain at one of them.
#
# Usage
# -----
#   scripts/macos/create_signing_cert.sh [options]
#
#   --out-dir DIR          Where to write the outputs (default: ~/kage-signing-cert).
#                          Must not exist yet or must be empty, unless --force.
#   --name CN              Certificate common name
#                          (default: "Kage Self-Signed Code Signing").
#   --days N               Validity in days (default: 3650). Replacing the
#                          certificate later changes the DR, which costs every
#                          user one more TCC re-grant, so pick a long validity.
#   --repo OWNER/NAME      Repo used in the printed `gh` commands
#                          (default: nachmore/Kage).
#   --import-keychain PATH Also import the identity into this keychain file
#                          (created if missing). Keychain password comes from
#                          $KAGE_KEYCHAIN_PASSWORD, or you are prompted for it.
#   --force                Overwrite outputs in an existing --out-dir.
#   -h, --help             Show this help.
#
# Environment
# -----------
#   KAGE_P12_PASSWORD      Use this .p12 password instead of a random one.
#   KAGE_KEYCHAIN_PASSWORD Keychain password for --import-keychain.
#   OPENSSL                openssl binary to use (default: first on PATH).
#                          Works with LibreSSL (/usr/bin/openssl) and OpenSSL 3.

set -euo pipefail

name="Kage Self-Signed Code Signing"
days=3650
out_dir="${HOME}/kage-signing-cert"
repo="nachmore/Kage"
import_keychain=""
force=0
openssl_bin="${OPENSSL:-openssl}"

log() { printf '[create_signing_cert] %s\n' "$*" >&2; }
die() { log "error: $*"; exit 1; }
usage() { sed -n '2,/^set -euo pipefail/p' "$0" | sed -e '$d' -e 's/^# \{0,1\}//'; }

while [[ $# -gt 0 ]]; do
    case "$1" in
        --out-dir)         out_dir="${2:?--out-dir needs a value}"; shift 2 ;;
        --name)            name="${2:?--name needs a value}"; shift 2 ;;
        --days)            days="${2:?--days needs a value}"; shift 2 ;;
        --repo)            repo="${2:?--repo needs a value}"; shift 2 ;;
        --import-keychain) import_keychain="${2:?--import-keychain needs a value}"; shift 2 ;;
        --force)           force=1; shift ;;
        -h|--help)         usage; exit 0 ;;
        *) log "unknown argument: $1"; usage >&2; exit 2 ;;
    esac
done

[[ "$days" =~ ^[0-9]+$ && "$days" -gt 0 ]] || die "--days must be a positive integer"
[[ "$name" != *$'\n'* && "$name" != *'/'* ]] || die "--name must not contain '/' or newlines"
command -v "$openssl_bin" >/dev/null 2>&1 || die "openssl not found (set OPENSSL=/path/to/openssl)"
if [[ -n "$import_keychain" && "$(uname -s)" != "Darwin" ]]; then
    die "--import-keychain needs macOS (the 'security' tool)"
fi

# Everything below writes private key material. Keep it owner-only.
umask 077

p12_file="kage-codesign.p12"
b64_file="kage-codesign.p12.base64"
pw_file="kage-codesign.p12.password"
cert_file="kage-codesign.cert.pem"

mkdir -p "$out_dir"
out_dir="$(cd "$out_dir" && pwd)"
for f in "$p12_file" "$b64_file" "$pw_file" "$cert_file"; do
    if [[ -e "$out_dir/$f" && "$force" -ne 1 ]]; then
        die "$out_dir/$f already exists. Refusing to overwrite an existing signing identity (use --force, or a different --out-dir). If that certificate is already in use, replacing it changes Kage's designated requirement and every user will have to re-grant TCC permissions."
    fi
done

work="$(mktemp -d "${TMPDIR:-/tmp}/kage-cert.XXXXXX")"
trap 'rm -rf "$work"' EXIT

# --- 1. key + self-signed certificate ------------------------------------
cat > "$work/openssl.cnf" <<EOF
[ req ]
distinguished_name = dn
prompt             = no
x509_extensions    = kage_codesign

[ dn ]
CN = ${name}

[ kage_codesign ]
basicConstraints     = critical, CA:FALSE
keyUsage             = critical, digitalSignature
extendedKeyUsage     = critical, codeSigning
subjectKeyIdentifier = hash
EOF

log "Using $("$openssl_bin" version)"
log "Generating RSA-3072 key and self-signed certificate \"$name\" (valid $days days)"
"$openssl_bin" req -x509 -new -newkey rsa:3072 -sha256 -days "$days" -nodes \
    -config "$work/openssl.cnf" \
    -keyout "$work/key.pem" -out "$work/cert.pem" 2>"$work/req.log" \
    || { cat "$work/req.log" >&2; die "openssl req failed"; }

# --- 2. password-protected .p12 ------------------------------------------
if [[ -n "${KAGE_P12_PASSWORD:-}" ]]; then
    p12_password="$KAGE_P12_PASSWORD"
else
    p12_password="$("$openssl_bin" rand -hex 24)"
fi
printf '%s' "$p12_password" > "$work/p12.password"

"$openssl_bin" pkcs12 -export \
    -inkey "$work/key.pem" -in "$work/cert.pem" \
    -name "$name" \
    -keypbe PBE-SHA1-3DES -certpbe PBE-SHA1-3DES -macalg sha1 \
    -passout "file:$work/p12.password" \
    -out "$work/cert.p12" 2>"$work/p12.log" \
    || { cat "$work/p12.log" >&2; die "openssl pkcs12 -export failed"; }

# Read it back so a broken export fails here, not in CI.
"$openssl_bin" pkcs12 -in "$work/cert.p12" -passin "file:$work/p12.password" -noout 2>"$work/p12-verify.log" \
    || { cat "$work/p12-verify.log" >&2; die "the exported .p12 does not read back"; }

"$openssl_bin" base64 -A -in "$work/cert.p12" -out "$work/cert.p12.base64"

cp "$work/cert.p12" "$out_dir/$p12_file"
cp "$work/cert.p12.base64" "$out_dir/$b64_file"
cp "$work/p12.password" "$out_dir/$pw_file"
cp "$work/cert.pem" "$out_dir/$cert_file"
chmod 600 "$out_dir/$p12_file" "$out_dir/$b64_file" "$out_dir/$pw_file"
chmod 644 "$out_dir/$cert_file"

sha1="$("$openssl_bin" x509 -in "$work/cert.pem" -noout -fingerprint -sha1 | sed -e 's/^.*=//' -e 's/://g' | tr 'a-f' 'A-F')"
not_after="$("$openssl_bin" x509 -in "$work/cert.pem" -noout -enddate | sed 's/^notAfter=//')"

# --- 3. optional keychain import (local dev signing) ---------------------
if [[ -n "$import_keychain" ]]; then
    case "$import_keychain" in
        /*) ;;
        *) import_keychain="$PWD/$import_keychain" ;;
    esac
    kc_password="${KAGE_KEYCHAIN_PASSWORD:-}"
    if [[ -z "$kc_password" ]]; then
        [[ -t 0 ]] || die "--import-keychain needs KAGE_KEYCHAIN_PASSWORD when stdin is not a terminal"
        read -r -s -p "Password for keychain $import_keychain: " kc_password; echo >&2
        [[ -n "$kc_password" ]] || die "empty keychain password"
    fi
    if [[ ! -e "$import_keychain" ]]; then
        log "Creating keychain $import_keychain"
        security create-keychain -p "$kc_password" "$import_keychain"
    fi
    security unlock-keychain -p "$kc_password" "$import_keychain"
    log "Importing identity into $import_keychain (codesign gets access to the key)"
    security import "$out_dir/$p12_file" -k "$import_keychain" -f pkcs12 \
        -P "$p12_password" -T /usr/bin/codesign >/dev/null
    # Without this, the first codesign run shows a "codesign wants to use
    # key ..." dialog (and headless runs just hang on it).
    security set-key-partition-list -S apple-tool:,apple:,codesign: -s \
        -k "$kc_password" "$import_keychain" >/dev/null
    log "Code-signing identities now in that keychain:"
    security find-identity -p codesigning "$import_keychain" >&2 || true
fi

# --- 4. report -----------------------------------------------------------
q_b64="$(printf '%q' "$out_dir/$b64_file")"
q_pw="$(printf '%q' "$out_dir/$pw_file")"

cat <<EOF

================================================================================
 Self-signed code-signing certificate created
================================================================================
 Common name : $name
 SHA-1       : $sha1
 Valid until : $not_after
 Output dir  : $out_dir
   $p12_file           identity (private key + certificate). SECRET.
   $b64_file    base64 of the .p12 (GitHub secret MACOS_CERTIFICATE). SECRET.
   $pw_file  .p12 password (GitHub secret MACOS_CERTIFICATE_PASSWORD). SECRET.
   $cert_file      public certificate, for inspection only.

 Back up the .p12 and its password (for example in a password manager). Every
 Kage build signed with this certificate shares the designated requirement
 below. If you lose the certificate and make a new one, every macOS user has
 to re-grant Accessibility, Input Monitoring, Screen Recording and similar
 permissions one more time.

   designated => identifier "com.kage.launcher" and certificate leaf = H"$(printf '%s' "$sha1" | tr 'A-F' 'a-f')"

 (codesign derives this itself. Check it on a signed build with:
    codesign -d -r- /path/to/Kage.app)

--------------------------------------------------------------------------------
 base64 of $p12_file (the value of the MACOS_CERTIFICATE secret):
--------------------------------------------------------------------------------
$(cat "$out_dir/$b64_file")
--------------------------------------------------------------------------------

 To enable signing in .github/workflows/release.yml, run these yourself.
 This script does not run them:

   gh secret set MACOS_CERTIFICATE --repo $repo < $q_b64
   gh secret set MACOS_CERTIFICATE_PASSWORD --repo $repo < $q_pw

 Optional: pin the identity explicitly instead of letting CI find it in the .p12:

   gh secret set MACOS_SIGNING_IDENTITY --repo $repo --body $sha1

EOF

if [[ -n "$import_keychain" ]]; then
    in_search_list=0
    search_list_cmd="security list-keychains -d user -s"
    while IFS= read -r line; do
        line="${line#"${line%%[![:space:]]*}"}"
        line="${line#\"}"; line="${line%\"}"
        [[ -z "$line" ]] && continue
        [[ "$line" == "$import_keychain" ]] && in_search_list=1
        search_list_cmd+=" $(printf '%q' "$line")"
    done < <(security list-keychains -d user)
    search_list_cmd+=" $(printf '%q' "$import_keychain")"

    cat <<EOF
 Local dev signing (scripts/build_dev_installer.sh). Run these yourself; this
 script does not change trust settings or the keychain search list.

 1. Trust the certificate for code signing (current user only; macOS asks for
    your password). Without this, codesign reports "no identity found":

   security add-trusted-cert -r trustRoot -p codeSign $(printf '%q' "$out_dir/$cert_file")

EOF
    if [[ "$in_search_list" -eq 0 ]]; then
        cat <<EOF
 2. $import_keychain is NOT in your keychain search list. tauri runs codesign
    without --keychain, so codesign only looks in keychains on that list. Add
    it, keeping the existing entries:

   $search_list_cmd

EOF
    fi
    cat <<EOF
 Then check that "security find-identity -v -p codesigning" lists $sha1, and build with:

   KAGE_MACOS_SIGNING_IDENTITY=$sha1 scripts/build_dev_installer.sh

EOF
fi
