use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

use serde_json::{Value, json};

pub const C_HEAP_ARENAS: usize = 4;
pub const C_HEAP_RELEASE_THRESHOLD_BYTES: u64 = 64 * 1024 * 1024;

static RELEASING: AtomicBool = AtomicBool::new(false);
static FREE_AFTER_LAST_RELEASE: AtomicU64 = AtomicU64::new(0);
static RELEASES: AtomicU64 = AtomicU64::new(0);
static RELEASED_BYTES: AtomicU64 = AtomicU64::new(0);

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
            "c_heap_releases": RELEASES.load(Ordering::Relaxed),
            "c_heap_released_bytes": RELEASED_BYTES.load(Ordering::Relaxed),
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
    // SAFETY: `mallinfo2` takes no arguments and returns its totals by value.
    let info = unsafe { libc::mallinfo2() };
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
pub fn limit_c_heap_arenas() -> bool {
    // SAFETY: `mallopt` takes two integers and changes allocator tuning only.
    unsafe { libc::mallopt(libc::M_ARENA_MAX, C_HEAP_ARENAS as libc::c_int) == 1 }
}

#[cfg(not(all(target_os = "linux", target_env = "gnu")))]
pub fn limit_c_heap_arenas() -> bool {
    false
}

pub fn c_heap_release_is_due() -> bool {
    c_heap().is_some_and(|heap| release_is_due(heap.free_bytes))
}

fn release_is_due(free_bytes: u64) -> bool {
    free_bytes.saturating_sub(FREE_AFTER_LAST_RELEASE.load(Ordering::Relaxed))
        > C_HEAP_RELEASE_THRESHOLD_BYTES
}

#[cfg(all(target_os = "linux", target_env = "gnu"))]
#[allow(unsafe_code)]
pub fn release_free_c_heap() -> Option<u64> {
    if RELEASING.swap(true, Ordering::AcqRel) {
        return None;
    }
    let before = ProcessMemory::sample().rss_anon_bytes.unwrap_or(0);
    // SAFETY: `malloc_trim` takes one integer and releases pages of free chunks only.
    unsafe { libc::malloc_trim(0) };
    let after = ProcessMemory::sample();
    let released = before.saturating_sub(after.rss_anon_bytes.unwrap_or(before));
    FREE_AFTER_LAST_RELEASE.store(
        after.c_heap.map_or(0, |heap| heap.free_bytes),
        Ordering::Relaxed,
    );
    RELEASES.fetch_add(1, Ordering::Relaxed);
    RELEASED_BYTES.fetch_add(released, Ordering::Relaxed);
    RELEASING.store(false, Ordering::Release);
    Some(released)
}

#[cfg(not(all(target_os = "linux", target_env = "gnu")))]
pub fn release_free_c_heap() -> Option<u64> {
    None
}

#[cfg(test)]
pub(crate) fn fragmented_c_heap() -> (Vec<Vec<u8>>, u64) {
    const CHUNK_BYTES: usize = 64 * 1024;
    const CHUNKS: usize = 4096;
    const KEEP_EVERY: usize = 64;
    let chunks: Vec<Vec<u8>> = (0..CHUNKS).map(|_| vec![1_u8; CHUNK_BYTES]).collect();
    let kept = chunks
        .into_iter()
        .enumerate()
        .filter_map(|(index, chunk)| (index % KEEP_EVERY == 0).then_some(chunk))
        .collect();
    (kept, (CHUNK_BYTES * CHUNKS) as u64)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn status_fields_parse_their_first_number() {
        let status = "Name:\tdaemon\nRssAnon:\t   2048 kB\nRssFile:\t 16 kB\nThreads:\t9\n";
        assert_eq!(status_field(status, "RssAnon"), Some(2048));
        assert_eq!(status_field(status, "RssFile"), Some(16));
        assert_eq!(status_field(status, "Threads"), Some(9));
        assert_eq!(status_field(status, "RssShmem"), None);
        assert_eq!(status_field(status, "Rss"), None);
    }

    #[cfg(all(target_os = "linux", target_env = "gnu"))]
    #[test]
    fn a_release_returns_the_pages_of_freed_c_heap_chunks() {
        let (kept, total) = fragmented_c_heap();
        let held = ProcessMemory::sample();
        let heap = held.c_heap.expect("glibc reports its heap");
        assert!(heap.free_bytes > total / 2, "{heap:?}");
        assert!(c_heap_release_is_due());

        let released = release_free_c_heap().expect("no other release runs");
        assert!(released > total / 2, "released {released} of {total}");
        let after = ProcessMemory::sample();
        assert!(
            held.rss_anon_bytes.unwrap() - after.rss_anon_bytes.unwrap() > total / 2,
            "{held:?} then {after:?}"
        );
        assert!(!c_heap_release_is_due());
        assert_eq!(after.to_json()["c_heap_releases"], 1);
        drop(kept);
    }
}
