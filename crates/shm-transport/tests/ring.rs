//! Real-endpoint witnesses for the payload-pool ring: setup identity, sealed-object
//! attachment, and the two-process exchange that proves independent reuse, descriptor wakes,
//! maximum frames, and owned returns across a process boundary.
#![deny(clippy::undocumented_unsafe_blocks)]
use std::io::{Read, Write};
use std::os::fd::OwnedFd;
use std::os::fd::{AsRawFd, FromRawFd};
use std::process::{Child, Command, Stdio};
use std::time::Duration;
use std::time::Instant;

use shm_transport::MAX_FRAME_BYTES;
use shm_transport::backend::ring::{
    PoolGrant, ProducerError, Ring, RingAttachment, RingError, wire_v3_header,
};
use shm_transport::descriptor::HardwareProfileId;
use shm_transport::lease::PayloadLease;
use shm_transport::pool::{ClassSpec, Inventory, PoolGeometry};
use shm_transport::profile::{TargetProfile, host_payload_pool_profile, pool_profile};

/// Two ordinary descriptors and three 4 KiB blocks, so laps and class exhaustion are cheap.
fn small_geometry() -> PoolGeometry {
    PoolGeometry::new(
        2,
        1,
        [
            ClassSpec::new(4096, 3),
            ClassSpec::new(64 * 1024, 2),
            ClassSpec::new(1024 * 1024, 1),
            ClassSpec::new(8 * 1024 * 1024, 1),
            ClassSpec::new(64 * 1024 * 1024 + 4096, 1),
        ],
        ClassSpec::new(4096, 2),
        ClassSpec::new(32 * 1024, 2),
    )
    .unwrap()
}

fn profile() -> TargetProfile {
    pool_profile(
        HardwareProfileId::new("ring-contract-host").unwrap(),
        small_geometry(),
    )
    .unwrap()
}

fn publish(ring: &Ring, body: &[u8]) -> u32 {
    let mut reservation = ring
        .try_reserve(body.len(), wire_v3_header(body.len()).unwrap())
        .unwrap();
    reservation.write(body).unwrap();
    reservation.commit(body.len()).unwrap().block()
}

fn receive(ring: &Ring, deadline: Instant) -> PayloadLease {
    loop {
        if let Some(lease) = ring.try_receive().unwrap() {
            return lease;
        }
        assert!(ring.wait_for_data(deadline).unwrap(), "no frame arrived");
    }
}

fn mapped_region_count(name: &str) -> usize {
    let marker = format!("/memfd:{name}");
    std::fs::read_to_string("/proc/self/maps")
        .expect("read process mappings")
        .lines()
        .filter(|line| line.contains(&marker))
        .count()
}

#[test]
fn production_profile_round_trips_a_maximum_frame_in_both_directions() {
    let profile = host_payload_pool_profile().unwrap();
    let host = shm_transport::backend::ring::DuplexRing::create(&profile).unwrap();
    let peer_first = host.first.attachment().unwrap().attach().unwrap();
    let peer_second = host.second.attachment().unwrap().attach().unwrap();
    let body: Vec<u8> = (0..MAX_FRAME_BYTES)
        .map(|index| (index % 253) as u8)
        .collect();
    // Host to peer.
    publish(&host.first, &body);
    let lease = receive(&peer_first, Instant::now() + Duration::from_secs(5));
    assert_eq!(lease.len(), MAX_FRAME_BYTES);
    assert_eq!(lease.to_vec().unwrap(), body);
    lease.release().unwrap();
    // Peer to host.
    publish(&peer_second, &body);
    let lease = receive(&host.second, Instant::now() + Duration::from_secs(5));
    assert_eq!(lease.to_vec().unwrap(), body);
    lease.release().unwrap();
    for ring in [&host.first, &peer_second] {
        assert!(matches!(
            ring.try_reserve(MAX_FRAME_BYTES + 1, wire_v3_header(1).unwrap()),
            Err(ProducerError::BoundExceedsClass)
        ));
    }
    host.first.probe().unwrap();
    assert!(host.first.inventory().conserves(profile.geometry()));
}

#[test]
fn artifact_mismatch_fails_before_mapping_and_unsealed_objects_are_rejected() {
    let ring = Ring::create(&profile(), 21).unwrap();
    let base = ring.grant().encode();
    let total_offset = PoolGrant::encoded_len() - 12;

    // Layout-identity and geometry mismatches fail in the pure decoder before an object
    // descriptor can reach mapping or attachment.
    let mut version = base;
    version[0..2].copy_from_slice(&1u16.to_le_bytes());
    let mut zero_descriptors = base;
    zero_descriptors[22..26].copy_from_slice(&0u32.to_le_bytes());
    let mut empty_class = base;
    empty_class[30 + 8..30 + 12].copy_from_slice(&0u32.to_le_bytes());
    let mut unaligned_class = base;
    unaligned_class[30..38].copy_from_slice(&4000u64.to_le_bytes());
    let mut total = base;
    total[total_offset..total_offset + 8]
        .copy_from_slice(&(ring.object_size() as u64 + 1).to_le_bytes());
    let mut reserved = base;
    reserved[PoolGrant::encoded_len() - 1] = 1;
    for bytes in [
        version,
        zero_descriptors,
        empty_class,
        unaligned_class,
        total,
        reserved,
    ] {
        assert_eq!(PoolGrant::decode(bytes), Err(RingError::InvalidGrant));
    }

    let mut incarnation = base;
    incarnation[2] ^= 1;
    let mut lane = base;
    lane[18] ^= 1;
    for bytes in [incarnation, lane] {
        let grant = PoolGrant::decode(bytes).unwrap();
        let ring = Ring::create(&profile(), 21).unwrap();
        assert!(matches!(
            Ring::attach(ring.attachment().unwrap().into_parts().0, grant),
            Err(RingError::InvalidGrant)
        ));
    }

    const UNSEALED_NAME: &str = "shm-unsealed-test";
    let name = c"shm-unsealed-test";
    // SAFETY: static name and flags are valid for memfd_create.
    let raw = unsafe {
        libc::syscall(
            libc::SYS_memfd_create,
            name.as_ptr(),
            libc::MFD_CLOEXEC | libc::MFD_ALLOW_SEALING,
        ) as libc::c_int
    };
    assert!(raw >= 0);
    // SAFETY: successful memfd_create returned a new owned descriptor.
    let unsealed = unsafe { OwnedFd::from_raw_fd(raw) };
    // SAFETY: `unsealed` is open for the call; ftruncate takes no pointers.
    let sized = unsafe { libc::ftruncate(unsealed.as_raw_fd(), ring.object_size() as libc::off_t) };
    assert_eq!(sized, 0);
    // SAFETY: same descriptor; fchmod takes no pointers.
    let owner_only = unsafe { libc::fchmod(unsealed.as_raw_fd(), 0o600) };
    assert_eq!(owner_only, 0);
    assert_eq!(mapped_region_count(UNSEALED_NAME), 0);
    let [_, data_ready, capacity_ready] = ring.attachment().unwrap().into_parts().0;
    assert!(matches!(
        Ring::attach([unsealed, data_ready, capacity_ready], ring.grant()),
        Err(RingError::ObjectValidationFailed)
    ));
    assert_eq!(
        mapped_region_count(UNSEALED_NAME),
        0,
        "unsealed object was mapped"
    );
}

#[test]
fn non_regular_attachment_object_is_rejected_before_mapping() {
    let ring = Ring::create(&profile(), 41).unwrap();
    let fd: OwnedFd = std::fs::File::open("/dev/null").unwrap().into();
    let [_, data_ready, capacity_ready] = ring.attachment().unwrap().into_parts().0;
    assert!(matches!(
        Ring::attach([fd, data_ready, capacity_ready], ring.grant()),
        Err(RingError::ObjectValidationFailed)
    ));
}

#[test]
fn ring_memfd_carries_the_registered_name() {
    let _ring = Ring::create(&profile(), 29).unwrap();
    assert!(
        mapped_region_count("shm-transport") >= 1,
        "ring mapping must appear under the registered memfd name"
    );
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().fold(String::new(), |mut text, byte| {
        use std::fmt::Write;
        write!(text, "{byte:02x}").unwrap();
        text
    })
}

fn decode_hex<const N: usize>(text: &str) -> [u8; N] {
    assert_eq!(text.len(), N * 2);
    let mut bytes = [0u8; N];
    for (index, byte) in bytes.iter_mut().enumerate() {
        *byte = u8::from_str_radix(&text[index * 2..index * 2 + 2], 16).unwrap();
    }
    bytes
}

/// Kills and reaps the child if the test unwinds before `wait_with_output`, so a failed
/// assertion cannot leave a process holding the ring mapping and doorbells.
struct ChildGuard(Option<Child>);

impl ChildGuard {
    fn into_inner(mut self) -> Child {
        self.0.take().expect("child is taken once")
    }
}

impl Drop for ChildGuard {
    fn drop(&mut self) {
        if let Some(mut child) = self.0.take() {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

/// Clears `FD_CLOEXEC` so the descriptor survives `exec` into the child.
fn make_inheritable(fd: &OwnedFd) {
    // SAFETY: F_GETFD and F_SETFD act on a live owned descriptor.
    unsafe {
        let flags = libc::fcntl(fd.as_raw_fd(), libc::F_GETFD);
        assert!(flags >= 0);
        assert_eq!(
            libc::fcntl(fd.as_raw_fd(), libc::F_SETFD, flags & !libc::FD_CLOEXEC),
            0
        );
    }
}

/// Passes one attachment to the child through inheritable descriptors and the environment.
/// Returns the parent's descriptor copies, which the caller drops once the child has spawned
/// so the child becomes the sole holder of the peer ends.
fn export_attachment(
    command: &mut Command,
    prefix: &str,
    attachment: RingAttachment,
) -> [OwnedFd; 3] {
    let (descriptors, grant) = attachment.into_parts();
    for descriptor in &descriptors {
        make_inheritable(descriptor);
    }
    let [mapping, data_ready, capacity_ready] = descriptors.each_ref().map(AsRawFd::as_raw_fd);
    command
        .env(format!("EIDNARA_SHM_{prefix}_FD"), mapping.to_string())
        .env(
            format!("EIDNARA_SHM_{prefix}_DATA_READY_FD"),
            data_ready.to_string(),
        )
        .env(
            format!("EIDNARA_SHM_{prefix}_CAPACITY_READY_FD"),
            capacity_ready.to_string(),
        )
        .env(format!("EIDNARA_SHM_{prefix}_GRANT"), hex(&grant.encode()));
    descriptors
}

fn child_command(role: &str) -> Command {
    let mut command = Command::new(std::env::current_exe().unwrap());
    command
        .args(["--ignored", "--exact", role, "--nocapture"])
        .stdout(Stdio::piped());
    command
}

fn child_attach(prefix: &str) -> Ring {
    let fd = std::env::var(format!("EIDNARA_SHM_{prefix}_FD")).unwrap();
    let data_ready = std::env::var(format!("EIDNARA_SHM_{prefix}_DATA_READY_FD")).unwrap();
    let capacity_ready = std::env::var(format!("EIDNARA_SHM_{prefix}_CAPACITY_READY_FD")).unwrap();
    let grant = std::env::var(format!("EIDNARA_SHM_{prefix}_GRANT")).unwrap();
    let grant = PoolGrant::decode(decode_hex(&grant)).unwrap();
    // SAFETY: the parent process opened these descriptors, left them inheritable, and named
    // them in the environment; this child is their only owner.
    let descriptors = unsafe {
        [
            OwnedFd::from_raw_fd(fd.parse().unwrap()),
            OwnedFd::from_raw_fd(data_ready.parse().unwrap()),
            OwnedFd::from_raw_fd(capacity_ready.parse().unwrap()),
        ]
    };
    Ring::attach(descriptors, grant).unwrap()
}

fn poll_readable(fd: &OwnedFd, timeout_ms: libc::c_int) -> bool {
    let mut descriptor = libc::pollfd {
        fd: fd.as_raw_fd(),
        events: libc::POLLIN,
        revents: 0,
    };
    // SAFETY: `descriptor` is one initialized pollfd and the count passed is one.
    let ready = unsafe { libc::poll(&raw mut descriptor, 1, timeout_ms) };
    assert!(
        ready >= 0,
        "poll failed: {}",
        std::io::Error::last_os_error()
    );
    ready > 0
}

fn two_process_skipped() -> bool {
    if std::env::var_os("EIDNARA_SHM_SKIP_TWO_PROCESS").is_some() {
        eprintln!("skipped: EIDNARA_SHM_SKIP_TWO_PROCESS is set");
        return true;
    }
    false
}

const CHILD_DEADLINE: Duration = Duration::from_secs(10);
const REUSE_CYCLES: usize = 2 * 3 + 1;

/// Parent produces on `TO_CHILD` and consumes on `FROM_CHILD`; the child mirrors that. The
/// child holds A across `REUSE_CYCLES` reuses of B, verifies a maximum frame, and reports
/// every observed block id back before waiting for the parent's goodbye, so the parent reads
/// the report while both doorbells are still open.
#[test]
fn two_process_exchange_holds_a_reuses_b_and_wakes_on_return() {
    if two_process_skipped() {
        return;
    }
    let geometry = small_geometry();
    let to_child = Ring::create(&profile(), 0).unwrap();
    let from_child = Ring::create(&profile(), 1).unwrap();
    let mut command = child_command("ring_child_exchange");
    let to_child_fds = export_attachment(&mut command, "TO_CHILD", to_child.attachment().unwrap());
    let from_child_fds =
        export_attachment(&mut command, "FROM_CHILD", from_child.attachment().unwrap());
    let child = ChildGuard(Some(command.spawn().unwrap()));
    // The child owns the peer ends now; closing the parent's copies is what lets the parent
    // observe the child's exit through the doorbells.
    drop(to_child_fds);
    drop(from_child_fds);
    let deadline = Instant::now() + CHILD_DEADLINE;

    let a_bytes: Vec<u8> = (0..1000).map(|index| (index % 7) as u8).collect();
    let a_block = publish(&to_child, &a_bytes);
    let mut b_blocks = Vec::new();
    for cycle in 0..REUSE_CYCLES {
        let b_bytes = vec![cycle as u8 + 1; 900];
        // Three 4 KiB blocks: A plus two B generations fit, so the third cycle parks on the
        // capacity doorbell until the child returns an earlier B from its own process.
        let mut reservation = to_child
            .reserve_until(
                b_bytes.len(),
                wire_v3_header(b_bytes.len()).unwrap(),
                deadline,
            )
            .unwrap();
        reservation.write(&b_bytes).unwrap();
        b_blocks.push(reservation.commit(b_bytes.len()).unwrap().block());
    }
    assert!(b_blocks.iter().all(|block| *block != a_block));
    assert!(
        (1..b_blocks.len()).any(|index| b_blocks[..index].contains(&b_blocks[index])),
        "a B block is reused while A is held; the LIFO free list picks which one: {b_blocks:?}"
    );
    let max: Vec<u8> = (0..MAX_FRAME_BYTES)
        .map(|index| (index % 251) as u8)
        .collect();
    let mut reservation = to_child
        .reserve_until(
            MAX_FRAME_BYTES,
            wire_v3_header(MAX_FRAME_BYTES).unwrap(),
            deadline,
        )
        .unwrap();
    reservation.write(&max).unwrap();
    reservation.commit(MAX_FRAME_BYTES).unwrap();

    let report = receive(&from_child, deadline);
    let text = String::from_utf8(report.to_vec().unwrap()).unwrap();
    report.release().unwrap();
    let mut fields = text.split(',');
    assert_eq!(fields.next(), Some("EIDNARA_SHM_CHILD_EXCHANGE_OK"));
    assert_eq!(fields.next(), Some(a_block.to_string().as_str()));
    let child_b: Vec<u32> = fields
        .next()
        .unwrap()
        .split(' ')
        .map(|id| id.parse().unwrap())
        .collect();
    assert_eq!(child_b, b_blocks, "the child saw every B block in order");
    // A is still held by the child, so its block is outstanding; every B has returned.
    let inventory = to_child.inventory();
    assert!(inventory.conserves(&geometry));
    assert_eq!(
        inventory.classes[0].published, 1,
        "only A remains outstanding"
    );
    assert_eq!(
        inventory.classes[4].published, 0,
        "the maximum frame returned"
    );
    publish(&to_child, b"bye");

    let output = child.into_inner().wait_with_output().unwrap();
    assert!(output.status.success());
    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(stdout.contains("EIDNARA_SHM_CHILD_DONE"), "{stdout}");
    // A returned on the child's exit path; the block is reusable here.
    to_child.probe().unwrap();
    assert!(
        to_child
            .inventory()
            .classes
            .iter()
            .all(|class| class.published == 0)
    );
}

#[test]
#[ignore = "child role for two_process_exchange_holds_a_reuses_b_and_wakes_on_return"]
fn ring_child_exchange() {
    if std::env::var_os("EIDNARA_SHM_TO_CHILD_FD").is_none() {
        return;
    }
    let from_parent = child_attach("TO_CHILD");
    let to_parent = child_attach("FROM_CHILD");
    let deadline = Instant::now() + CHILD_DEADLINE;
    let a = receive(&from_parent, deadline);
    let a_bytes = a.to_vec().unwrap();
    let a_block = a.identity().block();
    let mut b_blocks = Vec::new();
    for cycle in 0..REUSE_CYCLES {
        let b = receive(&from_parent, deadline);
        assert_eq!(b.to_vec().unwrap(), vec![cycle as u8 + 1; 900]);
        b_blocks.push(b.identity().block());
        // Returned from another thread: the owned lease crosses without the ring.
        std::thread::spawn(move || drop(b)).join().unwrap();
        assert_eq!(a.to_vec().unwrap(), a_bytes, "A changed under B's reuse");
    }
    let max = receive(&from_parent, deadline);
    assert_eq!(max.len(), MAX_FRAME_BYTES);
    let body = max.body().unwrap();
    assert_eq!(body.read_byte(0), Some(0));
    assert_eq!(
        body.read_byte(MAX_FRAME_BYTES - 1),
        Some(((MAX_FRAME_BYTES - 1) % 251) as u8)
    );
    assert_eq!(max.to_vec().unwrap().len(), MAX_FRAME_BYTES);
    max.release().unwrap();
    let ids: Vec<String> = b_blocks.iter().map(u32::to_string).collect();
    let report = format!("EIDNARA_SHM_CHILD_EXCHANGE_OK,{a_block},{}", ids.join(" "));
    publish(&to_parent, report.as_bytes());
    let bye = receive(&from_parent, deadline);
    assert_eq!(bye.to_vec().unwrap(), b"bye");
    bye.release().unwrap();
    drop(a);
    println!("EIDNARA_SHM_CHILD_DONE");
}

/// One ordinary descriptor: the parent's second reservation parks solely on descriptor
/// exhaustion and is woken by the child's consumption while the child still holds the payload.
/// The child consumes only after the parent writes `b"go"` to its stdin.
#[test]
fn two_process_descriptor_consumption_wakes_a_parked_producer_without_a_return() {
    if two_process_skipped() {
        return;
    }
    let geometry = PoolGeometry::new(
        1,
        1,
        [
            ClassSpec::new(4096, 4),
            ClassSpec::new(64 * 1024, 1),
            ClassSpec::new(1024 * 1024, 1),
            ClassSpec::new(8 * 1024 * 1024, 1),
            ClassSpec::new(64 * 1024 * 1024 + 4096, 1),
        ],
        ClassSpec::new(4096, 1),
        ClassSpec::new(32 * 1024, 1),
    )
    .unwrap();
    let profile = pool_profile(
        HardwareProfileId::new("ring-descriptor-wake").unwrap(),
        geometry,
    )
    .unwrap();
    let to_child = Ring::create(&profile, 0).unwrap();
    let from_child = Ring::create(&profile, 1).unwrap();
    let mut command = child_command("ring_child_hold");
    command.stdin(Stdio::piped());
    let to_child_fds = export_attachment(&mut command, "TO_CHILD", to_child.attachment().unwrap());
    let from_child_fds =
        export_attachment(&mut command, "FROM_CHILD", from_child.attachment().unwrap());
    let mut child = ChildGuard(Some(command.spawn().unwrap()));
    let mut go = child.0.as_mut().unwrap().stdin.take().unwrap();
    drop(to_child_fds);
    drop(from_child_fds);
    let deadline = Instant::now() + CHILD_DEADLINE;

    publish(&to_child, b"held");
    assert!(matches!(
        to_child.try_reserve(4, wire_v3_header(4).unwrap()),
        Err(ProducerError::Exhausted)
    ));
    assert_eq!(
        to_child.arm_capacity_wait(Inventory::Ordinary, 4),
        Ok(true),
        "the producer parks on the one ordinary descriptor"
    );
    let ready = to_child.duplicate_capacity_ready().unwrap();
    assert!(
        !poll_readable(&ready, 0),
        "no token before the child consumes"
    );
    go.write_all(b"go").unwrap();
    let remaining = deadline.saturating_duration_since(Instant::now());
    assert!(
        poll_readable(&ready, remaining.as_millis().try_into().unwrap()),
        "the child's consumption rang the capacity doorbell"
    );
    assert_eq!(
        to_child.inventory().classes[0].published,
        1,
        "the held payload is still outstanding at the wake"
    );
    to_child.complete_capacity_wait().unwrap();
    to_child
        .try_reserve(4, wire_v3_header(4).unwrap())
        .unwrap()
        .abort();
    publish(&to_child, b"release");
    let report = receive(&from_child, deadline);
    assert_eq!(report.to_vec().unwrap(), b"EIDNARA_SHM_CHILD_RELEASED");
    report.release().unwrap();
    publish(&to_child, b"bye");
    let output = child.into_inner().wait_with_output().unwrap();
    assert!(output.status.success());
    to_child.probe().unwrap();
    assert_eq!(to_child.inventory().classes[0].published, 0);
}

#[test]
#[ignore = "child role for two_process_descriptor_consumption_wakes_a_parked_producer_without_a_return"]
fn ring_child_hold() {
    if std::env::var_os("EIDNARA_SHM_TO_CHILD_FD").is_none() {
        return;
    }
    let from_parent = child_attach("TO_CHILD");
    let to_parent = child_attach("FROM_CHILD");
    let deadline = Instant::now() + CHILD_DEADLINE;
    let mut go = [0u8; 2];
    std::io::stdin().read_exact(&mut go).unwrap();
    assert_eq!(&go, b"go");
    let held = receive(&from_parent, deadline);
    assert_eq!(held.to_vec().unwrap(), b"held");
    let release = receive(&from_parent, deadline);
    assert_eq!(release.to_vec().unwrap(), b"release");
    release.release().unwrap();
    held.release().unwrap();
    publish(&to_parent, b"EIDNARA_SHM_CHILD_RELEASED");
    let bye = receive(&from_parent, deadline);
    assert_eq!(bye.to_vec().unwrap(), b"bye");
    bye.release().unwrap();
}
