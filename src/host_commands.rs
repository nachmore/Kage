//! Commands an agent asks Kage to run outside its sandbox.
//!
//! The agent (Kiro CLI under a credential sandbox, …) runs shell commands in
//! an OS sandbox of its own, and some legitimate ones can't run there
//! (`brew install`: macOS refuses Homebrew's nested sandbox). Kage's MCP
//! sidecar offers the agent a `run_outside_sandbox` tool; the sidecar is
//! sandboxed with the agent, so it hands the request to Kage over the
//! host-command bridge (`kage_core::host_bridge`: an AF_UNIX socket / named
//! pipe the OS lets only this user reach). Kage runs it with the "Run it
//! here" runner (`commands::system::user_shell::runner`) once the user
//! approves it in the window that owns the asking session, or a rule they
//! made covers it, and replies with the exit code and the output's tail.
//!
//! - [`rules`]: when a request may run without asking (hidden characters,
//!   simple-command rules, allow-all).
//! - [`guard`]: the credential guard no rule or switch gets past.
//! - [`tracker`]: the agent's in-flight `run_outside_sandbox` tool calls,
//!   to match a request to its session.
//! - [`approvals`]: prompts waiting for the user.
//! - [`server`]: the bridge listener and the request flow.
//! - [`commands`]: the Tauri commands the prompt and Settings use.
//!
//! See docs/SECURITY_MODEL.md § "Commands an agent runs outside its
//! sandbox".

pub mod approvals;
pub mod commands;
pub mod guard;
pub mod rules;
pub mod server;
pub mod tracker;

use crate::lock_ext::LockExt;
use std::sync::Mutex;

/// The sidecar's tool name.
pub const TOOL_NAME: &str = "run_outside_sandbox";

/// Whether an ACP tool call title names exactly Kage's own
/// `run_outside_sandbox` tool: `Running: @kage-computer-control/
/// run_outside_sandbox` (kiro-cli) or `mcp__kage-computer-control__
/// run_outside_sandbox` (Claude Code). Same server name Kage registers in
/// the agent's `mcp.json` (`mcp_registration::MCP_SERVER_KEY`), same tool.
pub fn is_host_command_title(title: &str) -> bool {
    crate::config::mcp_tool_from_title(title)
        == Some((crate::mcp_registration::MCP_SERVER_KEY, TOOL_NAME))
}

/// Feed one `tool_call` / `tool_call_update` to the [`tracker`].
/// `first_title` is the title the call was first seen with.
pub fn observe_tool_call(
    session_id: Option<&str>,
    update: &serde_json::Value,
    first_title: Option<&str>,
) {
    let Some(call_id) = update.get("toolCallId").and_then(|v| v.as_str()) else {
        return;
    };
    let ours = first_title.is_some_and(is_host_command_title)
        || update
            .get("title")
            .and_then(|t| t.as_str())
            .is_some_and(is_host_command_title);
    let tracker = tracker::tracker();
    if !ours && !tracker.knows(call_id) {
        return;
    }
    let command = update
        .get("rawInput")
        .and_then(|ri| ri.get("command"))
        .and_then(|c| c.as_str());
    let status = update.get("status").and_then(|s| s.as_str());
    tracker.observe(session_id, call_id, command, status);
}

/// The bridge and its waiting prompts. Lives in
/// [`crate::state::ChildProcesses`].
#[derive(Default)]
pub struct HostCommands {
    pub approvals: approvals::Approvals,
    bridge: Mutex<Option<server::BridgeHandle>>,
    /// Set by [`Self::stop`]: a start still in flight doesn't listen.
    stopped: std::sync::atomic::AtomicBool,
}

impl HostCommands {
    pub fn new() -> Self {
        Self::default()
    }

    /// Bind the bridge and accept on its own thread. Call off the main
    /// thread (setup spawns it). A bridge that can't start is logged and
    /// stays off; agents' calls then report that Kage isn't reachable.
    pub fn start(&self, app: tauri::AppHandle) {
        let mut slot = self.bridge.lock_or_recover();
        if slot.is_some() || self.stopped.load(std::sync::atomic::Ordering::SeqCst) {
            return;
        }
        *slot = server::start(app);
    }

    /// Stop listening, remove the socket, and answer every waiting prompt
    /// with "Kage is quitting". Idempotent; called on every exit path
    /// (`graceful_shutdown`, `RunEvent::Exit`, the signal handler).
    pub fn stop(&self) {
        self.stopped
            .store(true, std::sync::atomic::Ordering::SeqCst);
        if let Some(mut handle) = self.bridge.lock_or_recover().take() {
            handle.stop();
        }
        self.approvals.shutdown_all();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_kages_own_tool_is_recognised() {
        assert!(is_host_command_title(
            "Running: @kage-computer-control/run_outside_sandbox"
        ));
        assert!(is_host_command_title(
            "mcp__kage-computer-control__run_outside_sandbox"
        ));
        for other in [
            "Running: @kage-computer-control/run_script",
            "Running: @evil-mcp/run_outside_sandbox",
            "Running: @kage-computer-control/run_outside_sandbox2",
            "Running: @kage-computer-control/run_outside_sandbox extra",
            "mcp__ka-computer-control__run_outside_sandbox",
            "run_outside_sandbox",
            "Running: run_outside_sandbox",
        ] {
            assert!(!is_host_command_title(other), "{other}");
        }
    }

    #[test]
    fn tool_calls_of_our_tool_are_tracked_by_command() {
        let update = serde_json::json!({
            "sessionUpdate": "tool_call",
            "toolCallId": "host-cmd-test-1",
            "title": "Running: @kage-computer-control/run_outside_sandbox",
            "status": "pending",
            "rawInput": { "command": "brew install host-cmd-test", "reason": "r" },
        });
        observe_tool_call(Some("s1"), &update, None);
        let m = tracker::tracker()
            .claim("brew install host-cmd-test")
            .expect("tracked");
        assert_eq!(m.session_id.as_deref(), Some("s1"));

        // Another tool with the same arguments isn't.
        let other = serde_json::json!({
            "toolCallId": "host-cmd-test-2",
            "title": "Running: @kage-computer-control/run_script",
            "rawInput": { "command": "brew install host-cmd-test-2" },
        });
        observe_tool_call(Some("s1"), &other, None);
        assert!(tracker::tracker()
            .claim("brew install host-cmd-test-2")
            .is_none());
    }
}
