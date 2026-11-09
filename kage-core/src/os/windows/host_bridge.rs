//! Windows transport for the host-command bridge: a named pipe that only
//! the current user (and SYSTEM) may open, refusing remote clients, whose
//! every client process must run as the current user. See
//! [`crate::os::host_bridge`] for the contract.

use std::fs::File;
use std::io::{self, Read, Write};
use std::os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle, RawHandle};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;
use std::time::Duration;

use windows::core::{HSTRING, PWSTR};
use windows::Win32::Foundation::{
    LocalFree, ERROR_PIPE_BUSY, ERROR_PIPE_CONNECTED, GENERIC_READ, GENERIC_WRITE, HANDLE, HLOCAL,
};
use windows::Win32::Security::Authorization::{
    ConvertSidToStringSidW, ConvertStringSecurityDescriptorToSecurityDescriptorW, SDDL_REVISION_1,
};
use windows::Win32::Security::{
    EqualSid, GetTokenInformation, TokenUser, PSECURITY_DESCRIPTOR, PSID, SECURITY_ATTRIBUTES,
    TOKEN_QUERY, TOKEN_USER,
};
use windows::Win32::Storage::FileSystem::{
    CreateFileW, FlushFileBuffers, FILE_FLAGS_AND_ATTRIBUTES, FILE_FLAG_FIRST_PIPE_INSTANCE,
    FILE_SHARE_NONE, OPEN_EXISTING, PIPE_ACCESS_DUPLEX, SECURITY_IDENTIFICATION,
    SECURITY_SQOS_PRESENT,
};
use windows::Win32::System::Pipes::{
    ConnectNamedPipe, CreateNamedPipeW, GetNamedPipeClientProcessId, GetNamedPipeServerProcessId,
    PeekNamedPipe, WaitNamedPipeW, NAMED_PIPE_MODE, PIPE_READMODE_BYTE, PIPE_REJECT_REMOTE_CLIENTS,
    PIPE_TYPE_BYTE, PIPE_UNLIMITED_INSTANCES, PIPE_WAIT,
};
use windows::Win32::System::Threading::{
    GetCurrentProcess, OpenProcess, OpenProcessToken, PROCESS_QUERY_LIMITED_INFORMATION,
};

use crate::os::host_bridge::Endpoint;

const PIPE_BUFFER: u32 = 64 * 1024;
const CONNECT_ATTEMPTS: u32 = 5;
const BUSY_WAIT_MS: u32 = 2000;

fn win_err(e: windows::core::Error) -> io::Error {
    let hr = e.code().0 as u32;
    if hr & 0xFFFF_0000 == 0x8007_0000 {
        io::Error::from_raw_os_error((hr & 0xFFFF) as i32)
    } else {
        io::Error::other(e.to_string())
    }
}

fn raw(h: &OwnedHandle) -> HANDLE {
    HANDLE(h.as_raw_handle())
}

/// A process token's `TOKEN_USER`, in a buffer aligned for it (the SID it
/// points to lives in the same buffer).
struct TokenUserBuf(Vec<u64>);

impl TokenUserBuf {
    fn sid(&self) -> PSID {
        // SAFETY: the buffer was filled by GetTokenInformation(TokenUser)
        // and is 8-byte aligned.
        unsafe { (*(self.0.as_ptr() as *const TOKEN_USER)).User.Sid }
    }

    fn same_user(&self, other: &TokenUserBuf) -> bool {
        // SAFETY: both SIDs point into live TOKEN_USER buffers.
        unsafe { EqualSid(self.sid(), other.sid()).is_ok() }
    }
}

fn token_user(process: HANDLE) -> io::Result<TokenUserBuf> {
    // SAFETY: plain Win32 calls on a process handle we hold; every
    // out-pointer refers to a live local, and the token handle is owned.
    unsafe {
        let mut token = HANDLE::default();
        OpenProcessToken(process, TOKEN_QUERY, &mut token).map_err(win_err)?;
        let token = OwnedHandle::from_raw_handle(token.0 as RawHandle);
        let mut len = 0u32;
        let _ = GetTokenInformation(raw(&token), TokenUser, None, 0, &mut len);
        if len == 0 {
            return Err(io::Error::last_os_error());
        }
        let mut buf = vec![0u64; (len as usize).div_ceil(8)];
        GetTokenInformation(
            raw(&token),
            TokenUser,
            Some(buf.as_mut_ptr().cast()),
            len,
            &mut len,
        )
        .map_err(win_err)?;
        Ok(TokenUserBuf(buf))
    }
}

fn current_user() -> io::Result<TokenUserBuf> {
    // SAFETY: the pseudo-handle needs no closing.
    token_user(unsafe { GetCurrentProcess() })
}

fn process_user(pid: u32) -> io::Result<TokenUserBuf> {
    // SAFETY: the process handle is owned and closed on drop.
    let process = unsafe {
        let h = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid).map_err(win_err)?;
        OwnedHandle::from_raw_handle(h.0 as RawHandle)
    };
    token_user(raw(&process))
}

fn sid_string(sid: PSID) -> io::Result<String> {
    // SAFETY: `sid` is valid; the returned string is freed with LocalFree.
    unsafe {
        let mut s = PWSTR::null();
        ConvertSidToStringSidW(sid, &mut s).map_err(win_err)?;
        let out = s.to_string().map_err(io::Error::other);
        let _ = LocalFree(Some(HLOCAL(s.0.cast())));
        out
    }
}

/// `\\.\pipe\kage-host-bridge-<the current user's SID>`.
pub fn pipe_name() -> Option<String> {
    let user = current_user().ok()?;
    let sid = sid_string(user.sid()).ok()?;
    // SID strings are `S-1-5-21-…`: nothing else belongs in a pipe name.
    if sid.is_empty() || !sid.chars().all(|c| c.is_ascii_alphanumeric() || c == '-') {
        return None;
    }
    Some(format!(r"\\.\pipe\kage-host-bridge-{sid}"))
}

/// A security descriptor (from SDDL) freed on drop. Immutable once built.
struct SecurityDescriptor(PSECURITY_DESCRIPTOR);

// SAFETY: the descriptor is never written after creation.
unsafe impl Send for SecurityDescriptor {}
unsafe impl Sync for SecurityDescriptor {}

impl Drop for SecurityDescriptor {
    fn drop(&mut self) {
        // SAFETY: allocated by ConvertStringSecurityDescriptorToSecurityDescriptorW.
        unsafe {
            let _ = LocalFree(Some(HLOCAL(self.0 .0)));
        }
    }
}

/// Full access for `sid` and SYSTEM, nobody else; protected from inherited
/// entries.
fn user_only_descriptor(sid: &str) -> io::Result<SecurityDescriptor> {
    let sddl = HSTRING::from(format!("D:P(A;;GA;;;{sid})(A;;GA;;;SY)"));
    let mut sd = PSECURITY_DESCRIPTOR::default();
    // SAFETY: valid SDDL string and out-pointer.
    unsafe {
        ConvertStringSecurityDescriptorToSecurityDescriptorW(&sddl, SDDL_REVISION_1, &mut sd, None)
            .map_err(win_err)?;
    }
    Ok(SecurityDescriptor(sd))
}

fn create_instance(
    name: &HSTRING,
    sd: &SecurityDescriptor,
    first: bool,
) -> io::Result<OwnedHandle> {
    let attrs = SECURITY_ATTRIBUTES {
        nLength: std::mem::size_of::<SECURITY_ATTRIBUTES>() as u32,
        lpSecurityDescriptor: sd.0 .0,
        bInheritHandle: false.into(),
    };
    let mut open_mode = PIPE_ACCESS_DUPLEX.0;
    if first {
        open_mode |= FILE_FLAG_FIRST_PIPE_INSTANCE.0;
    }
    let pipe_mode =
        PIPE_TYPE_BYTE.0 | PIPE_READMODE_BYTE.0 | PIPE_WAIT.0 | PIPE_REJECT_REMOTE_CLIENTS.0;
    // SAFETY: valid name, attributes that outlive the call.
    let h = unsafe {
        CreateNamedPipeW(
            name,
            FILE_FLAGS_AND_ATTRIBUTES(open_mode),
            NAMED_PIPE_MODE(pipe_mode),
            PIPE_UNLIMITED_INSTANCES,
            PIPE_BUFFER,
            PIPE_BUFFER,
            0,
            Some(&attrs),
        )
    };
    if h.is_invalid() {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: a fresh handle we own.
    Ok(unsafe { OwnedHandle::from_raw_handle(h.0 as RawHandle) })
}

/// One verified connection.
pub struct Stream {
    file: File,
    peer: u32,
}

impl Stream {
    pub fn peer_closed(&self) -> bool {
        let mut avail = 0u32;
        // SAFETY: a live pipe handle; only the byte count is requested.
        unsafe {
            PeekNamedPipe(
                HANDLE(self.file.as_raw_handle()),
                None,
                0,
                None,
                Some(&mut avail as *mut u32),
                None,
            )
            .is_err()
        }
    }

    pub fn set_read_timeout(&self, _timeout: Option<Duration>) -> io::Result<()> {
        Ok(())
    }

    pub fn peer_pid(&self) -> Option<u32> {
        (self.peer != 0).then_some(self.peer)
    }

    pub fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        self.file.read(buf)
    }

    pub fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.file.write(buf)
    }

    pub fn flush(&mut self) -> io::Result<()> {
        // SAFETY: a live pipe handle. Waits until the peer has read what
        // we wrote, so closing doesn't discard it.
        unsafe { FlushFileBuffers(HANDLE(self.file.as_raw_handle())).map_err(win_err) }
    }
}

pub struct Listener {
    name: HSTRING,
    sd: SecurityDescriptor,
    user: TokenUserBuf,
    /// The instance waiting for the next client. A new one is created
    /// before a connected one is handed out, so the name is never free for
    /// another process to claim.
    next: Mutex<Option<OwnedHandle>>,
}

impl Listener {
    pub fn bind(endpoint: &Endpoint) -> io::Result<Self> {
        let Endpoint::Pipe(name) = endpoint;
        let user = current_user()?;
        let sid = sid_string(user.sid())?;
        let sd = user_only_descriptor(&sid)?;
        let name = HSTRING::from(name.as_str());
        let first = create_instance(&name, &sd, true).map_err(|e| {
            io::Error::new(
                e.kind(),
                format!("another process already serves the bridge pipe ({e})"),
            )
        })?;
        Ok(Self {
            name,
            sd,
            user,
            next: Mutex::new(Some(first)),
        })
    }

    pub fn accept(&self, stop: &AtomicBool) -> io::Result<Option<Stream>> {
        loop {
            if stop.load(Ordering::SeqCst) {
                return Ok(None);
            }
            let pending = self.next.lock().unwrap_or_else(|p| p.into_inner()).take();
            let pipe = match pending {
                Some(h) => h,
                None => create_instance(&self.name, &self.sd, false)?,
            };
            // SAFETY: a live server instance; blocking connect.
            let connected = unsafe { ConnectNamedPipe(raw(&pipe), None) };
            if let Err(e) = connected {
                if e.code() != ERROR_PIPE_CONNECTED.to_hresult() {
                    // The client went away before we got to it.
                    continue;
                }
            }
            if stop.load(Ordering::SeqCst) {
                return Ok(None);
            }
            let next = create_instance(&self.name, &self.sd, false)?;
            *self.next.lock().unwrap_or_else(|p| p.into_inner()) = Some(next);

            let mut pid = 0u32;
            // SAFETY: a connected server instance and an out-pointer.
            let known = unsafe { GetNamedPipeClientProcessId(raw(&pipe), &mut pid) }.is_ok();
            let same = known && process_user(pid).is_ok_and(|u| u.same_user(&self.user));
            if same {
                return Ok(Some(Stream {
                    file: File::from(pipe),
                    peer: pid,
                }));
            }
            log::warn!("Host bridge: dropped a pipe client that isn't this user (pid {pid})");
        }
    }

    /// Nothing to remove: the pipe goes when its last handle closes.
    pub fn remove_endpoint(&self) {}

    /// Connect to ourselves so a blocked ConnectNamedPipe returns and the
    /// accept loop sees its stop flag.
    pub fn wake(&self) {
        // SAFETY: opening and immediately closing a client handle.
        unsafe {
            if let Ok(h) = CreateFileW(
                &self.name,
                GENERIC_READ.0 | GENERIC_WRITE.0,
                FILE_SHARE_NONE,
                None,
                OPEN_EXISTING,
                FILE_FLAGS_AND_ATTRIBUTES(0),
                None,
            ) {
                drop(OwnedHandle::from_raw_handle(h.0 as RawHandle));
            }
        }
    }
}

pub fn connect(endpoint: &Endpoint) -> io::Result<Stream> {
    let Endpoint::Pipe(name) = endpoint;
    let name = HSTRING::from(name.as_str());
    let mut attempt = 0;
    let pipe = loop {
        attempt += 1;
        // SAFETY: a valid pipe name. Identification-level impersonation
        // only: the server may learn who we are, never act as us.
        let opened = unsafe {
            CreateFileW(
                &name,
                GENERIC_READ.0 | GENERIC_WRITE.0,
                FILE_SHARE_NONE,
                None,
                OPEN_EXISTING,
                FILE_FLAGS_AND_ATTRIBUTES(SECURITY_SQOS_PRESENT.0 | SECURITY_IDENTIFICATION.0),
                None,
            )
        };
        match opened {
            // SAFETY: a fresh handle we own.
            Ok(h) => break unsafe { OwnedHandle::from_raw_handle(h.0 as RawHandle) },
            Err(e) if e.code() == ERROR_PIPE_BUSY.to_hresult() && attempt < CONNECT_ATTEMPTS => {
                // SAFETY: a valid pipe name.
                let _ = unsafe { WaitNamedPipeW(&name, BUSY_WAIT_MS) };
            }
            Err(e) => return Err(win_err(e)),
        }
    };
    let mut pid = 0u32;
    // SAFETY: a connected client handle and an out-pointer.
    unsafe { GetNamedPipeServerProcessId(raw(&pipe), &mut pid) }.map_err(win_err)?;
    let me = current_user()?;
    if !process_user(pid)?.same_user(&me) {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "the bridge pipe is served by another user",
        ));
    }
    Ok(Stream {
        file: File::from(pipe),
        peer: pid,
    })
}
