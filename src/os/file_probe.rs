//! Opening a file the user picked, safely, and telling whether reading it
//! waits for a cloud download (docs/design/EXTENSION_FILE_TEXT.md §5.4,
//! §8).
//!
//! Per OS:
//! - macOS and Linux: open with `O_NONBLOCK`, so a FIFO swapped in after
//!   the caller's checks can't block the open; check again on the open
//!   file (`fstat`) that it is a regular file; then clear `O_NONBLOCK`.
//! - Windows: open, then check the open handle the same way.
//! - Placeholders: a dataless file on macOS (`SF_DATALESS`, iCloud Drive
//!   with "Optimise Mac Storage", File Provider apps), a Cloud Files
//!   recall attribute on Windows (OneDrive "Files On-Demand"). Linux sync
//!   apps keep whole files, so nothing is a placeholder there. Checking
//!   never downloads anything.

use std::fs::{File, Metadata};
use std::io;
use std::path::Path;

/// What opening a picked file found.
#[derive(Debug)]
pub enum Opened {
    /// A regular file, open for blocking reads.
    File(File),
    /// Not a regular file once open (a FIFO, a device, a folder).
    NotRegular,
}

/// Opens `path` for reading without blocking on a FIFO or a device, and
/// checks on the open file that it is a regular file.
pub fn open_regular(path: &Path) -> io::Result<Opened> {
    crate::os::platform::file_probe::open_regular_impl(path)
}

/// Whether reading the file at `path` (with its followed `metadata`) waits
/// for the sync app to download it.
pub fn is_placeholder(path: &Path, metadata: &Metadata) -> bool {
    crate::os::platform::file_probe::is_placeholder_impl(path, metadata)
}

// --- Shared by the macOS and Linux implementations -----------------------

/// The Unix open: `O_NONBLOCK`, `fstat`, then blocking reads again.
#[cfg(unix)]
pub(crate) fn open_regular_unix(path: &Path) -> io::Result<Opened> {
    use std::os::unix::fs::OpenOptionsExt;
    use std::os::unix::io::AsRawFd;

    let file = std::fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NONBLOCK)
        .open(path)?;
    if !file.metadata()?.is_file() {
        return Ok(Opened::NotRegular);
    }
    let fd = file.as_raw_fd();
    // SAFETY: `fd` is a descriptor `file` owns for the whole call.
    let flags = unsafe { libc::fcntl(fd, libc::F_GETFL) };
    if flags < 0 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: as above; only the status flags change.
    if unsafe { libc::fcntl(fd, libc::F_SETFL, flags & !libc::O_NONBLOCK) } < 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(Opened::File(file))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Read;

    #[test]
    fn a_regular_file_opens_for_reading() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("note.txt");
        std::fs::write(&path, b"hello").unwrap();
        let Opened::File(mut file) = open_regular(&path).unwrap() else {
            panic!("a regular file must open");
        };
        let mut text = String::new();
        file.read_to_string(&mut text).unwrap();
        assert_eq!(text, "hello");
        let meta = std::fs::metadata(&path).unwrap();
        assert!(!is_placeholder(&path, &meta));
    }

    #[test]
    fn a_missing_file_is_an_error() {
        let dir = tempfile::tempdir().unwrap();
        assert!(open_regular(&dir.path().join("missing.txt")).is_err());
    }

    #[cfg(unix)]
    #[test]
    fn a_folder_and_a_fifo_are_not_regular_and_never_block() {
        let dir = tempfile::tempdir().unwrap();
        assert!(matches!(open_regular(dir.path()), Ok(Opened::NotRegular)));

        let fifo = dir.path().join("pipe.txt");
        let c = std::ffi::CString::new(fifo.to_str().unwrap()).unwrap();
        // SAFETY: a valid NUL-terminated path.
        assert_eq!(unsafe { libc::mkfifo(c.as_ptr(), 0o600) }, 0);
        // Without O_NONBLOCK this open would wait for a writer forever.
        assert!(matches!(open_regular(&fifo), Ok(Opened::NotRegular)));
    }
}
