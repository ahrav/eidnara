//! Spawns detached daemons for `eidnara-host start` and `restart`.
//!
//! This module contains `fork`, session separation, stdio redirection, descriptor
//! closure, and descriptor-based re-exec so `host-runtime` can deny unsafe code.
//! Production re-execs the retained launcher descriptor for the selected staged
//! generation. Debug test fixtures may re-exec the running test executable only
//! when `EIDNARA_HOST_TEST_ALLOW_SELF_EXEC=1`.
#![allow(unsafe_code)]
#![deny(clippy::undocumented_unsafe_blocks, unsafe_op_in_unsafe_fn)]

use std::ffi::CString;
use std::fs::OpenOptions;
use std::io::{Read, Write};
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
use std::path::Path;

/// `message` never includes native error text. Pre-exec child failures report the child's `errno` in `child_error`.
#[derive(Debug)]
pub struct SpawnError {
    pub message: &'static str,
    pub child_error: Option<std::io::Error>,
}

impl SpawnError {
    const fn new(message: &'static str) -> Self {
        Self {
            message,
            child_error: None,
        }
    }
}

/// `MAX_ENVELOPE_BYTES` matches pipe capacity so the post-fork write cannot block when the child never execs.
pub const MAX_ENVELOPE_BYTES: usize = 64 * 1024;

/// The child places the executable at this descriptor and execs it through the descriptor path.
const EXE_SLOT: libc::c_int = 3;

/// The child writes its `errno` as this many little-endian bytes before `_exit`.
const ERRNO_BYTES: usize = std::mem::size_of::<libc::c_int>();

fn cvt(ret: libc::c_int, what: &'static str) -> Result<libc::c_int, SpawnError> {
    if ret < 0 {
        Err(SpawnError::new(what))
    } else {
        Ok(ret)
    }
}

/// Opens an owner-only regular log file without following links.
///
/// Existing files must have one link, belong to the effective user, and grant no
/// group or other permissions. The returned descriptor has mode `0o600`.
fn open_log(log_path: &Path) -> Result<OwnedFd, SpawnError> {
    let file = OpenOptions::new()
        .append(true)
        .create(true)
        .mode(0o600)
        // `O_NONBLOCK` keeps a planted FIFO at this name from hanging the open;
        // `O_NONBLOCK` does not bypass the regular-file check.
        .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC | libc::O_NONBLOCK)
        .open(log_path)
        .map_err(|_| SpawnError::new("daemon log open failed"))?;
    let meta = file
        .metadata()
        .map_err(|_| SpawnError::new("daemon log stat failed"))?;
    // SAFETY: geteuid never fails and has no memory effects.
    let euid = unsafe { libc::geteuid() };
    // `nlink == 1` rejects hard links: another name could receive the daemon's stdout and stderr and share the inode re-moded below.
    if !meta.is_file() || meta.nlink() != 1 || meta.uid() != euid || meta.mode() & 0o077 != 0 {
        return Err(SpawnError::new("daemon log failed security checks"));
    }
    // `umask` can create the log as `0000`; it passes the group/other-mode check and writes through the open descriptor, but future opens fail. `fchmod` normalizes the validated descriptor without reopening `log_path`.
    // SAFETY: `file` is a live descriptor for the validated regular file we own, and `fchmod` has no memory effects.
    if unsafe { libc::fchmod(file.as_raw_fd(), 0o600) } < 0 {
        return Err(SpawnError::new("daemon log chmod failed"));
    }
    Ok(OwnedFd::from(file))
}

/// Child-side sources must sit above every `dup2` destination the child writes (fds 0 through 2, and fd 3 for the executable) because `dup2` overwrites them.
fn relocate_at_least(fd: OwnedFd, floor: libc::c_int) -> Result<OwnedFd, SpawnError> {
    if fd.as_raw_fd() >= floor {
        return Ok(fd);
    }
    // SAFETY: `fd` is a live owned descriptor; `F_DUPFD_CLOEXEC` returns a new owned descriptor at or above `floor`, or -1.
    let raw = unsafe { libc::fcntl(fd.as_raw_fd(), libc::F_DUPFD_CLOEXEC, floor) };
    let raw = cvt(raw, "descriptor relocation failed")?;
    // SAFETY: `raw` was returned by `F_DUPFD_CLOEXEC` and is owned here; `fd` is dropped at scope exit.
    Ok(unsafe { OwnedFd::from_raw_fd(raw) })
}

fn relocate_above_stderr(fd: OwnedFd) -> Result<OwnedFd, SpawnError> {
    relocate_at_least(fd, EXE_SLOT)
}

fn cloexec_pipe(what: &'static str) -> Result<(OwnedFd, OwnedFd), SpawnError> {
    let mut pipe_fds = [0 as libc::c_int; 2];
    #[cfg(target_os = "linux")]
    {
        // SAFETY: `pipe2` writes exactly two descriptors into the two-element array.
        let ret = unsafe { libc::pipe2(pipe_fds.as_mut_ptr(), libc::O_CLOEXEC) };
        cvt(ret, what)?;
    }
    #[cfg(target_os = "macos")]
    {
        // SAFETY: `pipe` writes exactly two descriptors into the two-element array.
        let ret = unsafe { libc::pipe(pipe_fds.as_mut_ptr()) };
        cvt(ret, what)?;
    }
    // SAFETY: the descriptors were just returned by pipe2/pipe and are owned here.
    let (read_end, write_end) = unsafe {
        (
            OwnedFd::from_raw_fd(pipe_fds[0]),
            OwnedFd::from_raw_fd(pipe_fds[1]),
        )
    };
    // `pipe` cannot atomically set `FD_CLOEXEC`, so another thread can inherit the pipe ends by execing before the flag is set.
    #[cfg(target_os = "macos")]
    for fd in [read_end.as_raw_fd(), write_end.as_raw_fd()] {
        // SAFETY: `fd` is owned by `read_end` or `write_end` and open for this call.
        cvt(
            unsafe { libc::fcntl(fd, libc::F_SETFD, libc::FD_CLOEXEC) },
            what,
        )?;
    }
    Ok((read_end, write_end))
}

/// The pre-5.9 close fallback closes descriptors through this number.
///
/// When `rlim_cur` is at most `CLAMP`, the fallback closes through `rlim_cur`.
/// Resolve the ceiling before `fork` to avoid querying resource limits in the child.
fn close_fallback_ceiling() -> libc::c_int {
    // `RLIM_INFINITY` and soft limits above `CLAMP` are clamped to `CLAMP`.
    const CLAMP: u64 = 1 << 20;
    const FLOOR: u64 = 8192;
    let mut limit = libc::rlimit {
        rlim_cur: 0,
        rlim_max: 0,
    };
    // SAFETY: `getrlimit` writes only to the caller-provided struct and has no other memory effects.
    if unsafe { libc::getrlimit(libc::RLIMIT_NOFILE, &mut limit) } < 0 {
        return FLOOR as libc::c_int;
    }
    // `rlim_t` widths differ across supported targets; compare `rlim_cur` as `u64`.
    #[allow(clippy::unnecessary_cast)]
    let soft = limit.rlim_cur as u64;
    soft.clamp(FLOOR, CLAMP) as libc::c_int
}

/// # Safety
///
/// Reading `errno` through its thread-local pointer is async-signal-safe and touches no shared state. commentlint: allow(JUDGE)
unsafe fn errno() -> libc::c_int {
    #[cfg(target_os = "linux")]
    // SAFETY: `__errno_location` returns a valid pointer to the calling thread's `errno`.
    unsafe {
        *libc::__errno_location()
    }
    #[cfg(target_os = "macos")]
    // SAFETY: `__error` returns a valid pointer to the calling thread's `errno`.
    unsafe {
        *libc::__error()
    }
}

/// The parent distinguishes an exec from a pre-exec failure by whether the status pipe carries bytes before end of file.
///
/// # Safety
///
/// The forked child calls this function after `fork`; `status_w` is the open status-pipe write end. commentlint: allow(JUDGE)
/// Uses only `write` and `_exit`, both async-signal-safe, on a stack buffer. commentlint: allow(JUDGE)
unsafe fn child_fail(status_w: libc::c_int, exit_code: libc::c_int) -> ! {
    // SAFETY: The child reads its thread-local `errno` immediately after the failing call.
    let code = unsafe { errno() };
    let bytes = code.to_le_bytes();
    // SAFETY: `bytes` is a live `ERRNO_BYTES`-byte stack buffer; `write` reads no more than that length.
    unsafe {
        libc::write(status_w, bytes.as_ptr().cast(), ERRNO_BYTES);
    }
    // SAFETY: `_exit` terminates the child without running exit handlers or unwinding.
    unsafe { libc::_exit(exit_code) }
}

/// `exec` closes the child's `FD_CLOEXEC` write end, so end of file means the child reached `exec`.
fn await_child_status(status_r: OwnedFd) -> Result<(), SpawnError> {
    let mut reader = std::fs::File::from(status_r);
    let mut buf = [0u8; ERRNO_BYTES];
    let mut filled = 0;
    while filled < ERRNO_BYTES {
        match reader.read(&mut buf[filled..]) {
            Ok(0) => break,
            Ok(n) => filled += n,
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => {}
            Err(_) => return Err(SpawnError::new("child status pipe read failed")),
        }
    }
    match filled {
        0 => Ok(()),
        ERRNO_BYTES => Err(SpawnError {
            message: "daemon child failed before exec",
            child_error: Some(std::io::Error::from_raw_os_error(
                libc::c_int::from_le_bytes(buf),
            )),
        }),
        // A truncated status means the child died mid-write; treat it as an unknown pre-exec failure.
        _ => Err(SpawnError::new("daemon child failed before exec")),
    }
}

/// Production callers must supply a retained launcher descriptor.
/// A successful return proves only that the child reached `exec`; callers must wait for publication evidence before treating the daemon as ready. commentlint: allow(JUDGE)
///
/// # Errors
///
/// Child setup and `exec` failures carry the child's `errno` in `child_error`; launcher-side failures leave it `None`.
pub fn spawn_detached(
    log_path: &Path,
    envelope: &[u8],
    generation_launcher: Option<OwnedFd>,
) -> Result<(), SpawnError> {
    if envelope.len() > MAX_ENVELOPE_BYTES {
        return Err(SpawnError::new("startup envelope exceeds size bound"));
    }
    let log_fd = relocate_above_stderr(open_log(log_path)?)?;
    let exe_fd = match generation_launcher {
        Some(fd) => relocate_above_stderr(fd)?,
        None if test_self_exec_allowed() => {
            #[cfg(target_os = "linux")]
            let path = Path::new("/proc/self/exe").to_path_buf();
            #[cfg(target_os = "macos")]
            let path = std::env::current_exe()
                .map_err(|_| SpawnError::new("test executable path unavailable"))?;
            let exe = std::fs::File::open(path)
                .map_err(|_| SpawnError::new("test executable open failed"))?;
            let exe_meta = exe
                .metadata()
                .map_err(|_| SpawnError::new("executable stat failed"))?;
            // SAFETY: geteuid never fails and has no memory effects.
            let euid = unsafe { libc::geteuid() };
            // The launcher rejects other-writable executables because any user can modify the retained inode before `exec`.
            if !exe_meta.is_file() || exe_meta.uid() != euid || exe_meta.mode() & 0o002 != 0 {
                return Err(SpawnError::new("executable failed identity checks"));
            }
            relocate_above_stderr(OwnedFd::from(exe))?
        }
        None => return Err(SpawnError::new("verified generation launcher is required")),
    };

    let (pipe_r, pipe_w) = cloexec_pipe("envelope pipe creation failed")?;
    // `pipe_r` must not occupy a `dup2` destination slot because `dup2(0, 0)` preserves `FD_CLOEXEC`.
    let pipe_r = relocate_above_stderr(pipe_r)?;
    // The status pipe write end must survive the child's `dup2` calls onto fds 0 through 3 and the close sweep from fd 4.
    let (status_r, status_w) = cloexec_pipe("child status pipe creation failed")?;
    let status_w = relocate_at_least(status_w, EXE_SLOT + 1)?;
    let status_w_raw = status_w.as_raw_fd();

    // `fork` must follow initialization of all non-async-signal-safe child state; with Tokio workers alive, the child may call only async-signal-safe, nonallocating functions until `exec`.
    let argv0 = CString::new("eidnara-host").expect("static argv");
    let argv1 = CString::new("serve").expect("static argv");
    let argv: [*const libc::c_char; 3] = [argv0.as_ptr(), argv1.as_ptr(), std::ptr::null()];
    // glibc's `fexecve` falls back to `snprintf` plus `execve` when `execveat` is unavailable or seccomp-blocked, and `snprintf` is not async-signal-safe. commentlint: allow(JUDGE)
    // The kernel resolves this path before closing `FD_CLOEXEC` descriptors, so fd 3 keeps the flag and does not leak into the daemon. commentlint: allow(JUDGE)
    #[cfg(target_os = "linux")]
    let exe_path = CString::new("/proc/self/fd/3").expect("static exe path");
    #[cfg(target_os = "macos")]
    let exe_path = CString::new("/dev/fd/3").expect("static exe path");
    let envp: [*const libc::c_char; 1] = [std::ptr::null()];
    let root = CString::new("/").expect("static path");
    let close_ceiling = close_fallback_ceiling();
    // `exec` preserves blocked signals, so the child must clear its signal mask before `exec`.
    // An ignored `SIGCHLD` disposition auto-reaps child processes before Tokio can wait for them.
    // A blocked `SIGTERM` leaves the daemon unable to observe its termination signal.
    // SAFETY: `sigset_t` is a plain bit set for which the all-zero pattern is valid; `sigemptyset` overwrites it below.
    let mut empty_mask: libc::sigset_t = unsafe { std::mem::zeroed() };
    // SAFETY: `sigemptyset` writes only to the caller-provided set and has no other memory effects.
    unsafe {
        libc::sigemptyset(&mut empty_mask);
    }
    // SAFETY: `sigset_t` is a plain bit set for which the all-zero pattern is valid; `sigfillset` overwrites it below.
    let mut full_mask: libc::sigset_t = unsafe { std::mem::zeroed() };
    // SAFETY: `sigfillset` writes only to the caller-provided set and has no other memory effects.
    unsafe {
        libc::sigfillset(&mut full_mask);
    }
    // SAFETY: `sigset_t` is a plain bit set for which the all-zero pattern is valid; `pthread_sigmask` overwrites it with the saved mask below.
    let mut saved_mask: libc::sigset_t = unsafe { std::mem::zeroed() };
    // `max_signal` must be resolved before `fork` so the child makes no library call that could consult allocator or lock state.
    #[cfg(target_os = "linux")]
    let max_signal = libc::SIGRTMAX();
    // Darwin has no realtime signals or `libc::NSIG`; resetting signals above the highest named signal only returns `EINVAL`.
    #[cfg(not(target_os = "linux"))]
    let max_signal = libc::SIGUSR2;

    // Blocking every signal across `fork` keeps inherited handlers (std's `SIGSEGV` alternate-stack handler, Tokio's signal driver) from running in the child's copy of the address space before the child resets dispositions.
    // SAFETY: `full_mask` and `saved_mask` are initialized; `pthread_sigmask` writes only to `saved_mask`.
    let blocked = unsafe { libc::pthread_sigmask(libc::SIG_SETMASK, &full_mask, &mut saved_mask) };
    cvt(blocked, "signal mask block failed")?;
    // SAFETY: the child uses only async-signal-safe operations on preallocated descriptors and buffers before `exec`; the parent restores its signal mask immediately.
    let pid = unsafe { libc::fork() };
    if pid != 0 {
        // SAFETY: `saved_mask` was written by the successful `pthread_sigmask` above; this branch runs in the parent on both fork outcomes.
        unsafe {
            libc::pthread_sigmask(libc::SIG_SETMASK, &saved_mask, std::ptr::null_mut());
        }
    }
    if pid < 0 {
        return Err(SpawnError::new("fork failed"));
    }
    if pid == 0 {
        // SAFETY: `fork` leaves the child with one thread and copies of the parent's descriptors. commentlint: allow(JUDGE)
        // Every call is async-signal-safe libc on values resolved before `fork`: no allocation, no Rust I/O, no formatting, no unwinding. commentlint: allow(JUDGE)
        // Every failure diverges through `child_fail`, which writes `errno` and calls `_exit`; the final `child_fail` runs only if `execve` returns.
        // `status_w_raw` is at least fd 4, so no `dup2` below overwrites it, and the close sweep skips it.
        unsafe {
            // A new session detaches the daemon from the launcher's controlling terminal and process group.
            if libc::setsid() < 0 {
                child_fail(status_w_raw, 125);
            }
            libc::umask(0o077);
            if libc::chdir(root.as_ptr()) < 0 {
                child_fail(status_w_raw, 125);
            }
            // Reset every resettable disposition, then unblock all signals so the daemon does not inherit the launcher's signal state.
            // `SIGKILL` and `SIGSTOP` cannot be reset; `signal` returns `EINVAL` for both and the loop ignores it.
            for signum in 1..=max_signal {
                libc::signal(signum, libc::SIG_DFL);
            }
            libc::sigprocmask(libc::SIG_SETMASK, &empty_mask, std::ptr::null_mut());
            // The daemon has no terminal: stdin carries the envelope and both output streams go to the log.
            if libc::dup2(pipe_r.as_raw_fd(), 0) < 0
                || libc::dup2(log_fd.as_raw_fd(), 1) < 0
                || libc::dup2(log_fd.as_raw_fd(), 2) < 0
            {
                child_fail(status_w_raw, 125);
            }
            // `dup2` clears `FD_CLOEXEC` on its destination, and a launcher descriptor already at fd 3 may lack the flag, so the flag is set explicitly in both cases.
            if exe_fd.as_raw_fd() != EXE_SLOT && libc::dup2(exe_fd.as_raw_fd(), EXE_SLOT) < 0 {
                child_fail(status_w_raw, 125);
            }
            if libc::fcntl(EXE_SLOT, libc::F_SETFD, libc::FD_CLOEXEC) < 0 {
                child_fail(status_w_raw, 125);
            }
            // The status pipe write end is skipped so `exec` can close it and signal success.
            #[cfg(target_os = "linux")]
            let closed_range = {
                let below = status_w_raw as u32;
                let above = below + 1;
                (below == 4 || libc::syscall(libc::SYS_close_range, 4u32, below - 1, 0u32) >= 0)
                    && libc::syscall(libc::SYS_close_range, above, u32::MAX, 0u32) >= 0
            };
            #[cfg(not(target_os = "linux"))]
            let closed_range = false;
            if !closed_range {
                for fd in 4..=close_ceiling {
                    if fd != status_w_raw {
                        libc::close(fd);
                    }
                }
            }
            libc::execve(exe_path.as_ptr(), argv.as_ptr(), envp.as_ptr());
            child_fail(status_w_raw, 127);
        }
    }

    // Closing the parent's copies prevents them from keeping the child's pipe ends open.
    drop(pipe_r);
    drop(exe_fd);
    drop(log_fd);
    drop(status_w);
    // The child needs no envelope bytes before `exec`, so waiting for its status first cannot deadlock.
    await_child_status(status_r)?;
    let mut writer = std::fs::File::from(pipe_w);
    writer
        .write_all(envelope)
        .and_then(|()| writer.flush())
        .map_err(|_| SpawnError::new("startup envelope delivery failed"))?;
    Ok(())
}

pub(super) fn test_self_exec_allowed() -> bool {
    #[cfg(debug_assertions)]
    {
        std::env::var_os("EIDNARA_HOST_TEST_ALLOW_SELF_EXEC").is_some_and(|value| value == "1")
    }
    #[cfg(not(debug_assertions))]
    {
        false
    }
}

/// The launcher ignores `SIGPIPE` so a child that dies before consuming the envelope causes a write error instead of a fatal signal.
pub fn ignore_sigpipe() {
    // SAFETY: `SIG_IGN` is valid for `SIGPIPE` and does not access Rust memory.
    unsafe {
        libc::signal(libc::SIGPIPE, libc::SIG_IGN);
    }
}
