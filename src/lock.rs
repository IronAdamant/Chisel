//! Cross-process shared/exclusive lock. Direct OS calls, no libc crate.
//! Unix uses flock. Windows uses LockFileEx. Rust std already links the system library.

use std::fs::{File, OpenOptions};
use std::io;
use std::path::{Path, PathBuf};

pub struct ProcessLock {
    _file: File,
    path: PathBuf,
}

impl ProcessLock {
    pub fn open(dir: &Path) -> io::Result<Self> {
        std::fs::create_dir_all(dir)?;
        let path = dir.join("chisel.lock");
        let file = OpenOptions::new().read(true).write(true).create(true).truncate(false).open(&path)?;
        Ok(ProcessLock { _file: file, path })
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn shared(&self) -> io::Result<LockGuard<'_>> {
        lock_fd(&self._file, LockKind::Shared)?;
        Ok(LockGuard { file: &self._file })
    }

    pub fn exclusive(&self) -> io::Result<LockGuard<'_>> {
        lock_fd(&self._file, LockKind::Exclusive)?;
        Ok(LockGuard { file: &self._file })
    }
}

pub struct LockGuard<'a> {
    file: &'a File,
}

impl Drop for LockGuard<'_> {
    fn drop(&mut self) {
        let _ = lock_fd(self.file, LockKind::Unlock);
    }
}

enum LockKind {
    Shared,
    Exclusive,
    Unlock,
}

fn lock_fd(file: &File, kind: LockKind) -> io::Result<()> {
    #[cfg(unix)]
    {
        unix_flock(file, kind)
    }
    #[cfg(windows)]
    {
        windows_lock(file, kind)
    }
    #[cfg(not(any(unix, windows)))]
    {
        let _ = (file, kind);
        Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "file locking is not implemented on this platform",
        ))
    }
}

#[cfg(unix)]
fn unix_flock(file: &File, kind: LockKind) -> io::Result<()> {
    use std::os::unix::io::AsRawFd;
    const LOCK_SH: i32 = 1;
    const LOCK_EX: i32 = 2;
    const LOCK_UN: i32 = 8;
    extern "C" {
        fn flock(fd: i32, op: i32) -> i32;
    }
    let op = match kind {
        LockKind::Shared => LOCK_SH,
        LockKind::Exclusive => LOCK_EX,
        LockKind::Unlock => LOCK_UN,
    };
    loop {
        let rc = unsafe { flock(file.as_raw_fd(), op) };
        if rc == 0 {
            return Ok(());
        }
        let err = io::Error::last_os_error();
        if err.raw_os_error() == Some(4) {
            continue;
        }
        return Err(err);
    }
}

#[cfg(windows)]
fn windows_lock(file: &File, kind: LockKind) -> io::Result<()> {
    use std::os::windows::io::AsRawHandle;
    const LOCKFILE_EXCLUSIVE_LOCK: u32 = 0x00000002;
    const LOCKFILE_FAIL_IMMEDIATELY: u32 = 0x00000001;
    #[repr(C)]
    struct Overlapped {
        internal: usize,
        internal_high: usize,
        offset: u32,
        offset_high: u32,
        event: isize,
    }
    extern "system" {
        fn LockFileEx(
            file: isize,
            flags: u32,
            reserved: u32,
            low: u32,
            high: u32,
            overlapped: *mut Overlapped,
        ) -> i32;
        fn UnlockFileEx(
            file: isize,
            reserved: u32,
            low: u32,
            high: u32,
            overlapped: *mut Overlapped,
        ) -> i32;
    }
    let handle = file.as_raw_handle() as isize;
    let mut ov = Overlapped {
        internal: 0,
        internal_high: 0,
        offset: 0,
        offset_high: 0,
        event: 0,
    };
    let rc = match kind {
        LockKind::Shared => unsafe {
            LockFileEx(handle, 0, 0, 1, 0, &mut ov)
        },
        LockKind::Exclusive => unsafe {
            LockFileEx(handle, LOCKFILE_EXCLUSIVE_LOCK, 0, 1, 0, &mut ov)
        },
        LockKind::Unlock => unsafe { UnlockFileEx(handle, 0, 1, 0, &mut ov) },
    };
    let _ = LOCKFILE_FAIL_IMMEDIATELY;
    if rc != 0 {
        Ok(())
    } else {
        Err(io::Error::last_os_error())
    }
}
