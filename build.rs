fn main() {
    // Announce this build.rs run. Cargo runs build.rs exactly once per
    // invocation that needs it, so this gives us a tag in CI logs that
    // lines up 1:1 with each compile pass. (Scripts that invoke cargo
    // for a specific purpose — dev_server.py, CI steps — print their
    // own tags; we deliberately don't track a reason env var here
    // because `rerun-if-env-changed` on it forced a build.rs re-run on
    // every sidecar/main alternation.)
    let profile = std::env::var("PROFILE").unwrap_or_else(|_| "?".into());
    let target = std::env::var("TARGET").unwrap_or_else(|_| "?".into());
    let pkg_version = std::env::var("CARGO_PKG_VERSION").unwrap_or_else(|_| "?".into());
    println!("cargo:warning=[build.rs] kage {pkg_version} profile={profile} target={target}");

    // Only re-run this build script when these inputs change.
    println!("cargo:rerun-if-changed=tauri.conf.json");
    println!("cargo:rerun-if-changed=tauri.macos.conf.json");
    println!("cargo:rerun-if-changed=capabilities/");
    println!("cargo:rerun-if-changed=icons/");
    println!("cargo:rerun-if-changed=src/builtin_steering.md");
    println!("cargo:rerun-if-changed=locales/");
    println!("cargo:rerun-if-changed=pocket_tts/");
    println!("cargo:rerun-if-changed=Cargo.toml");
    println!("cargo:rerun-if-changed=.aptabase-key");
    println!("cargo:rerun-if-env-changed=APTABASE_KEY");
    println!("cargo:rerun-if-env-changed=KAGE_LOCAL_DEV_BUILD");
    // The sidecars are provisioned by this script (see provision_sidecar),
    // so their sources — and those of kage-core, their shared path
    // dependency — are inputs of ours. Without these, an edit to a sidecar
    // would leave a stale staged binary in src-tauri/binaries/ that
    // tauri-build happily bundles.
    println!("cargo:rerun-if-changed=computer_control_mcp/src");
    println!("cargo:rerun-if-changed=computer_control_mcp/Cargo.toml");
    println!("cargo:rerun-if-changed=kage-calendar-helper/src");
    println!("cargo:rerun-if-changed=kage-calendar-helper/Cargo.toml");
    println!("cargo:rerun-if-changed=kage-core/src");
    println!("cargo:rerun-if-changed=kage-core/Cargo.toml");
    println!("cargo:rerun-if-changed=Cargo.lock");

    // Surface the local-dev-build marker to the binary as a compile-time
    // env. `option_env!("KAGE_LOCAL_DEV_BUILD")` returns Some(_) only when
    // build.rs ran with the var set — i.e. when one of our dev-installer
    // scripts kicked off the build. CI's release.yml leaves it unset, so
    // beta/stable binaries don't pick up trace-level logging from this
    // path. See `init_logger` in `src/logger.rs`.
    if std::env::var("KAGE_LOCAL_DEV_BUILD")
        .ok()
        .filter(|s| !s.is_empty())
        .is_some()
    {
        println!("cargo:rustc-env=KAGE_LOCAL_DEV_BUILD=1");
    }

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
    // Both debug and release builds read the same table now: the dev channel
    // points at a real GitHub Release that CI auto-publishes on every push to
    // main, so `cargo tauri dev` on the dev channel can hit the same
    // endpoints real users do without any local-server scaffolding. The build
    // is signed (or unsigned) the same way either way, so the network
    // endpoint isn't the trust boundary.
    let manifest: toml::Value = {
        let content = std::fs::read_to_string("Cargo.toml").expect("Failed to read Cargo.toml");
        content.parse().expect("Failed to parse Cargo.toml")
    };
    let urls = &manifest["package"]["metadata"]["update"];
    let is_release = std::env::var("PROFILE")
        .map(|p| p == "release")
        .unwrap_or(false);

    let endpoint_stable = pluck_url(urls, "endpoint_stable", is_release);
    let endpoint_beta = pluck_url(urls, "endpoint_beta", is_release);
    let endpoint_dev = pluck_url(urls, "endpoint_dev", is_release);
    let changelog_url = pluck_url(urls, "changelog_url", is_release);

    println!("cargo:rustc-env=UPDATE_ENDPOINT_STABLE={endpoint_stable}");
    println!("cargo:rustc-env=UPDATE_ENDPOINT_BETA={endpoint_beta}");
    println!("cargo:rustc-env=UPDATE_ENDPOINT_DEV={endpoint_dev}");
    println!("cargo:rustc-env=UPDATE_CHANGELOG_URL={changelog_url}");

    // Tauri updater public key — read from tauri.conf.json's
    // plugins.updater.pubkey field (the single source of truth).
    // The plugin compares the manifest's signature against this pubkey
    // before running anything; a missing or mismatched signature aborts
    // the install. See docs/RELEASE.md.
    //
    // Absent key is fatal for release builds — an unsigned release would
    // mean the updater silently refuses every update forever. Debug builds
    // tolerate a missing key because `cargo tauri dev` is useful even
    // without update infrastructure.
    let updater_pubkey = {
        let conf_str =
            std::fs::read_to_string("tauri.conf.json").expect("failed to read tauri.conf.json");
        let conf: serde_json::Value =
            serde_json::from_str(&conf_str).expect("failed to parse tauri.conf.json");
        conf["plugins"]["updater"]["pubkey"]
            .as_str()
            .unwrap_or("")
            .to_string()
    };

    if updater_pubkey.is_empty() {
        if is_release {
            panic!(
                "No Tauri updater public key found in tauri.conf.json → plugins.updater.pubkey. \
                 Release builds must ship with a public key so the updater can verify signed \
                 artifacts. Run ./scripts/generate_signing_keys.sh to set it up."
            );
        }
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

    // Embed a ComCtl32-v6 manifest into TEST executables (MSVC only).
    // tauri-build embeds one into kage.exe, but test binaries link the
    // same tauri/rfd code — which imports TaskDialogIndirect, a
    // v6-only export — and without a manifest they die at load with
    // STATUS_ENTRYPOINT_NOT_FOUND before any test runs.
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows")
        && std::env::var("CARGO_CFG_TARGET_ENV").as_deref() == Ok("msvc")
    {
        let manifest = std::env::current_dir()
            .expect("cwd")
            .join("tests")
            .join("windows-test.manifest");
        println!("cargo:rerun-if-changed=tests/windows-test.manifest");
        println!("cargo:rustc-link-arg-tests=/MANIFEST:EMBED");
        println!(
            "cargo:rustc-link-arg-tests=/MANIFESTINPUT:{}",
            manifest.display()
        );
    }

    // Build + stage the sidecars. Never placeholders: every build path
    // (bare `cargo build`, `cargo tauri dev`, CI, run_bundled_dev.sh)
    // self-provisions genuine binaries, so nothing fake can be bundled
    // or clobber a real sidecar. The MCP sidecar ships on every platform
    // (tauri.conf.json externalBin); the calendar helper is macOS-only
    // (tauri.macos.conf.json overlay), so other targets skip it entirely.
    provision_sidecar("kage-computer-control-mcp");
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("macos") {
        provision_sidecar("kage-calendar-helper");
    }

    tauri_build::build()
}

/// Build the named workspace package and stage its binary at
/// `src-tauri/binaries/<name>-<triple>[.exe]` — the path
/// `tauri_build::build()` validates and copies into `target/<profile>/`
/// on EVERY cargo build of this crate (including plain `cargo build` /
/// `cargo check` / `cargo test`, which never run the Tauri CLI's
/// `beforeBuildCommand` hook).
///
/// The nested cargo invocation MUST use a separate `--target-dir`: cargo
/// holds a lock on the parent's target dir for the duration of this build
/// script, so a nested build into the same tree deadlocks. The sidecars'
/// dep graphs are tiny (kage-core, no Tauri), so the separate tree costs
/// seconds and megabytes. Cargo itself is the freshness checker — a warm
/// no-op rebuild is sub-second, and this function only runs when one of
/// the `rerun-if-changed` inputs above changed.
///
/// A failed sidecar build fails the main build. That's deliberate: the
/// alternative is silently bundling a stale or missing binary.
fn provision_sidecar(package: &str) {
    let triple = match std::env::var("TARGET") {
        Ok(t) if !t.is_empty() => t,
        _ => return,
    };
    let host = std::env::var("HOST").unwrap_or_default();
    let profile = std::env::var("PROFILE").unwrap_or_else(|_| "debug".into());
    let target_os = std::env::var("CARGO_CFG_TARGET_OS").unwrap_or_default();
    // CARGO points at the exact cargo binary driving this build — using it
    // (instead of whatever `cargo` is on PATH) keeps toolchain selection
    // consistent between the parent and nested builds.
    let cargo = std::env::var("CARGO").unwrap_or_else(|_| "cargo".into());

    let base_target_dir = std::env::var("CARGO_TARGET_DIR")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|_| std::path::PathBuf::from("target"));
    let sidecar_target_dir = base_target_dir.join("sidecar-build");

    // Only pass --target when actually cross-compiling — mirrors the rule
    // the old build_mcp.py used (d57f6c2): for host builds the plain
    // layout (`<dir>/<profile>/`) is where the binary lands; with --target
    // it moves under `<dir>/<triple>/<profile>/`.
    let cross = triple != host;
    let mut cmd = std::process::Command::new(&cargo);
    cmd.args(["build", "--package", package])
        .arg("--target-dir")
        .arg(&sidecar_target_dir)
        // Nested cargo's stdout is muted so its output can't be parsed as
        // `cargo:` directives from THIS build script. Diagnostics (compile
        // errors, progress) go to stderr, which cargo surfaces whenever
        // the build script fails.
        .stdout(std::process::Stdio::null());
    if cross {
        cmd.args(["--target", &triple]);
    }
    if profile == "release" {
        cmd.arg("--release");
    }

    let status = cmd
        .status()
        .unwrap_or_else(|e| panic!("failed to spawn `{cargo} build --package {package}`: {e}"));
    if !status.success() {
        panic!(
            "{package} failed to build ({status}). The main build cannot \
             continue without a real sidecar — fix the sidecar compile error \
             above (its sources live in {package}/ and kage-core/)."
        );
    }

    let suffix = if target_os == "windows" { ".exe" } else { "" };
    let mut built = sidecar_target_dir.clone();
    if cross {
        built.push(&triple);
    }
    let built = built.join(&profile).join(format!("{package}{suffix}"));

    let staged_dir = std::path::PathBuf::from("src-tauri/binaries");
    let staged = staged_dir.join(format!("{package}-{triple}{suffix}"));

    // Only copy when the built binary is fresher than the staged one —
    // rewriting the staged file bumps its mtime, which re-triggers both
    // tauri-build's rerun-if-changed on it and the `cargo tauri dev`
    // watcher (unconditional copies put the watcher in an endless rebuild
    // loop). The size check guards against a stale non-binary at the
    // staged path (e.g. a placeholder written by an older checkout of
    // this script): mtime alone would call it "fresh" forever, because
    // a warm no-op sidecar build never touches the built binary's mtime.
    let same_size = match (std::fs::metadata(&staged), std::fs::metadata(&built)) {
        (Ok(s), Ok(b)) => s.len() == b.len(),
        _ => false,
    };
    if same_size && is_newer_than(&staged, &built) {
        return;
    }
    let _ = std::fs::create_dir_all(&staged_dir);
    if let Err(e) = std::fs::copy(&built, &staged) {
        panic!(
            "failed to stage sidecar at {}: {e}. If the file is locked by a \
             running instance, kill it first (Windows: `Get-Process -Name \
             {package} | Stop-Process -Force`; macOS/Linux: `pkill -f {package}`).",
            staged.display()
        );
    }
    println!(
        "cargo:warning=staged sidecar at {} (from {})",
        staged.display(),
        built.display()
    );
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
