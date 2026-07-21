---
inclusion: always
---
1. After making changes don't forget to do a build (incremental, only do clean if absolutely neccessary) and fix any compile time issues 
2. when building, look at the full build output to find and fix all errors and warnings (and not just the last 30 lines)
3. NEVER commit changes unless the user has EXPLICITLY told you to commit. Do not commit proactively, do not commit after completing a task, do not commit when asked to "do it" or "go ahead". The ONLY trigger for a commit is the user saying words like "commit", "please commit", or "commit this". Always wait for the user to test first.
4. The kage-computer-control-mcp is a SEPARATE workspace package. If you change `computer_control_mcp/src/main.rs`, you MUST rebuild it with `cargo build --package kage-computer-control-mcp` (kill running instances first if locked). `cargo tauri dev` and `cargo check` do NOT rebuild it.
5. After editing any Rust file, run `cargo fmt` before declaring the task done. CI rejects unformatted code, so always format before handing back. `cargo fmt --check` is fine when you only need to verify.
6. After editing any JS / HTML / CSS under `ui/`, run `npm run lint:fix` (Biome) before declaring the task done. CI runs `biome ci ui --error-on-warnings`, which fails on warnings too — the same strict-by-default posture as `cargo clippy -- -D warnings`.
7. After editing any Rust file, run `cargo clippy -- -D warnings` (not just `cargo check`). CI runs clippy with `-D warnings` on BOTH macOS and Windows. `cargo check` does not catch clippy lints, and macOS-only builds won't catch issues in `#[cfg(not(target_os = "macos"))]` code or unused variables outside cfg blocks. If you can't cross-compile, at minimum ensure no warnings exist for the host platform.
