fn main() {
    // Only re-run this build script when these inputs change.
    println!("cargo:rerun-if-changed=tauri.conf.json");
    println!("cargo:rerun-if-changed=capabilities/");
    println!("cargo:rerun-if-changed=icons/");
    println!("cargo:rerun-if-changed=src/builtin_steering.md");
    println!("cargo:rerun-if-changed=pocket_tts/");
    println!("cargo:rerun-if-changed=Cargo.toml");

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

    tauri_build::build()
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
