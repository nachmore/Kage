fn main() {
    // Only re-run this build script when these inputs change.
    // Without this, Cargo re-runs build.rs on every invocation,
    // which invalidates the cache and causes a full recompile.
    println!("cargo:rerun-if-changed=tauri.conf.json");
    println!("cargo:rerun-if-changed=capabilities/");
    println!("cargo:rerun-if-changed=icons/");

    tauri_build::build()
}
