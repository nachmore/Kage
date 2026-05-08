fn main() {
    // Only re-run this build script when these inputs change.
    println!("cargo:rerun-if-changed=tauri.conf.json");
    println!("cargo:rerun-if-changed=capabilities/");
    println!("cargo:rerun-if-changed=icons/");
    println!("cargo:rerun-if-changed=src/builtin_steering.md");
    println!("cargo:rerun-if-changed=pocket_tts/");
    println!("cargo:rerun-if-changed=Cargo.toml");
    println!("cargo:rerun-if-changed=src-tauri/macos/calendar-helper.swift");

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

    let version_url = pluck_url(urls, "version_url", is_release);
    let installer_url = pluck_url(urls, "installer_url", is_release);
    let changelog_url = pluck_url(urls, "changelog_url", is_release);

    println!("cargo:rustc-env=UPDATE_VERSION_URL={version_url}");
    println!("cargo:rustc-env=UPDATE_INSTALLER_URL={installer_url}");
    println!("cargo:rustc-env=UPDATE_CHANGELOG_URL={changelog_url}");

    // Compile the macOS EventKit calendar helper. Skipped on other platforms
    // and skipped quietly if swiftc isn't on PATH — the runtime falls back to
    // the icalBuddy backend in that case.
    build_macos_calendar_helper();

    tauri_build::build()
}

/// Compile `src-tauri/macos/calendar-helper.swift` into
/// `target/{profile}/kage-calendar-helper`. Best-effort: prints a warning
/// and returns without failing the build if swiftc isn't available or the
/// compile fails — the runtime gracefully falls back to icalBuddy.
fn build_macos_calendar_helper() {
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("macos") {
        return;
    }

    let src = std::path::PathBuf::from("src-tauri/macos/calendar-helper.swift");
    if !src.exists() {
        return;
    }

    // OUT_DIR is a build-dir under target/{profile}/build/kage-*/out.
    // We deposit the binary in CARGO_TARGET_DIR/{profile}/ so it sits next
    // to the kage + kage-computer-control-mcp binaries at runtime — the
    // Rust side resolves it relative to the current_exe dir.
    let profile = std::env::var("PROFILE").unwrap_or_else(|_| "debug".into());
    let target_dir = std::env::var("CARGO_TARGET_DIR")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|_| std::path::PathBuf::from("target"));
    let out_bin = target_dir.join(&profile).join("kage-calendar-helper");

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

    let status = std::process::Command::new(&swiftc)
        .arg("-O")
        .arg("-o")
        .arg(&out_bin)
        .arg(&src)
        .status();

    match status {
        Ok(s) if s.success() => {
            println!("cargo:rustc-env=KAGE_CALENDAR_HELPER={}", out_bin.display());
            println!("cargo:warning=built calendar-helper at {}", out_bin.display());
        }
        Ok(s) => {
            println!(
                "cargo:warning=swiftc exited with {} — calendar-helper not built (icalBuddy \
                 fallback will still work)",
                s
            );
        }
        Err(e) => {
            println!(
                "cargo:warning=failed to spawn swiftc ({e}) — calendar-helper not built"
            );
        }
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
