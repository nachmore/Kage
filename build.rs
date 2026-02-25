fn main() {
    // Only re-run this build script when these inputs change.
    println!("cargo:rerun-if-changed=tauri.conf.json");
    println!("cargo:rerun-if-changed=capabilities/");
    println!("cargo:rerun-if-changed=icons/");
    println!("cargo:rerun-if-changed=src/builtin_steering.md");
    println!("cargo:rerun-if-changed=Cargo.toml");

    // Expose update URLs from [package.metadata.update] as compile-time env vars
    let manifest: toml::Value = {
        let content = std::fs::read_to_string("Cargo.toml").expect("Failed to read Cargo.toml");
        content.parse().expect("Failed to parse Cargo.toml")
    };
    let update = &manifest["package"]["metadata"]["update"];
    println!("cargo:rustc-env=UPDATE_VERSION_URL={}", update["version_url"].as_str().unwrap_or(""));
    println!("cargo:rustc-env=UPDATE_INSTALLER_URL={}", update["installer_url"].as_str().unwrap_or(""));
    println!("cargo:rustc-env=UPDATE_CHANGELOG_URL={}", update["changelog_url"].as_str().unwrap_or(""));

    tauri_build::build()
}
