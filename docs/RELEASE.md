# Release & Update System

Kage ships signed in-app updates through [`tauri-plugin-updater`](https://v2.tauri.app/plugin/updater/). Every installer is signed with a private key held in CI, and every running binary carries the matching public key compiled in at build time. The plugin refuses to install any artefact whose signature doesn't verify — a MITM attacker who swaps an installer download in flight still can't ship malicious code.

## Release channels

Three channels, each served from a GitHub Release tag alias:

| Channel  | Tag            | Triggered by                                   | Stability              |
|----------|----------------|------------------------------------------------|------------------------|
| `stable` | `v*.*.*`       | Human-pushed version tag                       | Curated                |
| `beta`   | `beta-latest`  | `workflow_dispatch` with channel=beta          | Staged pre-release     |
| `dev`    | `dev-latest`   | Every push to `main`                           | Bleeding edge          |

The rolling tags (`beta-latest`, `dev-latest`) are force-moved by CI on each run so `https://github.com/nachmore/Kage/releases/download/<tag>/latest.json` always points at the current release for that channel. The updater plugin fetches this URL based on the user's choice in **Settings → Updates → Update Channel**.

## One-time setup — generating the signing keypair

Done once per project. Run the helper script:

```bash
./scripts/generate_signing_keys.sh
```

This generates the keypair, writes the public key to `.tauri-updater-pubkey` (gitignored, read by `build.rs`), and prints the private key with instructions for adding GitHub secrets.

If you prefer to do it manually:

```bash
cargo tauri signer generate -w .tauri-signing-key
cp .tauri-signing-key.pub .tauri-updater-pubkey
```

Then add the secrets listed below to GitHub.

**⚠️ If you lose the private key**, every user who has already installed Kage will stop receiving updates — the new key's signatures won't verify against their embedded old public key. Back up the private key somewhere safe (1Password, an encrypted volume, etc.) the moment you generate it.

## Local release build (optional, for testing)

```bash
# Set the private key (one time, from the generate step above):
export TAURI_SIGNING_PRIVATE_KEY="<private key contents>"
export TAURI_SIGNING_PRIVATE_KEY_PASSWORD="<passphrase, or omit if empty>"

cargo tauri build
```

Output goes to `target/release/bundle/` and includes the `.sig` sidecar for each installer.

## Cutting a stable release

> [!WARNING]
> **Before tagging your first stable v1.0.0**: this workflow signs the *update artefacts* (so the in-app updater verifies them) but does NOT add OS-level code signatures. On macOS, Gatekeeper will reject the .app on first open until it's notarized with a Developer ID. On Windows, SmartScreen will warn aggressively until the .exe is signed with an EV certificate. Both are tracked separately and worth resolving before a public 1.0 announcement — they're orthogonal to the cryptographic update verification this workflow already provides.

```bash
# Bump Cargo.toml + tauri.conf.json version first (they must match).
git commit -am "Release v1.2.3"

# Push, then tag:
git push origin main
git tag v1.2.3
git push origin v1.2.3
```

The push of the `v*.*.*` tag triggers the `Release` workflow. It builds on Windows + both macOS architectures, signs with the CI private key, generates `latest.json`, and creates a GitHub Release with the tag. Users on `stable` will see the update on their next check (manual or automatic).

## Cutting a beta release

Use GitHub's **Actions → Release → Run workflow** button, pick `beta` from the channel dropdown, optionally pick a ref to build from (main by default). The workflow moves the `beta-latest` tag to that ref, overwrites the `beta-latest` release's assets, and beta-channel users see the update on their next check.

## Dev channel

Every push to `main` triggers a dev build and force-moves `dev-latest`. No human action needed. Dev-channel users see updates within 24 hours (or however often they re-check).

## Secrets required in CI

Set these as GitHub Actions repository secrets:

| Secret                               | What it is                                                                                  |
|--------------------------------------|---------------------------------------------------------------------------------------------|
| `TAURI_SIGNING_PRIVATE_KEY`          | Full private key contents — CI uses this to sign release bundles.                            |
| `TAURI_SIGNING_PRIVATE_KEY_PASSWORD` | Passphrase for the private key. Skip this secret if you used an empty password.              |
| `TAURI_UPDATER_PUBKEY`               | Public key string — baked into every binary by `build.rs` to verify updates at runtime.      |
| `APTABASE_KEY`                       | (Unrelated to updates.) Aptabase telemetry key — see `docs/PRIVACY.md`.                     |

## What if a build ships without a public key?

Release builds (`cargo tauri build`, i.e. `--release` profile) **fail the build** if neither the `TAURI_UPDATER_PUBKEY` env var nor the `.tauri-updater-pubkey` file is set. This is deliberate — a release binary without a public key can't verify updates, so the updater would silently refuse every update forever. The build script panics loudly with a pointer to this doc.

Debug builds (`cargo tauri dev`, `cargo build`) tolerate a missing key so local development doesn't require anyone to configure update infrastructure.

## What ships in each release

For each channel + platform combination, the GitHub Release contains:

- The installer (`.exe` on Windows, `.app.tar.gz` on macOS, plus the `.dmg` for manual downloads).
- A `.sig` sidecar for each signed artefact.
- A single `latest.json` manifest pointing at the installers + signatures for every supported platform.

The updater plugin fetches `latest.json`, picks the platform block matching the user's machine, downloads the installer, verifies the `.sig` against the compile-time pubkey, and only then runs the installer.
