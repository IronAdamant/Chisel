//! Directory watcher. Direct OS calls, no notify crate.
//! Linux uses inotify, macOS uses kqueue, Windows uses a change notification.
//! A timeout with no event returns false. The caller still checks git.

#[cfg(any(target_os = "linux", target_os = "macos", target_os = "freebsd"))]
use std::fs;
use std::io;
use std::path::Path;
use std::time::Duration;

pub fn backend_name() -> &'static str {
    #[cfg(target_os = "linux")]
    { "inotify" }
    #[cfg(any(target_os = "macos", target_os = "freebsd"))]
    { "kqueue" }
    #[cfg(target_os = "windows")]
    { "readdirectorychanges" }
    #[cfg(not(any(target_os = "linux", target_os = "macos", target_os = "freebsd", target_os = "windows")))]
    { "poll" }
}

pub fn wait_for_change(root: &Path, timeout: Duration) -> io::Result<bool> {
    #[cfg(target_os = "linux")]
    { wait_inotify(root, timeout) }
    #[cfg(any(target_os = "macos", target_os = "freebsd"))]
    { wait_kqueue(root, timeout) }
    #[cfg(target_os = "windows")]
    { wait_windows(root, timeout) }
    #[cfg(not(any(target_os = "linux", target_os = "macos", target_os = "freebsd", target_os = "windows")))]
    { let _ = (root, timeout); Ok(false) }
}

#[cfg(any(target_os = "linux", target_os = "macos", target_os = "freebsd"))]
fn skip_dir(name: &str) -> bool {
    matches!(name, ".git" | ".chisel" | "node_modules" | "target" | ".next")
}

#[cfg(any(target_os = "linux", target_os = "macos", target_os = "freebsd"))]
fn collect_dirs(root: &Path, out: &mut Vec<std::path::PathBuf>) {
    out.push(root.to_path_buf());
    let Ok(read) = fs::read_dir(root) else { return };
    for entry in read.flatten() {
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if skip_dir(&name) {
            continue;
        }
        let path = entry.path();
        if path.is_dir() {
            collect_dirs(&path, out);
        }
    }
}

#[cfg(any(target_os = "macos", target_os = "freebsd"))]
fn collect_files(root: &Path, out: &mut Vec<std::path::PathBuf>) {
    let Ok(read) = fs::read_dir(root) else { return };
    for entry in read.flatten() {
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if skip_dir(&name) {
            continue;
        }
        let path = entry.path();
        if path.is_dir() {
            collect_files(&path, out);
        } else {
            out.push(path);
        }
    }
}

#[cfg(target_os = "linux")]
fn wait_inotify(root: &Path, timeout: Duration) -> io::Result<bool> {
    use std::os::raw::{c_char, c_int};
    const IN_CLOEXEC: c_int = 0x80000;
    const IN_MODIFY: u32 = 0x2;
    const IN_ATTRIB: u32 = 0x4;
    const IN_CREATE: u32 = 0x100;
    const IN_DELETE: u32 = 0x200;
    const IN_MOVED_FROM: u32 = 0x40;
    const IN_MOVED_TO: u32 = 0x80;
    const POLLIN: i16 = 1;
    #[repr(C)]
    struct PollFd { fd: c_int, events: i16, revents: i16 }
    extern "C" {
        fn inotify_init1(flags: c_int) -> c_int;
        fn inotify_add_watch(fd: c_int, pathname: *const c_char, mask: u32) -> c_int;
        fn poll(fds: *mut PollFd, nfds: u64, timeout: c_int) -> c_int;
        fn close(fd: c_int) -> c_int;
    }
    let mut dirs = Vec::new();
    collect_dirs(root, &mut dirs);
    if dirs.len() > 4000 {
        return Err(io::Error::other("too many directories to watch"));
    }
    let fd = unsafe { inotify_init1(IN_CLOEXEC) };
    if fd < 0 {
        return Err(io::Error::last_os_error());
    }
    let mask = IN_MODIFY | IN_ATTRIB | IN_CREATE | IN_DELETE | IN_MOVED_FROM | IN_MOVED_TO;
    for dir in &dirs {
        let Ok(text) = std::ffi::CString::new(dir.to_string_lossy().as_bytes()) else { continue };
        let wd = unsafe { inotify_add_watch(fd, text.as_ptr(), mask) };
        if wd < 0 {
            unsafe { close(fd); }
            return Err(io::Error::last_os_error());
        }
    }
    let mut pfd = PollFd { fd, events: POLLIN, revents: 0 };
    let ms = timeout.as_millis().min(i32::MAX as u128) as c_int;
    let n = unsafe { poll(&mut pfd, 1, ms) };
    unsafe { close(fd); }
    if n < 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(n > 0)
}

#[cfg(any(target_os = "macos", target_os = "freebsd"))]
fn wait_kqueue(root: &Path, timeout: Duration) -> io::Result<bool> {
    const EVFILT_VNODE: i16 = -4;
    const EV_ADD: u16 = 0x1;
    const EV_CLEAR: u16 = 0x20;
    const NOTE_DELETE: u32 = 0x1;
    const NOTE_WRITE: u32 = 0x2;
    const NOTE_EXTEND: u32 = 0x4;
    const NOTE_ATTRIB: u32 = 0x8;
    const NOTE_RENAME: u32 = 0x20;
    #[repr(C)]
    struct Kevent {
        ident: usize,
        filter: i16,
        flags: u16,
        fflags: u32,
        data: isize,
        udata: *mut c_void,
    }
    #[repr(C)]
    struct Timespec { tv_sec: i64, tv_nsec: i64 }
    extern "C" {
        fn kqueue() -> c_int;
        fn kevent(kq: c_int, changelist: *const Kevent, nchanges: c_int, eventlist: *mut Kevent, nevents: c_int, timeout: *const Timespec) -> c_int;
        fn close(fd: c_int) -> c_int;
    }
    use std::fs::OpenOptions;
    use std::os::raw::{c_int, c_void};
    use std::os::unix::io::AsRawFd;
    let mut paths = Vec::new();
    collect_dirs(root, &mut paths);
    collect_files(root, &mut paths);
    if paths.len() > 4000 {
        return Err(io::Error::other("too many paths to watch"));
    }
    let mut files = Vec::new();
    for path in &paths {
        if let Ok(file) = OpenOptions::new().read(true).open(path) {
            files.push(file);
        }
    }
    if files.is_empty() {
        return Ok(false);
    }
    let kq = unsafe { kqueue() };
    if kq < 0 {
        return Err(io::Error::last_os_error());
    }
    let notes = NOTE_DELETE | NOTE_WRITE | NOTE_EXTEND | NOTE_ATTRIB | NOTE_RENAME;
    let mut changes = Vec::with_capacity(files.len());
    for file in &files {
        changes.push(Kevent {
            ident: file.as_raw_fd() as usize,
            filter: EVFILT_VNODE,
            flags: EV_ADD | EV_CLEAR,
            fflags: notes,
            data: 0,
            udata: std::ptr::null_mut(),
        });
    }
    let registered = unsafe { kevent(kq, changes.as_ptr(), changes.len() as c_int, std::ptr::null_mut(), 0, std::ptr::null()) };
    if registered < 0 {
        unsafe { close(kq); }
        return Err(io::Error::last_os_error());
    }
    let ts = Timespec { tv_sec: timeout.as_secs() as i64, tv_nsec: timeout.subsec_nanos() as i64 };
    let mut event = Kevent { ident: 0, filter: 0, flags: 0, fflags: 0, data: 0, udata: std::ptr::null_mut() };
    let n = unsafe { kevent(kq, std::ptr::null(), 0, &mut event, 1, &ts) };
    unsafe { close(kq); }
    if n < 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(n > 0)
}

#[cfg(target_os = "windows")]
fn wait_windows(root: &Path, timeout: Duration) -> io::Result<bool> {
    use std::os::raw::c_void;
    use std::os::windows::ffi::OsStrExt;
    const FILE_NOTIFY_CHANGE_FILE_NAME: u32 = 0x1;
    const FILE_NOTIFY_CHANGE_DIR_NAME: u32 = 0x2;
    const FILE_NOTIFY_CHANGE_LAST_WRITE: u32 = 0x10;
    const WAIT_OBJECT_0: u32 = 0;
    extern "system" {
        fn FindFirstChangeNotificationW(path: *const u16, watch_subtree: i32, filter: u32) -> *mut c_void;
        fn FindCloseChangeNotification(handle: *mut c_void) -> i32;
        fn WaitForSingleObject(handle: *mut c_void, millis: u32) -> u32;
    }
    let mut wide: Vec<u16> = root.as_os_str().encode_wide().collect();
    wide.push(0);
    let filter = FILE_NOTIFY_CHANGE_FILE_NAME | FILE_NOTIFY_CHANGE_DIR_NAME | FILE_NOTIFY_CHANGE_LAST_WRITE;
    let handle = unsafe { FindFirstChangeNotificationW(wide.as_ptr(), 1, filter) };
    if handle.is_null() || handle == (-1isize as *mut c_void) {
        return Err(io::Error::last_os_error());
    }
    let ms = timeout.as_millis().min(u32::MAX as u128) as u32;
    let wait = unsafe { WaitForSingleObject(handle, ms) };
    unsafe { FindCloseChangeNotification(handle); }
    Ok(wait == WAIT_OBJECT_0)
}

