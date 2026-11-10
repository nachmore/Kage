//! The `kage-sync` thread and the handle the app talks to it through
//! (docs/design/SYNC.md §14). Requests travel over a channel and wait with
//! a timeout; a wake never blocks.
//!
//! | Trigger | What runs |
//! |---|---|
//! | Start | The thread sleeps `delay`, then opens the engine and ticks once. A request ends the wait early. |
//! | Local change | Capture 2 s after the last change (10 s on battery) |
//! | Folder change | Ingest 3 s after the last watcher event |
//! | Poll | Every 5 minutes on mains power, 15 on battery, 30 on low battery or in power-saving mode |
//! | Focus | Ingest, if the last one was more than 60 s ago |
//! | Wake from sleep | Ingest after 10 s |
//! | A pending publish | When the engine's debounce ends (`next_deadline`) |
//! | Exit | `flush_for_exit`: capture and publish own edits within the budget |

use super::engine::{EngineEnv, SyncEngine, Wake};
use super::sources::Source;
use super::types::Origin;
use super::{unavailable, EngineReply, EngineRequest};
use crate::error::{AppError, ErrorKind};
use crate::os::power::PowerState;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, RecvTimeoutError};
use std::sync::{Arc, OnceLock};
use std::time::{Duration, Instant};

/// Debounce of a local change before capture.
const CAPTURE_DELAY: Duration = Duration::from_secs(2);
const CAPTURE_DELAY_BATTERY: Duration = Duration::from_secs(10);
/// Debounce of folder events before an ingest.
const FOLDER_DELAY: Duration = Duration::from_secs(3);
/// After waking from sleep.
const RESUME_DELAY: Duration = Duration::from_secs(10);
/// A focus ingests when the last ingest is older than this.
const FOCUS_MIN_GAP_MS: i64 = 60_000;

/// What the app sends the engine thread.
enum Msg {
    Request(
        EngineRequest,
        mpsc::SyncSender<Result<EngineReply, AppError>>,
    ),
    Wake(Wake),
    Flush(Duration, mpsc::SyncSender<()>),
}

#[derive(Default)]
struct Inner {
    tx: OnceLock<mpsc::Sender<Msg>>,
    started: AtomicBool,
}

/// A cheap handle to the engine thread. Built on the Tauri Builder before
/// the thread exists.
#[derive(Clone, Default)]
pub struct SyncHandle {
    inner: Arc<Inner>,
}

impl SyncHandle {
    /// A handle with no thread yet: every request answers
    /// `sync_unavailable` until [`SyncHandle::start`] runs.
    pub fn pending() -> Self {
        Self::default()
    }

    /// Start the engine thread after `delay`. `make` runs on that thread.
    /// A second call is a no-op. Does nothing in a build sync isn't offered
    /// in ([`super::sync_available`]).
    pub fn start(
        &self,
        make: impl FnOnce() -> anyhow::Result<(EngineEnv, Vec<Box<dyn Source>>)> + Send + 'static,
        delay: Duration,
    ) {
        if !super::sync_available() || self.inner.started.swap(true, Ordering::SeqCst) {
            return;
        }
        let (tx, rx) = mpsc::channel();
        let watcher_tx = tx.clone();
        if self.inner.tx.set(tx).is_err() {
            return;
        }
        let spawned = std::thread::Builder::new()
            .name("kage-sync".into())
            .spawn(move || run(rx, watcher_tx, make, delay));
        if let Err(e) = spawned {
            log::warn!("sync: couldn't start the sync thread: {e}");
        }
    }

    /// Ask the engine and wait up to `timeout`.
    pub fn request(&self, req: EngineRequest, timeout: Duration) -> Result<EngineReply, AppError> {
        let Some(tx) = self.inner.tx.get() else {
            return Err(unavailable());
        };
        let (rtx, rrx) = mpsc::sync_channel(1);
        tx.send(Msg::Request(req, rtx)).map_err(|_| unavailable())?;
        match rrx.recv_timeout(timeout) {
            Ok(r) => r,
            Err(RecvTimeoutError::Timeout) => Err(AppError::keyed(
                ErrorKind::Timeout,
                "errors.sync.timeout",
                &[],
            )),
            Err(RecvTimeoutError::Disconnected) => Err(unavailable()),
        }
    }

    /// Wake the engine. Never blocks.
    pub fn wake(&self, w: Wake) {
        if let Some(tx) = self.inner.tx.get() {
            let _ = tx.send(Msg::Wake(w));
        }
    }

    /// Flush own edits before exit, within `budget`.
    pub fn flush_for_exit(&self, budget: Duration) {
        let Some(tx) = self.inner.tx.get() else {
            return;
        };
        let (rtx, rrx) = mpsc::sync_channel(1);
        if tx.send(Msg::Flush(budget, rtx)).is_ok() {
            let _ = rrx.recv_timeout(budget + Duration::from_millis(100));
        }
    }

    /// Whether the thread was started.
    pub fn started(&self) -> bool {
        self.inner.started.load(Ordering::SeqCst)
    }
}

static GLOBAL: OnceLock<SyncHandle> = OnceLock::new();

/// Make `h` the handle [`notify`] wakes. The first call wins.
pub fn install_global(h: SyncHandle) {
    let _ = GLOBAL.set(h);
}

/// The installed handle, for commands that have no `AppState` to reach it
/// through (an extension's `context.sync` calls, WP-S5).
pub fn global_handle() -> Option<SyncHandle> {
    GLOBAL.get().cloned()
}

/// A local change happened in `source` (a config save, a steering write,
/// extension data). A no-op until a handle is installed.
pub fn notify(source: &'static str, origin: Origin) {
    if let Some(h) = GLOBAL.get() {
        h.wake(Wake::Local { source, origin });
    }
}

/// The poll interval for this power state (§14.1).
fn poll_every(power: PowerState, saving: bool) -> Duration {
    match power {
        _ if saving => Duration::from_secs(30 * 60),
        PowerState::LowBattery => Duration::from_secs(30 * 60),
        PowerState::Battery => Duration::from_secs(15 * 60),
        PowerState::AC | PowerState::Unknown => Duration::from_secs(5 * 60),
    }
}

/// Answer every request with `sync_unavailable` (the engine didn't open).
fn serve_unavailable(rx: mpsc::Receiver<Msg>) {
    while let Ok(msg) = rx.recv() {
        match msg {
            Msg::Request(_, reply) => {
                let _ = reply.send(Err(unavailable()));
            }
            Msg::Flush(_, reply) => {
                let _ = reply.send(());
            }
            Msg::Wake(_) => {}
        }
    }
}

fn run(
    rx: mpsc::Receiver<Msg>,
    self_tx: mpsc::Sender<Msg>,
    make: impl FnOnce() -> anyhow::Result<(EngineEnv, Vec<Box<dyn Source>>)>,
    delay: Duration,
) {
    // Nothing on the startup path: wait first. A request means a window
    // wants sync now.
    let until = Instant::now() + delay;
    let mut early: Vec<Msg> = Vec::new();
    loop {
        let left = until.saturating_duration_since(Instant::now());
        if left.is_zero() {
            break;
        }
        match rx.recv_timeout(left) {
            Ok(Msg::Wake(_)) => {}
            Ok(Msg::Flush(_, reply)) => {
                let _ = reply.send(());
            }
            Ok(m @ Msg::Request(..)) => {
                early.push(m);
                break;
            }
            Err(RecvTimeoutError::Timeout) => break,
            Err(RecvTimeoutError::Disconnected) => return,
        }
    }
    let engine = make().and_then(|(env, sources)| SyncEngine::open(env, sources));
    let mut engine = match engine {
        Ok(e) => e,
        Err(e) => {
            log::warn!("sync: the sync engine didn't start: {e:#}");
            for m in early {
                if let Msg::Request(_, reply) = m {
                    let _ = reply.send(Err(unavailable()));
                }
            }
            serve_unavailable(rx);
            return;
        }
    };
    let mut watch = Watch::default();
    watch.update(&engine, &self_tx);
    engine.tick(Wake::Startup);
    let mut capture_at: Option<Instant> = None;
    let mut ingest_at: Option<Instant> = None;
    let mut last_poll = Instant::now();
    let mut pending: Vec<Msg> = early;
    loop {
        let msg = if let Some(m) = pending.pop() {
            Some(m)
        } else {
            let now_ms = engine.now_ms();
            let poll = poll_every(engine.power(), engine.power_saving());
            let mut next = last_poll + poll;
            for t in [capture_at, ingest_at].into_iter().flatten() {
                next = next.min(t);
            }
            if let Some(d) = engine.next_deadline() {
                // Never spin: a deadline that passed waits half a second.
                let wait = Duration::from_millis((d - now_ms).max(500) as u64);
                next = next.min(Instant::now() + wait);
            }
            let wait = next.saturating_duration_since(Instant::now());
            match rx.recv_timeout(wait) {
                Ok(m) => Some(m),
                Err(RecvTimeoutError::Timeout) => None,
                Err(RecvTimeoutError::Disconnected) => break,
            }
        };
        match msg {
            Some(Msg::Request(req, reply)) => {
                let r = engine.handle(req);
                let _ = reply.send(r);
                watch.update(&engine, &self_tx);
            }
            Some(Msg::Flush(budget, reply)) => {
                engine.flush_for_exit(budget);
                let _ = reply.send(());
            }
            Some(Msg::Wake(w)) => match w {
                Wake::Local { .. } => {
                    engine.note_wake(w);
                    let d = if engine.power() == PowerState::AC {
                        CAPTURE_DELAY
                    } else {
                        CAPTURE_DELAY_BATTERY
                    };
                    capture_at = Some(Instant::now() + d);
                }
                Wake::Folder => ingest_at = Some(Instant::now() + FOLDER_DELAY),
                Wake::Resume => ingest_at = Some(Instant::now() + RESUME_DELAY),
                Wake::Focus => {
                    if engine.ms_since_ingest() > FOCUS_MIN_GAP_MS {
                        engine.tick(Wake::Focus);
                    }
                }
                Wake::Exit => engine.flush_for_exit(Duration::from_millis(1500)),
                other => {
                    engine.tick(other);
                    last_poll = Instant::now();
                }
            },
            None => {
                let now = Instant::now();
                if capture_at.is_some_and(|t| t <= now) {
                    capture_at = None;
                    engine.tick(Wake::Local {
                        source: "scheduler",
                        origin: Origin::User,
                    });
                } else if ingest_at.is_some_and(|t| t <= now) {
                    ingest_at = None;
                    engine.tick(Wake::Folder);
                } else if now >= last_poll + poll_every(engine.power(), engine.power_saving()) {
                    last_poll = now;
                    engine.tick(Wake::Poll);
                } else {
                    // A publish whose debounce ended.
                    engine.tick(Wake::Folder);
                }
            }
        }
    }
}

/// The folder watcher (`notify`), on `Kage Sync/devices/`. Events under
/// this computer's own directory and for temp names are ignored.
#[derive(Default)]
struct Watch {
    dir: Option<PathBuf>,
    watcher: Option<notify::RecommendedWatcher>,
}

impl Watch {
    fn update(&mut self, engine: &SyncEngine, tx: &mpsc::Sender<Msg>) {
        let want = engine.watch_dir();
        if want == self.dir {
            return;
        }
        self.watcher = None;
        self.dir = want.clone();
        let Some((dir, own)) = want.zip(engine.own_dir()) else {
            return;
        };
        let tx = tx.clone();
        let handler = move |res: notify::Result<notify::Event>| {
            let Ok(ev) = res else { return };
            let relevant = ev.paths.iter().any(|p| {
                !p.starts_with(&own)
                    && p.file_name()
                        .and_then(|n| n.to_str())
                        .is_some_and(|n| !n.starts_with("~kage-") && !n.ends_with(".tmp"))
            });
            if relevant {
                let _ = tx.send(Msg::Wake(Wake::Folder));
            }
        };
        use notify::Watcher;
        match notify::recommended_watcher(handler) {
            Ok(mut w) => {
                if w.watch(&dir, notify::RecursiveMode::Recursive).is_ok() {
                    self.watcher = Some(w);
                }
            }
            Err(e) => log::info!("sync: no folder watcher (the poll still runs): {e}"),
        }
    }
}

impl SyncEngine {
    /// Note a wake without ticking: its source is dirty, its origin counts.
    pub fn note_wake(&mut self, w: Wake) {
        if let Wake::Local { source, origin } = w {
            self.rt.dirty_sources.insert(source);
            let prev = self.rt.pending_origin;
            self.rt.pending_origin = Some(match (prev, origin) {
                (Some(Origin::Import), _) | (_, Origin::Import) => Origin::Import,
                (Some(Origin::Reset), _) | (_, Origin::Reset) => Origin::Reset,
                (Some(p), _) => p,
                (None, o) => o,
            });
        }
    }

    pub fn now_ms(&self) -> i64 {
        self.env.clock.now_ms()
    }

    fn power(&self) -> PowerState {
        self.env.clock.power()
    }

    /// Power-saving mode of automations also slows the poll.
    fn power_saving(&self) -> bool {
        self.env.config.snapshot().1.automation_power.mode == "saving"
    }

    fn ms_since_ingest(&self) -> i64 {
        self.rt
            .last_ingest_ms
            .map_or(i64::MAX, |t| self.now_ms() - t)
    }

    /// The directory the watcher watches: `Kage Sync/devices/`.
    pub fn watch_dir(&self) -> Option<PathBuf> {
        self.core.as_ref().map(|c| c.layout.devices_dir())
    }

    fn own_dir(&self) -> Option<PathBuf> {
        self.core.as_ref().map(|c| c.layout.device_dir(c.me))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn polls_slow_down_on_battery() {
        assert_eq!(poll_every(PowerState::AC, false), Duration::from_secs(300));
        assert_eq!(
            poll_every(PowerState::Battery, false),
            Duration::from_secs(900)
        );
        assert_eq!(
            poll_every(PowerState::LowBattery, false),
            Duration::from_secs(1800)
        );
        assert_eq!(poll_every(PowerState::AC, true), Duration::from_secs(1800));
    }

    #[test]
    fn a_pending_handle_never_blocks() {
        let h = SyncHandle::pending();
        h.wake(Wake::Manual);
        assert!(!h.started());
        assert!(h
            .request(EngineRequest::Status, Duration::from_millis(1))
            .is_err());
    }

    struct Port;

    impl super::super::engine::ConfigPort for Port {
        fn snapshot(&self) -> (u64, crate::config::Config) {
            (1, crate::config::Config::default())
        }
        fn commit(
            &self,
            _: u64,
            _: crate::config::Config,
            _: Origin,
        ) -> Result<u64, super::super::engine::CommitError> {
            Ok(2)
        }
        fn load_was_reset(&self) -> super::super::engine::LoadReset {
            super::super::engine::LoadReset::None
        }
        fn saves_suspended(&self) -> bool {
            false
        }
        fn installed(&self) -> Vec<crate::extensions::InstalledItem> {
            Vec::new()
        }
    }

    struct Events;

    impl super::super::engine::HostEvents for Events {
        fn status_changed(&self, _: &super::super::status::SyncStatus) {}
        fn conflicts_changed(&self, _: usize) {}
        fn applied(&self, _: &[String]) {}
        fn extension_data_changed(&self, _: &str, _: &[String]) {}
        fn extension_sync_changes(&self, _: &str, _: &[String], _: &str) {}
        fn notify_user(&self, _: &str) {}
        fn track(&self, _: &'static str, _: Vec<(&'static str, String)>) {}
    }

    /// The thread starts after its delay (or at the first request),
    /// answers requests, takes wakes without blocking and flushes at exit.
    #[test]
    fn the_engine_thread_answers_requests() {
        use super::super::testing::{FakeClock, FakeCloudFiles, FaultyFs, MemoryKeyStore};
        let dir = tempfile::tempdir().unwrap();
        let local = dir.path().to_path_buf();
        let h = SyncHandle::pending();
        h.start(
            move || {
                Ok((
                    EngineEnv {
                        local_dir: local,
                        os: super::super::types::OsKind::Linux,
                        kage_version: "test".into(),
                        fingerprint: "fp".into(),
                        fs: Arc::new(FaultyFs::new()),
                        cloud: Arc::new(FakeCloudFiles::new()),
                        keys: Arc::new(MemoryKeyStore::new()),
                        clock: Arc::new(FakeClock::new(1_762_682_400_000)),
                        events: Arc::new(Events),
                        config: Arc::new(Port),
                    },
                    Vec::new(),
                ))
            },
            Duration::from_secs(30),
        );
        assert!(h.started());
        let t = Instant::now();
        h.wake(Wake::Local {
            source: "config",
            origin: Origin::User,
        });
        assert!(
            t.elapsed() < Duration::from_millis(50),
            "a wake never blocks"
        );
        // A request ends the start delay.
        match h.request(EngineRequest::Status, Duration::from_secs(10)) {
            Ok(EngineReply::Status(s)) => {
                assert_eq!(s.state, super::super::status::SyncState::Off)
            }
            Ok(_) => panic!("not a status"),
            Err(e) => panic!("{e}"),
        }
        assert!(h
            .request(EngineRequest::ListDevices, Duration::from_secs(10))
            .is_err());
        h.flush_for_exit(Duration::from_millis(200));
        // A second start is a no-op.
        h.start(|| anyhow::bail!("not called"), Duration::ZERO);
    }
}
