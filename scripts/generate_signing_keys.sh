#!/bin/bash
# Generate Tauri updater signing keys and set up the repo.
#
# Usage: ./scripts/generate_signing_keys.sh
#
# This script:
#   1. Generates a new keypair via `cargo tauri signer generate`
#   2. Saves the public key to .tauri-updater-pubkey (read by build.rs)
#   3. Prints instructions for adding secrets to GitHub

set -euo pipefail

REPO_ROOT="$(cd "$(dirname "$0")/.." && pwd)"
KEYFILE="$REPO_ROOT/.tauri-signing-key"
PUBKEY_FILE="$REPO_ROOT/.tauri-updater-pubkey"

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

# cargo tauri signer generate writes files as base64-encoded blobs.
# Tauri expects the decoded plaintext format:
#   untrusted comment: ...
#   <key data>
# Detect and decode if needed.

decode_if_base64() {
    local file="$1"
    if grep -q "^untrusted comment:" "$file"; then
        # Already plaintext
        cat "$file"
    else
        # Base64-encoded — decode it
        base64 -d < "$file"
    fi
}

PUBKEY=$(decode_if_base64 "${KEYFILE}.pub")

# Private key: Tauri CLI expects the raw file content (base64-encoded blob)
PRIVKEY=$(cat "$KEYFILE")

# Write decoded public key to the repo location build.rs reads
echo "$PUBKEY" > "$PUBKEY_FILE"

# Clean up the temp files (private key should not linger on disk)
rm -f "$KEYFILE" "${KEYFILE}.pub"

echo ""
echo "✓ Public key written to: .tauri-updater-pubkey"
echo ""
echo "==========================================="
echo "  NEXT STEPS: Add GitHub Repository Secrets"
echo "==========================================="
echo ""
echo "Go to: https://github.com/nachmore/Kage/settings/secrets/actions"
echo ""
echo "Add these three secrets:"
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
echo "─────────────────────────────────────────"
echo "Name:  TAURI_UPDATER_PUBKEY"
echo "Value:"
echo "$PUBKEY"
echo ""
echo "==========================================="
echo ""
echo "⚠️  IMPORTANT:"
echo "  • Store the private key in a password manager."
echo "  • If you lose it, you cannot push updates to existing users."
echo "  • The .tauri-updater-pubkey file is gitignored — it stays local."
echo "  • build.rs embeds the public key into every binary at compile time."
