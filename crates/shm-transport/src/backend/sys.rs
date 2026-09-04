//! Every libc call the ring backend makes, one safe wrapper per call.
//!
//! Each wrapper holds exactly one `unsafe` block, so the crate's remaining raw syscall surface
//! is enumerable by reading this file. Wrappers that must pair a call with
//! `OwnedFd::from_raw_fd` or `assume_init` keep those steps inside the same block. The three
//! wrappers that act on a raw mapping address (`munmap`, `madvise_remove`, `mincore`) are
//! `unsafe fn`: no signature can prove the address describes a live mapping.

use std::ffi::CStr;
use std::io;
use std::mem::{MaybeUninit, size_of};
use std::os::fd::{AsRawFd, BorrowedFd, FromRawFd, OwnedFd};
use std::ptr::NonNull;

fn check(result: libc::c_int) -> io::Result<libc::c_int> {
    if result < 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(result)
    }
}

/// Seals every ring object carries: the size is fixed and no further seal can be added.
pub(crate) const RING_SEALS: libc::c_int =
    libc::F_SEAL_GROW | libc::F_SEAL_SHRINK | libc::F_SEAL_SEAL;

/// A close-on-exec memfd that accepts seals.
pub(crate) fn memfd_create(name: &CStr) -> io::Result<OwnedFd> {
    // SAFETY: `name` is a valid NUL-terminated string for the call's duration; a
    // non-negative return is a new descriptor this process owns and nothing else has seen.
    unsafe {
        let raw = libc::syscall(
            libc::SYS_memfd_create,
            name.as_ptr(),
            libc::MFD_CLOEXEC | libc::MFD_ALLOW_SEALING,
        ) as libc::c_int;
        check(raw).map(|raw| OwnedFd::from_raw_fd(raw))
    }
}

pub(crate) fn ftruncate(fd: BorrowedFd<'_>, len: libc::off_t) -> io::Result<()> {
    // SAFETY: `fd` is borrowed and therefore open; ftruncate takes no pointers.
    check(unsafe { libc::ftruncate(fd.as_raw_fd(), len) }).map(drop)
}

pub(crate) fn fchmod(fd: BorrowedFd<'_>, mode: libc::mode_t) -> io::Result<()> {
    // SAFETY: `fd` is borrowed and therefore open; fchmod takes no pointers.
    check(unsafe { libc::fchmod(fd.as_raw_fd(), mode) }).map(drop)
}

pub(crate) fn add_seals(fd: BorrowedFd<'_>, seals: libc::c_int) -> io::Result<()> {
    // SAFETY: `fd` is borrowed and therefore open; F_ADD_SEALS takes an integer argument.
    check(unsafe { libc::fcntl(fd.as_raw_fd(), libc::F_ADD_SEALS, seals) }).map(drop)
}

pub(crate) fn get_seals(fd: BorrowedFd<'_>) -> io::Result<libc::c_int> {
    // SAFETY: `fd` is borrowed and therefore open; F_GET_SEALS takes no argument.
    check(unsafe { libc::fcntl(fd.as_raw_fd(), libc::F_GET_SEALS) })
}

/// The `fstat` fields the ring validates.
pub(crate) struct FileStatus {
    pub(crate) mode: libc::mode_t,
    pub(crate) uid: libc::uid_t,
    pub(crate) size: libc::off_t,
}

pub(crate) fn fstat(fd: BorrowedFd<'_>) -> io::Result<FileStatus> {
    let mut stat = MaybeUninit::<libc::stat>::uninit();
    // SAFETY: `fd` is borrowed and therefore open; `stat` is writable storage of exactly
    // `sizeof(struct stat)`, and fstat fills it completely on a zero return, so the
    // `assume_init` only runs on success.
    let stat = unsafe {
        check(libc::fstat(fd.as_raw_fd(), stat.as_mut_ptr()))?;
        stat.assume_init()
    };
    Ok(FileStatus {
        mode: stat.st_mode,
        uid: stat.st_uid,
        size: stat.st_size,
    })
}

pub(crate) fn geteuid() -> libc::uid_t {
    // SAFETY: geteuid takes no arguments and cannot fail.
    unsafe { libc::geteuid() }
}

/// Maps `len` bytes of `fd` shared, read-write, from offset zero.
pub(crate) fn mmap_shared(fd: BorrowedFd<'_>, len: usize) -> io::Result<NonNull<u8>> {
    // SAFETY: a null hint lets the kernel pick the address; `fd` is borrowed and therefore
    // open; the caller has already checked `len` against the object's size, and a failed call
    // returns MAP_FAILED rather than touching memory.
    let mapped = unsafe {
        libc::mmap(
            std::ptr::null_mut(),
            len,
            libc::PROT_READ | libc::PROT_WRITE,
            libc::MAP_SHARED,
            fd.as_raw_fd(),
            0,
        )
    };
    if mapped == libc::MAP_FAILED {
        return Err(io::Error::last_os_error());
    }
    NonNull::new(mapped.cast()).ok_or_else(|| io::Error::from(io::ErrorKind::Other))
}

#[cfg(test)]
pub(crate) fn mmap_anonymous(len: usize) -> io::Result<NonNull<u8>> {
    // SAFETY: A null hint lets the kernel pick the address; no descriptor is involved. A failed
    // call returns `MAP_FAILED` rather than touching memory.
    let mapped = unsafe {
        libc::mmap(
            std::ptr::null_mut(),
            len,
            libc::PROT_READ | libc::PROT_WRITE,
            libc::MAP_PRIVATE | libc::MAP_ANONYMOUS,
            -1,
            0,
        )
    };
    if mapped == libc::MAP_FAILED {
        return Err(io::Error::last_os_error());
    }
    NonNull::new(mapped.cast()).ok_or_else(|| io::Error::from(io::ErrorKind::Other))
}

/// # Safety
///
/// `base` and `len` must be exactly what one successful `mmap_shared` or `mmap_anonymous`
/// call returned and accepted, and nothing may reference the mapping afterwards.
pub(crate) unsafe fn munmap(base: NonNull<u8>, len: usize) -> io::Result<()> {
    // SAFETY: the caller guarantees `base..base + len` is one live mapping unmapped once here.
    check(unsafe { libc::munmap(base.as_ptr().cast(), len) }).map(drop)
}

/// Punches `len` bytes at `base + offset` back to the kernel with `MADV_REMOVE`.
///
/// # Safety
///
/// `base + offset .. base + offset + len` must be a page-aligned range inside a live shared
/// mapping, and no live object may occupy it.
pub(crate) unsafe fn madvise_remove(
    base: NonNull<u8>,
    offset: usize,
    len: usize,
) -> io::Result<()> {
    // SAFETY: the caller guarantees the offset stays inside the mapping, so the pointer
    // arithmetic is in bounds, and the range holds no live byte.
    check(unsafe { libc::madvise(base.as_ptr().add(offset).cast(), len, libc::MADV_REMOVE) })
        .map(drop)
}

/// Fills `residency` with one byte per page of `base + offset .. + len`.
///
/// # Safety
///
/// `base + offset .. base + offset + len` must lie inside a live mapping.
pub(crate) unsafe fn mincore(
    base: NonNull<u8>,
    offset: usize,
    len: usize,
    residency: &mut [u8],
) -> io::Result<()> {
    let page = page_size();
    if residency.len() < len.div_ceil(page.max(1)) {
        return Err(io::Error::from(io::ErrorKind::InvalidInput));
    }
    // SAFETY: the caller guarantees the range is inside a live mapping, so the pointer
    // arithmetic is in bounds; `residency` was checked above to hold one byte per page, which
    // is all mincore writes.
    check(unsafe {
        libc::mincore(
            base.as_ptr().add(offset).cast(),
            len,
            residency.as_mut_ptr().cast(),
        )
    })
    .map(drop)
}

/// The kernel page size, or zero if `sysconf` reports none.
pub(crate) fn page_size() -> usize {
    // SAFETY: sysconf takes an integer name and no pointers.
    let size = unsafe { libc::sysconf(libc::_SC_PAGESIZE) };
    usize::try_from(size).unwrap_or(0)
}

/// Sends `token` with `MSG_DONTWAIT | MSG_NOSIGNAL`, so neither a peer that cleared
/// `O_NONBLOCK` nor a closed peer end can block or raise `SIGPIPE`.
pub(crate) fn send_token(fd: BorrowedFd<'_>, token: &[u8]) -> io::Result<usize> {
    // SAFETY: `fd` is borrowed and therefore open; `token` is an initialized slice, so the
    // pointer and length describe readable memory for the call's duration.
    let sent = unsafe {
        libc::send(
            fd.as_raw_fd(),
            token.as_ptr().cast(),
            token.len(),
            libc::MSG_DONTWAIT | libc::MSG_NOSIGNAL,
        )
    };
    usize::try_from(sent).map_err(|_| io::Error::last_os_error())
}

/// Receives into `buffer` with `MSG_DONTWAIT`; zero means the peer closed.
pub(crate) fn recv_tokens(fd: BorrowedFd<'_>, buffer: &mut [u8]) -> io::Result<usize> {
    // SAFETY: `fd` is borrowed and therefore open; `buffer` is an exclusive slice, so the
    // pointer and length describe writable memory for the call's duration.
    let received = unsafe {
        libc::recv(
            fd.as_raw_fd(),
            buffer.as_mut_ptr().cast(),
            buffer.len(),
            libc::MSG_DONTWAIT,
        )
    };
    usize::try_from(received).map_err(|_| io::Error::last_os_error())
}

/// Polls `fd` for `POLLIN`; `Ok(true)` when readable, `Ok(false)` on timeout.
pub(crate) fn poll_readable(fd: BorrowedFd<'_>, timeout_ms: libc::c_int) -> io::Result<bool> {
    let mut descriptor = libc::pollfd {
        fd: fd.as_raw_fd(),
        events: libc::POLLIN,
        revents: 0,
    };
    // SAFETY: `descriptor` is one initialized pollfd and the count passed is one.
    check(unsafe { libc::poll(&raw mut descriptor, 1, timeout_ms) }).map(|ready| ready > 0)
}

pub(crate) fn set_cloexec(fd: BorrowedFd<'_>) -> io::Result<()> {
    // SAFETY: `fd` is borrowed and therefore open; F_GETFD and F_SETFD take integers only.
    unsafe {
        let flags = check(libc::fcntl(fd.as_raw_fd(), libc::F_GETFD))?;
        check(libc::fcntl(
            fd.as_raw_fd(),
            libc::F_SETFD,
            flags | libc::FD_CLOEXEC,
        ))
        .map(drop)
    }
}

/// `SO_TYPE` of a socket; `ENOTSOCK` for anything else.
pub(crate) fn socket_type(fd: BorrowedFd<'_>) -> io::Result<libc::c_int> {
    let mut value: libc::c_int = 0;
    let mut len = size_of::<libc::c_int>() as libc::socklen_t;
    // SAFETY: `fd` is borrowed and therefore open; `value` and `len` are writable storage of
    // the size `len` declares, and getsockopt writes at most `len` bytes.
    let result = unsafe {
        libc::getsockopt(
            fd.as_raw_fd(),
            libc::SOL_SOCKET,
            libc::SO_TYPE,
            (&raw mut value).cast(),
            &raw mut len,
        )
    };
    check(result)?;
    if len != size_of::<libc::c_int>() as libc::socklen_t {
        return Err(io::Error::from(io::ErrorKind::InvalidData));
    }
    Ok(value)
}

#[cfg(test)]
pub(crate) fn eventfd() -> io::Result<OwnedFd> {
    // SAFETY: eventfd takes integers only; a non-negative return is a new descriptor this
    // process owns and nothing else has seen.
    unsafe {
        check(libc::eventfd(0, libc::EFD_CLOEXEC | libc::EFD_NONBLOCK))
            .map(|raw| OwnedFd::from_raw_fd(raw))
    }
}

/// An unconnected `AF_UNIX` stream socket.
#[cfg(test)]
pub(crate) fn unix_stream_socket() -> io::Result<OwnedFd> {
    // SAFETY: socket takes integers only; a non-negative return is a new descriptor this
    // process owns and nothing else has seen.
    unsafe {
        check(libc::socket(
            libc::AF_UNIX,
            libc::SOCK_STREAM | libc::SOCK_CLOEXEC,
            0,
        ))
        .map(|raw| OwnedFd::from_raw_fd(raw))
    }
}

#[cfg(test)]
pub(crate) fn is_cloexec(fd: BorrowedFd<'_>) -> io::Result<bool> {
    // SAFETY: `fd` is borrowed and therefore open; F_GETFD takes no argument.
    check(unsafe { libc::fcntl(fd.as_raw_fd(), libc::F_GETFD) })
        .map(|flags| flags & libc::FD_CLOEXEC != 0)
}

#[cfg(test)]
pub(crate) fn clear_cloexec(fd: BorrowedFd<'_>) -> io::Result<()> {
    // SAFETY: `fd` is borrowed and therefore open; F_GETFD and F_SETFD take integers only.
    unsafe {
        let flags = check(libc::fcntl(fd.as_raw_fd(), libc::F_GETFD))?;
        check(libc::fcntl(
            fd.as_raw_fd(),
            libc::F_SETFD,
            flags & !libc::FD_CLOEXEC,
        ))
        .map(drop)
    }
}
