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
//! Each request is a job Kage owns ([`jobs`]): one tool call waits for it
//! at most a minute and a half, then tells the agent "pending" with the
//! job's id, and the agent's `get_outside_sandbox_result` calls wait on
//! it the same way — so no agent's own limit on a tool call can cut off
//! the user's answer or the run.
//!
//! - [`rules`]: when a request may run without asking (hidden characters,
//!   simple-command rules, allow-all).
//! - [`guard`]: the credential guard no rule or switch gets past.
//! - [`tracker`]: the agent's in-flight `run_outside_sandbox` tool calls,
//!   to match a request to its session.
//! - [`approvals`]: prompts waiting for the user.
//! - [`jobs`]: the requests Kage owns, and their results.
//! - [`server`]: the bridge listener and the request flow.
//! - [`commands`]: the Tauri commands the prompt and Settings use.
//!
//! See docs/SECURITY_MODEL.md § "Commands an agent runs outside its
//! sandbox".

pub mod approvals;
pub mod commands;
pub mod guard;
pub mod jobs;
pub mod rules;
pub mod server;
pub mod tracker;

use crate::lock_ext::LockExt;
use std::sync::Mutex;

/// The sidecar's tool name.
pub const TOOL_NAME: &str = "run_outside_sandbox";

/// The sidecar's tool that waits on a job `run_outside_sandbox` answered
/// "pending" for. It only reads the job.
pub const RESULT_TOOL_NAME: &str = "get_outside_sandbox_result";

/// Whether an ACP tool call title names exactly Kage's own
/// `run_outside_sandbox` tool: `Running: @kage-computer-control/
/// run_outside_sandbox` (kiro-cli) or `mcp__kage-computer-control__
/// run_outside_sandbox` (Claude Code). Same server name Kage registers in
/// the agent's `mcp.json` (`mcp_registration::MCP_SERVER_KEY`), same tool.
pub fn is_host_command_title(title: &str) -> bool {
    bridge_tool_of_title(title) == Some(TOOL_NAME)
}

/// Which of Kage's two bridge tools (`run_outside_sandbox`,
/// `get_outside_sandbox_result`) an ACP tool call title names exactly, on
/// Kage's own server; `None` for anything else.
pub fn bridge_tool_of_title(title: &str) -> Option<&'static str> {
    match crate::config::mcp_tool_from_title(title)? {
        (crate::mcp_registration::MCP_SERVER_KEY, TOOL_NAME) => Some(TOOL_NAME),
        (crate::mcp_registration::MCP_SERVER_KEY, RESULT_TOOL_NAME) => Some(RESULT_TOOL_NAME),
        _ => None,
    }
}

/// Whether an ACP tool kind is what agents report for an MCP tool: absent
/// or `other` (the same rule as `classify_mcp_server` in the permission
/// handler). A built-in tool (above all `execute`, whose kiro-cli title is
/// the command itself) is never ours, whatever its title says.
fn is_mcp_kind(kind: Option<&str>) -> bool {
    matches!(kind.filter(|k| !k.is_empty()), None | Some("other"))
}

/// Feed one `tool_call` / `tool_call_update` to the [`tracker`].
/// `first_title` / `first_kind` are what the call was first seen with.
/// Returns the call's id when this update ended one of our calls
/// (`completed`, `failed`, `cancelled`), for
/// [`HostCommands::tool_call_ended`].
pub fn observe_tool_call(
    session_id: Option<&str>,
    update: &serde_json::Value,
    first_title: Option<&str>,
    first_kind: Option<&str>,
) -> Option<String> {
    let call_id = update.get("toolCallId").and_then(|v| v.as_str())?;
    let tracker = tracker::tracker();
    let kind = update
        .get("kind")
        .and_then(|k| k.as_str())
        .filter(|k| !k.is_empty())
        .or(first_kind);
    if !is_mcp_kind(kind) {
        // Not an MCP call after all (a shell command titled like ours).
        tracker.forget(call_id);
        return None;
    }
    let ours = first_title.is_some_and(is_host_command_title)
        || update
            .get("title")
            .and_then(|t| t.as_str())
            .is_some_and(is_host_command_title);
    if !ours && !tracker.knows(call_id) {
        return None;
    }
    // Command and working directory from the same `rawInput`, so the
    // match is against what this call asked for as a whole.
    let input = update.get("rawInput").and_then(|ri| {
        let command = ri.get("command").and_then(|c| c.as_str())?;
        let cwd = ri.get("cwd").and_then(|c| c.as_str());
        Some(tracker::CallInput::new(command, cwd))
    });
    let status = update.get("status").and_then(|s| s.as_str());
    tracker.observe(session_id, call_id, input, status);
    tracker::is_terminal(status).then(|| call_id.to_string())
}

/// Runs one command string (what [`HostCommandOptions::shell`] holds).
pub type ShellFor = fn(&str) -> crate::commands::system::user_shell::runner::ShellInvocation;

/// How the bridge treats requests. Production uses the defaults; the
/// end-to-end tests shorten the wait and keep the user's login profile
/// out of their runs. Never read from config or reachable from a window.
#[derive(Clone, Copy)]
pub struct HostCommandOptions {
    /// How long a prompt waits for the user before the request is denied
    /// (at most [`kage_core::host_bridge::APPROVAL_TIMEOUT`], which the
    /// sidecar's own wait is sized for).
    pub approval_timeout: std::time::Duration,
    /// What runs an approved command: `None` for the user's login shell
    /// (`runner::user_shell_invocation`).
    pub shell: Option<ShellFor>,
    /// How long a request or poll waits for its job before answering
    /// "pending" (at most [`kage_core::host_bridge::LONG_POLL`], which the
    /// sidecar's own wait is sized for).
    pub long_poll: std::time::Duration,
    /// How long a finished job's result is kept for polling (at most
    /// [`kage_core::host_bridge::RESULT_RETENTION`]).
    pub result_retention: std::time::Duration,
}

impl Default for HostCommandOptions {
    fn default() -> Self {
        Self {
            approval_timeout: kage_core::host_bridge::APPROVAL_TIMEOUT,
            shell: None,
            long_poll: kage_core::host_bridge::LONG_POLL,
            result_retention: kage_core::host_bridge::RESULT_RETENTION,
        }
    }
}

/// The bridge, its jobs and their waiting prompts. Lives in
/// [`crate::state::ChildProcesses`].
#[derive(Default)]
pub struct HostCommands {
    pub approvals: approvals::Approvals,
    pub jobs: jobs::Jobs,
    bridge: Mutex<Option<server::BridgeHandle>>,
    /// Set by [`Self::stop`]: a start still in flight doesn't listen.
    stopped: std::sync::atomic::AtomicBool,
    options: HostCommandOptions,
}

impl HostCommands {
    pub fn new() -> Self {
        Self::default()
    }

    /// [`Self::new`] with other [`HostCommandOptions`] (tests).
    #[allow(dead_code)] // The end-to-end tests reach it through the library.
    pub fn with_options(mut options: HostCommandOptions) -> Self {
        use kage_core::host_bridge as hb;
        options.approval_timeout = options.approval_timeout.min(hb::APPROVAL_TIMEOUT);
        options.long_poll = options.long_poll.min(hb::LONG_POLL);
        options.result_retention = options.result_retention.min(hb::RESULT_RETENTION);
        Self {
            options,
            jobs: jobs::Jobs::with_retention(options.result_retention),
            ..Self::default()
        }
    }

    pub(crate) fn options(&self) -> HostCommandOptions {
        self.options
    }

    /// The user stopped `session_id`'s turn: its prompts are withdrawn
    /// (the agent won't read their results). A command already running
    /// goes on (the user approved it, and its card has Stop); its result
    /// stays for the agent to collect.
    pub fn cancel_session(&self, session_id: &str) {
        let n = self.approvals.cancel_session(session_id);
        if n > 0 {
            log::info!("Host bridge: withdrew {n} prompt(s) of a stopped turn");
        }
    }

    /// The agent disconnected (`_kage/agent_disconnected`): the prompts of
    /// its sessions are withdrawn. Prompts Kage couldn't match to one of
    /// its sessions (an agent outside Kage) wait on.
    pub fn agent_disconnected(&self) {
        let n = self.approvals.cancel_matched();
        if n > 0 {
            log::info!("Host bridge: withdrew {n} prompt(s) of a disconnected agent");
        }
    }

    /// Our `run_outside_sandbox` tool call `tool_call_id` ended. When the
    /// agent was given its job's id ("pending"), that's how the call ends
    /// normally and nothing changes. Otherwise the call failed or was
    /// cancelled before Kage answered: nobody can read the result, so a
    /// prompt still waiting on it is withdrawn. Returns whether one was.
    pub fn tool_call_ended(&self, tool_call_id: &str) -> bool {
        if let Some(job) = self.jobs.for_call(tool_call_id) {
            if job.id_given() {
                return false;
            }
            // Its prompt may not be up yet: it checks this before showing.
            job.abandon();
        }
        let withdrew = self.approvals.cancel_tool_call(tool_call_id);
        if withdrew {
            log::info!("Host bridge: withdrew a prompt whose tool call ended");
        }
        withdrew
    }

    /// Bind the bridge and accept on its own thread. Call off the main
    /// thread (setup spawns it). A bridge that can't start is logged and
    /// stays off; agents' calls then report that Kage isn't reachable.
    pub fn start<R: tauri::Runtime>(&self, app: tauri::AppHandle<R>) {
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

    /// Listen again after [`Self::stop`], for a shutdown that didn't
    /// happen (an update that failed to install after `graceful_shutdown`
    /// tore things down): without it, agents would hear that Kage isn't
    /// running until the user restarted it. Binds on its own thread.
    pub fn resume<R: tauri::Runtime>(self: &std::sync::Arc<Self>, app: &tauri::AppHandle<R>) {
        self.stopped
            .store(false, std::sync::atomic::Ordering::SeqCst);
        let (this, app) = (self.clone(), app.clone());
        let spawned = std::thread::Builder::new()
            .name("host-bridge-start".into())
            .spawn(move || this.start(app));
        if let Err(e) = spawned {
            log::warn!("Host bridge: couldn't restart: {e}");
        }
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
    fn both_bridge_tools_are_named_by_exact_server_and_tool() {
        for (title, tool) in [
            (
                "Running: @kage-computer-control/run_outside_sandbox",
                TOOL_NAME,
            ),
            (
                "Running: @kage-computer-control/get_outside_sandbox_result",
                RESULT_TOOL_NAME,
            ),
            (
                "mcp__kage-computer-control__get_outside_sandbox_result",
                RESULT_TOOL_NAME,
            ),
        ] {
            assert_eq!(bridge_tool_of_title(title), Some(tool), "{title}");
        }
        // Only `run_outside_sandbox` calls are tracked for matching.
        assert!(!is_host_command_title(
            "Running: @kage-computer-control/get_outside_sandbox_result"
        ));
        for other in [
            "Running: @evil-mcp/get_outside_sandbox_result",
            "Running: @kage-computer-control/get_outside_sandbox_result2",
            "Running: @kage-computer-control/run_script",
            "get_outside_sandbox_result",
        ] {
            assert_eq!(bridge_tool_of_title(other), None, "{other}");
        }
    }

    #[test]
    fn tool_calls_of_our_tool_are_tracked_by_command_and_cwd() {
        let update = serde_json::json!({
            "sessionUpdate": "tool_call",
            "toolCallId": "host-cmd-test-1",
            "title": "Running: @kage-computer-control/run_outside_sandbox",
            "status": "pending",
            "rawInput": {
                "command": "brew install host-cmd-test",
                "cwd": "/tmp/host-cmd-test",
                "reason": "r"
            },
        });
        observe_tool_call(Some("s1"), &update, None, None);
        assert!(tracker::tracker()
            .claim("brew install host-cmd-test", None)
            .is_none());
        let m = tracker::tracker()
            .claim("brew install host-cmd-test", Some("/tmp/host-cmd-test"))
            .expect("tracked");
        assert_eq!(m.session_id.as_deref(), Some("s1"));

        // Another tool with the same arguments isn't.
        let other = serde_json::json!({
            "toolCallId": "host-cmd-test-2",
            "title": "Running: @kage-computer-control/run_script",
            "rawInput": { "command": "brew install host-cmd-test-2" },
        });
        observe_tool_call(Some("s1"), &other, None, None);
        assert!(tracker::tracker()
            .claim("brew install host-cmd-test-2", None)
            .is_none());
    }

    #[test]
    fn a_shell_command_titled_like_our_tool_is_not_tracked() {
        // kiro-cli titles a shell call `Running: <command>`, so the command
        // `@kage-computer-control/run_outside_sandbox` reads like our tool;
        // its kind says otherwise.
        let title = "Running: @kage-computer-control/run_outside_sandbox";
        let shell = serde_json::json!({
            "toolCallId": "host-cmd-test-3",
            "title": title,
            "kind": "execute",
            "rawInput": { "command": "brew install host-cmd-test-3" },
        });
        observe_tool_call(Some("s1"), &shell, None, None);
        assert!(!tracker::tracker().knows("host-cmd-test-3"));

        // The kind recorded from the call's first update counts too.
        let later = serde_json::json!({
            "toolCallId": "host-cmd-test-4",
            "rawInput": { "command": "brew install host-cmd-test-4" },
        });
        observe_tool_call(Some("s1"), &later, Some(title), Some("execute"));
        assert!(!tracker::tracker().knows("host-cmd-test-4"));

        // `other` (or no kind) is an MCP call.
        let mcp = serde_json::json!({
            "toolCallId": "host-cmd-test-5",
            "title": title,
            "kind": "other",
            "rawInput": { "command": "brew install host-cmd-test-5" },
        });
        observe_tool_call(Some("s1"), &mcp, None, None);
        assert!(tracker::tracker()
            .claim("brew install host-cmd-test-5", None)
            .is_some());
    }
}
