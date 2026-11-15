#![cfg_attr(not(all(target_os = "linux", target_env = "gnu")), allow(dead_code))]

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

use serde_json::{Value, json};

/// One glibc malloc arena for each SQLite connection the default route holds open: the memory
/// store's, the kernel store's writer, and its two readers.
pub const C_HEAP_ARENAS: usize = 4;
/// The size of one glibc arena heap.
pub const C_HEAP_RELEASE_THRESHOLD_BYTES: u64 = 64 * 1024 * 1024;

static ARENA_LIMIT_SET: AtomicBool = AtomicBool::new(false);
static RELEASING: AtomicBool = AtomicBool::new(false);
static RELEASES: Releases = Releases {
    resident_floor: AtomicU64::new(0),
    free_floor: AtomicU64::new(0),
    count: AtomicU64::new(0),
    resident_drop_bytes: AtomicU64::new(0),
};

/// The floors a release is judged against and the totals the status reports.
#[derive(Default)]
struct Releases {
    resident_floor: AtomicU64,
    free_floor: AtomicU64,
    count: AtomicU64,
    resident_drop_bytes: AtomicU64,
}

/// Freed C-heap bytes, where glibc reports them, and anonymous resident bytes at one instant.
#[derive(Debug, Clone, Copy)]
struct HeapReading {
    free_bytes: Option<u64>,
    resident_bytes: u64,
}

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

    fn heap_reading(&self, resident_bytes: u64) -> HeapReading {
        HeapReading {
            free_bytes: self.c_heap.map(|heap| heap.free_bytes),
            resident_bytes: self.rss_anon_bytes.unwrap_or(resident_bytes),
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
            "c_heap_releases": RELEASES.count.load(Ordering::Relaxed),
            "c_heap_release_resident_drop_bytes": RELEASES.resident_drop_bytes.load(Ordering::Relaxed),
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
    let status = std::fs::read_to_string("/proc/self/status").unwrap_or_default();
    let operator_set = arena_limit_is_operator_set(
        std::env::var("MALLOC_ARENA_MAX").ok().as_deref(),
        std::env::var("GLIBC_TUNABLES").ok().as_deref(),
    );
    if status_field(&status, "Threads") != Some(1) || operator_set {
        return;
    }
    // SAFETY: `mallopt` takes two integers, and the `Threads` count read above is 1, so no thread reads the limit during the write.
    let set = unsafe { libc::mallopt(libc::M_ARENA_MAX, C_HEAP_ARENAS as libc::c_int) == 1 };
    ARENA_LIMIT_SET.store(set, Ordering::Relaxed);
}

#[cfg(not(all(target_os = "linux", target_env = "gnu")))]
pub fn limit_c_heap_arenas() {}

fn arena_limit_is_operator_set(
    malloc_arena_max: Option<&str>,
    glibc_tunables: Option<&str>,
) -> bool {
    let sets_arena_max = |tunable: &str| {
        tunable
            .split_once('=')
            .is_some_and(|(name, value)| name == "glibc.malloc.arena_max" && !value.is_empty())
    };
    malloc_arena_max.is_some_and(|value| !value.is_empty())
        || glibc_tunables.is_some_and(|tunables| tunables.split(':').any(sets_arena_max))
}

fn lowered(floor: &AtomicU64, now: u64) -> u64 {
    floor.fetch_min(now, Ordering::Relaxed).min(now)
}

/// Freed bytes or resident bytes have grown one threshold past their floor since the last release.
/// `free_bytes` is `None` where glibc lacks `mallinfo2`; resident growth alone decides there.
fn release_is_due(
    free_bytes: Option<u64>,
    free_floor: u64,
    resident_bytes: u64,
    resident_floor: u64,
) -> bool {
    let grew = |now: u64, floor: u64| now.saturating_sub(floor) >= C_HEAP_RELEASE_THRESHOLD_BYTES;
    match free_bytes {
        Some(free) => {
            free >= C_HEAP_RELEASE_THRESHOLD_BYTES
                && (grew(free, free_floor) || grew(resident_bytes, resident_floor))
        }
        None => grew(resident_bytes, resident_floor),
    }
}

impl Releases {
    /// Lowers each floor to its reading, then judges the reading against the floors.
    fn is_due(&self, now: HeapReading) -> bool {
        let resident_floor = lowered(&self.resident_floor, now.resident_bytes);
        let free_floor = now
            .free_bytes
            .map_or(0, |free| lowered(&self.free_floor, free));
        release_is_due(
            now.free_bytes,
            free_floor,
            now.resident_bytes,
            resident_floor,
        )
    }

    /// Resets the floors after a trim and counts it as a release when resident memory fell.
    ///
    /// The freed-byte floor is the lower of the two readings, so bytes freed while the trim ran
    /// count as growth at the next check.
    fn record(&self, before: HeapReading, after: HeapReading, trimmed: bool) {
        self.resident_floor
            .store(after.resident_bytes, Ordering::Relaxed);
        let free_floor = after.free_bytes.map_or(0, |after| {
            before.free_bytes.map_or(after, |before| before.min(after))
        });
        self.free_floor.store(free_floor, Ordering::Relaxed);
        let dropped = before.resident_bytes.saturating_sub(after.resident_bytes);
        if trimmed && dropped > 0 {
            self.resident_drop_bytes
                .fetch_add(dropped, Ordering::Relaxed);
            self.count.fetch_add(1, Ordering::Relaxed);
        }
    }
}

/// Runs one trim when a release is due and reports whether it ran one.
#[cfg(all(target_os = "linux", target_env = "gnu"))]
#[allow(unsafe_code)]
pub fn release_free_c_heap_when_due() -> bool {
    if RELEASING.swap(true, Ordering::AcqRel) {
        return false;
    }
    let before = ProcessMemory::sample().heap_reading(0);
    let due = RELEASES.is_due(before);
    if due {
        // SAFETY: `malloc_trim` takes one integer and releases pages of free chunks only.
        let trimmed = unsafe { libc::malloc_trim(0) } == 1;
        let after = ProcessMemory::sample().heap_reading(before.resident_bytes);
        RELEASES.record(before, after, trimmed);
    }
    RELEASING.store(false, Ordering::Release);
    due
}

#[cfg(not(all(target_os = "linux", target_env = "gnu")))]
pub fn release_free_c_heap_when_due() -> bool {
    false
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
    fn a_release_is_due_once_freed_or_resident_bytes_grow_one_threshold_past_their_floor() {
        let floor = 500 * 1024 * 1024;
        assert!(!release_is_due(Some(floor), floor, floor, floor));
        assert!(!release_is_due(
            Some(floor),
            floor,
            floor + THRESHOLD - 1,
            floor
        ));
        assert!(release_is_due(Some(floor), floor, floor + THRESHOLD, floor));
        assert!(!release_is_due(
            Some(floor + THRESHOLD - 1),
            floor,
            floor,
            floor
        ));
        assert!(release_is_due(Some(floor + THRESHOLD), floor, floor, floor));
        assert!(release_is_due(Some(THRESHOLD), 0, 0, 0));
    }

    #[test]
    fn freed_bytes_under_one_threshold_start_no_release() {
        assert!(!release_is_due(Some(THRESHOLD - 1), 0, THRESHOLD * 8, 0));
    }

    #[test]
    fn resident_growth_alone_decides_where_glibc_reports_no_heap() {
        assert!(!release_is_due(None, 0, THRESHOLD - 1, 0));
        assert!(release_is_due(None, 0, THRESHOLD, 0));
    }

    #[test]
    fn a_floor_follows_a_reading_down_and_stays_when_the_reading_rises() {
        let floor = AtomicU64::new(500);
        assert_eq!(lowered(&floor, 300), 300);
        assert_eq!(lowered(&floor, 400), 300);
        assert_eq!(floor.load(Ordering::Relaxed), 300);
    }

    const MIB: u64 = 1024 * 1024;

    fn reading(free_bytes: Option<u64>, resident_bytes: u64) -> HeapReading {
        HeapReading {
            free_bytes,
            resident_bytes,
        }
    }

    #[test]
    fn a_reading_under_its_floor_lowers_the_floor_the_next_reading_is_judged_against() {
        let releases = Releases {
            resident_floor: AtomicU64::new(500 * MIB),
            ..Releases::default()
        };
        assert!(!releases.is_due(reading(None, 300 * MIB)));
        assert!(releases.is_due(reading(None, 300 * MIB + THRESHOLD)));

        let releases = Releases {
            free_floor: AtomicU64::new(900 * MIB),
            ..Releases::default()
        };
        assert!(!releases.is_due(reading(Some(300 * MIB), 0)));
        assert!(releases.is_due(reading(Some(300 * MIB + THRESHOLD), 0)));
    }

    #[test]
    fn a_release_that_lowers_resident_memory_is_counted_and_resets_the_floors() {
        let releases = Releases::default();
        let before = reading(Some(300 * MIB), 800 * MIB);
        assert!(releases.is_due(before));
        releases.record(before, reading(Some(280 * MIB), 520 * MIB), true);

        assert_eq!(releases.count.load(Ordering::Relaxed), 1);
        assert_eq!(
            releases.resident_drop_bytes.load(Ordering::Relaxed),
            280 * MIB
        );
        assert_eq!(releases.free_floor.load(Ordering::Relaxed), 280 * MIB);
        assert_eq!(releases.resident_floor.load(Ordering::Relaxed), 520 * MIB);
        assert!(!releases.is_due(reading(Some(280 * MIB), 520 * MIB)));
        assert!(releases.is_due(reading(Some(280 * MIB + THRESHOLD), 520 * MIB)));
    }

    #[test]
    fn a_trim_that_returns_no_pages_resets_the_floors_and_is_not_counted() {
        for (after_resident, trimmed) in [(800 * MIB, true), (700 * MIB, false)] {
            let releases = Releases::default();
            let before = reading(Some(300 * MIB), 800 * MIB);
            assert!(releases.is_due(before));
            let after = reading(Some(300 * MIB), after_resident);
            releases.record(before, after, trimmed);

            assert_eq!(releases.count.load(Ordering::Relaxed), 0);
            assert_eq!(releases.resident_drop_bytes.load(Ordering::Relaxed), 0);
            assert!(!releases.is_due(after));
        }
    }

    #[test]
    fn bytes_freed_while_a_release_runs_count_as_growth_for_the_next_check() {
        let releases = Releases::default();
        let before = reading(Some(100 * MIB), 400 * MIB);
        assert!(releases.is_due(before));
        let after = reading(Some(100 * MIB + THRESHOLD), 300 * MIB);
        releases.record(before, after, true);

        assert_eq!(releases.free_floor.load(Ordering::Relaxed), 100 * MIB);
        assert!(releases.is_due(after));
    }

    #[test]
    fn where_glibc_reports_no_heap_a_release_leaves_the_freed_byte_floor_at_zero() {
        let releases = Releases::default();
        let before = reading(None, 400 * MIB);
        assert!(releases.is_due(before));
        releases.record(before, reading(None, 300 * MIB), true);

        assert_eq!(releases.free_floor.load(Ordering::Relaxed), 0);
        assert_eq!(releases.resident_floor.load(Ordering::Relaxed), 300 * MIB);
    }

    #[test]
    fn an_operator_arena_setting_is_read_from_either_glibc_variable() {
        assert!(!arena_limit_is_operator_set(None, None));
        assert!(!arena_limit_is_operator_set(
            None,
            Some("glibc.malloc.tcache_count=0")
        ));
        assert!(arena_limit_is_operator_set(Some("8"), None));
        assert!(!arena_limit_is_operator_set(Some(""), None));
        assert!(arena_limit_is_operator_set(
            None,
            Some("glibc.malloc.arena_max=8")
        ));
        assert!(arena_limit_is_operator_set(
            None,
            Some("glibc.malloc.tcache_count=0:glibc.malloc.arena_max=2")
        ));
        assert!(!arena_limit_is_operator_set(
            None,
            Some("glibc.malloc.arena_maximum=8:glibc.malloc.arena_max=")
        ));
    }
}
