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

This generates the keypair, writes the public key into `tauri.conf.json → plugins.updater.pubkey` (committed; read by `build.rs`), and prints the private key with instructions for adding GitHub secrets.

If you prefer to do it manually:

```bash
cargo tauri signer generate -w .tauri-signing-key
# Copy the contents of .tauri-signing-key.pub into
# tauri.conf.json → plugins.updater.pubkey (a JSON string).
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

### Faster iteration with `build_dev_installer`

A clean release rebuild on Windows takes ~12-15 minutes, dominated by the full LTO link of the main `kage` binary. When you're iterating on a bug that only repros in the bundled installer (not under `cargo tauri dev`), that round-trip becomes the limit.

The `scripts/build_dev_installer.{ps1,sh}` wrapper sets `CARGO_PROFILE_RELEASE_LTO=false` and `CARGO_PROFILE_RELEASE_CODEGEN_UNITS=16` for the duration of one build. Cargo honors those env vars per-invocation, overriding `Cargo.toml`. Build time drops to ~3 minutes; the resulting binary is a few MB larger and slightly slower at runtime, neither of which matter for diagnostic iteration. **CI ship builds and any plain `cargo tauri build` are unaffected** — the env vars are scoped to the wrapper.

Edits to `ui/**` are picked up incrementally — `tauri-codegen`'s embedded asset cache invalidates correctly off `cargo:rerun-if-changed` directives Tauri emits internally, so only the proc-macro re-runs and the rest of the binary relinks. No manual cache busting needed.

```powershell
# Windows
pwsh scripts/build_dev_installer.ps1            # fast iteration (default)
pwsh scripts/build_dev_installer.ps1 -NoBundle  # skip NSIS, ~30s saved
pwsh scripts/build_dev_installer.ps1 -Release   # use Cargo.toml defaults

# macOS / Linux
./scripts/build_dev_installer.sh
./scripts/build_dev_installer.sh --no-bundle
./scripts/build_dev_installer.sh --release
```

Use the plain `cargo tauri build` when you actually want to verify a ship-quality binary; use the wrapper otherwise.

## Cutting a stable release

> [!WARNING]
> **Before tagging your first stable v1.0.0**: the workflow always signs the *update artefacts*, so the in-app updater can verify them. OS-level code signatures are a separate matter.
> - **macOS**: signed only when the `MACOS_CERTIFICATE` secrets are set (see [macOS code signing](#macos-code-signing)). A self-signed certificate keeps TCC permissions stable across updates, but Gatekeeper still blocks the first open of a downloaded dmg. Only a Developer ID certificate plus notarization removes that.
> - **Windows**: SmartScreen will warn aggressively until the .exe is signed with an EV certificate.
>
> Both are worth resolving before a public 1.0 announcement. Neither affects the cryptographic update verification.

```bash
# Bump the version in Cargo.toml ONLY. tauri.conf.json must NOT carry a
# "version" field — if it does, Tauri uses that instead of CARGO_PKG_VERSION
# and defeats the CI version stamp, trapping the updater in an "update
# available" loop. Cargo.toml is the single source of truth.
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
| `APTABASE_KEY`                       | (Unrelated to updates.) Aptabase telemetry key — see `docs/PRIVACY.md`.                     |

Optional macOS code-signing and notarization secrets are listed under [macOS code signing → Secrets](#secrets). Without them, macOS builds are unsigned, exactly as before.

The public key is **not** a secret — it lives in `tauri.conf.json → plugins.updater.pubkey`, committed to the repo, and `build.rs` reads it from there. (There is no `TAURI_UPDATER_PUBKEY` secret; build.rs re-exports that name as a compile-time env var *from* the config field.)

## What if a build ships without a public key?

Release builds (`cargo tauri build`, i.e. `--release` profile) **fail the build** if `tauri.conf.json → plugins.updater.pubkey` is empty. This is deliberate — a release binary without a public key can't verify updates, so the updater would silently refuse every update forever. The build script panics loudly with a pointer to this doc.

Debug builds (`cargo tauri dev`, `cargo build`) tolerate a missing key so local development doesn't require anyone to configure update infrastructure.

## What ships in each release

For each channel + platform combination, the GitHub Release contains:

- The installer (`.exe` on Windows, `.app.tar.gz` on macOS, plus the `.dmg` for manual downloads).
- A `.sig` sidecar for each signed artefact.
- A single `latest.json` manifest pointing at the installers + signatures for every supported platform.

The updater plugin fetches `latest.json`, picks the platform block matching the user's machine, downloads the installer, verifies the `.sig` against the compile-time pubkey, and only then runs the installer.

## macOS code signing

### Why it matters: TCC pins permissions to the code signature

macOS stores every privacy grant (Accessibility, Input Monitoring, Screen Recording, Automation, Calendars, Microphone) together with the app's *designated requirement* (DR). That is a rule saying which code counts as "the same app". Without a signing certificate, Kage's binaries are only *linker-signed ad-hoc* (the arm64 linker does that automatically), and an ad-hoc DR is just the binary's hash:

```text
$ codesign -dvvv /Applications/Kage.app
Identifier=kage-73d16a16dab2644e
CodeDirectory v=20400 ... flags=0x20002(adhoc,linker-signed)
Info.plist=not bound   Sealed Resources=none   TeamIdentifier=not set
$ codesign -d -r- /Applications/Kage.app
# designated => cdhash H"…"
```

Every auto-update and every local rebuild changes that hash. TCC then treats the new binary as a different app, and every grant silently stops applying. System Settings still shows **Kage** switched on, because the row belongs to the old binary. `AXIsProcessTrusted()` returns `false`, hotkey capture and accessibility features fail, and users reasonably say "but I already approved it".

When every build is signed with the **same certificate**, codesign writes a certificate-based DR instead:

```text
designated => identifier "com.kage.launcher" and certificate leaf = H"<sha1 of the signing certificate>"
```

That DR is identical for every build signed with that certificate, so grants survive updates. Measured locally: rebuilding a binary changed its cdhash and left its DR byte-identical. A second self-signed certificate with the same common name did **not** satisfy the DR.

### Options

|                                         | Self-signed certificate                                                                 | Developer ID Application + notarization                              |
|-----------------------------------------|-----------------------------------------------------------------------------------------|---------------------------------------------------------------------|
| Cost                                    | Free (`scripts/macos/create_signing_cert.sh`)                                           | Apple Developer Program ($99/yr)                                     |
| TCC grants survive updates              | Yes                                                                                     | Yes (`anchor apple generic … certificate leaf[subject.OU] = <team>`) |
| Gatekeeper, first open of a downloaded dmg | Still blocked as an unidentified developer. Users follow the same `xattr -d com.apple.quarantine` / "Open Anyway" steps as today (see README). | Opens normally once notarized                                         |
| In-app updates                          | Fine. The updater downloads with reqwest, which sets no quarantine flag, so Gatekeeper never re-checks. | Fine                                                               |
| Replacing the certificate later         | New DR, so every user re-grants once                                                     | Same team id, so the DR is unchanged even when the cert is renewed    |

The workflow handles both. Start with self-signed. Moving to Developer ID later costs users one more re-grant, because the DR changes from "this certificate" to "Apple anchor + team id".

### What users will see once: the first signed update

The first signed build replaces a cdhash-pinned binary with a certificate-pinned one, so existing grants do not carry over. This is a **one-time** re-grant, and it happens again only if the signing certificate changes. Put this in the release notes of the first signed build:

> After this update macOS may ask again for Accessibility, Input Monitoring, Screen Recording and similar permissions, even though System Settings shows Kage enabled. Open **System Settings → Privacy & Security → Accessibility** (and Input Monitoring / Screen Recording), select **Kage**, remove it with **−**, then re-add or re-enable it. Or run `tccutil reset All com.kage.launcher` in Terminal, relaunch Kage and approve the prompts. From then on, updates keep the permissions.

### How CI signs (release.yml only)

In the macOS leg of `release.yml`:

1. **Import macOS signing certificate (optional)** runs `scripts/macos/ci_signing_keychain.sh import`. If `MACOS_CERTIFICATE` is empty it logs one line and exports nothing, so the build is exactly what it was before (no codesign call at all). Otherwise it:
   - creates a throwaway keychain in `$RUNNER_TEMP`, imports the `.p12` with codesign access and a partition list (no GUI prompt), and puts the keychain on the user search list;
   - for a self-signed identity only, adds an **admin trust setting limited to the `codeSign` policy**. codesign refuses an untrusted identity with `no identity found`, whether you name it or give its SHA-1. To add the setting non-interactively, the script briefly opens the `com.apple.trust-settings.admin` authorization right with the runner's passwordless sudo, then restores it;
   - writes `APPLE_SIGNING_IDENTITY=<sha1>` to `$GITHUB_ENV`, plus notarization credentials when the identity is a Developer ID and the credentials are complete.
2. **Build signed release** (`cargo tauri build`). tauri-bundler signs from the inside out: each `externalBin` sidecar (`kage-computer-control-mcp`, `kage-calendar-helper`), then the main binary, then the `.app`. Every one gets `--options runtime` and `src-tauri/macos/entitlements.plist`. With Developer ID credentials it then notarizes and staples the `.app`. Only after that does it build and sign the `.dmg`, and build the `Kage.app.tar.gz` updater artefact. So **both contain the signed app**.
3. **Verify macOS code signature** runs `scripts/macos/verify_signature.sh` and fails the release if:
   - any executable is unsigned or has a cdhash DR,
   - hardened runtime is off,
   - the app inside the tarball has a different DR,
   - the dmg is unsigned.
4. **Remove macOS signing keychain** (`always()`) deletes the keychain and removes the trust setting.

Why the import isn't left to tauri. This behaviour comes from the tauri-bundler 2.9.2 / tauri-macos-sign 2.3.4 sources that tauri-cli 2.11.2 locks:

- tauri reads `APPLE_CERTIFICATE`, `APPLE_SIGNING_IDENTITY`, `APPLE_ID` and the rest with `var_os()`, so an **empty** variable counts as set. `${{ secrets.X }}` expands to `""` for an unset secret. Putting these straight into the build step's `env:` would break every fork and secret-less build in one of three ways:
  - an empty `.p12` import,
  - `codesign -s ""`,
  - a notarization attempt with blank credentials (`APPLE_ID` + `APPLE_PASSWORD` without `APPLE_TEAM_ID` is a hard error).

  The import step therefore writes only non-empty values to `$GITHUB_ENV`.
- tauri's own `APPLE_CERTIFICATE` import only recognises Apple-issued identities. It looks for CN prefixes like `Developer ID Application:` and requires an OU team id, so a self-signed certificate fails with "failed to resolve signing identity".
- Missing notarization credentials are fine. tauri logs `skipping app notarization` and carries on.

`ci.yml` (the PR / push debug build) is deliberately **not** signed. The certificate is what TCC trusts, so anything signed with it inherits users' permissions, and that job builds unreviewed PR code. Fork PRs get no secrets anyway, and its smoke test runs the raw `target/debug/kage`, not the bundle.

### Hardened runtime

`bundle.macOS.hardenedRuntime` is `true`. That is tauri's default, now stated explicitly in `tauri.conf.json`. Notarization requires it, and nothing Kage does in-process needs an exception:

- Agent CLIs, sidecars, python/pocket-tts, osascript and pip run as separate processes, and a parent's hardened runtime doesn't apply to its children.
- WKWebView runs JavaScript out of process.
- Kage loads no third-party dylibs.

What hardened runtime does gate is access to TCC resources that need an entitlement. `src-tauri/macos/entitlements.plist` therefore declares Apple Events, audio input and **calendars**. The calendars entitlement is new in this change: without it, hardened runtime silently denies EventKit access from `kage-calendar-helper`. The full rationale is in the plist's header comment.

One side effect: lldb can't attach to signed bundles, so use `cargo tauri dev` when you need a debugger.

### Setting it up: self-signed (no Apple account)

```bash
scripts/macos/create_signing_cert.sh          # writes ~/kage-signing-cert/ (refuses to overwrite)
```

The script creates an RSA-3072 key and a 10-year self-signed certificate (`CN=Kage Self-Signed Code Signing`, `keyUsage=digitalSignature`, `extendedKeyUsage=codeSigning`). It exports a password-protected `.p12` using 3DES/SHA-1, the PKCS#12 format `security import` reads on every macOS version. It then prints the base64 and the exact commands to run. **It never runs `gh` itself.**

```bash
gh secret set MACOS_CERTIFICATE --repo nachmore/Kage < ~/kage-signing-cert/kage-codesign.p12.base64
gh secret set MACOS_CERTIFICATE_PASSWORD --repo nachmore/Kage < ~/kage-signing-cert/kage-codesign.p12.password
```

**Back up the `.p12` and its password**, for example in a password manager. Losing them means a new certificate, a new DR, and one more re-grant for every user.

### Setting it up: Developer ID + notarization

1. In Keychain Access, export your **Developer ID Application** certificate *with its private key* as a `.p12`. Then run `base64 -i DeveloperID.p12 | gh secret set MACOS_CERTIFICATE --repo nachmore/Kage` and set `MACOS_CERTIFICATE_PASSWORD`.
2. Notarization. Preferred: create an App Store Connect API key (Users and Access → Integrations) and set `APPLE_API_KEY` (key id), `APPLE_API_ISSUER` (issuer UUID) and `APPLE_API_KEY_P8` (the contents of `AuthKey_<id>.p8`). Alternative: `APPLE_ID`, `APPLE_PASSWORD` (an app-specific password) and `APPLE_TEAM_ID`.

Without notarization secrets the app is still signed with Developer ID, so TCC grants stay stable, but it is not notarized. tauri notarizes and staples only the `.app`. The `.dmg` is signed but never submitted for notarization.

### Secrets

| Secret                         | Required? | What it is                                                                                         |
|--------------------------------|-----------|----------------------------------------------------------------------------------------------------|
| `MACOS_CERTIFICATE`            | to sign   | base64 of a `.p12` holding exactly one code-signing identity (self-signed or Developer ID).         |
| `MACOS_CERTIFICATE_PASSWORD`   | to sign   | Password of that `.p12`. If the certificate is set and this is empty, the release fails instead of shipping unsigned. |
| `MACOS_SIGNING_IDENTITY`       | no        | Selects one identity, by SHA-1 or exact common name, when the `.p12` holds several.                |
| `APPLE_API_KEY`, `APPLE_API_ISSUER`, `APPLE_API_KEY_P8` | no | Notarization through an App Store Connect API key (Developer ID only).                    |
| `APPLE_ID`, `APPLE_PASSWORD`, `APPLE_TEAM_ID` | no | Notarization through Apple ID + app-specific password (Developer ID only).                         |

The certificate secrets are deliberately not named `APPLE_CERTIFICATE*`. The workflow imports the identity itself, and the value must never reach tauri's own import, which accepts only Apple-issued certificates and treats an empty variable as set.

### Local dev builds that keep their permissions

```bash
scripts/macos/create_signing_cert.sh --import-keychain ~/Library/Keychains/login.keychain-db
# then follow the printed one-time steps (trust for code signing; search list if needed)
KAGE_MACOS_SIGNING_IDENTITY=<sha1 printed by the script> scripts/build_dev_installer.sh
```

`build_dev_installer.sh` passes the identity to tauri as `APPLE_SIGNING_IDENTITY`, so the bundle is signed exactly like CI's. With `--replace`, it also re-signs the swapped binaries and the installed `.app`, so hot-swapping keeps the signature. With the variable unset, nothing changes. If you use the same certificate as CI, TCC treats your local build and the released build as the same app.

To check any build: `scripts/macos/verify_signature.sh path/to/Kage.app`.
