//! macOS / Linux transport for the host-command bridge: an AF_UNIX stream
//! socket in a directory private to the current user. See
//! [`crate::os::host_bridge`] for the contract.

use std::fs;
use std::io::{self, Read, Write};
use std::os::unix::fs::{DirBuilderExt, FileTypeExt, MetadataExt, PermissionsExt};
use std::os::unix::io::AsRawFd;
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use crate::os::host_bridge::Endpoint;

/// `sizeof(sockaddr_un.sun_path)`, including its terminating NUL.
#[cfg(target_os = "macos")]
const SUN_PATH_MAX: usize = 104;
#[cfg(not(target_os = "macos"))]
const SUN_PATH_MAX: usize = 108;

/// How often a blocked `accept` looks at its stop flag.
const ACCEPT_POLL_MS: i32 = 500;

fn euid() -> u32 {
    // SAFETY: geteuid has no preconditions and cannot fail.
    unsafe { libc::geteuid() }
}

fn refuse(what: &Path, why: impl std::fmt::Display) -> io::Error {
    io::Error::new(
        io::ErrorKind::PermissionDenied,
        format!("{}: {why}", what.display()),
    )
}

/// Whether `dir` may hold the bridge for `uid`: a real directory (not a
/// symlink), owned by `uid`, with no permissions for group or others.
pub fn check_private_dir(dir: &Path, uid: u32) -> io::Result<()> {
    let meta = fs::symlink_metadata(dir)?;
    if meta.file_type().is_symlink() {
        return Err(refuse(dir, "is a symlink"));
    }
    if !meta.is_dir() {
        return Err(refuse(dir, "is not a directory"));
    }
    if meta.uid() != uid {
        return Err(refuse(dir, format!("is owned by uid {}", meta.uid())));
    }
    let mode = meta.mode() & 0o777;
    if mode & 0o077 != 0 {
        return Err(refuse(
            dir,
            format!("has permissions {mode:o}, wider than 700"),
        ));
    }
    Ok(())
}

/// The directory that holds `dir` must not let others swap it out: not
/// writable by group or others unless sticky (like `/tmp`).
fn check_parent(dir: &Path) -> io::Result<()> {
    let Some(parent) = dir.parent() else {
        return Ok(());
    };
    let meta = fs::metadata(parent)?;
    let mode = meta.mode();
    if mode & 0o022 != 0 && mode & 0o1000 == 0 {
        return Err(refuse(
            parent,
            format!("is writable by others (permissions {:o})", mode & 0o7777),
        ));
    }
    Ok(())
}

/// Create `dir` (and its parents) with mode `0700` if it doesn't exist,
/// then check it is private to `uid` ([`check_private_dir`]). An existing
/// directory is never loosened or tightened: one that isn't private is
/// refused, so a misconfiguration is reported rather than papered over.
pub fn ensure_private_dir(dir: &Path, uid: u32) -> io::Result<()> {
    if let Some(parent) = dir.parent() {
        fs::create_dir_all(parent)?;
    }
    check_parent(dir)?;
    match fs::DirBuilder::new().mode(0o700).create(dir) {
        Ok(()) => {}
        Err(e) if e.kind() == io::ErrorKind::AlreadyExists => {}
        Err(e) => return Err(e),
    }
    check_private_dir(dir, uid)
}

/// Clear the way for binding `socket`: nothing there is fine; a stale
/// socket of ours (nobody accepts on it) is removed; anything else — a
/// symlink, a file that isn't a socket, someone else's socket, or one a
/// live process listens on — is refused.
fn clear_stale_socket(socket: &Path, uid: u32) -> io::Result<()> {
    let meta = match fs::symlink_metadata(socket) {
        Ok(m) => m,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(e) => return Err(e),
    };
    if meta.file_type().is_symlink() {
        return Err(refuse(socket, "is a symlink"));
    }
    if !meta.file_type().is_socket() {
        return Err(refuse(socket, "exists and is not a socket"));
    }
    if meta.uid() != uid {
        return Err(refuse(socket, format!("is owned by uid {}", meta.uid())));
    }
    match UnixStream::connect(socket) {
        Ok(_) => Err(io::Error::new(
            io::ErrorKind::AddrInUse,
            format!("{}: another process is listening", socket.display()),
        )),
        Err(_) => {
            log::info!("Host bridge: removing a stale socket");
            fs::remove_file(socket)
        }
    }
}

/// The effective uid of the process at the other end of `stream`.
#[cfg(not(target_os = "linux"))]
fn peer_uid(stream: &UnixStream) -> io::Result<u32> {
    let mut uid: libc::uid_t = 0;
    let mut gid: libc::gid_t = 0;
    // SAFETY: a valid socket fd and two out-pointers to locals.
    let r = unsafe { libc::getpeereid(stream.as_raw_fd(), &mut uid, &mut gid) };
    if r != 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(uid)
}

#[cfg(target_os = "linux")]
fn peer_ucred(stream: &UnixStream) -> io::Result<libc::ucred> {
    let mut cred = libc::ucred {
        pid: 0,
        uid: 0,
        gid: 0,
    };
    let mut len = std::mem::size_of::<libc::ucred>() as libc::socklen_t;
    // SAFETY: a valid socket fd; `cred` and `len` describe a buffer of the
    // size SO_PEERCRED writes.
    let r = unsafe {
        libc::getsockopt(
            stream.as_raw_fd(),
            libc::SOL_SOCKET,
            libc::SO_PEERCRED,
            (&mut cred as *mut libc::ucred).cast(),
            &mut len,
        )
    };
    if r != 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(cred)
}

#[cfg(target_os = "linux")]
fn peer_uid(stream: &UnixStream) -> io::Result<u32> {
    peer_ucred(stream).map(|c| c.uid)
}

#[cfg(target_os = "linux")]
fn peer_pid_of(stream: &UnixStream) -> Option<u32> {
    peer_ucred(stream)
        .ok()
        .and_then(|c| u32::try_from(c.pid).ok())
        .filter(|p| *p > 0)
}

#[cfg(target_os = "macos")]
fn peer_pid_of(stream: &UnixStream) -> Option<u32> {
    let mut pid: libc::pid_t = 0;
    let mut len = std::mem::size_of::<libc::pid_t>() as libc::socklen_t;
    // SAFETY: a valid socket fd; `pid` and `len` describe a pid_t buffer.
    let r = unsafe {
        libc::getsockopt(
            stream.as_raw_fd(),
            libc::SOL_LOCAL,
            libc::LOCAL_PEERPID,
            (&mut pid as *mut libc::pid_t).cast(),
            &mut len,
        )
    };
    if r != 0 {
        return None;
    }
    u32::try_from(pid).ok().filter(|p| *p > 0)
}

#[cfg(not(any(target_os = "linux", target_os = "macos")))]
fn peer_pid_of(_stream: &UnixStream) -> Option<u32> {
    None
}

/// One verified connection.
pub struct Stream {
    inner: UnixStream,
}

impl Stream {
    pub fn peer_closed(&self) -> bool {
        let fd = self.inner.as_raw_fd();
        let mut pfd = libc::pollfd {
            fd,
            events: libc::POLLIN,
            revents: 0,
        };
        // SAFETY: one valid pollfd, zero timeout.
        let r = unsafe { libc::poll(&mut pfd, 1, 0) };
        if r <= 0 {
            return false;
        }
        if pfd.revents & (libc::POLLHUP | libc::POLLERR | libc::POLLNVAL) != 0 {
            return true;
        }
        let mut byte = [0u8; 1];
        // SAFETY: a valid fd and a one-byte buffer; MSG_PEEK leaves the
        // data where it is.
        let n = unsafe {
            libc::recv(
                fd,
                byte.as_mut_ptr().cast(),
                1,
                libc::MSG_PEEK | libc::MSG_DONTWAIT,
            )
        };
        match n {
            0 => true,
            n if n > 0 => false,
            _ => {
                let e = io::Error::last_os_error();
                !matches!(
                    e.kind(),
                    io::ErrorKind::WouldBlock | io::ErrorKind::Interrupted
                )
            }
        }
    }

    pub fn set_read_timeout(&self, timeout: Option<Duration>) -> io::Result<()> {
        self.inner.set_read_timeout(timeout)
    }

    pub fn peer_pid(&self) -> Option<u32> {
        peer_pid_of(&self.inner)
    }

    pub fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        self.inner.read(buf)
    }

    pub fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.inner.write(buf)
    }

    pub fn flush(&mut self) -> io::Result<()> {
        self.inner.flush()
    }
}

/// The listening socket.
pub struct Listener {
    inner: UnixListener,
    socket: PathBuf,
    /// `(dev, ino)` of the socket we bound, so drop removes only ours.
    id: (u64, u64),
    /// The uid every peer must have (our effective uid; tests vary it).
    uid: u32,
}

impl Listener {
    pub fn bind(endpoint: &Endpoint) -> io::Result<Self> {
        Self::bind_for(endpoint, euid())
    }

    /// [`Self::bind`], accepting only peers running as `peer_uid`.
    pub(crate) fn bind_for(endpoint: &Endpoint, peer_uid: u32) -> io::Result<Self> {
        let Endpoint::Unix { dir, socket } = endpoint;
        if socket.parent() != Some(dir.as_path()) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "the socket must be directly inside the bridge directory",
            ));
        }
        if socket.as_os_str().len() >= SUN_PATH_MAX {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                format!(
                    "socket path is {} bytes, longer than the {} AF_UNIX allows",
                    socket.as_os_str().len(),
                    SUN_PATH_MAX - 1
                ),
            ));
        }
        let owner = euid();
        ensure_private_dir(dir, owner)?;
        clear_stale_socket(socket, owner)?;
        let inner = UnixListener::bind(socket)?;
        fs::set_permissions(socket, fs::Permissions::from_mode(0o600))?;
        let meta = fs::symlink_metadata(socket)?;
        Ok(Self {
            inner,
            socket: socket.clone(),
            id: (meta.dev(), meta.ino()),
            uid: peer_uid,
        })
    }

    pub fn accept(&self, stop: &AtomicBool) -> io::Result<Option<Stream>> {
        let fd = self.inner.as_raw_fd();
        loop {
            if stop.load(Ordering::SeqCst) {
                return Ok(None);
            }
            let mut pfd = libc::pollfd {
                fd,
                events: libc::POLLIN,
                revents: 0,
            };
            // SAFETY: one valid pollfd and a bounded timeout.
            let r = unsafe { libc::poll(&mut pfd, 1, ACCEPT_POLL_MS) };
            if r < 0 {
                let e = io::Error::last_os_error();
                if e.kind() == io::ErrorKind::Interrupted {
                    continue;
                }
                return Err(e);
            }
            if r == 0 || stop.load(Ordering::SeqCst) {
                continue;
            }
            let stream = match self.inner.accept() {
                Ok((stream, _)) => stream,
                Err(e)
                    if matches!(
                        e.kind(),
                        io::ErrorKind::Interrupted
                            | io::ErrorKind::WouldBlock
                            | io::ErrorKind::ConnectionAborted
                    ) =>
                {
                    continue
                }
                Err(e) => return Err(e),
            };
            // A BSD socket inherits O_NONBLOCK from its listener; ours is
            // blocking, but say so rather than rely on it.
            stream.set_nonblocking(false)?;
            match peer_uid(&stream) {
                Ok(uid) if uid == self.uid => return Ok(Some(Stream { inner: stream })),
                Ok(uid) => {
                    log::warn!("Host bridge: dropped a connection from uid {uid}");
                }
                Err(e) => {
                    log::warn!("Host bridge: dropped a connection (peer unknown: {e})");
                }
            }
        }
    }

    /// Wake a blocked accept so it sees its stop flag now: connecting to
    /// ourselves makes the poll return.
    pub fn wake(&self) {
        let _ = UnixStream::connect(&self.socket);
    }
}

impl Listener {
    /// Remove the socket file, if it is still the one we bound (a newer
    /// Kage may have replaced it). Idempotent.
    pub fn remove_endpoint(&self) {
        if let Ok(meta) = fs::symlink_metadata(&self.socket) {
            if (meta.dev(), meta.ino()) == self.id && meta.file_type().is_socket() {
                let _ = fs::remove_file(&self.socket);
            }
        }
    }
}

impl Drop for Listener {
    fn drop(&mut self) {
        self.remove_endpoint();
    }
}

pub fn connect(endpoint: &Endpoint) -> io::Result<Stream> {
    connect_for(endpoint, euid())
}

/// [`connect`], accepting only a server running as `server_uid`.
pub(crate) fn connect_for(endpoint: &Endpoint, server_uid: u32) -> io::Result<Stream> {
    let Endpoint::Unix { socket, .. } = endpoint;
    let stream = UnixStream::connect(socket)?;
    let uid = peer_uid(&stream)?;
    if uid != server_uid {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            format!("the bridge is served by uid {uid}, not this user"),
        ));
    }
    Ok(Stream { inner: stream })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::host_bridge::{read_line_bounded, LineRead};
    use std::sync::Arc;

    fn endpoint_in(root: &Path) -> Endpoint {
        let dir = crate::os::host_bridge::bridge_dir_in(root);
        let socket = dir.join(crate::os::host_bridge::SOCKET_NAME);
        Endpoint::Unix { dir, socket }
    }

    fn dir_of(e: &Endpoint) -> &Path {
        let Endpoint::Unix { dir, .. } = e;
        dir
    }

    fn socket_of(e: &Endpoint) -> &Path {
        let Endpoint::Unix { socket, .. } = e;
        socket
    }

    #[test]
    fn bind_creates_a_private_dir_and_socket_and_drop_removes_it() {
        let tmp = tempfile::tempdir().unwrap();
        let ep = endpoint_in(tmp.path());
        let listener = Listener::bind(&ep).unwrap();
        let dir_mode = fs::metadata(dir_of(&ep)).unwrap().mode() & 0o777;
        assert_eq!(dir_mode, 0o700);
        let sock = fs::symlink_metadata(socket_of(&ep)).unwrap();
        assert!(sock.file_type().is_socket());
        assert_eq!(sock.mode() & 0o777, 0o600);
        drop(listener);
        assert!(!socket_of(&ep).exists());
    }

    #[test]
    fn a_second_listener_is_refused_while_the_first_is_alive() {
        let tmp = tempfile::tempdir().unwrap();
        let ep = endpoint_in(tmp.path());
        let _first = Listener::bind(&ep).unwrap();
        let err = Listener::bind(&ep).err().expect("second bind must fail");
        assert_eq!(err.kind(), io::ErrorKind::AddrInUse);
    }

    #[test]
    fn a_stale_socket_of_ours_is_replaced() {
        let tmp = tempfile::tempdir().unwrap();
        let ep = endpoint_in(tmp.path());
        ensure_private_dir(dir_of(&ep), euid()).unwrap();
        // std's listener leaves its socket file behind when dropped.
        drop(UnixListener::bind(socket_of(&ep)).unwrap());
        assert!(socket_of(&ep).exists());
        let _listener = Listener::bind(&ep).unwrap();
    }

    #[test]
    fn a_symlinked_or_foreign_socket_path_is_refused() {
        let tmp = tempfile::tempdir().unwrap();
        let ep = endpoint_in(tmp.path());
        ensure_private_dir(dir_of(&ep), euid()).unwrap();
        let target = tmp.path().join("elsewhere.sock");
        std::os::unix::fs::symlink(&target, socket_of(&ep)).unwrap();
        let err = Listener::bind(&ep).err().unwrap();
        assert!(err.to_string().contains("symlink"), "{err}");
        fs::remove_file(socket_of(&ep)).unwrap();

        fs::write(socket_of(&ep), b"not a socket").unwrap();
        let err = Listener::bind(&ep).err().unwrap();
        assert!(err.to_string().contains("not a socket"), "{err}");
    }

    #[test]
    fn the_dir_must_be_private_real_and_ours() {
        let tmp = tempfile::tempdir().unwrap();
        let ep = endpoint_in(tmp.path());
        let dir = dir_of(&ep);
        fs::create_dir_all(dir).unwrap();
        fs::set_permissions(dir, fs::Permissions::from_mode(0o755)).unwrap();
        let err = Listener::bind(&ep).err().unwrap();
        assert!(err.to_string().contains("wider than 700"), "{err}");
        // Never loosened or tightened behind the user's back.
        assert_eq!(fs::metadata(dir).unwrap().mode() & 0o777, 0o755);

        fs::set_permissions(dir, fs::Permissions::from_mode(0o700)).unwrap();
        let err = check_private_dir(dir, euid().wrapping_add(1)).unwrap_err();
        assert!(err.to_string().contains("owned by uid"), "{err}");
        assert!(check_private_dir(dir, euid()).is_ok());

        let real = tmp.path().join("real");
        fs::create_dir(&real).unwrap();
        fs::set_permissions(&real, fs::Permissions::from_mode(0o700)).unwrap();
        fs::remove_dir(dir).unwrap();
        std::os::unix::fs::symlink(&real, dir).unwrap();
        let err = Listener::bind(&ep).err().unwrap();
        assert!(err.to_string().contains("symlink"), "{err}");
    }

    #[test]
    fn a_world_writable_parent_is_refused() {
        let tmp = tempfile::tempdir().unwrap();
        let ep = endpoint_in(tmp.path());
        let parent = dir_of(&ep).parent().unwrap().to_path_buf();
        fs::create_dir_all(&parent).unwrap();
        fs::set_permissions(&parent, fs::Permissions::from_mode(0o777)).unwrap();
        let err = Listener::bind(&ep).err().unwrap();
        assert!(err.to_string().contains("writable by others"), "{err}");
    }

    #[test]
    fn a_request_goes_through_to_a_peer_of_the_same_user() {
        let tmp = tempfile::tempdir().unwrap();
        let ep = endpoint_in(tmp.path());
        let listener = Listener::bind(&ep).unwrap();
        let stop = Arc::new(AtomicBool::new(false));
        let client_ep = ep.clone();
        let client = std::thread::spawn(move || {
            let mut s = connect(&client_ep).unwrap();
            s.inner.write_all(b"hello\n").unwrap();
            let mut reply = Vec::new();
            s.inner.read_to_end(&mut reply).unwrap();
            reply
        });
        let mut conn = listener.accept(&stop).unwrap().expect("a connection");
        conn.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
        assert_eq!(
            read_line_bounded(&mut conn.inner, 64).unwrap(),
            LineRead::Line(b"hello".to_vec())
        );
        assert!(!conn.peer_closed());
        assert_eq!(conn.peer_pid(), Some(std::process::id()));
        conn.inner.write_all(b"world\n").unwrap();
        drop(conn);
        assert_eq!(client.join().unwrap(), b"world\n");
    }

    #[test]
    fn a_peer_of_another_uid_is_dropped_unread() {
        let tmp = tempfile::tempdir().unwrap();
        let ep = endpoint_in(tmp.path());
        // Pretend Kage runs as someone else: our own connection is then a
        // foreign peer.
        let listener = Listener::bind_for(&ep, euid().wrapping_add(1)).unwrap();
        let stop = Arc::new(AtomicBool::new(false));
        let client_ep = ep.clone();
        let client = std::thread::spawn(move || {
            let mut s = connect(&client_ep).unwrap();
            let _ = s.inner.write_all(b"ignored\n");
            let mut reply = Vec::new();
            let _ = s.inner.read_to_end(&mut reply);
            reply
        });
        let stopper = stop.clone();
        let waker = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(300));
            stopper.store(true, Ordering::SeqCst);
        });
        assert!(listener.accept(&stop).unwrap().is_none());
        waker.join().unwrap();
        // Dropped without a reply.
        assert!(client.join().unwrap().is_empty());
    }

    #[test]
    fn the_client_refuses_a_server_of_another_uid() {
        let tmp = tempfile::tempdir().unwrap();
        let ep = endpoint_in(tmp.path());
        let _listener = Listener::bind(&ep).unwrap();
        let err = connect_for(&ep, euid().wrapping_add(1)).err().unwrap();
        assert_eq!(err.kind(), io::ErrorKind::PermissionDenied);
    }

    #[test]
    fn peer_closed_sees_the_client_go() {
        let tmp = tempfile::tempdir().unwrap();
        let ep = endpoint_in(tmp.path());
        let listener = Listener::bind(&ep).unwrap();
        let stop = AtomicBool::new(false);
        let client = connect(&ep).unwrap();
        let conn = listener.accept(&stop).unwrap().unwrap();
        assert!(!conn.peer_closed());
        drop(client);
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        while !conn.peer_closed() {
            assert!(std::time::Instant::now() < deadline, "close not seen");
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    #[test]
    fn wake_unblocks_accept_once_stopped() {
        let tmp = tempfile::tempdir().unwrap();
        let ep = endpoint_in(tmp.path());
        let listener = Arc::new(Listener::bind(&ep).unwrap());
        let stop = Arc::new(AtomicBool::new(false));
        let (l, s) = (listener.clone(), stop.clone());
        let started = std::time::Instant::now();
        let t = std::thread::spawn(move || l.accept(&s).unwrap().is_none());
        std::thread::sleep(Duration::from_millis(50));
        stop.store(true, Ordering::SeqCst);
        listener.wake();
        assert!(t.join().unwrap());
        assert!(started.elapsed() < Duration::from_secs(2));
    }

    #[test]
    fn an_overlong_socket_path_is_refused() {
        let tmp = tempfile::tempdir().unwrap();
        let deep = tmp.path().join("d".repeat(120));
        let ep = endpoint_in(&deep);
        let err = Listener::bind(&ep).err().unwrap();
        assert_eq!(err.kind(), io::ErrorKind::InvalidInput);
    }
}
