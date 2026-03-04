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
    let is_release = std::env::var("PROFILE").map(|p| p == "release").unwrap_or(false);
    let urls = if !is_release && update.get("dev").is_some() {
        &update["dev"]
    } else {
        update
    };
    println!("cargo:rustc-env=UPDATE_VERSION_URL={}", urls.get("version_url").and_then(|v: &toml::Value| v.as_str()).unwrap_or(""));
    println!("cargo:rustc-env=UPDATE_INSTALLER_URL={}", urls.get("installer_url").and_then(|v: &toml::Value| v.as_str()).unwrap_or(""));
    println!("cargo:rustc-env=UPDATE_CHANGELOG_URL={}", urls.get("changelog_url").and_then(|v: &toml::Value| v.as_str()).unwrap_or(""));

    tauri_build::build()
}
