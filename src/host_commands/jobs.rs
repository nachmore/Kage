//! Requests Kage owns as jobs (bridge protocol version 2), so that no
//! single MCP tool call has to last as long as the user's answer plus the
//! run: agents cut a call off (kiro-cli after about two minutes), and a
//! prompt or a run tied to that call's connection was withdrawn or lost
//! its result with it.
//!
//! A job is made when a request arrives (once it's matched to the asking
//! session, so its caps can count per session) and its own thread takes it
//! through the rules, the prompt and the run (`server`). The connection
//! that asked waits for it at most [`LONG_POLL`] and then answers
//! "pending" with the job's id; `get_outside_sandbox_result` polls wait the
//! same way. A poll only reads a job: nothing here lets one start, approve
//! or change a job.
//!
//! What ends a job waiting for the user is its prompt's business
//! (`approvals`): an answer, the five-minute limit, its turn stopped, the
//! agent disconnecting, its window closing, Kage quitting — and, only
//! while the agent hasn't been given the id, its tool call ending or its
//! requester going away ([`Job::abandon`]), since then nobody could read
//! the result. A finished job keeps its result for [`RESULT_RETENTION`].
//!
//! Job ids are the request ids (`hc-` and 96 random bits), so an id from an
//! earlier run of Kage never names a job of this one.
//!
//! [`LONG_POLL`]: kage_core::host_bridge::LONG_POLL
//! [`RESULT_RETENTION`]: kage_core::host_bridge::RESULT_RETENTION

use crate::lock_ext::LockExt;
use kage_core::host_bridge::{self, HostCommandResponse, JobProgress, JobState};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex, MutexGuard};
use std::time::{Duration, Instant};

/// Jobs kept at once: waiting, running, or finished and kept for polling.
pub const MAX_JOBS: usize = 32;

/// Of those, at most this many for one session (requests no session was
/// matched to count as one).
pub const MAX_SESSION_JOBS: usize = 8;

/// Output a running job keeps for its "still running" replies, in bytes.
const OUTPUT_KEEP_BYTES: usize = 16 * 1024;

/// How often a wait looks at its requester.
const WAIT_TICK: Duration = Duration::from_millis(250);

#[derive(Debug, Clone)]
enum Phase {
    /// Being checked (guard, rules) before a prompt or a run.
    Starting,
    WaitingForApproval {
        expires_at: Instant,
    },
    Running {
        started: Instant,
    },
    Finished {
        response: Box<HostCommandResponse>,
        at: Instant,
    },
}

struct State {
    phase: Phase,
    /// The tail of the run's output so far.
    output: String,
    /// The running command's stop flag.
    run_cancel: Option<Arc<AtomicBool>>,
}

/// One request Kage owns.
pub struct Job {
    pub id: String,
    pub session_id: Option<String>,
    pub tool_call_id: Option<String>,
    /// A version-1 request: bound to its connection as version 1 always
    /// was (the requester going away withdraws its prompt and stops its
    /// run).
    pub one_shot: bool,
    created: Instant,
    state: Mutex<State>,
    changed: Condvar,
    /// Kage told the agent the job's id ("pending"): from then on the job
    /// outlives the tool call and the connection that asked for it.
    id_given: AtomicBool,
    /// Nobody can read the result: the requester went away, or its tool
    /// call ended, before it was given the id.
    abandoned: AtomicBool,
    /// A version-1 requester went away: its run stops too.
    requester_left: AtomicBool,
}

/// How a wait on a job ended.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Waited {
    Finished(Box<HostCommandResponse>),
    /// Still going at the deadline: how far it got.
    Pending(JobProgress),
    /// The caller's `give_up` said so (its requester went away).
    GaveUp,
}

fn millis(d: Duration) -> u64 {
    u64::try_from(d.as_millis()).unwrap_or(u64::MAX)
}

impl Job {
    fn new(
        id: &str,
        session_id: Option<String>,
        tool_call_id: Option<String>,
        one_shot: bool,
    ) -> Self {
        Self {
            id: id.to_string(),
            session_id,
            tool_call_id,
            one_shot,
            created: Instant::now(),
            state: Mutex::new(State {
                phase: Phase::Starting,
                output: String::new(),
                run_cancel: None,
            }),
            changed: Condvar::new(),
            id_given: AtomicBool::new(false),
            abandoned: AtomicBool::new(false),
            requester_left: AtomicBool::new(false),
        }
    }

    fn lock(&self) -> MutexGuard<'_, State> {
        self.state.lock_or_recover()
    }

    /// Its prompt is up; Kage stops waiting for the user at `expires_at`.
    pub fn set_waiting(&self, expires_at: Instant) {
        let mut st = self.lock();
        if !matches!(st.phase, Phase::Finished { .. }) {
            st.phase = Phase::WaitingForApproval { expires_at };
        }
    }

    /// Its command started; `cancel` stops it.
    pub fn set_running(&self, cancel: Arc<AtomicBool>) {
        let mut st = self.lock();
        if !matches!(st.phase, Phase::Finished { .. }) {
            st.phase = Phase::Running {
                started: Instant::now(),
            };
            st.run_cancel = Some(cancel);
        }
    }

    /// Output the command printed (kept for "still running" replies).
    pub fn push_output(&self, text: &str) {
        let mut st = self.lock();
        st.output.push_str(text);
        if st.output.len() > OUTPUT_KEEP_BYTES * 2 {
            let keep = host_bridge::output_tail(&st.output, OUTPUT_KEEP_BYTES)
                .0
                .to_string();
            st.output = keep;
        }
    }

    /// The job is over: this is its result. Only the first one counts.
    pub fn finish(&self, response: HostCommandResponse) -> bool {
        let mut st = self.lock();
        if matches!(st.phase, Phase::Finished { .. }) {
            return false;
        }
        st.phase = Phase::Finished {
            response: Box::new(response),
            at: Instant::now(),
        };
        st.output = String::new();
        st.run_cancel = None;
        drop(st);
        self.changed.notify_all();
        true
    }

    /// Its result, once finished.
    #[allow(dead_code)] // The end-to-end tests reach it through the library.
    pub fn result(&self) -> Option<HostCommandResponse> {
        match &self.lock().phase {
            Phase::Finished { response, .. } => Some((**response).clone()),
            _ => None,
        }
    }

    pub fn is_finished(&self) -> bool {
        matches!(self.lock().phase, Phase::Finished { .. })
    }

    fn finished_at(&self) -> Option<Instant> {
        match self.lock().phase {
            Phase::Finished { at, .. } => Some(at),
            _ => None,
        }
    }

    fn progress_of(&self, st: &State) -> JobProgress {
        let now = Instant::now();
        let elapsed_ms = millis(now.saturating_duration_since(self.created));
        match &st.phase {
            Phase::Running { started } => JobProgress {
                state: JobState::Running,
                elapsed_ms,
                running_ms: Some(millis(now.saturating_duration_since(*started))),
                output_tail: Some(host_bridge::progress_lines(&st.output)),
                approval_expires_in_ms: None,
            },
            Phase::WaitingForApproval { expires_at } => JobProgress {
                state: JobState::WaitingForApproval,
                elapsed_ms,
                running_ms: None,
                output_tail: None,
                approval_expires_in_ms: Some(millis(expires_at.saturating_duration_since(now))),
            },
            Phase::Starting | Phase::Finished { .. } => JobProgress {
                state: JobState::WaitingForApproval,
                elapsed_ms,
                running_ms: None,
                output_tail: None,
                approval_expires_in_ms: None,
            },
        }
    }

    /// How far it got; `None` once finished.
    #[cfg(test)]
    pub fn progress(&self) -> Option<JobProgress> {
        let st = self.lock();
        (!matches!(st.phase, Phase::Finished { .. })).then(|| self.progress_of(&st))
    }

    /// Wait for the job to finish, until `deadline` (forever when `None`),
    /// asking `give_up` every quarter second (outside the job's lock).
    pub fn wait(&self, deadline: Option<Instant>, give_up: impl Fn() -> bool) -> Waited {
        loop {
            {
                let st = self.lock();
                if let Phase::Finished { response, .. } = &st.phase {
                    return Waited::Finished(response.clone());
                }
                let now = Instant::now();
                let tick = match deadline {
                    Some(d) if now >= d => return Waited::Pending(self.progress_of(&st)),
                    Some(d) => (d - now).min(WAIT_TICK),
                    None => WAIT_TICK,
                };
                let (st, _) = self
                    .changed
                    .wait_timeout(st, tick)
                    .unwrap_or_else(|p| p.into_inner());
                if let Phase::Finished { response, .. } = &st.phase {
                    return Waited::Finished(response.clone());
                }
            }
            if give_up() {
                return Waited::GaveUp;
            }
        }
    }

    /// Kage is about to tell the agent the job's id.
    pub fn mark_id_given(&self) {
        self.id_given.store(true, Ordering::SeqCst);
    }

    /// The reply carrying the id never reached the agent.
    pub fn unmark_id_given(&self) {
        self.id_given.store(false, Ordering::SeqCst);
    }

    pub fn id_given(&self) -> bool {
        self.id_given.load(Ordering::SeqCst)
    }

    /// Nobody can read the result any more (the requester went away, or
    /// its tool call ended, before it got the id): a prompt still waiting
    /// is withdrawn. Once the id was given, nothing changes.
    pub fn abandon(&self) {
        if !self.id_given() {
            self.abandoned.store(true, Ordering::SeqCst);
        }
    }

    /// Whether a waiting prompt should be withdrawn ([`Self::abandon`]).
    pub fn is_abandoned(&self) -> bool {
        self.abandoned.load(Ordering::SeqCst) && !self.id_given()
    }

    /// A version-1 requester went away: as version 1 always did, its
    /// prompt is withdrawn and its run stopped. Returns whether a run was
    /// going.
    pub fn requester_left(&self) -> bool {
        self.abandon();
        if !self.one_shot {
            return false;
        }
        self.requester_left.store(true, Ordering::SeqCst);
        let st = self.lock();
        match (&st.phase, &st.run_cancel) {
            (Phase::Running { .. }, Some(cancel)) => {
                cancel.store(true, Ordering::SeqCst);
                true
            }
            _ => false,
        }
    }

    /// Whether its run should stop because its version-1 requester left.
    pub fn run_should_stop(&self) -> bool {
        self.one_shot && self.requester_left.load(Ordering::SeqCst)
    }
}

/// Why a job couldn't be made.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum JobsFull {
    /// [`MAX_SESSION_JOBS`] of this session are waiting or running.
    Session,
    /// [`MAX_JOBS`] are waiting or running.
    Total,
}

/// Every job Kage holds, oldest first.
pub struct Jobs {
    jobs: Mutex<Vec<Arc<Job>>>,
    retention: Duration,
}

impl Default for Jobs {
    fn default() -> Self {
        Self::with_retention(host_bridge::RESULT_RETENTION)
    }
}

impl Jobs {
    /// Finished jobs are dropped `retention` after they finished.
    pub fn with_retention(retention: Duration) -> Self {
        Self {
            jobs: Mutex::new(Vec::new()),
            retention,
        }
    }

    fn prune(&self, jobs: &mut Vec<Arc<Job>>) {
        jobs.retain(|j| {
            j.finished_at()
                .is_none_or(|at| at.elapsed() < self.retention)
        });
    }

    /// Drop the oldest finished job `matches` picks; whether there was one.
    fn evict_oldest_finished(jobs: &mut Vec<Arc<Job>>, matches: impl Fn(&Job) -> bool) -> bool {
        match jobs.iter().position(|j| matches(j) && j.is_finished()) {
            Some(pos) => {
                jobs.remove(pos);
                true
            }
            None => false,
        }
    }

    /// Make job `id`. At the caps, the oldest finished results make room;
    /// when every slot holds a job still waiting or running, none is made.
    pub fn create(
        &self,
        id: &str,
        session_id: Option<String>,
        tool_call_id: Option<String>,
        one_shot: bool,
    ) -> Result<Arc<Job>, JobsFull> {
        let mut jobs = self.jobs.lock_or_recover();
        self.prune(&mut jobs);
        let same_session = |j: &Job| j.session_id == session_id;
        if jobs.iter().filter(|j| same_session(j)).count() >= MAX_SESSION_JOBS
            && !Self::evict_oldest_finished(&mut jobs, same_session)
        {
            return Err(JobsFull::Session);
        }
        if jobs.len() >= MAX_JOBS && !Self::evict_oldest_finished(&mut jobs, |_| true) {
            return Err(JobsFull::Total);
        }
        let job = Arc::new(Job::new(id, session_id.clone(), tool_call_id, one_shot));
        jobs.push(job.clone());
        Ok(job)
    }

    /// Job `id`, unless it's unknown or its result expired.
    pub fn get(&self, id: &str) -> Option<Arc<Job>> {
        let mut jobs = self.jobs.lock_or_recover();
        self.prune(&mut jobs);
        jobs.iter().find(|j| j.id == id).cloned()
    }

    /// The newest job made for tool call `tool_call_id`.
    pub fn for_call(&self, tool_call_id: &str) -> Option<Arc<Job>> {
        self.jobs
            .lock_or_recover()
            .iter()
            .rev()
            .find(|j| j.tool_call_id.as_deref() == Some(tool_call_id))
            .cloned()
    }

    #[cfg(test)]
    pub fn count(&self) -> usize {
        let mut jobs = self.jobs.lock_or_recover();
        self.prune(&mut jobs);
        jobs.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use kage_core::host_bridge::ResponseStatus;

    fn jobs() -> Jobs {
        Jobs::default()
    }

    fn make(jobs: &Jobs, id: &str, session: Option<&str>) -> Arc<Job> {
        jobs.create(id, session.map(str::to_string), None, false)
            .unwrap()
    }

    #[test]
    fn a_job_reports_where_it_is_until_it_finishes() {
        let jobs = jobs();
        let job = make(&jobs, "hc-1", Some("s1"));
        assert_eq!(job.progress().unwrap().state, JobState::WaitingForApproval);
        job.set_waiting(Instant::now() + Duration::from_secs(300));
        let p = job.progress().unwrap();
        assert_eq!(p.state, JobState::WaitingForApproval);
        assert!(p.approval_expires_in_ms.unwrap() > 290_000);

        job.set_running(Arc::new(AtomicBool::new(false)));
        job.push_output("==> Downloading\n 50%\r100%\n");
        job.push_output("==> Pouring jq\n");
        let p = job.progress().unwrap();
        assert_eq!(p.state, JobState::Running);
        assert_eq!(
            p.output_tail.as_deref(),
            Some("==> Downloading\n100%\n==> Pouring jq")
        );
        assert!(p.running_ms.is_some());

        assert!(job.finish(HostCommandResponse::denied("")));
        assert!(
            !job.finish(HostCommandResponse::error("late")),
            "first wins"
        );
        assert!(job.progress().is_none());
        assert_eq!(job.result().unwrap().status, ResponseStatus::Denied);
        // A finished job doesn't go back.
        job.set_waiting(Instant::now());
        job.set_running(Arc::new(AtomicBool::new(false)));
        assert!(job.is_finished());
    }

    #[test]
    fn running_output_is_bounded() {
        let jobs = jobs();
        let job = make(&jobs, "hc-1", None);
        job.set_running(Arc::new(AtomicBool::new(false)));
        for _ in 0..100 {
            job.push_output(&"x".repeat(1024));
        }
        assert!(job.lock().output.len() <= OUTPUT_KEEP_BYTES * 2);
    }

    #[test]
    fn a_wait_ends_with_the_result_the_deadline_or_the_requester() {
        let jobs = jobs();
        let job = make(&jobs, "hc-1", None);

        // The deadline: "pending", with where it is.
        let started = Instant::now();
        let waited = job.wait(Some(Instant::now() + Duration::from_millis(300)), || false);
        assert!(matches!(waited, Waited::Pending(_)), "{waited:?}");
        assert!(started.elapsed() >= Duration::from_millis(300));

        // The requester went away.
        let waited = job.wait(None, || true);
        assert_eq!(waited, Waited::GaveUp);

        // The result, as soon as it's there.
        let finisher = {
            let job = job.clone();
            std::thread::spawn(move || {
                std::thread::sleep(Duration::from_millis(100));
                job.finish(HostCommandResponse::approval_timed_out());
            })
        };
        let started = Instant::now();
        let waited = job.wait(Some(Instant::now() + Duration::from_secs(10)), || false);
        finisher.join().unwrap();
        assert!(
            matches!(waited, Waited::Finished(ref r) if r.status == ResponseStatus::ApprovalTimedOut)
        );
        assert!(started.elapsed() < Duration::from_secs(5));

        // And again, for every later poll.
        assert!(matches!(job.wait(None, || true), Waited::Finished(_)));
    }

    #[test]
    fn once_the_agent_has_the_id_nothing_abandons_the_job() {
        let jobs = jobs();
        let job = make(&jobs, "hc-1", Some("s1"));
        job.mark_id_given();
        job.abandon();
        assert!(!job.is_abandoned());

        let other = make(&jobs, "hc-2", Some("s1"));
        other.abandon();
        assert!(other.is_abandoned());
        // Given after the fact (a reply raced the call's end): it holds.
        other.mark_id_given();
        assert!(!other.is_abandoned());
    }

    #[test]
    fn only_a_one_shot_requester_leaving_stops_the_run() {
        let jobs = jobs();
        let v2 = make(&jobs, "hc-1", None);
        let cancel = Arc::new(AtomicBool::new(false));
        v2.set_running(cancel.clone());
        assert!(!v2.requester_left());
        assert!(!cancel.load(Ordering::SeqCst));
        assert!(!v2.run_should_stop());

        let v1 = jobs.create("hc-2", None, None, true).unwrap();
        // Left before the run started: it stops as soon as it does.
        assert!(!v1.requester_left());
        assert!(v1.run_should_stop());
        assert!(v1.is_abandoned());
        let cancel = Arc::new(AtomicBool::new(false));
        v1.set_running(cancel.clone());
        assert!(v1.requester_left());
        assert!(cancel.load(Ordering::SeqCst));
    }

    #[test]
    fn jobs_are_found_by_id_and_by_tool_call() {
        let jobs = jobs();
        let job = jobs
            .create("hc-1", Some("s1".into()), Some("call-1".into()), false)
            .unwrap();
        assert!(Arc::ptr_eq(&jobs.get("hc-1").unwrap(), &job));
        assert!(Arc::ptr_eq(&jobs.for_call("call-1").unwrap(), &job));
        assert!(jobs.get("hc-2").is_none());
        assert!(jobs.for_call("call-2").is_none());
    }

    #[test]
    fn a_finished_result_is_kept_for_its_retention_then_dropped() {
        let jobs = Jobs::with_retention(Duration::from_millis(200));
        let done = make(&jobs, "hc-done", None);
        let live = make(&jobs, "hc-live", None);
        done.finish(HostCommandResponse::denied(""));
        assert!(jobs.get("hc-done").is_some());
        std::thread::sleep(Duration::from_millis(300));
        assert!(jobs.get("hc-done").is_none(), "expired");
        // A job still going never expires.
        assert!(jobs.get("hc-live").is_some());
        assert_eq!(jobs.count(), 1);
        drop(live);
    }

    #[test]
    fn a_session_can_hold_only_so_many_and_old_results_make_room() {
        let jobs = jobs();
        let mut live = Vec::new();
        for i in 0..MAX_SESSION_JOBS {
            live.push(make(&jobs, &format!("hc-{i}"), Some("s1")));
        }
        assert_eq!(
            jobs.create("hc-x", Some("s1".into()), None, false).err(),
            Some(JobsFull::Session)
        );
        // Another session isn't held up by it.
        assert!(jobs.create("hc-y", Some("s2".into()), None, false).is_ok());
        // A finished result of that session makes room: the oldest goes.
        live[3].finish(HostCommandResponse::denied(""));
        live[5].finish(HostCommandResponse::denied(""));
        assert!(jobs.create("hc-x", Some("s1".into()), None, false).is_ok());
        assert!(jobs.get("hc-3").is_none());
        assert!(jobs.get("hc-5").is_some());
    }

    #[test]
    fn the_total_is_bounded_too() {
        let jobs = jobs();
        let mut live = Vec::new();
        for i in 0..MAX_JOBS {
            let session = format!("s{}", i % (MAX_JOBS / MAX_SESSION_JOBS + 1));
            live.push(make(&jobs, &format!("hc-{i}"), Some(&session)));
        }
        assert_eq!(
            jobs.create("hc-x", Some("new".into()), None, false).err(),
            Some(JobsFull::Total)
        );
        live[0].finish(HostCommandResponse::denied(""));
        assert!(jobs.create("hc-x", Some("new".into()), None, false).is_ok());
        assert_eq!(jobs.count(), MAX_JOBS);
        assert!(jobs.get("hc-0").is_none());
    }
}
