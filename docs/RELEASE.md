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

Every push to `main` that passes the `Debug Build + Test` workflow (`ci.yml`) automatically chains into a dev release. CI's matrix-completion gate triggers an `auto-release-dev` job inside `ci.yml` itself, which dispatches `release.yml` with `channel=dev` and the CI-verified commit SHA. No human action needed.

The dev release force-moves `dev-latest` to that commit. Dev-channel users see the new build on their next check (manual or daily). The release page for `dev-latest` always shows exactly the most recent successful build — older dev assets are replaced, not accumulated.

If you ever need to ship a dev build manually (e.g. to re-run against a different ref), the **Actions → Release → Run workflow** dispatch lets you pick `dev` as the channel, same as beta.

> [!NOTE]
> The chain is implemented as a dependent job in `ci.yml` (rather than via `on: workflow_run` in a separate workflow file) because the `workflow_run` event silently failed to deliver in this repo even when isolated to its own minimal file. The current shape composes two reliable primitives: a `needs:` dependency from `auto-release-dev` onto the matrix legs, and a `gh workflow run` dispatch (which `GITHUB_TOKEN` is explicitly allowed to fire per the docs, unlike `push`/`pull_request` triggers).

## Versioning rolling channels

Stable releases ship the version that's already checked into `Cargo.toml` (which the human bumps before tagging). The dev and beta channels can't do that — they fire on every commit, so the version baked into the binary needs to be monotonically increasing per build, otherwise the updater plugin would say "I'm 0.9.0, the manifest says 0.9.0, no update" and silently skip every dev build.

The release workflow rewrites `Cargo.toml`'s version line in CI before each rolling-channel build to:

```
<major>.<minor>.<YYYYMMDDHHMM>+<channel>.<short_sha>
```

For example: `0.9.202511171430+dev.abc1234`. The pieces:

- **`<major>.<minor>`** — the first two components of whatever's already in `Cargo.toml`. If `Cargo.toml` says `0.9.0`, dev builds become `0.9.<timestamp>`. When you cut stable `v0.10.0`, dev builds become `0.10.<timestamp>` going forward.
- **`<YYYYMMDDHHMM>`** — UTC timestamp slotted into the patch position. Each new build sorts numerically above the previous one (`202511171530 > 202511171430`), and the format is human-readable in version strings: "I'm on the build from Nov 17 14:30 UTC."
- **`+<channel>.<short_sha>`** — semver build metadata. Ignored by version-comparison logic but visible in **Settings → About**, so users can tell at a glance which channel a build is from and bug reports point at a precise commit.

Comparison against stable: dev `0.9.202511171430` is numerically greater than stable `0.9.0`, so a dev user always sees their own build as the latest of the dev line. To pull dev users back onto a stable line you bump the minor — `0.10.0 > 0.9.<anything>`. That's the right behavior: dev users have explicitly opted in to leading edge until you cut a new minor.

The rewrite is ephemeral. The workflow never commits back to git.

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
