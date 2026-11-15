use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

use serde_json::{Value, json};

pub const C_HEAP_ARENAS: usize = 4;
pub const C_HEAP_RELEASE_THRESHOLD_BYTES: u64 = 64 * 1024 * 1024;

static ARENA_LIMIT_SET: AtomicBool = AtomicBool::new(false);
static RELEASING: AtomicBool = AtomicBool::new(false);
static RESIDENT_AFTER_RELEASE: AtomicU64 = AtomicU64::new(0);
static RELEASES: AtomicU64 = AtomicU64::new(0);
static RELEASE_RESIDENT_DROP_BYTES: AtomicU64 = AtomicU64::new(0);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CHeap {
    pub in_use_bytes: u64,
    pub free_bytes: u64,
    pub mmapped_bytes: u64,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ProcessMemory {
    pub rss_anon_bytes: Option<u64>,
    pub rss_file_bytes: Option<u64>,
    pub rss_shmem_bytes: Option<u64>,
    pub threads: Option<u64>,
    pub c_heap: Option<CHeap>,
}

impl ProcessMemory {
    pub fn sample() -> Self {
        let status = std::fs::read_to_string("/proc/self/status").unwrap_or_default();
        Self {
            rss_anon_bytes: status_field(&status, "RssAnon").map(kib),
            rss_file_bytes: status_field(&status, "RssFile").map(kib),
            rss_shmem_bytes: status_field(&status, "RssShmem").map(kib),
            threads: status_field(&status, "Threads"),
            c_heap: c_heap(),
        }
    }

    pub fn to_json(&self) -> Value {
        json!({
            "rss_anon_bytes": self.rss_anon_bytes,
            "rss_file_bytes": self.rss_file_bytes,
            "rss_shmem_bytes": self.rss_shmem_bytes,
            "threads": self.threads,
            "c_heap": self.c_heap.map(|heap| json!({
                "in_use_bytes": heap.in_use_bytes,
                "free_bytes": heap.free_bytes,
                "mmapped_bytes": heap.mmapped_bytes,
            })),
            "c_heap_arena_limit": ARENA_LIMIT_SET.load(Ordering::Relaxed).then_some(C_HEAP_ARENAS),
            "c_heap_releases": RELEASES.load(Ordering::Relaxed),
            "c_heap_release_resident_drop_bytes": RELEASE_RESIDENT_DROP_BYTES.load(Ordering::Relaxed),
        })
    }
}

fn kib(value: u64) -> u64 {
    value.saturating_mul(1024)
}

fn status_field(status: &str, name: &str) -> Option<u64> {
    status
        .lines()
        .find_map(|line| line.strip_prefix(name)?.strip_prefix(':'))?
        .split_whitespace()
        .next()?
        .parse()
        .ok()
}

#[cfg(all(target_os = "linux", target_env = "gnu"))]
#[allow(unsafe_code)]
fn c_heap() -> Option<CHeap> {
    type Mallinfo2 = unsafe extern "C" fn() -> libc::mallinfo2;
    static MALLINFO2: std::sync::OnceLock<Option<Mallinfo2>> = std::sync::OnceLock::new();
    let mallinfo2 = (*MALLINFO2.get_or_init(|| {
        // SAFETY: `dlsym` reads the NUL-terminated name and returns null when no loaded object exports it.
        let symbol = unsafe { libc::dlsym(libc::RTLD_DEFAULT, c"mallinfo2".as_ptr()) };
        // SAFETY: a non-null `mallinfo2` symbol is glibc's `struct mallinfo2 mallinfo2(void)`.
        (!symbol.is_null())
            .then(|| unsafe { std::mem::transmute::<*mut libc::c_void, Mallinfo2>(symbol) })
    }))?;
    // SAFETY: `mallinfo2` takes no arguments and returns its totals by value.
    let info = unsafe { mallinfo2() };
    Some(CHeap {
        in_use_bytes: info.uordblks as u64,
        free_bytes: info.fordblks as u64,
        mmapped_bytes: info.hblkhd as u64,
    })
}

#[cfg(not(all(target_os = "linux", target_env = "gnu")))]
fn c_heap() -> Option<CHeap> {
    None
}

#[cfg(all(target_os = "linux", target_env = "gnu"))]
#[allow(unsafe_code)]
pub fn limit_c_heap_arenas() {
    let single_threaded = ProcessMemory::sample().threads == Some(1);
    if !single_threaded || std::env::var_os("MALLOC_ARENA_MAX").is_some() {
        return;
    }
    // SAFETY: `mallopt` takes two integers, and `single_threaded` holds, so no thread reads the limit during the write.
    let set = unsafe { libc::mallopt(libc::M_ARENA_MAX, C_HEAP_ARENAS as libc::c_int) == 1 };
    ARENA_LIMIT_SET.store(set, Ordering::Relaxed);
}

#[cfg(not(all(target_os = "linux", target_env = "gnu")))]
pub fn limit_c_heap_arenas() {}

pub fn release_in_flight() -> bool {
    RELEASING.load(Ordering::Acquire)
}

fn release_is_due(free_bytes: u64, resident_bytes: u64, resident_after_release: u64) -> bool {
    free_bytes >= C_HEAP_RELEASE_THRESHOLD_BYTES
        && resident_bytes.saturating_sub(resident_after_release) >= C_HEAP_RELEASE_THRESHOLD_BYTES
}

#[cfg(all(target_os = "linux", target_env = "gnu"))]
#[allow(unsafe_code)]
pub fn release_free_c_heap_when_due() -> Option<u64> {
    if RELEASING.swap(true, Ordering::AcqRel) {
        return None;
    }
    let before = ProcessMemory::sample();
    let resident = before.rss_anon_bytes.unwrap_or(0);
    let due = before.c_heap.is_some_and(|heap| {
        release_is_due(
            heap.free_bytes,
            resident,
            RESIDENT_AFTER_RELEASE.load(Ordering::Relaxed),
        )
    });
    let dropped = due.then(|| {
        // SAFETY: `malloc_trim` takes one integer and releases pages of free chunks only.
        unsafe { libc::malloc_trim(0) };
        let after = status_field(
            &std::fs::read_to_string("/proc/self/status").unwrap_or_default(),
            "RssAnon",
        )
        .map_or(resident, kib);
        RESIDENT_AFTER_RELEASE.store(after, Ordering::Relaxed);
        RELEASES.fetch_add(1, Ordering::Relaxed);
        let dropped = resident.saturating_sub(after);
        RELEASE_RESIDENT_DROP_BYTES.fetch_add(dropped, Ordering::Relaxed);
        dropped
    });
    RELEASING.store(false, Ordering::Release);
    dropped
}

#[cfg(not(all(target_os = "linux", target_env = "gnu")))]
pub fn release_free_c_heap_when_due() -> Option<u64> {
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    const THRESHOLD: u64 = C_HEAP_RELEASE_THRESHOLD_BYTES;

    #[test]
    fn status_fields_parse_their_first_number() {
        let status = "Name:\tdaemon\nRssAnon:\t   2048 kB\nRssFile:\t 16 kB\nThreads:\t9\n";
        assert_eq!(status_field(status, "RssAnon"), Some(2048));
        assert_eq!(status_field(status, "RssFile"), Some(16));
        assert_eq!(status_field(status, "Threads"), Some(9));
        assert_eq!(status_field(status, "RssShmem"), None);
        assert_eq!(status_field(status, "Rss"), None);
    }

    #[test]
    fn a_release_is_due_when_freed_bytes_and_resident_growth_both_reach_the_threshold() {
        let resident_after_release = 500 * 1024 * 1024;
        assert!(release_is_due(
            THRESHOLD,
            resident_after_release + THRESHOLD,
            resident_after_release
        ));
        assert!(!release_is_due(
            THRESHOLD - 1,
            resident_after_release + THRESHOLD,
            resident_after_release
        ));
        assert!(!release_is_due(
            THRESHOLD * 8,
            resident_after_release + THRESHOLD - 1,
            resident_after_release
        ));
        assert!(!release_is_due(
            THRESHOLD * 8,
            resident_after_release - THRESHOLD,
            resident_after_release
        ));
        assert!(release_is_due(THRESHOLD, THRESHOLD, 0));
    }
}
