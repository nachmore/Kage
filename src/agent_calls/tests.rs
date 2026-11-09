//! Engine tests with a fake agent, a fake clock and a fake extension
//! directory (docs/design/EXTENSION_AGENT_CALLS.md §12). Nothing here
//! touches the user's config, the usage file or the agent: the service is
//! built with in-memory parts and the config is never saved to disk.

use super::*;
use crate::config::{AcpMode, AgentConnection, ExtensionGrant};
use limits::fake::FakeClock;
use serde_json::{json, Value};
use std::collections::HashSet;
use std::sync::atomic::AtomicUsize;

const EXT: &str = "mindmap";
const OTHER: &str = "todo";

fn t0() -> DateTime<Utc> {
    DateTime::parse_from_rfc3339("2026-11-09T10:00:00Z")
        .unwrap()
        .with_timezone(&Utc)
}

// --- Fakes -------------------------------------------------------------------------

struct FakeDirectory(Mutex<HashMap<String, ManifestFacts>>);

impl ExtensionDirectory for FakeDirectory {
    fn manifest(&self, extension_id: &str) -> Option<ManifestFacts> {
        self.0.lock_or_recover().get(extension_id).cloned()
    }
}

fn facts(name: &str) -> ManifestFacts {
    ManifestFacts {
        name: name.into(),
        decl: Some(AgentBackgroundDecl {
            reason: "Organise new notes".into(),
            purposes: vec!["organize".into(), "tag".into()],
            max_prompt_chars: None,
            daily_calls: None,
        }),
        sensitive_fields: vec!["vault".into()],
    }
}

enum Script {
    Reply(String),
    Fail(String),
    /// Wait until cancelled, then return `partial`. `stream_len` is the
    /// reply size the watchdog sees meanwhile.
    Hang {
        partial: String,
        stream_len: usize,
    },
    /// Ignore `session/cancel`; return only when released.
    Stubborn,
}

#[derive(Default)]
struct FakeBackend {
    connected: AtomicBool,
    compacting: AtomicBool,
    in_flight: AtomicBool,
    low_power: AtomicBool,
    script: Mutex<VecDeque<Script>>,
    prompts: Mutex<Vec<String>>,
    cancels: Mutex<Vec<String>>,
    runs: AtomicUsize,
    cancelled: (Mutex<HashSet<String>>, Condvar),
    released: (Mutex<bool>, Condvar),
    reply_lens: Mutex<HashMap<String, usize>>,
}

impl FakeBackend {
    fn push(&self, s: Script) {
        self.script.lock_or_recover().push_back(s);
    }
    fn prompts(&self) -> Vec<String> {
        self.prompts.lock_or_recover().clone()
    }
    fn cancels(&self) -> Vec<String> {
        self.cancels.lock_or_recover().clone()
    }
    fn release(&self) {
        *self.released.0.lock_or_recover() = true;
        self.released.1.notify_all();
    }
}

impl AgentBackend for FakeBackend {
    fn connected(&self) -> bool {
        self.connected.load(Ordering::SeqCst)
    }
    fn compacting(&self) -> bool {
        self.compacting.load(Ordering::SeqCst)
    }
    fn user_prompt_in_flight(&self) -> bool {
        self.in_flight.load(Ordering::SeqCst)
    }
    fn low_power(&self) -> bool {
        self.low_power.load(Ordering::SeqCst)
    }
    fn run(&self, prompt: &str, ctl: &RunControl) -> Result<String, RunError> {
        let n = self.runs.fetch_add(1, Ordering::SeqCst) + 1;
        let sid = format!("fake-session-{n}");
        if !ctl.attach(&sid) {
            return Err(RunError::Stopped);
        }
        self.prompts.lock_or_recover().push(prompt.to_string());
        let step = self
            .script
            .lock_or_recover()
            .pop_front()
            .unwrap_or(Script::Reply("ok".into()));
        match step {
            Script::Reply(r) => Ok(r),
            Script::Fail(m) => Err(RunError::Agent(m)),
            Script::Hang {
                partial,
                stream_len,
            } => {
                self.reply_lens
                    .lock_or_recover()
                    .insert(sid.clone(), stream_len);
                let (lock, cvar) = &self.cancelled;
                let guard = lock.lock_or_recover();
                let _ = cvar
                    .wait_timeout_while(guard, std::time::Duration::from_secs(20), |c| {
                        !c.contains(&sid)
                    })
                    .unwrap();
                Ok(partial)
            }
            Script::Stubborn => {
                let (lock, cvar) = &self.released;
                let guard = lock.lock_or_recover();
                let _ = cvar
                    .wait_timeout_while(guard, std::time::Duration::from_secs(20), |r| !*r)
                    .unwrap();
                Ok("late".into())
            }
        }
    }
    fn reply_len(&self, session_id: &str) -> usize {
        self.reply_lens
            .lock_or_recover()
            .get(session_id)
            .copied()
            .unwrap_or(0)
    }
    fn cancel(&self, session_id: &str) {
        self.cancels.lock_or_recover().push(session_id.to_string());
        let (lock, cvar) = &self.cancelled;
        lock.lock_or_recover().insert(session_id.to_string());
        cvar.notify_all();
    }
}

#[derive(Default)]
struct Recorder {
    calls: Mutex<Vec<CallRecord>>,
    access: Mutex<Vec<AccessRecord>>,
    telemetry: Mutex<Vec<(String, Value)>>,
    activity: Mutex<Vec<ActivitySnapshot>>,
}

impl AgentCallHooks for Recorder {
    fn audit_call(&self, record: &CallRecord) {
        self.calls.lock_or_recover().push(record.clone());
    }
    fn audit_access(&self, record: &AccessRecord) {
        self.access.lock_or_recover().push(record.clone());
    }
    fn telemetry(&self, event: &str, props: Value) {
        self.telemetry
            .lock_or_recover()
            .push((event.to_string(), props));
    }
    fn activity(&self, snapshot: &ActivitySnapshot) {
        self.activity.lock_or_recover().push(snapshot.clone());
    }
    fn log_rows(&self, extension_id: &str, limit: usize) -> Vec<Value> {
        vec![json!({ "extension_id": extension_id, "limit": limit })]
    }
}

impl Recorder {
    fn calls(&self) -> Vec<CallRecord> {
        self.calls.lock_or_recover().clone()
    }
    fn events(&self, name: &str) -> Vec<Value> {
        self.telemetry
            .lock_or_recover()
            .iter()
            .filter(|(e, _)| e == name)
            .map(|(_, p)| p.clone())
            .collect()
    }
}

// --- Harness -------------------------------------------------------------------------

struct H {
    svc: Arc<AgentCallService>,
    backend: Arc<FakeBackend>,
    clock: Arc<FakeClock>,
    hooks: Arc<Recorder>,
    config: Arc<Mutex<Config>>,
    dir: Arc<FakeDirectory>,
}

fn connection() -> AgentConnection {
    AgentConnection {
        id: "c1".into(),
        name: "Claude Code".into(),
        preset_id: Some("claude-code".into()),
        mode: AcpMode::Local {
            spawn_command: "claude-agent-acp".into(),
        },
        sessions_directory: None,
        ollama_settings: None,
    }
}

fn consented() -> ExtensionAgentAccess {
    ExtensionAgentAccess {
        enabled: true,
        consent: Some(AgentConsent {
            agent_fingerprint: consent::fingerprint(&connection()),
            agent_label: "Claude Code".into(),
            granted_at: "2026-11-01T09:00:00Z".into(),
        }),
        allowance: AgentAllowance::Normal,
    }
}

fn base_config() -> Config {
    let mut c = Config::default();
    c.acp.connections = vec![connection()];
    c.acp.active_connection_id = "c1".into();
    for ext in [EXT, OTHER] {
        c.extension_grants.insert(
            ext.into(),
            ExtensionGrant {
                granted: vec!["storage".into(), "agent_background".into()],
                ..Default::default()
            },
        );
        c.extension_agent_access.insert(ext.into(), consented());
    }
    c
}

fn no_save(_: &Config) -> anyhow::Result<()> {
    Ok(())
}

fn harness_with(clock: FakeClock) -> H {
    let backend = Arc::new(FakeBackend::default());
    backend.connected.store(true, Ordering::SeqCst);
    let clock = Arc::new(clock);
    let config = Arc::new(Mutex::new(base_config()));
    let dir = Arc::new(FakeDirectory(Mutex::new(HashMap::from([
        (EXT.to_string(), facts("Mind Map")),
        (OTHER.to_string(), facts("Todo")),
    ]))));
    let svc = Arc::new(AgentCallService::with_parts(ServiceParts {
        config: config.clone(),
        backend: backend.clone(),
        directory: dir.clone(),
        clock: clock.clone(),
        usage_path: None,
        machine_id_path: None,
        save_config: no_save,
    }));
    let hooks = Arc::new(Recorder::default());
    svc.set_hooks(hooks.clone());
    H {
        svc,
        backend,
        clock,
        hooks,
        config,
        dir,
    }
}

fn harness() -> H {
    harness_with(FakeClock::new(t0()))
}

fn reserve_args(purpose: &str, when: Option<When>, key: Option<&str>) -> ReserveArgs {
    ReserveArgs {
        purpose: purpose.into(),
        when,
        dedupe_key: key.map(str::to_string),
    }
}

fn id(call_id: &str) -> CallIdArgs {
    CallIdArgs {
        call_id: call_id.into(),
    }
}

fn request(instructions: &str, input: Value) -> AgentCallRequest {
    AgentCallRequest {
        instructions: instructions.into(),
        input,
        ..Default::default()
    }
}

fn send_args(call_id: &str, request: AgentCallRequest) -> SendArgs {
    SendArgs {
        call_id: call_id.into(),
        request,
    }
}

impl H {
    fn reserve(&self, ext: &str) -> String {
        self.svc
            .reserve(ext, reserve_args("organize", None, None))
            .unwrap()
            .call_id
    }

    /// Reserve, give it the turn, and wait for `ready`.
    async fn turn(&self, ext: &str) -> String {
        let call_id = self.reserve(ext);
        self.svc.pump();
        self.svc.ready(ext, id(&call_id)).await.unwrap();
        call_id
    }

    /// A whole call: reserve, turn, send.
    async fn call(&self, ext: &str, req: AgentCallRequest) -> Result<AgentCallResult, AppError> {
        let call_id = self.turn(ext).await;
        self.svc.send(ext, send_args(&call_id, req)).await
    }

    /// Start `send` in the background, once the call has its turn, and
    /// wait until the fake agent has the prompt.
    async fn send_in_background(
        &self,
        ext: &str,
        req: AgentCallRequest,
    ) -> tokio::task::JoinHandle<Result<AgentCallResult, AppError>> {
        let call_id = self.turn(ext).await;
        let before = self.backend.prompts().len();
        let svc = self.svc.clone();
        let ext = ext.to_string();
        let task = tokio::spawn(async move { svc.send(&ext, send_args(&call_id, req)).await });
        let backend = self.backend.clone();
        until(move || backend.prompts().len() > before).await;
        task
    }

    /// Let the cooldowns pass.
    fn after_cooldown(&self) {
        self.clock.advance_secs(limits::EXTENSION_COOLDOWN_SECS + 1);
    }

    fn set_usage(&self, f: impl FnOnce(&mut usage::UsageBook)) {
        let mut st = self.svc.state.lock_or_recover();
        self.svc.ensure_usage(&mut st);
        f(st.usage.as_mut().unwrap());
    }

    fn charge(&self, ext: &str, calls: u32, chars: u64) {
        let now = self.clock.now();
        let day = limits::local_date(self.clock.as_ref(), now);
        let midnight = limits::next_local_midnight(self.clock.as_ref(), now);
        self.set_usage(|b| {
            for _ in 0..calls {
                b.charge(
                    ext,
                    day,
                    AgentCallOutcome::Ok,
                    chars / u64::from(calls.max(1)),
                    now,
                    midnight,
                );
            }
        });
    }
}

impl AgentCallService {
    /// Ids of the running calls.
    fn activity_ids(&self) -> Vec<String> {
        self.state
            .lock_or_recover()
            .calls
            .iter()
            .filter(|(_, c)| matches!(c.phase, Phase::Running(_)))
            .map(|(id, _)| id.clone())
            .collect()
    }
}

fn hang() -> Script {
    Script::Hang {
        partial: String::new(),
        stream_len: 0,
    }
}

async fn until(f: impl Fn() -> bool) {
    for _ in 0..500 {
        if f() {
            return;
        }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
    panic!("condition not met in time");
}

fn key(e: &AppError) -> &str {
    e.key.strip_prefix("errors.agent_call.").unwrap_or(&e.key)
}

fn kind(e: &AppError) -> String {
    serde_json::to_value(&e.kind)
        .unwrap()
        .as_str()
        .unwrap()
        .to_string()
}

// --- The life of a call ---------------------------------------------------------------

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_whole_call_returns_the_validated_reply_and_records_it() {
    let h = harness();
    let r = h
        .svc
        .reserve(EXT, reserve_args("organize", None, Some("organize")))
        .unwrap();
    assert_eq!(r.position, 1);
    assert_eq!(r.call_id.len(), 36, "a uuid");
    h.svc.pump();
    let turn = h.svc.ready(EXT, id(&r.call_id)).await.unwrap();
    assert_eq!(turn.send_by, "2026-11-09T10:00:10+00:00");

    h.backend.push(Script::Reply(
        "```json\n{\"items\": [{\"ref\": \"n1\"}]}\n```".into(),
    ));
    let mut req = request(
        "Organise these notes.",
        json!({ "notes": [{ "ref": "n1" }] }),
    );
    req.label = Some("Organize 1 note".into());
    req.schema = Some(json!({ "type": "object", "required": ["items"] }));
    let out = h.svc.send(EXT, send_args(&r.call_id, req)).await.unwrap();

    assert_eq!(out.call_id, r.call_id);
    assert_eq!(out.json, Some(json!({ "items": [{ "ref": "n1" }] })));
    let prompts = h.backend.prompts();
    assert_eq!(prompts.len(), 1);
    let sent = &prompts[0];
    assert!(sent.starts_with("[Kage background task"));
    assert!(sent.contains("Task from the extension \"Mind Map\""));
    assert!(sent.contains("Organise these notes."));
    assert!(
        !sent.contains("Organize 1 note"),
        "the label never reaches the agent"
    );
    // Counted in characters, after framing.
    assert_eq!(out.prompt_chars as usize, sent.chars().count());
    assert_eq!(out.response_chars as usize, out.text.chars().count());
    assert_eq!(out.redactions, 0);

    let audit = h.hooks.calls();
    assert_eq!(audit.len(), 1);
    let a = &audit[0];
    assert_eq!(a.outcome, AgentCallOutcome::Ok);
    assert_eq!(a.agent, "Claude Code");
    assert_eq!(a.purpose, "organize");
    assert_eq!(a.label.as_deref(), Some("Organize 1 note"));
    assert_eq!(a.prompt_chars, out.prompt_chars);

    let status = h.svc.status(EXT);
    assert_eq!(status.usage.calls_today, 1);
    assert_eq!(
        status.usage.chars_today,
        u64::from(out.prompt_chars + out.response_chars)
    );
    assert_eq!(status.usage.calls_limit, 40);
    assert!(!status.running);
    assert_eq!(status.waiting, 0);

    let detail = h.svc.request_detail(EXT, &r.call_id).unwrap();
    assert_eq!(&detail.prompt, sent);
    assert_eq!(detail.reply, out.text);
    assert!(h.svc.access_panel(EXT).viewable.contains(&r.call_id));

    // The activity event saw it run.
    let seen = h.hooks.activity.lock_or_recover().clone();
    assert!(seen.iter().any(|s| s.running.len() == 1
        && s.running[0].name == "Mind Map"
        && s.running[0].purpose == "organize"));
    assert_eq!(h.svc.activity(), ActivitySnapshot::default());

    // The id is finished now.
    let e = h.svc.ready(EXT, id(&r.call_id)).await.unwrap_err();
    assert_eq!(key(&e), "unknown_call");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn without_a_schema_the_reply_is_plain_text() {
    let h = harness();
    h.backend.push(Script::Reply("Just text.".into()));
    let out = h.call(EXT, request("Say hi", json!("data"))).await.unwrap();
    assert_eq!(out.text, "Just text.");
    assert_eq!(out.json, None);
    assert!(h.backend.prompts()[0].contains("3. Reply with plain text only."));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn reply_checks() {
    let h = harness();
    let schema = json!({ "type": "object", "additionalProperties": false,
        "properties": { "items": { "type": "array", "items": { "type": "string" } } } });

    h.backend.push(Script::Reply("Sorry, I can't.".into()));
    let mut req = request("i", json!("d"));
    req.schema = Some(schema.clone());
    let e = h.call(EXT, req.clone()).await.unwrap_err();
    assert_eq!(key(&e), "not_json");

    h.after_cooldown();
    h.backend
        .push(Script::Reply("{\"items\": [\"a\", 2]}".into()));
    let e = h.call(EXT, req.clone()).await.unwrap_err();
    assert_eq!(key(&e), "invalid_response");
    assert_eq!(e.params["path"], "$.items[1]");

    h.after_cooldown();
    h.backend.push(Script::Reply("x".repeat(101)));
    let mut long = request("i", json!("d"));
    long.max_response_chars = Some(100);
    let e = h.call(EXT, long).await.unwrap_err();
    assert_eq!(key(&e), "response_too_large");
    assert_eq!(e.params["limit"], "100");

    let outcomes: Vec<AgentCallOutcome> = h.hooks.calls().iter().map(|c| c.outcome).collect();
    assert_eq!(
        outcomes,
        vec![
            AgentCallOutcome::NotJson,
            AgentCallOutcome::InvalidResponse,
            AgentCallOutcome::ResponseTooLarge
        ]
    );
    // All three charged.
    assert_eq!(h.svc.status(EXT).usage.calls_today, 3);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn agent_errors_and_disconnects() {
    let h = harness();
    h.backend.push(Script::Fail("model overloaded".into()));
    let e = h.call(EXT, request("i", json!("d"))).await.unwrap_err();
    assert_eq!(key(&e), "agent_error");
    assert_eq!(e.params["reason"], "model overloaded");
    assert_eq!(kind(&e), "internal");

    // A disconnect mid-call.
    h.after_cooldown();
    h.backend.push(hang());
    let task = h.send_in_background(EXT, request("i", json!("d"))).await;
    h.backend.connected.store(false, Ordering::SeqCst);
    h.svc.on_disconnect();
    let e = task.await.unwrap().unwrap_err();
    assert_eq!(key(&e), "agent_disconnected");
    assert_eq!(kind(&e), "connection_lost");
    until(|| h.hooks.calls().len() == 2).await;
    assert_eq!(h.hooks.calls()[1].outcome, AgentCallOutcome::ConnectionLost);

    // Without an agent nothing is reserved, and nothing runs.
    let e = h
        .svc
        .reserve(EXT, reserve_args("organize", None, None))
        .unwrap_err();
    assert_eq!(key(&e), "agent_unavailable");
    assert_eq!(h.backend.runs.load(Ordering::SeqCst), 2);
}

// --- Gates ------------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn every_gate_is_checked_in_rust() {
    let h = harness();
    let reserve = |ext: &str| h.svc.reserve(ext, reserve_args("organize", None, None));

    let e = reserve("unknown").unwrap_err();
    assert_eq!(
        (key(&e), kind(&e).as_str()),
        ("not_available", "not_permitted")
    );
    assert_eq!(e.params["extension"], "unknown");

    let edit = |f: &dyn Fn(&mut Config)| f(&mut h.config.lock_or_recover());

    edit(&|c| c.extension_grants.get_mut(EXT).unwrap().granted = vec!["storage".into()]);
    assert_eq!(key(&reserve(EXT).unwrap_err()), "not_available");
    edit(&|c| *c = base_config());

    h.dir.0.lock_or_recover().get_mut(EXT).unwrap().decl = None;
    assert_eq!(key(&reserve(EXT).unwrap_err()), "not_available");
    h.dir
        .0
        .lock_or_recover()
        .insert(EXT.into(), facts("Mind Map"));

    edit(&|c| {
        c.extension_states.insert(EXT.into(), false);
    });
    let e = reserve(EXT).unwrap_err();
    assert_eq!(key(&e), "extension_disabled");
    assert_eq!(e.params["extension"], "Mind Map");
    edit(&|c| *c = base_config());

    edit(&|c| c.extension_agent_access.get_mut(EXT).unwrap().enabled = false);
    assert_eq!(key(&reserve(EXT).unwrap_err()), "access_off");
    edit(&|c| {
        c.extension_agent_access.remove(EXT);
    });
    assert_eq!(key(&reserve(EXT).unwrap_err()), "access_off");
    edit(&|c| *c = base_config());

    edit(&|c| c.extension_agent_access.get_mut(EXT).unwrap().consent = None);
    assert_eq!(key(&reserve(EXT).unwrap_err()), "consent_required");
    edit(&|c| *c = base_config());

    edit(&|c| {
        if let AcpMode::Local { spawn_command } = &mut c.acp.connections[0].mode {
            *spawn_command = "other-agent".into();
        }
    });
    assert_eq!(key(&reserve(EXT).unwrap_err()), "consent_stale");
    assert_eq!(h.svc.status(EXT).access, Access::ConsentStale);
    edit(&|c| *c = base_config());

    let e = h
        .svc
        .reserve(EXT, reserve_args("summarize", None, None))
        .unwrap_err();
    assert_eq!(
        (key(&e), kind(&e).as_str()),
        ("purpose_not_declared", "invalid_input")
    );
    assert_eq!(e.params["purpose"], "summarize");

    // Gates are checked again at send time.
    let call_id = h.turn(EXT).await;
    edit(&|c| c.extension_agent_access.get_mut(EXT).unwrap().enabled = false);
    let e = h
        .svc
        .send(EXT, send_args(&call_id, request("i", json!("d"))))
        .await
        .unwrap_err();
    assert_eq!(key(&e), "access_off");
    assert!(h.backend.prompts().is_empty());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn call_ids_are_bound_to_their_extension() {
    let h = harness();
    let call_id = h.reserve(EXT);
    h.svc.pump();
    let e = h.svc.ready(OTHER, id(&call_id)).await.unwrap_err();
    assert_eq!(key(&e), "unknown_call");
    h.svc.cancel(OTHER, id(&call_id)).unwrap();
    assert_eq!(
        h.svc.status(EXT).waiting,
        1,
        "another extension can't cancel it"
    );
    let e = h
        .svc
        .send(OTHER, send_args(&call_id, request("i", json!("d"))))
        .await
        .unwrap_err();
    assert_eq!(key(&e), "unknown_call");
    // Unknown ids are fine to cancel.
    h.svc.cancel(EXT, id("not-a-call")).unwrap();
    // The owner can still use it.
    h.svc
        .send(EXT, send_args(&call_id, request("i", json!("d"))))
        .await
        .unwrap();
    assert_eq!(
        key(&h.svc.request_detail(OTHER, &call_id).unwrap_err()),
        "unknown_call"
    );
    assert!(h.svc.request_detail(EXT, &call_id).is_ok());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn send_before_the_turn_is_not_ready() {
    let h = harness();
    h.svc.note_user_activity();
    let call_id = h.reserve(EXT);
    h.svc.pump();
    let e = h
        .svc
        .send(EXT, send_args(&call_id, request("i", json!("d"))))
        .await
        .unwrap_err();
    assert_eq!(key(&e), "not_ready");
    assert_eq!(h.svc.status(EXT).waiting, 1, "still waiting");
}

// --- Queue, scheduling and idle ------------------------------------------------------------

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn queue_cap_and_dedupe_key() {
    let h = harness();
    h.svc.note_user_activity();
    let mut ids = Vec::new();
    for i in 0..4 {
        let r = h
            .svc
            .reserve(EXT, reserve_args("organize", None, None))
            .unwrap();
        assert_eq!(r.position, i + 1);
        ids.push(r.call_id);
    }
    let e = h
        .svc
        .reserve(EXT, reserve_args("organize", None, None))
        .unwrap_err();
    assert_eq!((key(&e), kind(&e).as_str()), ("queue_full", "rate_limited"));
    assert_eq!(e.params["limit"], "4");
    // Another extension has its own queue.
    h.svc
        .reserve(OTHER, reserve_args("organize", None, None))
        .unwrap();
    for i in &ids {
        h.svc.cancel(EXT, id(i)).unwrap();
    }

    let a = h
        .svc
        .reserve(EXT, reserve_args("organize", None, Some("organize")))
        .unwrap();
    let e = h
        .svc
        .reserve(EXT, reserve_args("tag", None, Some("organize")))
        .unwrap_err();
    assert_eq!(key(&e), "duplicate");
    // Another extension may use the same key.
    h.svc
        .reserve(OTHER, reserve_args("organize", None, Some("organize")))
        .unwrap();
    h.svc.cancel(EXT, id(&a.call_id)).unwrap();
    h.svc
        .reserve(EXT, reserve_args("organize", None, Some("organize")))
        .unwrap();

    let e = h
        .svc
        .reserve(EXT, reserve_args("organize", None, Some("bad key")))
        .unwrap_err();
    assert_eq!(
        (key(&e), kind(&e).as_str()),
        ("field_invalid", "invalid_input")
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn one_turn_in_total_and_round_robin() {
    let h = harness();
    let a1 = h.reserve(EXT);
    let a2 = h.reserve(EXT);
    let b1 = h.reserve(OTHER);
    h.svc.pump();
    h.svc.ready(EXT, id(&a1)).await.unwrap();
    h.svc.pump();
    let st = h.svc.status(OTHER);
    assert_eq!(st.waiting, 1);
    assert!(
        h.svc.status(EXT).waiting == 2,
        "a1 holds the turn, a2 waits"
    );
    // Give the turn back unused: no cooldown, and the other extension is next.
    h.svc.cancel(EXT, id(&a1)).unwrap();
    h.svc.pump();
    h.svc.ready(OTHER, id(&b1)).await.unwrap();
    h.svc.cancel(OTHER, id(&b1)).unwrap();
    h.svc.pump();
    h.svc.ready(EXT, id(&a2)).await.unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn one_running_call_in_total() {
    let h = harness();
    h.backend.push(hang());
    let task = h.send_in_background(EXT, request("i", json!("d"))).await;
    h.backend.push(Script::Reply("second".into()));
    let other = h.reserve(OTHER);
    h.clock.advance_secs(30);
    h.svc.pump();
    assert!(h.svc.status(EXT).running);
    assert_eq!(h.svc.status(OTHER).waiting, 1);
    let snapshot = h.svc.activity();
    assert_eq!(snapshot.running.len(), 1);
    assert_eq!(snapshot.waiting, 1);
    let e = h
        .svc
        .send(OTHER, send_args(&other, request("i", json!("d"))))
        .await
        .unwrap_err();
    assert_eq!(key(&e), "not_ready");
    h.svc.cancel(EXT, id(&h.svc.activity_ids()[0])).unwrap();
    let e = task.await.unwrap().unwrap_err();
    assert_eq!(key(&e), "cancelled_by_extension");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn idle_gate_and_when_now() {
    let h = harness();
    h.svc.note_user_activity();
    let idle = h.reserve(EXT);
    h.svc.pump();
    assert_eq!(h.svc.status(EXT).waiting, 1);
    assert!(h.svc.state.lock_or_recover().sched.active().is_none());

    // `now` skips only the idle conditions.
    let now = h
        .svc
        .reserve(OTHER, reserve_args("organize", Some(When::Now), None))
        .unwrap()
        .call_id;
    h.svc.pump();
    h.svc.ready(OTHER, id(&now)).await.unwrap();
    h.svc.cancel(OTHER, id(&now)).unwrap();

    // 90 s after the last activity the user counts as away.
    h.clock.advance_secs(89);
    h.svc.pump();
    assert!(h.svc.state.lock_or_recover().sched.active().is_none());
    h.clock.advance_secs(2);
    // A user prompt in flight still holds it back.
    h.backend.in_flight.store(true, Ordering::SeqCst);
    h.svc.pump();
    assert!(h.svc.state.lock_or_recover().sched.active().is_none());
    h.backend.in_flight.store(false, Ordering::SeqCst);
    h.svc.pump();
    h.svc.ready(EXT, id(&idle)).await.unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn no_turn_while_the_agent_is_busy_or_power_is_low() {
    let h = harness();
    let call_id = h.reserve(EXT);
    for flag in [
        &h.backend.compacting,
        &h.backend.low_power,
        &h.backend.connected,
    ] {
        let on = flag.load(Ordering::SeqCst);
        flag.store(!on, Ordering::SeqCst);
        h.svc.pump();
        assert!(h.svc.state.lock_or_recover().sched.active().is_none());
        flag.store(on, Ordering::SeqCst);
    }
    h.svc.pump();
    h.svc.ready(EXT, id(&call_id)).await.unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn user_activity_after_the_turn_stops_an_idle_send() {
    let h = harness();
    let call_id = h.turn(EXT).await;
    h.svc.note_user_activity();
    let e = h
        .svc
        .send(EXT, send_args(&call_id, request("i", json!("d"))))
        .await
        .unwrap_err();
    assert_eq!(
        (key(&e), kind(&e).as_str()),
        ("cancelled_user_activity", "cancelled")
    );
    assert!(h.backend.prompts().is_empty());
    assert_eq!(
        h.svc.status(EXT).usage.calls_today,
        0,
        "refused before sending costs nothing"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_turn_expires_after_ten_seconds() {
    let h = harness();
    let first = h.turn(EXT).await;
    let second = h.reserve(EXT);
    h.clock.advance_secs(9);
    h.svc.pump();
    assert_eq!(
        h.svc.state.lock_or_recover().sched.active(),
        Some(first.as_str())
    );
    h.clock.advance_secs(1);
    h.svc.pump();
    let e = h
        .svc
        .send(EXT, send_args(&first, request("i", json!("d"))))
        .await
        .unwrap_err();
    assert_eq!((key(&e), kind(&e).as_str()), ("turn_expired", "timeout"));
    assert_eq!(e.params["seconds"], "10");
    // The next one got the turn at once: an unused turn starts no cooldown.
    h.svc.ready(EXT, id(&second)).await.unwrap();
    assert!(h.backend.prompts().is_empty());
}

// --- Limits ------------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn cooldown_is_measured_from_the_end_of_a_call() {
    let h = harness();
    h.backend.push(hang());
    let mut req = request("i", json!("d"));
    req.timeout_ms = Some(120_000);
    let task = h.send_in_background(EXT, req).await;
    // The call runs for a minute.
    h.clock.advance_secs(60);
    let call = h.svc.activity_ids()[0].clone();
    h.svc.cancel(EXT, id(&call)).unwrap();
    task.await.unwrap().unwrap_err();
    until(|| h.svc.activity().running.is_empty()).await;

    let next = h.reserve(EXT);
    h.clock.advance_secs(14);
    h.svc.pump();
    assert!(h.svc.state.lock_or_recover().sched.active().is_none());
    let status = h.svc.status(EXT);
    assert_eq!(
        status.next_start_at.as_deref(),
        Some("2026-11-09T10:01:15+00:00")
    );
    // Another extension only waits for the global 5 s.
    let other = h
        .svc
        .reserve(OTHER, reserve_args("organize", None, None))
        .unwrap()
        .call_id;
    h.svc.pump();
    h.svc.ready(OTHER, id(&other)).await.unwrap();
    h.svc.cancel(OTHER, id(&other)).unwrap();
    h.clock.advance_secs(1);
    h.svc.pump();
    h.svc.ready(EXT, id(&next)).await.unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn hourly_cap() {
    let h = harness();
    {
        let mut st = h.svc.state.lock_or_recover();
        for i in 0..20 {
            st.sched
                .record_start(EXT, t0() - Duration::minutes(50) + Duration::minutes(i));
        }
    }
    let e = h
        .svc
        .reserve(EXT, reserve_args("organize", None, None))
        .unwrap_err();
    assert_eq!(
        (key(&e), kind(&e).as_str()),
        ("rate_limited", "rate_limited")
    );
    assert_eq!(e.params["time"], "10:10");
    assert_eq!(
        h.svc.status(EXT).next_start_at.as_deref(),
        Some("2026-11-09T10:10:00+00:00")
    );
    h.svc
        .reserve(OTHER, reserve_args("organize", None, None))
        .unwrap();
    h.clock.advance_secs(10 * 60);
    h.svc
        .reserve(EXT, reserve_args("organize", None, None))
        .unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn daily_allowance_per_level_and_manifest_clamp() {
    let h = harness();
    h.charge(EXT, 40, 1000);
    let e = h
        .svc
        .reserve(EXT, reserve_args("organize", None, None))
        .unwrap_err();
    assert_eq!(key(&e), "budget_exhausted");
    assert_eq!(e.params["time"], "2026-11-10 00:00");
    assert_eq!(e.params["extension"], "Mind Map");
    let status = h.svc.status(EXT);
    assert_eq!(status.usage.calls_today, 40);
    assert_eq!(status.usage.resets_at, "2026-11-10T00:00:00+00:00");
    assert_eq!(
        status.next_start_at.as_deref(),
        Some("2026-11-10T00:00:00+00:00")
    );

    // High allows more.
    h.config
        .lock_or_recover()
        .extension_agent_access
        .get_mut(EXT)
        .unwrap()
        .allowance = AgentAllowance::High;
    assert_eq!(h.svc.status(EXT).usage.calls_limit, 150);
    h.svc
        .reserve(EXT, reserve_args("organize", None, None))
        .unwrap();

    // Low is 10 calls and 100k characters.
    h.config
        .lock_or_recover()
        .extension_agent_access
        .get_mut(OTHER)
        .unwrap()
        .allowance = AgentAllowance::Low;
    h.charge(OTHER, 1, 100_000);
    let e = h
        .svc
        .reserve(OTHER, reserve_args("organize", None, None))
        .unwrap_err();
    assert_eq!(key(&e), "budget_exhausted", "characters count too");

    // The manifest's dailyCalls lowers Normal only.
    h.dir
        .0
        .lock_or_recover()
        .get_mut(EXT)
        .unwrap()
        .decl
        .as_mut()
        .unwrap()
        .daily_calls = Some(25);
    let panel = h.svc.access_panel(EXT);
    assert_eq!(panel.allowances.normal.calls, 25);
    assert_eq!(panel.allowances.high.calls, 150);
    assert_eq!(panel.allowances.low.calls, 10);
    h.config
        .lock_or_recover()
        .extension_agent_access
        .get_mut(EXT)
        .unwrap()
        .allowance = AgentAllowance::Normal;
    assert_eq!(h.svc.status(EXT).usage.calls_limit, 25);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_prompt_that_would_go_over_the_character_allowance_is_refused() {
    let h = harness();
    h.charge(EXT, 1, 399_000);
    let call_id = h.turn(EXT).await;
    let e = h
        .svc
        .send(
            EXT,
            send_args(&call_id, request("i", json!("y".repeat(2_000)))),
        )
        .await
        .unwrap_err();
    assert_eq!(key(&e), "budget_exhausted");
    assert!(h.backend.prompts().is_empty());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn global_daily_cap() {
    let h = harness();
    h.charge("someone-else", 200, 200);
    let e = h
        .svc
        .reserve(EXT, reserve_args("organize", None, None))
        .unwrap_err();
    assert_eq!(key(&e), "rate_limited");
    assert_eq!(e.params["time"], "2026-11-10 00:00");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_allowance_resets_at_local_midnight_across_dst() {
    // UTC-4 → UTC-5 at 06:00Z on 2026-11-01 (fall back). Local midnight
    // of the 2nd is 05:00Z.
    let edt = chrono::FixedOffset::west_opt(4 * 3600).unwrap();
    let est = chrono::FixedOffset::west_opt(5 * 3600).unwrap();
    let start = DateTime::parse_from_rfc3339("2026-11-01T20:00:00Z")
        .unwrap()
        .with_timezone(&Utc);
    let switch = DateTime::parse_from_rfc3339("2026-11-01T06:00:00Z")
        .unwrap()
        .with_timezone(&Utc);
    let h = harness_with(FakeClock::with_offsets(start, edt, Some((switch, est))));
    h.charge(EXT, 40, 40);
    let e = h
        .svc
        .reserve(EXT, reserve_args("organize", None, None))
        .unwrap_err();
    assert_eq!(key(&e), "budget_exhausted");
    assert_eq!(
        h.svc.status(EXT).usage.resets_at,
        "2026-11-02T00:00:00-05:00"
    );
    // 04:59Z is still the 1st locally.
    h.clock.set(
        DateTime::parse_from_rfc3339("2026-11-02T04:59:00Z")
            .unwrap()
            .with_timezone(&Utc),
    );
    assert!(h
        .svc
        .reserve(EXT, reserve_args("organize", None, None))
        .is_err());
    h.clock.set(
        DateTime::parse_from_rfc3339("2026-11-02T05:00:00Z")
            .unwrap()
            .with_timezone(&Utc),
    );
    h.svc
        .reserve(EXT, reserve_args("organize", None, None))
        .unwrap();
    assert_eq!(h.svc.status(EXT).usage.calls_today, 0);
}

// --- Breakers -------------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn three_failures_in_a_row_pause_for_thirty_minutes() {
    let h = harness();
    for i in 0..2 {
        h.backend.push(Script::Fail(format!("failure {i}")));
        h.call(EXT, request("i", json!("d"))).await.unwrap_err();
        h.after_cooldown();
    }
    // The third failure is a timeout. A call waiting when the breaker
    // opens is given up.
    h.backend.push(hang());
    let mut req = request("i", json!("d"));
    req.timeout_ms = Some(5_000);
    let task = h.send_in_background(EXT, req).await;
    let waiting = h.reserve(EXT);
    h.clock.advance_secs(5);
    assert_eq!(key(&task.await.unwrap().unwrap_err()), "timeout");
    let e = h.svc.ready(EXT, id(&waiting)).await.unwrap_err();
    assert_eq!(key(&e), "paused");
    let status = h.svc.status(EXT);
    assert!(status.paused_until.is_some());
    let e = h
        .svc
        .reserve(EXT, reserve_args("organize", None, None))
        .unwrap_err();
    assert_eq!(key(&e), "paused");
    let paused = h.hooks.events("extension_agent_calls_paused");
    assert_eq!(
        paused,
        vec![json!({ "extension_id": EXT, "reason": "failures" })]
    );
    let panel = h.svc.access_panel(EXT);
    assert_eq!(panel.pause.unwrap().reason, PauseReason::Failures);
    // Other extensions are unaffected.
    h.svc
        .reserve(OTHER, reserve_args("organize", None, None))
        .unwrap();
    h.clock.advance_secs(30 * 60);
    h.svc
        .reserve(EXT, reserve_args("organize", None, None))
        .unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_success_resets_the_failure_count() {
    let h = harness();
    for script in [
        Script::Fail("a".into()),
        Script::Fail("b".into()),
        Script::Reply("fine".into()),
        Script::Fail("c".into()),
        Script::Fail("d".into()),
    ] {
        h.backend.push(script);
        let _ = h.call(EXT, request("i", json!("d"))).await;
        h.after_cooldown();
    }
    assert!(h.svc.status(EXT).paused_until.is_none());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_tool_attempt_stops_the_call_and_two_pause_until_midnight() {
    let h = harness();
    for round in 0..2 {
        h.backend.push(Script::Hang {
            partial: "partial".into(),
            stream_len: 7,
        });
        let task = h.send_in_background(EXT, request("i", json!("d"))).await;
        let session = format!("fake-session-{}", round + 1);
        // Another session's tool call is none of its business.
        h.svc.note_tool_attempt("someone-elses-session");
        assert!(h.svc.status(EXT).running);
        h.svc.note_tool_attempt(&session);
        let e = task.await.unwrap().unwrap_err();
        assert_eq!(key(&e), "tool_attempted");
        assert!(h.backend.cancels().contains(&session));
        until(|| h.hooks.calls().len() == round + 1).await;
        h.after_cooldown();
    }
    let audit = h.hooks.calls();
    assert!(audit
        .iter()
        .all(|c| c.outcome == AgentCallOutcome::ToolAttempted));
    // Charged for what was sent and what came back.
    assert_eq!(audit[0].response_chars, 7);
    let status = h.svc.status(EXT);
    assert_eq!(
        status.paused_until.as_deref(),
        Some("2026-11-10T00:00:00+00:00")
    );
    let panel = h.svc.access_panel(EXT);
    assert_eq!(panel.pause.unwrap().reason, PauseReason::ToolAttempt);
    assert_eq!(
        h.hooks.events("extension_agent_calls_paused"),
        vec![json!({ "extension_id": EXT, "reason": "tool_attempt" })]
    );
}

// --- Watchdog -------------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_slow_reply_times_out_and_is_cancelled() {
    let h = harness();
    h.backend.push(Script::Hang {
        partial: String::new(),
        stream_len: 0,
    });
    let mut req = request("i", json!("d"));
    req.timeout_ms = Some(5_000);
    let task = h.send_in_background(EXT, req).await;
    h.clock.advance_secs(5);
    let e = task.await.unwrap().unwrap_err();
    assert_eq!((key(&e), kind(&e).as_str()), ("timeout", "timeout"));
    assert_eq!(e.params["seconds"], "5");
    assert_eq!(h.backend.cancels(), vec!["fake-session-1".to_string()]);
    until(|| h.hooks.calls().len() == 1).await;
    assert_eq!(h.hooks.calls()[0].outcome, AgentCallOutcome::Timeout);
    assert_eq!(h.hooks.calls()[0].duration_ms, 5_000);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_long_reply_is_cut_off_while_streaming() {
    let h = harness();
    h.backend.push(Script::Hang {
        partial: "p".repeat(500),
        stream_len: 401,
    });
    let mut req = request("i", json!("d"));
    req.max_response_chars = Some(100);
    let task = h.send_in_background(EXT, req).await;
    let e = task.await.unwrap().unwrap_err();
    assert_eq!(key(&e), "response_too_large");
    assert_eq!(e.params["limit"], "100");
    assert!(h.backend.cancels().contains(&"fake-session-1".to_string()));
    until(|| h.hooks.calls().len() == 1).await;
    assert_eq!(
        h.hooks.calls()[0].outcome,
        AgentCallOutcome::ResponseTooLarge
    );
}

// --- Cancellation triggers (§5.7) ------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_user_prompt_stops_the_running_call_and_keeps_the_waiting_ones() {
    let h = harness();
    h.backend.push(Script::Hang {
        partial: String::new(),
        stream_len: 0,
    });
    let task = h.send_in_background(EXT, request("i", json!("d"))).await;
    let waiting = h.reserve(OTHER);
    h.svc.on_user_prompt_started();
    let e = task.await.unwrap().unwrap_err();
    assert_eq!(key(&e), "cancelled_user_activity");
    until(|| h.hooks.calls().len() == 1).await;
    let rec = &h.hooks.calls()[0];
    assert_eq!(rec.outcome, AgentCallOutcome::Cancelled);
    assert_eq!(rec.cancel_reason, Some(CancelReason::Activity));
    // The waiting one is kept, but held back while the user is active.
    assert_eq!(h.svc.status(OTHER).waiting, 1);
    h.clock.advance_secs(91);
    h.svc.pump();
    h.svc.ready(OTHER, id(&waiting)).await.unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_disconnect_keeps_the_waiting_ones_for_a_reconnect() {
    let h = harness();
    h.backend.push(Script::Hang {
        partial: String::new(),
        stream_len: 0,
    });
    let task = h.send_in_background(EXT, request("i", json!("d"))).await;
    let waiting = h.reserve(OTHER);
    h.backend.connected.store(false, Ordering::SeqCst);
    h.svc.on_disconnect();
    let e = task.await.unwrap().unwrap_err();
    assert_eq!(key(&e), "agent_disconnected");
    // The caller hears "disconnected" as soon as the stop lands; the
    // runner frees the turn a moment later, once the cancelled prompt
    // returns. Wait for that rather than racing it.
    let svc = h.svc.clone();
    until(move || svc.state.lock_or_recover().sched.active().is_none()).await;
    h.clock.advance_secs(30);
    h.svc.pump();
    assert_eq!(h.svc.status(OTHER).waiting, 1);
    assert!(h.svc.state.lock_or_recover().sched.active().is_none());
    h.backend.connected.store(true, Ordering::SeqCst);
    h.svc.pump();
    h.svc.ready(OTHER, id(&waiting)).await.unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_agent_change_makes_consent_stale() {
    let h = harness();
    h.backend.push(Script::Hang {
        partial: String::new(),
        stream_len: 0,
    });
    let task = h.send_in_background(EXT, request("i", json!("d"))).await;
    let waiting = h.reserve(EXT);
    let other = h.reserve(OTHER);
    {
        let mut cfg = h.config.lock_or_recover();
        let mut second = connection();
        second.id = "c2".into();
        second.name = "Other agent".into();
        cfg.acp.connections.push(second);
        cfg.acp.active_connection_id = "c2".into();
    }
    h.svc.on_agent_changed();
    let e = task.await.unwrap().unwrap_err();
    assert_eq!(
        (key(&e), kind(&e).as_str()),
        ("consent_stale", "not_permitted")
    );
    for (ext, call) in [(EXT, &waiting), (OTHER, &other)] {
        let e = h.svc.ready(ext, id(call)).await.unwrap_err();
        assert_eq!(key(&e), "consent_stale");
    }
    until(|| h.hooks.calls().len() == 1).await;
    assert_eq!(
        h.hooks.calls()[0].cancel_reason,
        Some(CancelReason::AccessOff)
    ); // The entry names the agent the text went to, not the new one.
    assert_eq!(h.hooks.calls()[0].agent, "Claude Code");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_stopped_call_frees_its_dedupe_key_but_holds_the_agent_until_it_ends() {
    let h = harness();
    h.backend.push(Script::Stubborn);
    let first = h
        .svc
        .reserve(EXT, reserve_args("organize", None, Some("organize")))
        .unwrap()
        .call_id;
    h.svc.pump();
    h.svc.ready(EXT, id(&first)).await.unwrap();
    let svc = h.svc.clone();
    let call = first.clone();
    let task = tokio::spawn(async move {
        svc.send(EXT, send_args(&call, request("i", json!("d"))))
            .await
    });
    until(|| !h.backend.prompts().is_empty()).await;
    h.svc.on_user_prompt_started();
    assert_eq!(
        key(&task.await.unwrap().unwrap_err()),
        "cancelled_user_activity"
    );
    assert!(h.svc.status(EXT).running, "the agent hasn't stopped yet");

    // The runtime's one retry reserves again with the same key.
    let again = h
        .svc
        .reserve(EXT, reserve_args("organize", None, Some("organize")))
        .unwrap()
        .call_id;
    // While the agent ignores the cancel, it is sent again, and nothing
    // else starts.
    h.clock.advance_secs(120);
    until(|| h.backend.cancels().len() >= 2).await;
    h.svc.pump();
    assert_eq!(
        h.svc.state.lock_or_recover().sched.active(),
        Some(first.as_str())
    );
    h.backend.release();
    until(|| !h.svc.status(EXT).running).await;
    h.after_cooldown();
    h.svc.pump();
    h.svc.ready(EXT, id(&again)).await.unwrap();
    // Charged for what came back after all.
    until(|| h.hooks.calls().len() == 1).await;
    assert_eq!(h.hooks.calls()[0].response_chars, 4);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn disabling_the_extension_cancels_its_calls_only() {
    let h = harness();
    h.backend.push(Script::Hang {
        partial: String::new(),
        stream_len: 0,
    });
    let task = h.send_in_background(EXT, request("i", json!("d"))).await;
    let waiting = h.reserve(EXT);
    let other = h.reserve(OTHER);
    h.config
        .lock_or_recover()
        .extension_states
        .insert(EXT.into(), false);
    h.svc.on_extension_changed(EXT);
    let e = task.await.unwrap().unwrap_err();
    assert_eq!(
        (key(&e), kind(&e).as_str()),
        ("cancelled_access_off", "cancelled")
    );
    assert_eq!(e.params["extension"], "Mind Map");
    let e = h.svc.ready(EXT, id(&waiting)).await.unwrap_err();
    assert_eq!(key(&e), "cancelled_access_off");
    assert_eq!(
        h.svc.status(OTHER).waiting,
        1,
        "{other} is another extension's"
    );

    // Uninstalled or grant removed: the same.
    let w = h.reserve(OTHER);
    h.config.lock_or_recover().extension_grants.remove(OTHER);
    h.svc.on_extension_changed(OTHER);
    let e = h.svc.ready(OTHER, id(&w)).await.unwrap_err();
    assert_eq!(key(&e), "cancelled_access_off");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn turning_the_switch_off_cancels_at_once() {
    let h = harness();
    h.backend.push(Script::Hang {
        partial: String::new(),
        stream_len: 0,
    });
    let task = h.send_in_background(EXT, request("i", json!("d"))).await;
    let waiting = h.reserve(EXT);
    h.svc.set_access(EXT, false, None, None).unwrap();
    assert_eq!(
        key(&task.await.unwrap().unwrap_err()),
        "cancelled_access_off"
    );
    assert_eq!(
        key(&h.svc.ready(EXT, id(&waiting)).await.unwrap_err()),
        "cancelled_access_off"
    );
    let cfg = h.config.lock_or_recover();
    assert!(cfg
        .extension_agent_access
        .get(EXT)
        .is_none_or(|e| !e.enabled && e.consent.is_none()));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn stop_now_cancels_and_pauses_for_an_hour() {
    let h = harness();
    h.backend.push(Script::Hang {
        partial: String::new(),
        stream_len: 0,
    });
    let task = h.send_in_background(EXT, request("i", json!("d"))).await;
    let waiting = h.reserve(EXT);
    let other = h.reserve(OTHER);
    h.svc.stop(Some(EXT));
    assert_eq!(key(&task.await.unwrap().unwrap_err()), "cancelled_by_user");
    assert_eq!(
        key(&h.svc.ready(EXT, id(&waiting)).await.unwrap_err()),
        "cancelled_by_user"
    );
    assert_eq!(h.svc.status(OTHER).waiting, 1, "{other} is untouched");
    let e = h
        .svc
        .reserve(EXT, reserve_args("organize", None, None))
        .unwrap_err();
    assert_eq!(key(&e), "paused");
    assert_eq!(e.params["time"], "11:00");
    assert_eq!(
        h.svc.access_panel(EXT).pause.unwrap().reason,
        PauseReason::Stopped
    );
    until(|| h.hooks.calls().len() == 1).await;
    assert_eq!(h.hooks.calls()[0].cancel_reason, Some(CancelReason::User));

    // Stop for everything.
    h.svc.stop(None);
    assert_eq!(
        key(&h.svc.ready(OTHER, id(&other)).await.unwrap_err()),
        "cancelled_by_user"
    );
    assert_eq!(
        key(&h
            .svc
            .reserve(OTHER, reserve_args("organize", None, None))
            .unwrap_err()),
        "paused"
    );
    h.clock.advance_secs(3600);
    h.svc
        .reserve(OTHER, reserve_args("organize", None, None))
        .unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_extension_can_cancel_at_every_stage() {
    let h = harness();
    // Waiting.
    h.svc.note_user_activity();
    let waiting = h.reserve(EXT);
    h.svc.cancel(EXT, id(&waiting)).unwrap();
    let e = h.svc.ready(EXT, id(&waiting)).await.unwrap_err();
    assert_eq!(key(&e), "cancelled_by_extension");
    h.clock.advance_secs(91);

    // Holding the turn (a builder that returned null).
    let turn = h.turn(EXT).await;
    h.svc.cancel(EXT, id(&turn)).unwrap();
    assert!(h.svc.state.lock_or_recover().sched.active().is_none());

    // Running.
    h.backend.push(Script::Hang {
        partial: String::new(),
        stream_len: 0,
    });
    let task = h.send_in_background(EXT, request("i", json!("d"))).await;
    let running = h.svc.activity_ids()[0].clone();
    h.svc.cancel(EXT, id(&running)).unwrap();
    h.svc.cancel(EXT, id(&running)).unwrap(); // idempotent
    assert_eq!(
        key(&task.await.unwrap().unwrap_err()),
        "cancelled_by_extension"
    );
    until(|| h.hooks.calls().len() == 1).await;
    assert_eq!(
        h.hooks.calls()[0].cancel_reason,
        Some(CancelReason::Extension)
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn shutdown_cancels_writes_the_audit_entry_and_refuses_new_calls() {
    let h = harness();
    h.backend.push(Script::Hang {
        partial: String::new(),
        stream_len: 0,
    });
    let task = h.send_in_background(EXT, request("i", json!("d"))).await;
    let waiting = h.reserve(OTHER);
    let svc = h.svc.clone();
    tokio::task::spawn_blocking(move || svc.shutdown())
        .await
        .unwrap();
    assert_eq!(key(&task.await.unwrap().unwrap_err()), "cancelled_shutdown");
    assert_eq!(
        key(&h.svc.ready(OTHER, id(&waiting)).await.unwrap_err()),
        "cancelled_shutdown"
    );
    // Shutdown waited for the session to end, so the entry is already written.
    let audit = h.hooks.calls();
    assert_eq!(audit.len(), 1);
    assert_eq!(audit[0].cancel_reason, Some(CancelReason::Shutdown));
    assert_eq!(*h.svc.runners.0.lock_or_recover(), 0);
    let e = h
        .svc
        .reserve(EXT, reserve_args("organize", None, None))
        .unwrap_err();
    assert_eq!(key(&e), "cancelled_shutdown");
}

// --- Credentials, redaction, size ----------------------------------------------------------------

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_extensions_own_credentials_are_refused() {
    let h = harness();
    h.config.lock_or_recover().extensions.insert(
        EXT.into(),
        json!({ "api_key": "mine-abcdefgh-1234", "vault": "vault-value-5678", "theme": "dark-mode-on" }),
    );
    for text in ["mine-abcdefgh-1234", "vault-value-5678"] {
        let e = h
            .call(
                EXT,
                request("i", json!({ "note": format!("my key is {text}") })),
            )
            .await
            .unwrap_err();
        assert_eq!(
            (key(&e), kind(&e).as_str()),
            ("contains_credential", "invalid_input")
        );
    }
    assert!(h.backend.prompts().is_empty(), "nothing sent");
    // A plain setting isn't a credential.
    h.call(EXT, request("i", json!("dark-mode-on")))
        .await
        .unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn secret_shaped_text_is_redacted_before_sending() {
    let h = harness();
    let out = h
        .call(
            EXT,
            request(
                "Use password: hunter2hunter2 nowhere",
                json!({ "note": "token ghp_abcdefghijklmnop1234 here", "n": 1 }),
            ),
        )
        .await
        .unwrap();
    assert_eq!(out.redactions, 2);
    let sent = &h.backend.prompts()[0];
    assert!(!sent.contains("hunter2hunter2"));
    assert!(!sent.contains("ghp_abcdefghijklmnop1234"));
    assert!(sent.contains("[redacted]"));
    assert_eq!(h.hooks.calls()[0].redactions, 2);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn request_limits() {
    let h = harness();
    let cases: Vec<(AgentCallRequest, &str, &str)> = vec![
        (
            request(&"i".repeat(4_001), json!("d")),
            "field_too_long",
            "instructions",
        ),
        (
            request("i", json!("d".repeat(24_001))),
            "field_too_long",
            "input",
        ),
        (
            AgentCallRequest {
                label: Some("l".repeat(81)),
                ..request("i", json!("d"))
            },
            "field_too_long",
            "label",
        ),
        (
            AgentCallRequest {
                schema: Some(json!({ "anyOf": [] })),
                ..request("i", json!("d"))
            },
            "schema_unsupported",
            "",
        ),
    ];
    for (req, expected, field) in cases {
        let e = h.call(EXT, req).await.unwrap_err();
        assert_eq!(key(&e), expected);
        assert_eq!(kind(&e), "invalid_input");
        if !field.is_empty() {
            assert_eq!(e.params["field"], field);
        }
    }
    assert!(h.backend.prompts().is_empty());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_framed_prompt_is_capped_with_the_manifests_lower_limit() {
    let h = harness();
    h.dir
        .0
        .lock_or_recover()
        .get_mut(EXT)
        .unwrap()
        .decl
        .as_mut()
        .unwrap()
        .max_prompt_chars = Some(2_000);
    // The input fits its own limit, but the framed prompt doesn't fit 2,000.
    let e = h
        .call(EXT, request("i", json!("d".repeat(1_900))))
        .await
        .unwrap_err();
    assert_eq!(key(&e), "prompt_too_large");
    assert_eq!(e.params["limit"], "2000");
    let size: usize = e.params["size"].parse().unwrap();
    assert!(size > 2_000 && size < 2_600, "{size}");
    // The host cap holds without a manifest limit: 32,000. JSON input is
    // counted compact against its own limit, but goes into the prompt
    // pretty-printed.
    h.dir
        .0
        .lock_or_recover()
        .get_mut(EXT)
        .unwrap()
        .decl
        .as_mut()
        .unwrap()
        .max_prompt_chars = None;
    let input = json!(vec![1; 11_000]);
    assert!(request::input_chars(&input) <= limits::MAX_INPUT_CHARS);
    let e = h.call(EXT, request("i", input)).await.unwrap_err();
    assert_eq!(key(&e), "prompt_too_large");
    assert_eq!(e.params["limit"], "32000");
    h.call(EXT, request("i", json!("d".repeat(1_900))))
        .await
        .unwrap();
}

// --- Consent and the host switch -----------------------------------------------------------------

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn turning_on_needs_the_current_agents_token() {
    let h = harness();
    {
        let mut cfg = h.config.lock_or_recover();
        cfg.extension_agent_access.remove(EXT);
        cfg.extension_grants
            .get_mut(EXT)
            .unwrap()
            .granted
            .extend(["clipboard".to_string(), "network".to_string()]);
    }
    let panel = h.svc.access_panel(EXT);
    assert_eq!(panel.access, Access::Off);
    assert!(!panel.enabled);
    assert!(panel.available);
    assert_eq!(panel.name, "Mind Map");
    assert_eq!(
        panel.agent,
        Some(AgentInfo {
            label: "Claude Code".into(),
            location: Location {
                kind: LocationKind::Cloud,
                host: None
            }
        })
    );
    assert_eq!(panel.reads, vec!["clipboard".to_string()]);
    assert_eq!(panel.reason.as_deref(), Some("Organise new notes"));
    assert_eq!(panel.log, vec![json!({ "extension_id": EXT, "limit": 20 })]);
    let token = panel.consent_token.clone().unwrap();
    assert_eq!(token, consent::fingerprint(&connection()));

    let e = h.svc.set_access(EXT, true, None, None).unwrap_err();
    assert_eq!(key(&e), "consent_required");
    let e = h
        .svc
        .set_access(EXT, true, Some("0".repeat(64)), None)
        .unwrap_err();
    assert_eq!(
        (key(&e), kind(&e).as_str()),
        ("agent_changed", "not_permitted")
    );
    assert_eq!(h.svc.status(EXT).access, Access::Off);

    h.svc
        .set_access(EXT, true, Some(token.clone()), None)
        .unwrap();
    assert_eq!(h.svc.status(EXT).access, Access::On);
    let entry = h.config.lock_or_recover().extension_agent_access[EXT].clone();
    let consent = entry.consent.unwrap();
    assert_eq!(consent.agent_label, "Claude Code");
    assert_eq!(consent.granted_at, "2026-11-09T10:00:00Z");
    assert_eq!(
        h.hooks.access.lock_or_recover().clone(),
        vec![AccessRecord {
            extension_id: EXT.into(),
            enabled: true,
            agent: Some("Claude Code".into())
        }]
    );
    assert_eq!(
        h.hooks.events("extension_agent_access_changed"),
        vec![json!({ "extension_id": EXT, "enabled": 1 })]
    );

    // While on, the allowance can change without a new token.
    h.svc
        .set_access(EXT, true, None, Some(AgentAllowance::High))
        .unwrap();
    assert_eq!(h.svc.status(EXT).usage.calls_limit, 150);
    assert_eq!(h.hooks.access.lock_or_recover().len(), 1, "no second entry");

    // The agent changes: stale, and a new token is needed.
    {
        let mut cfg = h.config.lock_or_recover();
        if let AcpMode::Local { spawn_command } = &mut cfg.acp.connections[0].mode {
            *spawn_command = "claude-agent-acp --other".into();
        }
    }
    let panel = h.svc.access_panel(EXT);
    assert_eq!(panel.access, Access::ConsentStale);
    assert_eq!(panel.consent_agent.as_deref(), Some("Claude Code"));
    assert_ne!(panel.consent_token.as_deref(), Some(token.as_str()));
    let e = h.svc.set_access(EXT, true, None, None).unwrap_err();
    assert_eq!(key(&e), "consent_required");
    assert_eq!(
        key(&h.svc.set_access(EXT, true, Some(token), None).unwrap_err()),
        "agent_changed"
    );
    h.svc
        .set_access(EXT, true, panel.consent_token.clone(), None)
        .unwrap();
    assert_eq!(h.svc.status(EXT).access, Access::On);

    // Off: consent goes, the allowance stays.
    h.svc.set_access(EXT, false, None, None).unwrap();
    let entry = h.config.lock_or_recover().extension_agent_access[EXT].clone();
    assert!(!entry.enabled && entry.consent.is_none());
    assert_eq!(entry.allowance, AgentAllowance::High);
    assert_eq!(h.hooks.access.lock_or_recover().len(), 2);
    assert_eq!(
        h.hooks.events("extension_agent_access_changed")[1]["enabled"],
        0
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn turning_on_needs_the_permission_and_the_manifest_block() {
    let h = harness();
    h.dir.0.lock_or_recover().get_mut(EXT).unwrap().decl = None;
    let token = h.svc.access_panel(EXT).consent_token;
    let e = h.svc.set_access(EXT, true, token, None).unwrap_err();
    assert_eq!(key(&e), "not_available");
    // Turning off always works, even for an unknown extension.
    h.svc.set_access("gone", false, None, None).unwrap();
    assert!(!h
        .config
        .lock_or_recover()
        .extension_agent_access
        .contains_key("gone"));
}

#[test]
fn a_failed_save_leaves_the_config_as_it_was() {
    fn fail(_: &Config) -> anyhow::Result<()> {
        anyhow::bail!("disk full")
    }
    let config = Arc::new(Mutex::new(base_config()));
    config.lock_or_recover().extension_agent_access.remove(EXT);
    let backend = Arc::new(FakeBackend::default());
    let svc = AgentCallService::with_parts(ServiceParts {
        config: config.clone(),
        backend,
        directory: Arc::new(FakeDirectory(Mutex::new(HashMap::from([(
            EXT.to_string(),
            facts("Mind Map"),
        )])))),
        clock: Arc::new(FakeClock::new(t0())),
        usage_path: None,
        machine_id_path: None,
        save_config: fail,
    });
    let token = svc.access_panel(EXT).consent_token;
    assert!(svc.set_access(EXT, true, token, None).is_err());
    assert!(!config
        .lock_or_recover()
        .extension_agent_access
        .contains_key(EXT));
}

// --- Status, telemetry, persistence ---------------------------------------------------------------

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn status_says_nothing_about_the_agent_or_other_extensions() {
    let h = harness();
    h.svc.note_user_activity();
    h.reserve(OTHER);
    let s = h.svc.status(EXT);
    assert!(s.available && s.connected && !s.running);
    assert_eq!(s.waiting, 0);
    assert_eq!(s.access, Access::On);
    assert_eq!(s.machine_tag.len(), 16);
    assert_eq!(h.svc.status(EXT).machine_tag, s.machine_tag, "stable");
    assert_ne!(h.svc.status(OTHER).machine_tag, s.machine_tag);
    let text = serde_json::to_string(&s).unwrap();
    assert!(!text.contains("Claude"), "{text}");
    assert!(!text.contains("fingerprint"), "{text}");
    let unknown = h.svc.status("nope");
    assert!(!unknown.available);
    assert_eq!(unknown.access, Access::Off);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_daily_event_reports_each_day_once_on_the_next_day() {
    let h = harness();
    h.call(EXT, request("i", json!("d"))).await.unwrap();
    h.svc.pump();
    assert!(h.hooks.events("extension_agent_calls_daily").is_empty());
    h.clock.set(t0() + Duration::hours(15));
    h.svc.pump();
    h.svc.pump();
    let daily = h.hooks.events("extension_agent_calls_daily");
    assert_eq!(daily.len(), 1);
    assert_eq!(
        daily[0],
        json!({ "extension_id": EXT, "calls": 1, "ok": 1, "failed": 0, "cancelled": 0,
                "tool_attempts": 0, "kchars_bucket": "<10" })
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn usage_survives_a_restart() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("kage").join("agent-calls-usage.json");
    let id_path = dir.path().join("kage").join("machine-id");
    let build = |backend: Arc<FakeBackend>| {
        Arc::new(AgentCallService::with_parts(ServiceParts {
            config: Arc::new(Mutex::new(base_config())),
            backend,
            directory: Arc::new(FakeDirectory(Mutex::new(HashMap::from([(
                EXT.to_string(),
                facts("Mind Map"),
            )])))),
            clock: Arc::new(FakeClock::new(t0())),
            usage_path: Some(path.clone()),
            machine_id_path: Some(id_path.clone()),
            save_config: no_save,
        }))
    };
    let backend = Arc::new(FakeBackend::default());
    backend.connected.store(true, Ordering::SeqCst);
    let svc = build(backend.clone());
    let call_id = svc
        .reserve(EXT, reserve_args("organize", None, None))
        .unwrap()
        .call_id;
    svc.pump();
    svc.ready(EXT, id(&call_id)).await.unwrap();
    svc.send(EXT, send_args(&call_id, request("i", json!("d"))))
        .await
        .unwrap();
    let tag = svc.status(EXT).machine_tag;
    assert!(path.exists());
    let again = build(backend);
    let status = again.status(EXT);
    assert_eq!(status.usage.calls_today, 1);
    assert_eq!(status.machine_tag, tag, "the machine id is kept");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn start_runs_the_scheduler_and_the_free_hooks_reach_it() {
    let h = harness();
    h.svc.start(h.hooks.clone());
    // No manual pump: the scheduler task gives the turn.
    let call_id = h.reserve(EXT);
    tokio::time::timeout(
        std::time::Duration::from_secs(5),
        h.svc.ready(EXT, id(&call_id)),
    )
    .await
    .expect("the task gave the turn")
    .unwrap();
    h.backend.push(Script::Hang {
        partial: String::new(),
        stream_len: 0,
    });
    let svc = h.svc.clone();
    let task = tokio::spawn(async move {
        svc.send(EXT, send_args(&call_id, request("i", json!("d"))))
            .await
    });
    let backend = h.backend.clone();
    until(move || !backend.prompts().is_empty()).await;
    super::note_tool_attempt("fake-session-1");
    assert_eq!(key(&task.await.unwrap().unwrap_err()), "tool_attempted");
    super::on_disconnect(); // nothing running: a no-op
    let svc = h.svc.clone();
    tokio::task::spawn_blocking(move || svc.shutdown())
        .await
        .unwrap();
    *GLOBAL.lock_or_recover() = None;
}

// --- Logging ---------------------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn no_content_in_any_log_line() {
    let h = harness();
    let markers = [
        "INSTR-MARKER-7f1",
        "INPUT-MARKER-7f2",
        "LABEL-MARKER-7f3",
        "REPLY-MARKER-7f4",
        "AGENTERR-MARKER-7f5",
        "SCHEMA-MARKER-7f6",
    ];
    let mut req = request(
        "Do INSTR-MARKER-7f1 now",
        json!({ "note": "INPUT-MARKER-7f2" }),
    );
    req.label = Some("LABEL-MARKER-7f3".into());
    h.backend.push(Script::Reply("REPLY-MARKER-7f4".into()));
    h.call(EXT, req.clone()).await.unwrap();
    h.after_cooldown();
    h.backend.push(Script::Fail("AGENTERR-MARKER-7f5".into()));
    h.call(EXT, req.clone()).await.unwrap_err();
    h.after_cooldown();
    req.schema = Some(json!({ "description": "SCHEMA-MARKER-7f6" }));
    h.backend
        .push(Script::Reply("REPLY-MARKER-7f4 not json".into()));
    h.call(EXT, req).await.unwrap_err();
    until(|| h.hooks.calls().len() == 3).await;

    let lines = LOG_CAPTURE.lock_or_recover().clone();
    assert!(
        lines
            .iter()
            .any(|l| l.contains("Background AI call") && l.contains("outcome=ok")),
        "one line per call"
    );
    for line in &lines {
        for m in markers {
            assert!(!line.contains(m), "log line carries content: {line}");
        }
    }
    // Audit entries carry no text either.
    let audit = serde_json::to_string(&h.hooks.calls()).unwrap();
    for m in markers.iter().filter(|m| !m.starts_with("LABEL")) {
        assert!(!audit.contains(m), "{audit}");
    }
}

#[test]
fn every_log_line_goes_through_the_wrappers() {
    let sources = [
        ("agent_calls.rs", include_str!("../agent_calls.rs")),
        ("consent.rs", include_str!("consent.rs")),
        ("exec.rs", include_str!("exec.rs")),
        ("limits.rs", include_str!("limits.rs")),
        ("prompt.rs", include_str!("prompt.rs")),
        ("recent.rs", include_str!("recent.rs")),
        ("redact.rs", include_str!("redact.rs")),
        ("request.rs", include_str!("request.rs")),
        ("scheduler.rs", include_str!("scheduler.rs")),
        ("schema.rs", include_str!("schema.rs")),
        ("usage.rs", include_str!("usage.rs")),
    ];
    let forbidden = [
        "info!(",
        "warn!(",
        "error!(",
        "debug!(",
        "trace!(",
        "println!(",
        "eprintln!(",
        "dbg!(",
    ];
    for (name, text) in sources {
        let uses: Vec<&str> = text
            .lines()
            .filter(|l| forbidden.iter().any(|f| l.contains(f)))
            .collect();
        let allowed = if name == "agent_calls.rs" {
            // The two wrappers themselves.
            vec!["    log::info!(\"{msg}\");", "    log::warn!(\"{msg}\");"]
        } else {
            Vec::new()
        };
        assert_eq!(uses, allowed, "{name} logs outside info_line / warn_line");
    }
}

// --- The real extension directory ----------------------------------------------------------

#[test]
fn installed_extensions_reads_the_manifest_block_name_and_sensitive_fields() {
    let base =
        crate::extensions::user_item_dir(crate::extensions::kind_to_subdir("extension").unwrap())
            .unwrap();
    let dir = base.join("ac1-probe");
    std::fs::create_dir_all(dir.join("_locales").join("en")).unwrap();
    std::fs::write(
        dir.join("manifest.json"),
        json!({
            "id": "ac1-probe", "name": "__MSG_extName__", "version": "1.0.0", "type": "extension",
            "permissions": ["storage", "agent_background"],
            "agentBackground": { "reason": "Because", "purposes": ["organize"], "dailyCalls": 12 },
            "config": {
                "vault": { "type": "string", "sensitive": true },
                "theme": { "type": "string" }
            }
        })
        .to_string(),
    )
    .unwrap();
    std::fs::write(
        dir.join("_locales").join("en").join("messages.json"),
        json!({ "extName": { "message": "Probe" } }).to_string(),
    )
    .unwrap();
    let found = InstalledExtensions.manifest("ac1-probe");
    let missing = InstalledExtensions.manifest("ac1-not-installed");
    let _ = std::fs::remove_dir_all(&dir);

    let facts = found.expect("installed");
    assert_eq!(facts.name, "Probe");
    assert_eq!(facts.sensitive_fields, vec!["vault".to_string()]);
    let decl = facts.decl.expect("declared");
    assert_eq!(decl.purposes, vec!["organize".to_string()]);
    assert_eq!(decl.daily_calls, Some(12));
    assert!(missing.is_none());
}

// --- More cancellation triggers -------------------------------------------------------------

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn floating_activity_keeps_the_running_call_and_holds_back_new_turns() {
    let h = harness();
    h.backend.push(hang());
    let task = h.send_in_background(EXT, request("i", json!("d"))).await;
    let waiting = h.reserve(OTHER);
    // Floating input, or the window shown: the call keeps running.
    h.svc.note_user_activity();
    tokio::time::sleep(std::time::Duration::from_millis(300)).await;
    assert!(h.svc.status(EXT).running);
    assert!(h.backend.cancels().is_empty());
    let running = h.svc.activity_ids()[0].clone();
    h.svc.cancel(EXT, id(&running)).unwrap();
    task.await.unwrap().unwrap_err();
    until(|| h.svc.activity().running.is_empty()).await;
    h.after_cooldown();
    h.svc.note_user_activity();
    h.svc.pump();
    assert_eq!(h.svc.status(OTHER).waiting, 1, "held back while active");
    assert!(h.svc.state.lock_or_recover().sched.active().is_none());
    h.clock.advance_secs(91);
    h.svc.pump();
    h.svc.ready(OTHER, id(&waiting)).await.unwrap();
}
