use std::hint::black_box;
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use shm_transport::backend::ring::{ProducerError, Ring, wire_v2_header};
use shm_transport::descriptor::HardwareProfileId;
use shm_transport::evidence::OperationCounters;
use shm_transport::profile::{TargetProfile, ring_profile as library_ring_profile};

const PROFILE: &str = "eventfd_sparse_ring";

/// Each ring producer/consumer wait and each h0 handshake step fails after this long.
const PEER_DEADLINE: Duration = Duration::from_secs(2);

/// Spins per burst before yielding; the yield lets a single-CPU host schedule the h0 peer.
const SPIN_BURST: u32 = 1024;

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

struct ArmRun {
    elapsed: Duration,
    body_copies: u64,
    native_allocations: u64,
    /// `park_wakes` counts producer capacity-doorbell slow paths and consumer data-doorbell
    /// waits.
    park_wakes: u64,
    /// `scheduler_handoffs` counts voluntary context switches by the timed process during the
    /// timed window.
    scheduler_handoffs: u64,
    checksum: u64,
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
                    .unwrap_or_else(|| failed(arm, payload, iterations, "arm process failed"));
                attempts.push(record);
            }
        }
    }
    let report = serde_json::json!({
        "schema": 1,
        "state": "complete",
        "local_verdict": "MECHANISM_SMOKE_ONLY",
        "designated_host_verdict": "BLOCKED",
        "blockers": ["no frozen ring A/A campaign", "no callback-budget sweep", "no designated-host campaign"],
        "campaign": if smoke { "smoke" } else { "manifest_schedule" },
        "manifest": "benches/manifests/v1.json",
        "period_unit": "fresh_arm_process",
        "paired_process_arms": ["h0_metadata_cacheline_ping_pong", "copied_producer_copied_receiver", "copied_producer_leased_receiver", "direct_producer_copied_receiver", "direct_producer_leased_receiver", "ring"],
        "gate_control_arms": ["injected_avoidable_operations"],
        "unimplemented_arms": ["h1_raw_descriptor_ring_payload_touch", "h2_rust_napi_runtime_crossing"],
        "aliased_arms": {
            "direct_producer_leased_receiver": "ring",
            "injected_avoidable_operations": "ring",
        },
        "order_blocks": ["ABBA", "BAAB"],
        "counter_fields": ["body_copies", "native_allocations", "syscalls", "park_wakes", "generic_queue_hops", "scheduler_handoffs"],
        "unmeasured_counter_fields": ["syscalls"],
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
            let mut syscalls = None;
            let mut counters = OperationCounters {
                body_copies: run.body_copies,
                native_allocations: run.native_allocations,
                syscalls: 0,
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
            let disqualifications = counters.disqualifications(true);
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

fn await_line(line: &AtomicU64, expected: u64, deadline: Instant) -> bool {
    loop {
        for _ in 0..SPIN_BURST {
            if line.load(Ordering::Acquire) == expected {
                return true;
            }
            std::hint::spin_loop();
        }
        if Instant::now() >= deadline {
            return false;
        }
        unsafe { libc::sched_yield() };
    }
}

fn run_h0(iterations: u64) -> Result<ArmRun, &'static str> {
    let mapped = unsafe {
        libc::mmap(
            std::ptr::null_mut(),
            4096,
            libc::PROT_READ | libc::PROT_WRITE,
            libc::MAP_SHARED | libc::MAP_ANONYMOUS,
            -1,
            0,
        )
    };
    if mapped == libc::MAP_FAILED {
        return Err("h0 mapping");
    }
    let line = mapped.cast::<AtomicU64>();
    unsafe { line.write(AtomicU64::new(0)) };
    let child = unsafe { libc::fork() };
    if child < 0 {
        unsafe { libc::munmap(mapped, 4096) };
        return Err("h0 fork");
    }
    if child == 0 {
        let line = unsafe { &*line };
        for sequence in 0..iterations {
            let request = sequence * 2 + 1;
            if !await_line(line, request, Instant::now() + PEER_DEADLINE) {
                unsafe { libc::_exit(6) };
            }
            line.store(request + 1, Ordering::Release);
        }
        unsafe { libc::_exit(0) };
    }
    let line_ref = unsafe { &*line };
    let switches_before = voluntary_switches();
    let start = Instant::now();
    let mut stalled = false;
    for sequence in 0..iterations {
        let request = sequence * 2 + 1;
        line_ref.store(request, Ordering::Release);
        if !await_line(line_ref, request + 1, Instant::now() + PEER_DEADLINE) {
            stalled = true;
            break;
        }
    }
    let elapsed = start.elapsed();
    let scheduler_handoffs = voluntary_switches().saturating_sub(switches_before);
    if stalled {
        unsafe { libc::kill(child, libc::SIGKILL) };
    }
    let status = wait_child(child);
    let checksum = line_ref.load(Ordering::Relaxed);
    unsafe { libc::munmap(mapped, 4096) };
    if stalled {
        return Err("h0 peer stalled");
    }
    if status? != 0 {
        return Err("h0 peer failed");
    }
    Ok(ArmRun {
        elapsed,
        body_copies: 0,
        native_allocations: 0,
        park_wakes: 0,
        scheduler_handoffs,
        checksum,
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
    let body = vec![0x5a; payload_len];
    let mapped = unsafe {
        libc::mmap(
            std::ptr::null_mut(),
            4096,
            libc::PROT_READ | libc::PROT_WRITE,
            libc::MAP_SHARED | libc::MAP_ANONYMOUS,
            -1,
            0,
        )
    };
    if mapped == libc::MAP_FAILED {
        return Err("ring counter mapping");
    }
    let park_wakes = mapped.cast::<AtomicU64>();
    unsafe { park_wakes.write(AtomicU64::new(0)) };
    let child = unsafe { libc::fork() };
    if child < 0 {
        unsafe { libc::munmap(mapped, 4096) };
        return Err("ring peer fork");
    }
    if child == 0 {
        let status = ring_consumer(&ring, iterations, copied_receiver, unsafe { &*park_wakes });
        unsafe { libc::_exit(status) };
    }
    let park_wakes = unsafe { &*park_wakes };

    let mut copies = 0u64;
    let mut allocations = 0u64;
    let header = wire_v2_header(payload_len).map_err(|_| "header")?;
    let switches_before = voluntary_switches();
    let start = Instant::now();
    for _ in 0..iterations {
        let copied;
        let source = if copied_producer {
            copied = body.clone();
            copies += 1;
            allocations += 1;
            copied.as_slice()
        } else {
            body.as_slice()
        };
        let mut reservation = match ring.try_reserve(payload_len, header) {
            Ok(reservation) => reservation,
            Err(ProducerError::Exhausted) => {
                park_wakes.fetch_add(1, Ordering::Relaxed);
                ring.reserve_until(payload_len, header, Instant::now() + PEER_DEADLINE)
                    .map_err(|_| "reserve")?
            }
            Err(_) => return Err("reserve"),
        };
        reservation.write(source).map_err(|_| "write")?;
        reservation.commit(payload_len).map_err(|_| "commit")?;
    }
    let elapsed = start.elapsed();
    let scheduler_handoffs = voluntary_switches().saturating_sub(switches_before);
    let status = wait_child(child);
    let park_wakes = park_wakes.load(Ordering::Relaxed);
    unsafe { libc::munmap(mapped, 4096) };
    if status? != 0 {
        return Err("ring peer failed");
    }
    if copied_receiver {
        copies += iterations;
        allocations += iterations;
    }
    let checksum = iterations
        .wrapping_mul(payload_len as u64)
        .wrapping_mul(0x5a);
    black_box(checksum);
    Ok(ArmRun {
        elapsed,
        body_copies: copies,
        native_allocations: allocations,
        park_wakes,
        scheduler_handoffs,
        checksum,
    })
}

fn ring_consumer(
    ring: &Ring,
    iterations: u64,
    copied_receiver: bool,
    park_wakes: &AtomicU64,
) -> i32 {
    for _ in 0..iterations {
        let deadline = Instant::now() + PEER_DEADLINE;
        let lease = loop {
            match ring.try_receive() {
                Ok(Some(lease)) => break lease,
                Ok(None) if Instant::now() < deadline => {
                    park_wakes.fetch_add(1, Ordering::Relaxed);
                    if ring.wait_for_data(deadline).is_err() {
                        return 2;
                    }
                }
                _ => return 2,
            }
        };
        if copied_receiver {
            if lease.to_vec().is_err() {
                return 3;
            }
        } else {
            for index in 0..lease.segment_count() {
                let Some(span) = lease.segment(index) else {
                    return 4;
                };
                black_box(span.checksum());
            }
        }
        if lease.release().is_err() {
            return 5;
        }
    }
    0
}

fn wait_child(child: libc::pid_t) -> Result<i32, &'static str> {
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
