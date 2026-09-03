use std::collections::BTreeSet;
use std::hint::black_box;
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use shm_transport::backend::ring::{DoorbellCounters, ProducerError, Ring, wire_v2_header};
use shm_transport::descriptor::HardwareProfileId;
use shm_transport::evidence::OperationCounters;
use shm_transport::profile::{TargetProfile, ring_profile as library_ring_profile};

const PROFILE: &str = "eventfd_sparse_ring";

/// Each ring producer/consumer wait and each h0 handshake step fails after this long.
const PEER_DEADLINE: Duration = Duration::from_secs(2);

/// Spins per burst before yielding; the yield lets a single-CPU host schedule the h0 peer.
const SPIN_BURST: u32 = 1024;

const PAGE_BYTES: usize = 4096;

const BODY_BYTE: u8 = 0x5a;

const ARMS: &[&str] = &[
    "h0_metadata_cacheline_ping_pong",
    "h1_raw_descriptor_ring_payload_touch",
    "copied_producer_copied_receiver",
    "copied_producer_leased_receiver",
    "direct_producer_copied_receiver",
    "direct_producer_leased_receiver",
    "h2_rust_napi_runtime_crossing",
    "injected_avoidable_operations",
    "ring",
];

/// Only eventfd-doorbell arms may have nonzero wake counters; h0 spins and yields, and
/// `injected_avoidable_operations` has no eventfd doorbell.
const EVENTFD_ARMS: &[&str] = &[
    "copied_producer_copied_receiver",
    "copied_producer_leased_receiver",
    "direct_producer_copied_receiver",
    "direct_producer_leased_receiver",
    "ring",
];

const UNIMPLEMENTED_ARMS: &[&str] = &[
    "h1_raw_descriptor_ring_payload_touch",
    "h2_rust_napi_runtime_crossing",
];

/// `syscalls` is `None` when an arm has no syscall counter.
#[derive(Clone, Debug, Deserialize, Serialize)]
struct Measurement {
    schema: u32,
    state: String,
    arm: String,
    profile: String,
    payload_bytes: usize,
    iterations: u64,
    elapsed_ns: u128,
    body_copies: u64,
    native_allocations: u64,
    syscalls: Option<u64>,
    park_wakes: u64,
    generic_queue_hops: u64,
    scheduler_handoffs: u64,
    checksum: u64,
    reason: Option<String>,
}

/// `syscalls` is `None` when the arm has no syscall counter.
struct ArmRun {
    elapsed: Duration,
    body_copies: u64,
    native_allocations: u64,
    syscalls: Option<u64>,
    park_wakes: u64,
    scheduler_handoffs: u64,
    checksum: u64,
}

/// `PeerReport` carries the peer's final timed-operation counters; the producer reads them
/// only after observing `done`.
#[repr(C)]
struct PeerReport {
    done: AtomicU64,
    checksum: AtomicU64,
    scheduler_handoffs: AtomicU64,
    syscalls: AtomicU64,
    park_wakes: AtomicU64,
}

impl PeerReport {
    const fn new() -> Self {
        Self {
            done: AtomicU64::new(0),
            checksum: AtomicU64::new(0),
            scheduler_handoffs: AtomicU64::new(0),
            syscalls: AtomicU64::new(0),
            park_wakes: AtomicU64::new(0),
        }
    }

    fn publish(&self, checksum: u64, scheduler_handoffs: u64, syscalls: u64, park_wakes: u64) {
        self.checksum.store(checksum, Ordering::Relaxed);
        self.scheduler_handoffs
            .store(scheduler_handoffs, Ordering::Relaxed);
        self.syscalls.store(syscalls, Ordering::Relaxed);
        self.park_wakes.store(park_wakes, Ordering::Relaxed);
        self.done.store(1, Ordering::Release);
    }
}

#[repr(C)]
struct PingPong {
    line: AtomicU64,
    report: PeerReport,
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.first().map(String::as_str) == Some("--child") {
        let arm = args.get(1).expect("child arm");
        let iterations = args.get(2).unwrap().parse().unwrap();
        let payload = args.get(3).unwrap().parse().unwrap();
        println!(
            "{}",
            serde_json::to_string(&measure(arm, iterations, payload)).unwrap()
        );
        return;
    }

    if args.iter().any(|arg| arg == "--designated-host") {
        eprintln!("--designated-host is not supported by the fixed ring smoke benchmark");
        std::process::exit(2);
    }
    // `cargo bench` passes `--bench`, while `cargo test --benches` passes no benchmark flag;
    // only `--bench` or `--campaign` enables the 20-period schedule.
    let campaign = args
        .iter()
        .any(|arg| arg == "--bench" || arg == "--campaign");
    let smoke = !campaign || args.iter().any(|arg| arg == "--smoke");
    let periods = if smoke { 1 } else { 20 };
    let iterations = if smoke { 64 } else { 100_000 };
    let payload = if smoke { 256 } else { 4096 };
    let executable = std::env::current_exe().unwrap();
    let mut attempts = Vec::new();
    for block in 0..periods {
        let forward = block % 2 == 0;
        for pass in 0..4 {
            let mut order = ARMS.to_vec();
            let reverse = matches!((forward, pass), (true, 1 | 2) | (false, 0 | 3));
            if reverse {
                order.reverse();
            }
            for arm in order {
                let output = Command::new(&executable)
                    .args([
                        "--child",
                        arm,
                        &iterations.to_string(),
                        &payload.to_string(),
                    ])
                    .output()
                    .expect("spawn isolated arm");
                let line = String::from_utf8_lossy(&output.stdout);
                let record = line
                    .lines()
                    .rev()
                    .find_map(|line| serde_json::from_str::<Measurement>(line).ok())
                    .unwrap_or_else(|| {
                        let stderr = String::from_utf8_lossy(&output.stderr);
                        let excerpt: String = stderr.trim().chars().take(240).collect();
                        let reason = format!(
                            "arm process produced no record: {}; stderr: {excerpt:?}",
                            output.status
                        );
                        failed(arm, payload, iterations, &reason)
                    });
                attempts.push(record);
            }
        }
    }
    let failed_arms: BTreeSet<&str> = attempts
        .iter()
        .filter(|record| record.state != "complete")
        .map(|record| record.arm.as_str())
        .filter(|arm| !UNIMPLEMENTED_ARMS.contains(arm))
        .collect();
    let mut blockers = vec![
        "no frozen ring A/A campaign".to_owned(),
        "no callback-budget sweep".to_owned(),
        "no designated-host campaign".to_owned(),
    ];
    blockers.extend(
        failed_arms
            .iter()
            .map(|arm| format!("implemented arm failed: {arm}")),
    );
    let complete = failed_arms.is_empty();
    let report = serde_json::json!({
        "schema": 1,
        "state": if complete { "complete" } else { "incomplete" },
        "local_verdict": if complete { "MECHANISM_SMOKE_ONLY" } else { "INCOMPLETE" },
        "designated_host_verdict": "BLOCKED",
        "blockers": blockers,
        "campaign": if smoke { "smoke" } else { "extended_smoke" },
        "manifest": "benches/manifests/v1.json",
        "manifest_scheduled": false,
        "period_unit": "fresh_arm_process",
        "paired_process_arms": ["h0_metadata_cacheline_ping_pong", "copied_producer_copied_receiver", "copied_producer_leased_receiver", "direct_producer_copied_receiver", "direct_producer_leased_receiver", "ring"],
        "eventfd_qualified_arms": EVENTFD_ARMS,
        "gate_control_arms": ["injected_avoidable_operations"],
        "unimplemented_arms": UNIMPLEMENTED_ARMS,
        "failed_implemented_arms": failed_arms,
        "aliased_arms": {
            "direct_producer_leased_receiver": "ring",
            "injected_avoidable_operations": "ring",
        },
        "order_blocks": ["ABBA", "BAAB"],
        "counter_fields": ["body_copies", "native_allocations", "syscalls", "park_wakes", "generic_queue_hops", "scheduler_handoffs"],
        "counter_scopes": {
            "body_copies": "producer body clones plus receiver to_vec copies, counted by the bench",
            "native_allocations": "one per counted copy",
            "syscalls": "ring doorbell read, write, and poll calls in both processes; h0 sched_yield calls in both processes",
            "park_wakes": "blocking doorbell polls in both processes",
            "generic_queue_hops": "structurally zero: the ring is the only hand-off",
            "scheduler_handoffs": "voluntary context switches (ru_nvcsw) of both processes over the timed window",
        },
        "attempts": attempts,
    });
    println!("{}", serde_json::to_string_pretty(&report).unwrap());
}

fn measure(arm: &str, iterations: u64, payload: usize) -> Measurement {
    let result = match arm {
        "h0_metadata_cacheline_ping_pong" => run_h0(iterations),
        "h1_raw_descriptor_ring_payload_touch" => {
            Err("raw descriptor control has no implementation distinct from the ring arm")
        }
        "direct_producer_leased_receiver" | "ring" | "injected_avoidable_operations" => {
            run_ring(iterations, payload, false, false)
        }
        "copied_producer_copied_receiver" => run_ring(iterations, payload, true, true),
        "copied_producer_leased_receiver" => run_ring(iterations, payload, true, false),
        "direct_producer_copied_receiver" => run_ring(iterations, payload, false, true),
        "h2_rust_napi_runtime_crossing" => {
            Err("runtime mechanism tests exist; paired H2 campaign has not run")
        }
        _ => Err("unknown arm"),
    };
    match result {
        Ok(run) => {
            let mut syscalls = run.syscalls;
            let mut counters = OperationCounters {
                body_copies: run.body_copies,
                native_allocations: run.native_allocations,
                syscalls: run.syscalls.unwrap_or(0),
                park_wakes: run.park_wakes,
                generic_queue_hops: 0,
                scheduler_handoffs: run.scheduler_handoffs,
            };
            if arm == "injected_avoidable_operations" {
                syscalls = Some(1);
                counters.body_copies = 1;
                counters.native_allocations = 1;
                counters.syscalls = 1;
                counters.park_wakes = 1;
                counters.generic_queue_hops = 1;
                counters.scheduler_handoffs = 1;
            }
            let disqualifications = counters.disqualifications(EVENTFD_ARMS.contains(&arm));
            let reason = if disqualifications.is_empty() {
                "smoke evidence is never designated-host qualification".to_owned()
            } else {
                format!("operation_counter_gate:{}", disqualifications.join(","))
            };
            Measurement {
                schema: 1,
                state: "complete".to_owned(),
                arm: arm.to_owned(),
                profile: PROFILE.to_owned(),
                payload_bytes: payload,
                iterations,
                elapsed_ns: run.elapsed.as_nanos(),
                body_copies: counters.body_copies,
                native_allocations: counters.native_allocations,
                syscalls,
                park_wakes: counters.park_wakes,
                generic_queue_hops: counters.generic_queue_hops,
                scheduler_handoffs: counters.scheduler_handoffs,
                checksum: run.checksum,
                reason: Some(reason),
            }
        }
        Err(reason) => failed(arm, payload, iterations, reason),
    }
}

fn failed(arm: &str, payload: usize, iterations: u64, reason: &str) -> Measurement {
    Measurement {
        schema: 1,
        state: "failed".to_owned(),
        arm: arm.to_owned(),
        profile: PROFILE.to_owned(),
        payload_bytes: payload,
        iterations,
        elapsed_ns: 0,
        body_copies: 0,
        native_allocations: 0,
        syscalls: None,
        park_wakes: 0,
        generic_queue_hops: 0,
        scheduler_handoffs: 0,
        checksum: 0,
        reason: Some(reason.to_owned()),
    }
}

fn voluntary_switches() -> u64 {
    let mut usage: libc::rusage = unsafe { std::mem::zeroed() };
    if unsafe { libc::getrusage(libc::RUSAGE_SELF, &mut usage) } != 0 {
        return 0;
    }
    u64::try_from(usage.ru_nvcsw).unwrap_or(0)
}

fn await_line(line: &AtomicU64, expected: u64, deadline: Instant) -> Option<u64> {
    let mut yields = 0u64;
    loop {
        for _ in 0..SPIN_BURST {
            if line.load(Ordering::Acquire) == expected {
                return Some(yields);
            }
            std::hint::spin_loop();
        }
        if Instant::now() >= deadline {
            return None;
        }
        unsafe { libc::sched_yield() };
        yields += 1;
    }
}

struct SharedPage(*mut libc::c_void);

impl SharedPage {
    fn map() -> Result<Self, &'static str> {
        let mapped = unsafe {
            libc::mmap(
                std::ptr::null_mut(),
                PAGE_BYTES,
                libc::PROT_READ | libc::PROT_WRITE,
                libc::MAP_SHARED | libc::MAP_ANONYMOUS,
                -1,
                0,
            )
        };
        if mapped == libc::MAP_FAILED {
            return Err("shared page mapping");
        }
        Ok(Self(mapped))
    }

    fn place<T>(&self, value: T) -> &T {
        const { assert!(std::mem::size_of::<T>() <= PAGE_BYTES) };
        let pointer = self.0.cast::<T>();
        // SAFETY: `T` fits in the page-aligned mapping, which outlives the returned borrow.
        unsafe {
            pointer.write(value);
            &*pointer
        }
    }
}

impl Drop for SharedPage {
    fn drop(&mut self) {
        unsafe { libc::munmap(self.0, PAGE_BYTES) };
    }
}

struct Peer(libc::pid_t);

impl Peer {
    fn spawn(body: impl FnOnce() -> i32) -> Result<Self, &'static str> {
        let pid = unsafe { libc::fork() };
        if pid < 0 {
            return Err("peer fork");
        }
        if pid == 0 {
            let status = body();
            unsafe { libc::_exit(status) };
        }
        Ok(Self(pid))
    }

    fn wait(self) -> Result<i32, &'static str> {
        let pid = self.0;
        std::mem::forget(self);
        reap(pid)
    }
}

impl Drop for Peer {
    fn drop(&mut self) {
        unsafe { libc::kill(self.0, libc::SIGKILL) };
        let _ = reap(self.0);
    }
}

fn reap(child: libc::pid_t) -> Result<i32, &'static str> {
    let mut status = 0;
    if unsafe { libc::waitpid(child, &mut status, 0) } != child {
        return Err("peer wait failed");
    }
    if libc::WIFEXITED(status) {
        Ok(libc::WEXITSTATUS(status))
    } else {
        Err("peer terminated")
    }
}

fn run_h0(iterations: u64) -> Result<ArmRun, &'static str> {
    let page = SharedPage::map()?;
    let shared = page.place(PingPong {
        line: AtomicU64::new(0),
        report: PeerReport::new(),
    });
    let peer = Peer::spawn(|| {
        let switches_before = voluntary_switches();
        let mut yields = 0u64;
        for sequence in 0..iterations {
            let request = sequence * 2 + 1;
            let Some(burst_yields) =
                await_line(&shared.line, request, Instant::now() + PEER_DEADLINE)
            else {
                return 6;
            };
            yields += burst_yields;
            shared.line.store(request + 1, Ordering::Release);
        }
        shared.report.publish(
            0,
            voluntary_switches().saturating_sub(switches_before),
            yields,
            0,
        );
        0
    })?;
    let switches_before = voluntary_switches();
    let mut yields = 0u64;
    let start = Instant::now();
    let mut stalled = false;
    for sequence in 0..iterations {
        let request = sequence * 2 + 1;
        shared.line.store(request, Ordering::Release);
        match await_line(&shared.line, request + 1, Instant::now() + PEER_DEADLINE) {
            Some(burst_yields) => yields += burst_yields,
            None => {
                stalled = true;
                break;
            }
        }
    }
    let elapsed = start.elapsed();
    let scheduler_handoffs = voluntary_switches().saturating_sub(switches_before);
    if stalled {
        return Err("h0 peer stalled");
    }
    if peer.wait()? != 0 {
        return Err("h0 peer failed");
    }
    if shared.report.done.load(Ordering::Acquire) != 1 {
        return Err("h0 peer reported nothing");
    }
    Ok(ArmRun {
        elapsed,
        body_copies: 0,
        native_allocations: 0,
        syscalls: Some(yields + shared.report.syscalls.load(Ordering::Relaxed)),
        park_wakes: 0,
        scheduler_handoffs: scheduler_handoffs
            + shared.report.scheduler_handoffs.load(Ordering::Relaxed),
        checksum: shared.line.load(Ordering::Relaxed),
    })
}

fn ring_profile() -> Result<TargetProfile, &'static str> {
    let hardware = HardwareProfileId::new(PROFILE).map_err(|_| "profile")?;
    library_ring_profile(hardware).map_err(|_| "profile")
}

fn run_ring(
    iterations: u64,
    payload_len: usize,
    copied_producer: bool,
    copied_receiver: bool,
) -> Result<ArmRun, &'static str> {
    let profile = ring_profile()?;
    let ring = Ring::create(&profile, 0).map_err(|_| "ring setup")?;
    let header = wire_v2_header(payload_len).map_err(|_| "header")?;
    let body = vec![BODY_BYTE; payload_len];
    let page = SharedPage::map()?;
    let report = page.place(PeerReport::new());
    let peer = Peer::spawn(|| ring_consumer(&ring, iterations, copied_receiver, report))?;

    let switches_before = voluntary_switches();
    let doorbells_before = ring.doorbell_counters();
    let start = Instant::now();
    let produced = produce(&ring, &body, header, iterations, copied_producer);
    let peer_finished =
        produced.is_ok() && await_line(&report.done, 1, Instant::now() + PEER_DEADLINE).is_some();
    let elapsed = start.elapsed();
    let scheduler_handoffs = voluntary_switches().saturating_sub(switches_before);
    let doorbells = ring.doorbell_counters().since(doorbells_before);

    let (copies, allocations) = produced?;
    if !peer_finished {
        return Err("ring peer stalled");
    }
    if peer.wait()? != 0 {
        return Err("ring peer failed");
    }
    let expected_checksum = iterations
        .wrapping_mul(payload_len as u64)
        .wrapping_mul(u64::from(BODY_BYTE));
    let checksum = report.checksum.load(Ordering::Relaxed);
    if checksum != expected_checksum {
        return Err("ring peer observed a different payload than the producer published");
    }
    let peer_doorbells = DoorbellCounters {
        syscalls: report.syscalls.load(Ordering::Relaxed),
        parks: report.park_wakes.load(Ordering::Relaxed),
    };
    let doorbells = doorbells.add(peer_doorbells);
    Ok(ArmRun {
        elapsed,
        body_copies: copies + if copied_receiver { iterations } else { 0 },
        native_allocations: allocations + if copied_receiver { iterations } else { 0 },
        syscalls: Some(doorbells.syscalls),
        park_wakes: doorbells.parks,
        scheduler_handoffs: scheduler_handoffs + report.scheduler_handoffs.load(Ordering::Relaxed),
        checksum,
    })
}

fn produce(
    ring: &Ring,
    body: &[u8],
    header: [u8; shm_transport::WIRE_V2_HEADER_BYTES],
    iterations: u64,
    copied_producer: bool,
) -> Result<(u64, u64), &'static str> {
    let payload_len = body.len();
    let mut copies = 0u64;
    let mut allocations = 0u64;
    for _ in 0..iterations {
        let mut reservation = match ring.try_reserve(payload_len, header) {
            Ok(reservation) => reservation,
            Err(ProducerError::Exhausted) => ring
                .reserve_until(payload_len, header, Instant::now() + PEER_DEADLINE)
                .map_err(|_| "reserve")?,
            Err(_) => return Err("reserve"),
        };
        if copied_producer {
            let copied = body.to_vec();
            copies += 1;
            allocations += 1;
            reservation.write(&copied).map_err(|_| "write")?;
        } else {
            for index in 0..reservation.segment_count() {
                let span = reservation
                    .segment(index)
                    .map_err(|_| "segment")?
                    .ok_or("segment")?;
                // SAFETY: `reservation` owns this span until `commit`.
                unsafe { std::ptr::write_bytes(span.as_mut_ptr(), BODY_BYTE, span.len()) };
                reservation.advance(span.len()).map_err(|_| "advance")?;
            }
        }
        reservation.commit(payload_len).map_err(|_| "commit")?;
    }
    black_box(body);
    Ok((copies, allocations))
}

fn ring_consumer(ring: &Ring, iterations: u64, copied_receiver: bool, report: &PeerReport) -> i32 {
    let switches_before = voluntary_switches();
    let doorbells_before = ring.doorbell_counters();
    let mut checksum = 0u64;
    for _ in 0..iterations {
        let deadline = Instant::now() + PEER_DEADLINE;
        let lease = loop {
            match ring.try_receive() {
                Ok(Some(lease)) => break lease,
                Ok(None) if Instant::now() < deadline => {
                    if ring.wait_for_data(deadline).is_err() {
                        return 2;
                    }
                }
                _ => return 2,
            }
        };
        if copied_receiver {
            let Ok(bytes) = lease.to_vec() else {
                return 3;
            };
            checksum = bytes
                .iter()
                .fold(checksum, |sum, byte| sum.wrapping_add(u64::from(*byte)));
        } else {
            for index in 0..lease.segment_count() {
                let Some(span) = lease.segment(index) else {
                    return 4;
                };
                checksum = checksum.wrapping_add(span.checksum());
            }
        }
        if lease.release().is_err() {
            return 5;
        }
    }
    let doorbells = ring.doorbell_counters().since(doorbells_before);
    report.publish(
        checksum,
        voluntary_switches().saturating_sub(switches_before),
        doorbells.syscalls,
        doorbells.parks,
    );
    0
}
