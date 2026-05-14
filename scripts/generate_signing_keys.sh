#!/bin/bash
# Generate Tauri updater signing keys and set up the repo.
#
# Usage: ./scripts/generate_signing_keys.sh
#
# This script:
#   1. Generates a new keypair via `cargo tauri signer generate`
#   2. Writes the public key into tauri.conf.json (plugins.updater.pubkey)
#   3. Prints the private key with instructions for GitHub secrets

set -euo pipefail

REPO_ROOT="$(cd "$(dirname "$0")/.." && pwd)"
KEYFILE="$REPO_ROOT/.tauri-signing-key"
CONF_FILE="$REPO_ROOT/tauri.conf.json"

echo "=== Tauri Updater Key Generation ==="
echo ""
echo "You'll be prompted to set a password for the private key."
echo "Remember it — you'll need it for the GitHub secret."
echo ""

# Generate the keypair
cargo tauri signer generate -w "$KEYFILE"

# The command writes private key to $KEYFILE and public key to ${KEYFILE}.pub
if [ ! -f "${KEYFILE}.pub" ]; then
    echo ""
    echo "ERROR: Expected ${KEYFILE}.pub to be created. Check tauri-cli version."
    exit 1
fi

# Decode the public key if it's base64-encoded.
# Tauri expects the plaintext format: "untrusted comment: ...\n<key>"
decode_if_base64() {
    local file="$1"
    if grep -q "^untrusted comment:" "$file"; then
        cat "$file"
    else
        base64 -d < "$file"
    fi
}

PUBKEY=$(decode_if_base64 "${KEYFILE}.pub")

# Private key: Tauri CLI expects the raw file content (base64-encoded blob)
PRIVKEY=$(cat "$KEYFILE")

# Write public key into tauri.conf.json → plugins.updater.pubkey
# The bundler expects the base64-encoded blob of the full key file
PUBKEY_JSON=$(echo "$PUBKEY" | python3 -c "import sys,json,base64; data=sys.stdin.read().strip(); print(json.dumps(base64.b64encode(data.encode()).decode()))")

# Use python to update the JSON
python3 -c "
import json, sys

with open('$CONF_FILE', 'r') as f:
    conf = json.load(f)

conf.setdefault('plugins', {}).setdefault('updater', {})['pubkey'] = $(echo "$PUBKEY_JSON")

with open('$CONF_FILE', 'w') as f:
    json.dump(conf, f, indent=2)
    f.write('\n')
"

# Clean up the temp files (private key should not linger on disk)
rm -f "$KEYFILE" "${KEYFILE}.pub"

# Write private key to .env (gitignored) for local builds
ENV_FILE="$REPO_ROOT/.env"
# Remove any existing TAURI_SIGNING entries
if [ -f "$ENV_FILE" ]; then
    grep -v "^TAURI_SIGNING_PRIVATE_KEY" "$ENV_FILE" > "$ENV_FILE.tmp" || true
    mv "$ENV_FILE.tmp" "$ENV_FILE"
fi
echo "TAURI_SIGNING_PRIVATE_KEY=$PRIVKEY" >> "$ENV_FILE"
echo "TAURI_SIGNING_PRIVATE_KEY_PASSWORD=" >> "$ENV_FILE"

echo ""
echo "✓ Public key written to tauri.conf.json (plugins.updater.pubkey)"
echo "✓ Private key written to .env (gitignored, for local builds)"
echo ""
echo "==========================================="
echo "  LOCAL BUILDS"
echo "==========================================="
echo ""
echo "Before running cargo tauri build, source the .env file:"
echo ""
echo "  macOS/Linux:  source .env && cargo tauri build --debug"
echo "  Any platform: npx dotenv-cli -- cargo tauri build --debug"
echo ""
echo "==========================================="
echo "  GITHUB CI: Add Repository Secret"
echo "==========================================="
echo ""
echo "Go to: https://github.com/nachmore/Kage/settings/secrets/actions"
echo ""
echo "─────────────────────────────────────────"
echo "Name:  TAURI_SIGNING_PRIVATE_KEY"
echo "Value:"
echo "$PRIVKEY"
echo ""
echo "─────────────────────────────────────────"
echo "Name:  TAURI_SIGNING_PRIVATE_KEY_PASSWORD"
echo "Value: (the password you just entered)"
echo "       Skip this secret if you used an empty password."
echo ""
echo "==========================================="
echo ""
echo "⚠️  IMPORTANT:"
echo "  • Commit tauri.conf.json — the public key is not secret."
echo "  • Store the private key in a password manager."
echo "  • If you lose it, you cannot push updates to existing users."
