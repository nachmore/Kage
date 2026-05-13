fn main() {
    // Only re-run this build script when these inputs change.
    println!("cargo:rerun-if-changed=tauri.conf.json");
    println!("cargo:rerun-if-changed=capabilities/");
    println!("cargo:rerun-if-changed=icons/");
    println!("cargo:rerun-if-changed=src/builtin_steering.md");
    println!("cargo:rerun-if-changed=pocket_tts/");
    println!("cargo:rerun-if-changed=Cargo.toml");
    println!("cargo:rerun-if-changed=src-tauri/macos/calendar-helper.swift");
    println!("cargo:rerun-if-changed=.aptabase-key");
    println!("cargo:rerun-if-env-changed=APTABASE_KEY");
    println!("cargo:rerun-if-changed=.tauri-updater-pubkey");
    println!("cargo:rerun-if-env-changed=TAURI_UPDATER_PUBKEY");

    // Make the Aptabase analytics key available to src/telemetry.rs via
    // `option_env!("APTABASE_KEY")`. Resolution order, highest priority first:
    //
    //   1. `APTABASE_KEY` env var (used by CI — set from a GitHub Actions
    //      secret so the key is never committed to the repo).
    //   2. `.aptabase-key` file at the repo root (gitignored — used for
    //      local release builds without needing to export an env var).
    //
    // If neither is set, the Aptabase plugin is never registered at
    // runtime and every `telemetry::track()` call is a cheap no-op. This
    // is the correct default for third-party forks: without their own
    // key, their users' events don't flow into anyone else's dashboard.
    //
    // The key itself is a public identifier (it appears in outbound
    // network requests from the shipped app), so this is defence against
    // accidental cross-pollination rather than protecting a secret.
    //
    // For release builds (where a missing key almost certainly means
    // someone forgot to set it up) we emit a loud cargo:warning. Debug
    // builds stay silent because running `cargo tauri dev` without a key
    // is a normal, intended workflow.
    let key_from_env = std::env::var("APTABASE_KEY").ok().filter(|s| !s.is_empty());
    let key_from_file = std::fs::read_to_string(".aptabase-key")
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty());
    let (source, aptabase_key) = match (key_from_env, key_from_file) {
        (Some(k), _) => ("APTABASE_KEY env var", k),
        (None, Some(k)) => (".aptabase-key file", k),
        (None, None) => ("", String::new()),
    };

    let is_release = std::env::var("PROFILE")
        .map(|p| p == "release")
        .unwrap_or(false);

    if aptabase_key.is_empty() {
        if is_release {
            println!(
                "cargo:warning=No Aptabase key found (neither APTABASE_KEY env var nor \
                 .aptabase-key file). This release binary will ship with telemetry \
                 disabled — no events will ever reach your dashboard. If that's \
                 intentional (local dev release, third-party fork) you can ignore \
                 this. Otherwise, copy .aptabase-key.example to .aptabase-key and \
                 paste your Aptabase app key."
            );
        }
        // Debug build with no key: stay silent. This is the common dev path.
    } else {
        println!("cargo:rustc-env=APTABASE_KEY={aptabase_key}");
        // Informational only — cargo:warning is the only channel that
        // actually surfaces to the developer, so we use it here but
        // keep the message short and non-alarming.
        if is_release {
            println!("cargo:warning=Aptabase telemetry enabled (key sourced from {source}).");
        }
    }

    // Expose update URLs from [package.metadata.update] as compile-time env vars.
    // Uses [package.metadata.update.dev] for debug builds, falling back to the
    // top-level [package.metadata.update] for release builds.
    let manifest: toml::Value = {
        let content = std::fs::read_to_string("Cargo.toml").expect("Failed to read Cargo.toml");
        content.parse().expect("Failed to parse Cargo.toml")
    };
    let update = &manifest["package"]["metadata"]["update"];
    let is_release = std::env::var("PROFILE")
        .map(|p| p == "release")
        .unwrap_or(false);
    let urls = if !is_release && update.get("dev").is_some() {
        &update["dev"]
    } else {
        update
    };

    let endpoint_stable = pluck_url(urls, "endpoint_stable", is_release);
    let endpoint_beta = pluck_url(urls, "endpoint_beta", is_release);
    let endpoint_dev = pluck_url(urls, "endpoint_dev", is_release);
    let changelog_url = pluck_url(urls, "changelog_url", is_release);

    println!("cargo:rustc-env=UPDATE_ENDPOINT_STABLE={endpoint_stable}");
    println!("cargo:rustc-env=UPDATE_ENDPOINT_BETA={endpoint_beta}");
    println!("cargo:rustc-env=UPDATE_ENDPOINT_DEV={endpoint_dev}");
    println!("cargo:rustc-env=UPDATE_CHANGELOG_URL={changelog_url}");

    // Tauri updater public key — provisioned the same way as APTABASE_KEY:
    //
    //   1. `TAURI_UPDATER_PUBKEY` env var (used by CI — set from a GitHub
    //      Actions secret so the key is never committed).
    //   2. `.tauri-updater-pubkey` file at the repo root (gitignored).
    //
    // The pubkey corresponds to the private key CI signs releases with
    // (`cargo tauri signer generate`). The plugin compares the manifest's
    // signature against this pubkey before running anything; a missing or
    // mismatched signature aborts the install. See docs/RELEASE.md.
    //
    // Absent key is fatal for release builds — an unsigned release would
    // mean the updater silently refuses every update forever. Debug builds
    // tolerate a missing key because `cargo tauri dev` is useful even
    // without update infrastructure.
    let pubkey_from_env = std::env::var("TAURI_UPDATER_PUBKEY")
        .ok()
        .filter(|s| !s.is_empty());
    let pubkey_from_file = std::fs::read_to_string(".tauri-updater-pubkey")
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty());
    let updater_pubkey = pubkey_from_env.or(pubkey_from_file).unwrap_or_default();

    if updater_pubkey.is_empty() {
        if is_release {
            panic!(
                "No Tauri updater public key found (neither TAURI_UPDATER_PUBKEY env \
                 var nor .tauri-updater-pubkey file). Release builds must ship with a \
                 public key so the updater can verify signed artifacts — without one, \
                 every update check would fail. Copy .tauri-updater-pubkey.example to \
                 .tauri-updater-pubkey and paste your public key, or set the env var."
            );
        }
        // Debug: silently permit — the runtime simply disables the updater
        // (no endpoint is configured anyway for a bare `cargo tauri dev`).
    } else {
        println!("cargo:rustc-env=TAURI_UPDATER_PUBKEY={updater_pubkey}");
    }

    // Expose UI-facing links from [package.metadata.links] as compile-time
    // env vars. Rust reads these via env!() in `commands::system::get_app_info`
    // and the frontend receives them as part of the app-info payload — so
    // no hardcoded github.com/... URLs in welcome.html, privacy.js, etc.
    // Missing entries fall back to empty strings, which the UI treats as
    // "link unavailable" and suppresses rendering.
    let links = manifest
        .get("package")
        .and_then(|p| p.get("metadata"))
        .and_then(|m| m.get("links"));
    let pluck_link = |key: &str| -> String {
        links
            .and_then(|l| l.get(key))
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string()
    };
    println!(
        "cargo:rustc-env=KAGE_LINK_REPOSITORY={}",
        pluck_link("repository")
    );
    println!("cargo:rustc-env=KAGE_LINK_ISSUES={}", pluck_link("issues"));
    println!(
        "cargo:rustc-env=KAGE_LINK_PRIVACY={}",
        pluck_link("privacy")
    );

    // Compile the macOS EventKit calendar helper. Skipped on other platforms
    // and skipped quietly if swiftc isn't on PATH — the runtime falls back to
    // the icalBuddy backend in that case.
    build_macos_calendar_helper();

    // Ensure the path Tauri's `bundle.externalBin` expects exists for every
    // target, so bundling succeeds even when we don't have a real helper
    // (non-macOS targets, macOS without swiftc, or a swiftc compile error).
    // The stub is never invoked — the consumer is `#[cfg(target_os = "macos")]`
    // and will already have fallen back to icalBuddy for the real macOS case.
    stage_externalbin_placeholder_if_missing();

    tauri_build::build()
}

/// Write a harmless placeholder at
/// `src-tauri/macos/bin/kage-calendar-helper-<triple>[.exe]` if no file
/// exists there yet. Tauri's bundler validates every `externalBin` path
/// at build time; without this, any build environment missing the real
/// binary fails to bundle. The placeholder prints a message and exits 1
/// if it ever runs, but it shouldn't — the macOS call site only invokes
/// the compiled Swift helper, and other platforms don't touch it.
fn stage_externalbin_placeholder_if_missing() {
    let triple = match std::env::var("TARGET") {
        Ok(t) if !t.is_empty() => t,
        _ => return,
    };
    let target_os = std::env::var("CARGO_CFG_TARGET_OS").unwrap_or_default();
    let bin_dir = std::path::PathBuf::from("src-tauri/macos/bin");
    let _ = std::fs::create_dir_all(&bin_dir);

    // Tauri's Windows bundler expects the .exe suffix on disk for
    // externalBin files; on macOS/Linux the filename has no extension.
    let filename = if target_os == "windows" {
        format!("kage-calendar-helper-{}.exe", triple)
    } else {
        format!("kage-calendar-helper-{}", triple)
    };
    let path = bin_dir.join(&filename);
    if path.exists() {
        return;
    }

    let content: &[u8] = if target_os == "windows" {
        b"@echo off\r\necho kage-calendar-helper is macOS-only\r\nexit /b 1\r\n"
    } else {
        b"#!/bin/sh\necho \"kage-calendar-helper is macOS-only\" >&2\nexit 1\n"
    };
    if let Err(e) = std::fs::write(&path, content) {
        println!(
            "cargo:warning=failed to stage externalBin placeholder at {}: {}",
            path.display(),
            e
        );
        return;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if target_os != "windows" {
            let _ = std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755));
        }
    }
    println!(
        "cargo:warning=staged externalBin placeholder at {} \
         (real helper not built; runtime falls back to icalBuddy)",
        path.display()
    );
}

/// Compile `src-tauri/macos/calendar-helper.swift` into
/// `target/{profile}/kage-calendar-helper` (for dev runs) and copy it to
/// `src-tauri/macos/bin/kage-calendar-helper-<target-triple>` so Tauri's
/// `bundle.externalBin` can pick it up during `cargo tauri build`.
///
/// Best-effort: prints a warning and returns without failing the build if
/// swiftc isn't available or the compile fails — the runtime gracefully
/// falls back to icalBuddy.
///
/// Both outputs are only rewritten when the Swift source is newer than
/// them. `cargo tauri dev` watches `src-tauri/` for externalBin-resolved
/// sidecars, so unconditionally `fs::copy`ing on every build would
/// trigger an endless rebuild loop.
fn build_macos_calendar_helper() {
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("macos") {
        return;
    }

    let src = std::path::PathBuf::from("src-tauri/macos/calendar-helper.swift");
    if !src.exists() {
        return;
    }

    // Primary output: target/{profile}/ so it sits next to the kage +
    // kage-computer-control-mcp binaries at runtime — the Rust side
    // resolves it relative to current_exe.parent().
    let profile = std::env::var("PROFILE").unwrap_or_else(|_| "debug".into());
    let target_dir = std::env::var("CARGO_TARGET_DIR")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|_| std::path::PathBuf::from("target"));
    let out_bin = target_dir.join(&profile).join("kage-calendar-helper");

    // Bundle-time output: `src-tauri/macos/bin/kage-calendar-helper-<triple>`.
    // Tauri's externalBin mechanism appends `-<target>` to the configured
    // path and expects the file to exist when `tauri_build::build()` runs.
    let triple = std::env::var("TARGET").unwrap_or_default();
    let bundle_bin = if !triple.is_empty() {
        let bin_dir = std::path::PathBuf::from("src-tauri/macos/bin");
        let _ = std::fs::create_dir_all(&bin_dir);
        Some(bin_dir.join(format!("kage-calendar-helper-{}", triple)))
    } else {
        None
    };

    // Fast path: both outputs already newer than the source → nothing to do.
    // This is the hot path during `cargo tauri dev`, where ANY mtime change
    // on the sidecar tells Tauri to rebuild the app.
    let out_fresh = is_newer_than(&out_bin, &src);
    let bundle_fresh = bundle_bin.as_ref().is_none_or(|p| is_newer_than(p, &src));
    if out_fresh && bundle_fresh {
        println!("cargo:rustc-env=KAGE_CALENDAR_HELPER={}", out_bin.display());
        if let Some(ref path) = bundle_bin {
            println!(
                "cargo:warning=calendar-helper up to date at {} (bundle staging: {})",
                out_bin.display(),
                path.display()
            );
        } else {
            println!(
                "cargo:warning=calendar-helper up to date at {}",
                out_bin.display()
            );
        }
        return;
    }

    let swiftc = match which_swiftc() {
        Some(p) => p,
        None => {
            println!(
                "cargo:warning=swiftc not found — skipping calendar-helper build (icalBuddy \
                 fallback will still work; install Xcode CLI tools to enable EventKit)"
            );
            return;
        }
    };

    if !out_fresh {
        let status = std::process::Command::new(&swiftc)
            .arg("-O")
            .arg("-o")
            .arg(&out_bin)
            .arg(&src)
            .status();
        match status {
            Ok(s) if s.success() => {
                println!("cargo:rustc-env=KAGE_CALENDAR_HELPER={}", out_bin.display());
                println!(
                    "cargo:warning=built calendar-helper at {}",
                    out_bin.display()
                );
            }
            Ok(s) => {
                println!(
                    "cargo:warning=swiftc exited with {} — calendar-helper not built (icalBuddy \
                     fallback will still work)",
                    s
                );
                return;
            }
            Err(e) => {
                println!("cargo:warning=failed to spawn swiftc ({e}) — calendar-helper not built");
                return;
            }
        }
    } else {
        println!("cargo:rustc-env=KAGE_CALENDAR_HELPER={}", out_bin.display());
    }

    // Mirror into the bundle-time path only when necessary.
    if let Some(ref bundle_path) = bundle_bin {
        if !bundle_fresh {
            match std::fs::copy(&out_bin, bundle_path) {
                Ok(_) => println!(
                    "cargo:warning=staged calendar-helper for bundling at {}",
                    bundle_path.display()
                ),
                Err(e) => println!(
                    "cargo:warning=failed to stage helper at {} ({}); \
                     release bundle will be missing the binary",
                    bundle_path.display(),
                    e
                ),
            }
        }
    }
}

/// `true` if `a` exists and its mtime is >= `b`'s mtime. Used to skip
/// unnecessary recompiles — the mtime bump from rewriting an output file
/// is what triggers Tauri's dev watcher into an infinite rebuild loop.
fn is_newer_than(a: &std::path::Path, b: &std::path::Path) -> bool {
    let (Ok(am), Ok(bm)) = (std::fs::metadata(a), std::fs::metadata(b)) else {
        return false;
    };
    match (am.modified(), bm.modified()) {
        (Ok(at), Ok(bt)) => at >= bt,
        _ => false,
    }
}

fn which_swiftc() -> Option<std::path::PathBuf> {
    // Try PATH via `which`, fall back to the known Xcode CLI tools location.
    let from_path = std::process::Command::new("which")
        .arg("swiftc")
        .output()
        .ok()
        .filter(|o| o.status.success())
        .and_then(|o| {
            let s = String::from_utf8_lossy(&o.stdout).trim().to_string();
            if s.is_empty() {
                None
            } else {
                Some(std::path::PathBuf::from(s))
            }
        });
    if let Some(p) = from_path {
        return Some(p);
    }
    let xcode_default = std::path::PathBuf::from("/usr/bin/swiftc");
    if xcode_default.exists() {
        Some(xcode_default)
    } else {
        None
    }
}

/// Read a URL out of `[package.metadata.update]` (or its `.dev` sibling for
/// debug builds). Fail the build for release if the value is missing, blank,
/// or still pointing at the placeholder `example.com` host — the
/// auto-updater hits these at runtime via `env!()`-embedded `&'static str`
/// constants, and a release binary that silently shipped with placeholder
/// URLs would phone home to whatever happens to live at example.com (or
/// fail at runtime far from the build site).
///
/// Debug builds tolerate the placeholder so `cargo tauri dev` works without
/// the developer wiring up a private update server.
fn pluck_url(table: &toml::Value, key: &str, is_release: bool) -> String {
    let value = table
        .get(key)
        .and_then(|v: &toml::Value| v.as_str())
        .unwrap_or("");
    if is_release {
        if value.is_empty() {
            panic!(
                "Cargo.toml [package.metadata.update].{key} is missing or empty — \
                 release builds require a real URL. Set it before building, or build \
                 in debug mode to use the [package.metadata.update.dev] fallback."
            );
        }
        if value.contains("example.com") {
            panic!(
                "Cargo.toml [package.metadata.update].{key} = {value:?} still uses the \
                 placeholder example.com host. Refusing to ship it in a release \
                 binary — the auto-updater would hit example.com at runtime. \
                 Replace with the real update server URL before building."
            );
        }
    }
    value.to_string()
}
