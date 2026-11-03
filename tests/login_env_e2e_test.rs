//! Manual end-to-end check for `os::login_env`: in a Finder/Dock-like
//! environment, user tools that only the login shell's PATH knows about
//! are invisible to plain spawns but resolve through `login_env::apply` —
//! both for a child looking up a name (the agent starting an MCP server
//! from mcp.json) and for Kage resolving a bare program itself (a
//! `spawn_command` of `kiro-cli acp`).
//!
//! Ignored by default: it depends on the machine's tools and rc files.
//! Run it with launchd's minimal environment and no TTY:
//!
//! ```sh
//! cargo test --test login_env_e2e_test --no-run   # prints the test binary path
//! env -i HOME="$HOME" USER="$USER" SHELL="$SHELL" \
//!     PATH=/usr/bin:/bin:/usr/sbin:/sbin \
//!     KAGE_E2E_TOOLS=builder-mcp,kiro-cli KAGE_E2E_BARE=rustc \
//!     target/debug/deps/login_env_e2e_test-<hash> --ignored --nocapture \
//!     < /dev/null 2>&1 | cat
//! ```
//!
//! `KAGE_E2E_TOOLS` are only looked up (`command -v`), never run.
//! `KAGE_E2E_BARE` *is* run with `--version`, so pick something harmless
//! that lives only on the login PATH (default: `rustc` from ~/.cargo/bin).

#![cfg(unix)]

use std::process::{Command, Output};

const LAUNCHD_PATH: &str = "/usr/bin:/bin:/usr/sbin:/sbin";

fn lookup(tool: &str, through_login_env: bool) -> Output {
    let mut cmd = Command::new("/bin/sh");
    cmd.args(["-c", "command -v \"$0\"", tool]);
    if through_login_env {
        // Exactly what the agent spawn does in acp_client/transport.rs.
        kage::os::login_env::apply(&mut cmd);
        kage::os::configure_process_spawn(&mut cmd);
    }
    cmd.output().expect("spawn /bin/sh")
}

#[test]
#[ignore = "manual: needs a Finder-like env (see module docs) and tools only on the login PATH"]
fn finder_like_env_resolves_login_only_tools() {
    assert_eq!(
        std::env::var("PATH").as_deref(),
        Ok(LAUNCHD_PATH),
        "run this under `env -i ... PATH={LAUNCHD_PATH}` (see module docs)"
    );

    let overlay = kage::os::login_env::overlay(&[]);
    let path = overlay
        .iter()
        .find(|(k, _)| k == "PATH")
        .map(|(_, v)| v.as_str())
        .expect("login env produced a PATH");
    // Names only: values can be credentials.
    let names: Vec<&str> = overlay
        .iter()
        .map(|(k, _)| k.as_str())
        .filter(|k| *k != "PATH")
        .collect();
    println!("login-env PATH = {path}");
    println!("login-env also imports: {}", names.join(", "));

    let tools = std::env::var("KAGE_E2E_TOOLS").unwrap_or_else(|_| "builder-mcp,kiro-cli".into());
    for tool in tools.split(',').filter(|t| !t.is_empty()) {
        let before = lookup(tool, false);
        assert!(
            !before.status.success(),
            "`{tool}` is already on {LAUNCHD_PATH}; pick a tool that only the login shell knows"
        );
        println!("plain spawn:          `{tool}` -> not found");

        let after = lookup(tool, true);
        assert!(
            after.status.success(),
            "`{tool}` not found even through login_env"
        );
        println!(
            "through login_env:    `{tool}` -> {}",
            String::from_utf8_lossy(&after.stdout).trim()
        );
    }

    let bare = std::env::var("KAGE_E2E_BARE").unwrap_or_else(|_| "rustc".into());
    let plain = Command::new(&bare).arg("--version").output();
    assert!(
        plain.is_err(),
        "`{bare}` resolved without login_env; pick one only the login PATH has"
    );
    println!(
        "plain Command::new(\"{bare}\"):        {}",
        plain.unwrap_err()
    );

    let mut cmd = Command::new(&bare);
    cmd.arg("--version");
    kage::os::login_env::apply(&mut cmd);
    kage::os::configure_process_spawn(&mut cmd);
    let out = cmd
        .output()
        .expect("bare program resolves via the overlaid PATH");
    assert!(out.status.success());
    println!(
        "Command::new(\"{bare}\") + login_env:  {}",
        String::from_utf8_lossy(&out.stdout).trim()
    );
}
