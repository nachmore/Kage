//! Transport for the host-command bridge ([`crate::host_bridge`]): a local
//! endpoint only the current user can reach, authenticated by the OS.
//!
//! - macOS / Linux: an AF_UNIX stream socket, `host.sock`, in
//!   `<local data dir>/kage/bridge/` (a directory created `0700` and
//!   checked to be ours and not a symlink before anything is bound in it).
//!   Each connection's peer uid (`getpeereid` / `SO_PEERCRED`) must equal
//!   our effective uid; the client checks the server's the same way.
//! - Windows: a named pipe, `\\.\pipe\kage-host-bridge-<user SID>`, whose
//!   security descriptor grants only the current user (and SYSTEM),
//!   created as the first instance and refusing remote clients. The
//!   client process's token user must equal ours.
//!
//! No TCP, no listening network port, no token: the sidecar and Kage find
//! the endpoint the same way ([`default_endpoint`]), so nothing is passed
//! through the agent's `mcp.json`.

use std::io;
#[cfg(unix)]
use std::path::PathBuf;
use std::sync::atomic::AtomicBool;
use std::time::Duration;

/// Where the bridge listens.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Endpoint {
    /// An AF_UNIX socket at `socket`, inside the private directory `dir`.
    #[cfg(unix)]
    Unix { dir: PathBuf, socket: PathBuf },
    /// A named pipe (`\\.\pipe\…`).
    #[cfg(windows)]
    Pipe(String),
}

impl std::fmt::Display for Endpoint {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            #[cfg(unix)]
            Endpoint::Unix { socket, .. } => write!(f, "{}", socket.display()),
            #[cfg(windows)]
            Endpoint::Pipe(name) => write!(f, "{name}"),
        }
    }
}

/// File name of the Unix socket.
#[cfg(unix)]
pub const SOCKET_NAME: &str = "host.sock";

/// The bridge's directory under a local data dir: `<it>/kage/bridge`.
#[cfg(unix)]
pub fn bridge_dir_in(data_local_dir: &std::path::Path) -> PathBuf {
    data_local_dir.join("kage").join("bridge")
}

/// The endpoint Kage listens on and the sidecar connects to, computed the
/// same way on both sides: Unix from [`crate::paths::data_local_dir`]
/// (which honours test isolation), Windows from the user's SID.
pub fn default_endpoint() -> Option<Endpoint> {
    #[cfg(unix)]
    {
        let dir = bridge_dir_in(&crate::paths::data_local_dir()?);
        let socket = dir.join(SOCKET_NAME);
        Some(Endpoint::Unix { dir, socket })
    }
    #[cfg(windows)]
    {
        crate::os::platform::host_bridge::pipe_name().map(Endpoint::Pipe)
    }
}

/// A connection, verified to be with a process of the current user.
pub struct BridgeStream(pub(crate) crate::os::platform::host_bridge::Stream);

impl BridgeStream {
    /// Whether the other end has gone (closed its end, or the process
    /// died). Doesn't consume anything a well-behaved peer sends.
    pub fn peer_closed(&self) -> bool {
        self.0.peer_closed()
    }

    /// Bound how long a read may block (`None`: forever). A no-op on
    /// Windows, whose pipe handles have no read timeout.
    pub fn set_read_timeout(&self, timeout: Option<Duration>) -> io::Result<()> {
        self.0.set_read_timeout(timeout)
    }

    /// The peer's process id, when the OS says.
    pub fn peer_pid(&self) -> Option<u32> {
        self.0.peer_pid()
    }
}

impl io::Read for BridgeStream {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        self.0.read(buf)
    }
}

impl io::Write for BridgeStream {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.0.write(buf)
    }
    fn flush(&mut self) -> io::Result<()> {
        self.0.flush()
    }
}

/// The listening end. Dropping it removes the socket (Unix) when it is
/// still the one we bound.
pub struct BridgeListener(crate::os::platform::host_bridge::Listener);

impl BridgeListener {
    /// Listen on `endpoint`. Refuses (with an error saying why) when the
    /// endpoint can't be made private to the current user, or another
    /// live process already listens there.
    pub fn bind(endpoint: &Endpoint) -> io::Result<Self> {
        crate::os::platform::host_bridge::Listener::bind(endpoint).map(Self)
    }

    /// The next connection from a process of the current user. Others are
    /// dropped (and logged) without reading anything from them. Returns
    /// `Ok(None)` once `stop` is set (checked at least every half second;
    /// [`Self::wake`] makes it immediate).
    pub fn accept(&self, stop: &AtomicBool) -> io::Result<Option<BridgeStream>> {
        self.0.accept(stop).map(|s| s.map(BridgeStream))
    }

    /// Unblock a pending [`Self::accept`] after `stop` was set.
    pub fn wake(&self) {
        self.0.wake();
    }

    /// Remove the endpoint now (Unix: unlink the socket if it is still
    /// ours; Windows: nothing — the pipe goes with its handles), so no
    /// new client finds it while the listener winds down. Idempotent.
    pub fn remove_endpoint(&self) {
        self.0.remove_endpoint();
    }
}

/// Connect to the bridge at `endpoint` and check that the server runs as
/// the current user.
pub fn connect(endpoint: &Endpoint) -> io::Result<BridgeStream> {
    crate::os::platform::host_bridge::connect(endpoint).map(BridgeStream)
}
