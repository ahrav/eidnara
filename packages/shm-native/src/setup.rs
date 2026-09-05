use std::collections::BTreeSet;
use std::io::{self, IoSliceMut, Read, Write};
use std::os::fd::{AsFd, AsRawFd, BorrowedFd, OwnedFd};
use std::os::unix::net::UnixStream;
use std::path::Path;
use std::time::{Duration, Instant};

use rustix::net::{
    AddressFamily, RecvAncillaryBuffer, RecvAncillaryMessage, RecvFlags, ReturnFlags,
    SocketAddrUnix, SocketFlags, SocketType, recvmsg, sockopt,
};
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use shm_transport::backend::ring::RingGrant;
use shm_transport::descriptor::SETUP_DESCRIPTOR_COUNT;
use subtle::ConstantTimeEq;

use shm_transport::setup_auth::{
    self, CLIENT_AUTH_DOMAIN, DAEMON_ID_LEN, DEFAULT_CLIENT_ROLE, MAX_AUTH_MESSAGE_LEN,
    MAX_SETUP_MESSAGE_LEN, NONCE_LEN, PROOF_LEN, PROTOCOL_VERSION, SERVER_PROOF_DOMAIN,
};

#[derive(Serialize)]
struct ClientHello {
    client_nonce: [u8; NONCE_LEN],
    role: &'static str,
}

#[derive(Deserialize)]
struct ServerProof {
    daemon_id: [u8; DAEMON_ID_LEN],
    server_nonce: [u8; NONCE_LEN],
    daemon_ver: String,
    server_proof: [u8; PROOF_LEN],
}

#[derive(Serialize)]
struct ClientAuth {
    client_auth: [u8; PROOF_LEN],
}

#[derive(Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
enum GrantMessage {
    Grant {
        wire_version: u8,
        descriptor_schema: u16,
        activation_token: String,
        descriptor: Descriptor,
    },
}

struct Grant {
    wire_version: u8,
    descriptor_schema: u16,
    activation_token: String,
    descriptor: Descriptor,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Descriptor {
    profile: String,
    host_to_peer_grant: String,
    peer_to_host_grant: String,
}

#[derive(Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum ClientMessage<'a> {
    Activate {
        wire_version: u8,
        descriptor_schema: u16,
        activation_token: &'a str,
    },
    Commit,
    Goodbye,
}

#[derive(Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
enum ServerMessage {
    Activated,
    Committed,
}

pub struct PendingSetup {
    stream: UnixStream,
    descriptors: Option<[OwnedFd; SETUP_DESCRIPTOR_COUNT]>,
    pub host_to_peer_grant: RingGrant,
    pub peer_to_host_grant: RingGrant,
    wire_version: u8,
    descriptor_schema: u16,
    activation_token: String,
    deadline: Instant,
}

pub fn begin_connect(
    path: &Path,
    key: &[u8],
    expected_daemon_id: &[u8],
    expected_daemon_ver: &str,
    timeout: Duration,
) -> io::Result<PendingSetup> {
    if key.len() != 32 || expected_daemon_id.len() != DAEMON_ID_LEN || timeout.is_zero() {
        return Err(invalid());
    }
    let deadline = Instant::now().checked_add(timeout).ok_or_else(timed_out)?;
    let mut stream = connect_until(path, deadline)?;
    authenticate(
        &mut stream,
        key,
        expected_daemon_id,
        expected_daemon_ver,
        deadline,
    )?;
    let (grant, descriptors) = receive_grant(&mut stream, deadline)?;
    if grant.wire_version != PROTOCOL_VERSION
        || grant.descriptor_schema != shm_transport::descriptor::DESCRIPTOR_SCHEMA_VERSION
    {
        return Err(invalid());
    }
    let host_to_peer_grant = decode_grant(&grant.descriptor.host_to_peer_grant)?;
    let peer_to_host_grant = decode_grant(&grant.descriptor.peer_to_host_grant)?;
    if grant.descriptor.profile != super::PROFILE
        || host_to_peer_grant == peer_to_host_grant
        || !super::grant_matches_profile(host_to_peer_grant)
        || !super::grant_matches_profile(peer_to_host_grant)
    {
        return Err(invalid());
    }
    Ok(PendingSetup {
        stream,
        descriptors: Some(descriptors),
        host_to_peer_grant,
        peer_to_host_grant,
        wire_version: grant.wire_version,
        descriptor_schema: grant.descriptor_schema,
        activation_token: grant.activation_token,
        deadline,
    })
}

/// `UnixStream::connect` can block indefinitely when the listener's backlog is full: Linux
/// parks a blocking `AF_UNIX` connect for the socket's `SO_SNDTIMEO`, which std never sets.
/// Setting that timeout to the remaining budget before `connect(2)` makes the kernel enforce
/// the deadline.
fn connect_until(path: &Path, deadline: Instant) -> io::Result<UnixStream> {
    let remaining = deadline.saturating_duration_since(Instant::now());
    if remaining.is_zero() {
        return Err(timed_out());
    }
    // The kernel rejects `tv_usec >= 1_000_000` and rustix rounds sub-microsecond nanos
    // up, so a budget just under a whole second would fail with `EDOM` unless floored.
    let remaining = Duration::from_micros(u64::try_from(remaining.as_micros()).unwrap_or(u64::MAX))
        .max(Duration::from_micros(1));
    let socket = rustix::net::socket_with(
        AddressFamily::UNIX,
        SocketType::STREAM,
        SocketFlags::CLOEXEC,
        None,
    )?;
    sockopt::set_socket_timeout(&socket, sockopt::Timeout::Send, Some(remaining))?;
    let address = SocketAddrUnix::new(path)?;
    loop {
        match rustix::net::connect(&socket, &address) {
            Ok(()) => return Ok(UnixStream::from(socket)),
            Err(rustix::io::Errno::INTR) => {}
            Err(rustix::io::Errno::AGAIN) => return Err(timed_out()),
            Err(errno) => return Err(errno.into()),
        }
    }
}

/// A ring alone cannot express peer death: a host that exits without a Goodbye frame leaves its rings looking merely idle, so the setup socket is the only liveness signal. `MSG_PEEK` keeps the probe side-effect free and repeatable.
pub fn peer_closed(stream: &UnixStream) -> bool {
    let mut probe = [0u8; 1];
    match rustix::net::recv(
        stream.as_fd(),
        &mut probe,
        rustix::net::RecvFlags::PEEK | rustix::net::RecvFlags::DONTWAIT,
    ) {
        Ok(_) => true,
        Err(rustix::io::Errno::AGAIN) | Err(rustix::io::Errno::INTR) => false,
        Err(_) => true,
    }
}

impl PendingSetup {
    pub fn take_descriptors(&mut self) -> io::Result<[OwnedFd; SETUP_DESCRIPTOR_COUNT]> {
        self.descriptors.take().ok_or_else(invalid)
    }

    pub fn activate(mut self) -> io::Result<UnixStream> {
        write_message(
            &mut self.stream,
            &ClientMessage::Activate {
                wire_version: self.wire_version,
                descriptor_schema: self.descriptor_schema,
                activation_token: &self.activation_token,
            },
            self.deadline,
            MAX_SETUP_MESSAGE_LEN,
        )?;
        if !matches!(
            read_message::<ServerMessage>(&mut self.stream, self.deadline, MAX_SETUP_MESSAGE_LEN)?,
            ServerMessage::Activated
        ) {
            return Err(invalid());
        }
        write_message(
            &mut self.stream,
            &ClientMessage::Commit,
            self.deadline,
            MAX_SETUP_MESSAGE_LEN,
        )?;
        if !matches!(
            read_message::<ServerMessage>(&mut self.stream, self.deadline, MAX_SETUP_MESSAGE_LEN)?,
            ServerMessage::Committed
        ) {
            return Err(invalid());
        }
        Ok(self.stream)
    }
}

pub fn goodbye(stream: &mut UnixStream) {
    let deadline = Instant::now() + Duration::from_millis(100);
    let _ = write_message(
        stream,
        &ClientMessage::Goodbye,
        deadline,
        MAX_SETUP_MESSAGE_LEN,
    );
    let _ = stream.shutdown(std::net::Shutdown::Both);
}

fn authenticate(
    stream: &mut UnixStream,
    key: &[u8],
    expected_daemon_id: &[u8],
    expected_daemon_ver: &str,
    deadline: Instant,
) -> io::Result<()> {
    let mut client_nonce = [0u8; NONCE_LEN];
    getrandom::getrandom(&mut client_nonce).map_err(|_| invalid())?;
    write_message(
        stream,
        &ClientHello {
            client_nonce,
            role: DEFAULT_CLIENT_ROLE,
        },
        deadline,
        MAX_AUTH_MESSAGE_LEN,
    )?;
    let server: ServerProof = read_message(stream, deadline, MAX_AUTH_MESSAGE_LEN)?;
    let expected = proof(
        key,
        SERVER_PROOF_DOMAIN,
        &client_nonce,
        &server.server_nonce,
        &server.daemon_ver,
        &server.daemon_id,
    );
    if !bool::from(expected.ct_eq(&server.server_proof))
        || !bool::from(server.daemon_id.as_slice().ct_eq(expected_daemon_id))
        || server.daemon_ver != expected_daemon_ver
    {
        return Err(identity_mismatch());
    }
    write_message(
        stream,
        &ClientAuth {
            client_auth: proof(
                key,
                CLIENT_AUTH_DOMAIN,
                &client_nonce,
                &server.server_nonce,
                &server.daemon_ver,
                &server.daemon_id,
            ),
        },
        deadline,
        MAX_AUTH_MESSAGE_LEN,
    )
}

fn proof(
    key: &[u8],
    domain: &str,
    client_nonce: &[u8; NONCE_LEN],
    server_nonce: &[u8; NONCE_LEN],
    daemon_ver: &str,
    daemon_id: &[u8; DAEMON_ID_LEN],
) -> [u8; PROOF_LEN] {
    setup_auth::compute_proof(
        key,
        domain,
        client_nonce,
        server_nonce,
        daemon_ver,
        daemon_id,
    )
}

fn receive_grant(
    stream: &mut UnixStream,
    deadline: Instant,
) -> io::Result<(Grant, [OwnedFd; SETUP_DESCRIPTOR_COUNT])> {
    set_timeout(stream, deadline)?;
    let mut bytes = vec![0u8; MAX_SETUP_MESSAGE_LEN + 4];
    let mut control = [std::mem::MaybeUninit::uninit();
        rustix::cmsg_space!(ScmRights(SETUP_DESCRIPTOR_COUNT + 1))];
    let mut ancillary = RecvAncillaryBuffer::new(&mut control);
    let mut iov = [IoSliceMut::new(&mut bytes)];
    let received = recvmsg(
        stream.as_fd(),
        &mut iov,
        &mut ancillary,
        RecvFlags::CMSG_CLOEXEC,
    )?;
    if received.bytes == 0 || received.flags.contains(ReturnFlags::CTRUNC) {
        return Err(invalid());
    }
    bytes.truncate(received.bytes);
    let mut descriptors = Vec::new();
    for message in ancillary.drain() {
        match message {
            RecvAncillaryMessage::ScmRights(rights) => descriptors.extend(rights),
            _ => return Err(invalid()),
        }
    }
    if descriptors.len() != SETUP_DESCRIPTOR_COUNT {
        return Err(invalid());
    }
    reject_aliased_descriptors(&descriptors, stream)?;
    let descriptors = descriptors.try_into().map_err(|_| invalid())?;
    let message: GrantMessage =
        read_message_from_prefix(stream, bytes, deadline, MAX_SETUP_MESSAGE_LEN)?;
    let GrantMessage::Grant {
        wire_version,
        descriptor_schema,
        activation_token,
        descriptor,
    } = message;
    let grant = Grant {
        wire_version,
        descriptor_schema,
        activation_token,
        descriptor,
    };
    Ok((grant, descriptors))
}

/// `KCMP_FILE` selects open-file-description comparison for `kcmp(2)` (`linux/kcmp.h`).
const KCMP_FILE: libc::c_int = 0;

/// `SCM_RIGHTS` installs a fresh fd per slot even when two slots name one open file.
/// Duplicates are therefore detected on the open file description, never the fd number.
/// The setup socket is included: a slot must not alias the stream being read.
fn reject_aliased_descriptors(descriptors: &[OwnedFd], stream: &UnixStream) -> io::Result<()> {
    let files: Vec<BorrowedFd<'_>> = descriptors
        .iter()
        .map(OwnedFd::as_fd)
        .chain([stream.as_fd()])
        .collect();
    reject_aliased_files(&files)
}

/// Rejects two slots that name one open file description. `files` holds the six ring
/// descriptors in wire order, optionally followed by the setup socket.
///
/// `kcmp(2)` catches duplicates of any descriptor type. When `kcmp(2)` is unavailable,
/// compare `(st_dev, st_ino)` instead: unique for memfd mappings and `socketpair` doorbells,
/// but shared by every anonymous-inode descriptor such as an eventfd.
pub(crate) fn reject_aliased_files(files: &[BorrowedFd<'_>]) -> io::Result<()> {
    for (index, first) in files.iter().enumerate() {
        for second in &files[index + 1..] {
            match same_open_file(*first, *second)? {
                Some(true) => return Err(invalid()),
                Some(false) => {}
                None => return reject_aliased_inodes(files),
            }
        }
    }
    Ok(())
}

fn reject_aliased_inodes(files: &[BorrowedFd<'_>]) -> io::Result<()> {
    let mut identities = BTreeSet::new();
    for file in files {
        let stat = rustix::fs::fstat(file)?;
        if !identities.insert((stat.st_dev, stat.st_ino)) {
            return Err(invalid());
        }
    }
    Ok(())
}

/// `Some(true)` when both fds name one open file description. `None` when the kernel or the
/// sandbox refuses `kcmp(2)`, leaving identity undecidable this way.
fn same_open_file(first: BorrowedFd<'_>, second: BorrowedFd<'_>) -> io::Result<Option<bool>> {
    let pid = std::process::id();
    // SAFETY: kcmp reads only its integer arguments; both fds are open for this call.
    let ordering = unsafe {
        libc::syscall(
            libc::SYS_kcmp,
            pid,
            pid,
            KCMP_FILE,
            first.as_raw_fd(),
            second.as_raw_fd(),
        )
    };
    // kcmp(2) returns 0 for one open file description and 1, 2, or 3 for distinct ones (the
    // latter when no ordering is available); only -1 carries an errno.
    match ordering {
        0 => Ok(Some(true)),
        1..=3 => Ok(Some(false)),
        _ => {
            let error = io::Error::last_os_error();
            match error.raw_os_error() {
                Some(libc::ENOSYS) | Some(libc::EPERM) => Ok(None),
                _ => Err(error),
            }
        }
    }
}

fn read_message_from_prefix<T: DeserializeOwned>(
    stream: &mut UnixStream,
    mut prefix: Vec<u8>,
    deadline: Instant,
    max: usize,
) -> io::Result<T> {
    while prefix.len() < 4 {
        let mut byte = [0u8; 1];
        read_exact(stream, &mut byte, deadline)?;
        prefix.push(byte[0]);
    }
    let len = u32::from_le_bytes(prefix[..4].try_into().expect("four-byte prefix")) as usize;
    if len > max {
        return Err(invalid());
    }
    let total = 4usize.checked_add(len).ok_or_else(invalid)?;
    if prefix.len() > total {
        return Err(invalid());
    }
    let received = prefix.len();
    prefix.resize(total, 0);
    read_exact(stream, &mut prefix[received..], deadline)?;
    serde_json::from_slice(&prefix[4..]).map_err(|_| invalid())
}

fn read_message<T: DeserializeOwned>(
    stream: &mut UnixStream,
    deadline: Instant,
    max: usize,
) -> io::Result<T> {
    let mut len = [0u8; 4];
    read_exact(stream, &mut len, deadline)?;
    let len = u32::from_le_bytes(len) as usize;
    if len > max {
        return Err(invalid());
    }
    let mut body = vec![0u8; len];
    read_exact(stream, &mut body, deadline)?;
    serde_json::from_slice(&body).map_err(|_| invalid())
}

fn write_message<T: Serialize>(
    stream: &mut UnixStream,
    value: &T,
    deadline: Instant,
    max: usize,
) -> io::Result<()> {
    let body = serde_json::to_vec(value).map_err(|_| invalid())?;
    if body.len() > max {
        return Err(invalid());
    }
    let mut frame = Vec::with_capacity(4 + body.len());
    frame.extend_from_slice(&(body.len() as u32).to_le_bytes());
    frame.extend_from_slice(&body);
    write_all(stream, &frame, deadline)
}

fn write_all(stream: &mut UnixStream, mut bytes: &[u8], deadline: Instant) -> io::Result<()> {
    // `std::io::Write::write_all` grants each underlying send the full remaining budget, so a
    // slowly draining peer could stretch wall time to remaining × chunks. Re-arming the timeout
    // per chunk caps the whole write at the deadline, as `read_exact` does.
    while !bytes.is_empty() {
        set_timeout(stream, deadline)?;
        match stream.write(bytes) {
            Ok(0) => return Err(io::ErrorKind::WriteZero.into()),
            Ok(written) => bytes = &bytes[written..],
            Err(error)
                if matches!(
                    error.kind(),
                    io::ErrorKind::Interrupted
                        | io::ErrorKind::WouldBlock
                        | io::ErrorKind::TimedOut
                ) => {}
            Err(error) => return Err(error),
        }
    }
    Ok(())
}

fn read_exact(stream: &mut UnixStream, mut bytes: &mut [u8], deadline: Instant) -> io::Result<()> {
    // `std::io::Read::read_exact` grants each underlying recv the full
    // remaining budget, so a trickling peer could stretch wall time to
    // remaining × len. Re-arming the timeout per chunk caps the whole read
    // at the deadline.
    while !bytes.is_empty() {
        set_timeout(stream, deadline)?;
        match stream.read(bytes) {
            Ok(0) => return Err(io::ErrorKind::UnexpectedEof.into()),
            Ok(read) => bytes = &mut bytes[read..],
            Err(error)
                if matches!(
                    error.kind(),
                    io::ErrorKind::Interrupted
                        | io::ErrorKind::WouldBlock
                        | io::ErrorKind::TimedOut
                ) => {}
            Err(error) => return Err(error),
        }
    }
    Ok(())
}

fn set_timeout(stream: &UnixStream, deadline: Instant) -> io::Result<()> {
    let remaining = deadline.saturating_duration_since(Instant::now());
    if remaining.is_zero() {
        return Err(timed_out());
    }
    stream.set_read_timeout(Some(remaining))?;
    stream.set_write_timeout(Some(remaining))
}

fn decode_grant(text: &str) -> io::Result<RingGrant> {
    let bytes = super::strict_hex(text).ok_or_else(invalid)?;
    RingGrant::decode(bytes).map_err(|_| invalid())
}

fn invalid() -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, "shared-memory setup failed")
}

/// Payload that marks a failed authentication proof. The kind alone cannot: `connect(2)` on a
/// socket the process may not open also reports `PermissionDenied`.
#[derive(Debug)]
pub(crate) struct IdentityMismatch;

impl std::fmt::Display for IdentityMismatch {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("shared-memory identity mismatch")
    }
}

impl std::error::Error for IdentityMismatch {}

fn identity_mismatch() -> io::Error {
    io::Error::new(io::ErrorKind::PermissionDenied, IdentityMismatch)
}

pub(crate) fn is_identity_mismatch(failure: &io::Error) -> bool {
    failure
        .get_ref()
        .is_some_and(|inner| inner.is::<IdentityMismatch>())
}

fn timed_out() -> io::Error {
    io::Error::new(
        io::ErrorKind::TimedOut,
        "shared-memory setup deadline expired",
    )
}

#[cfg(test)]
mod tests {
    use super::{CLIENT_AUTH_DOMAIN, GrantMessage, SERVER_PROOF_DOMAIN, proof};
    use shm_transport::setup_auth::vectors;

    #[test]
    fn grant_message_accepts_tagged_setup_envelope() {
        let message: GrantMessage = serde_json::from_value(serde_json::json!({
            "type": "grant",
            "wire_version": 2,
            "descriptor_schema": shm_transport::descriptor::DESCRIPTOR_SCHEMA_VERSION,
            "activation_token": "token",
            "descriptor": {
                "profile": "host-test-ring-v1",
                "host_to_peer_grant": "aa",
                "peer_to_host_grant": "bb"
            }
        }))
        .expect("tagged grant envelope decodes");

        let GrantMessage::Grant { wire_version, .. } = message;
        assert_eq!(wire_version, 2);
    }

    #[test]
    fn auth_proofs_match_committed_wire_vectors() {
        let (key, client_nonce, server_nonce, daemon_id) = vectors::inputs();
        assert_eq!(
            proof(
                &key,
                SERVER_PROOF_DOMAIN,
                &client_nonce,
                &server_nonce,
                vectors::DAEMON_VER,
                &daemon_id,
            ),
            vectors::SERVER_PROOF,
        );
        assert_eq!(
            proof(
                &key,
                CLIENT_AUTH_DOMAIN,
                &client_nonce,
                &server_nonce,
                vectors::DAEMON_VER,
                &daemon_id,
            ),
            vectors::CLIENT_AUTH,
        );
    }

    /// Builds a bundle shaped like the transport's: memfd mappings and `socketpair` doorbells.
    fn bundle() -> Vec<std::os::fd::OwnedFd> {
        use std::os::unix::net::UnixStream;

        let memfd = |name: &str| {
            rustix::fs::memfd_create(name, rustix::fs::MemfdFlags::CLOEXEC).expect("memfd")
        };
        let doorbell = || UnixStream::pair().expect("socketpair").0.into();
        vec![
            memfd("host"),
            doorbell(),
            doorbell(),
            memfd("peer"),
            doorbell(),
            doorbell(),
        ]
    }

    #[test]
    fn distinct_descriptors_are_accepted_and_a_dup_is_rejected() {
        use std::os::fd::AsFd;
        use std::os::unix::net::UnixStream;

        let (stream, _peer) = UnixStream::pair().expect("socket pair");
        let bundle = bundle();
        super::reject_aliased_descriptors(&bundle, &stream).expect("distinct bundle");

        let mut aliased_doorbell = bundle;
        aliased_doorbell[4] = aliased_doorbell[1].try_clone().expect("dup");
        assert!(super::reject_aliased_descriptors(&aliased_doorbell, &stream).is_err());

        let mut aliased_mapping = aliased_doorbell;
        aliased_mapping[4] = UnixStream::pair().expect("socketpair").0.into();
        aliased_mapping[3] = aliased_mapping[0].try_clone().expect("dup");
        assert!(super::reject_aliased_descriptors(&aliased_mapping, &stream).is_err());

        let mut aliased_socket = aliased_mapping;
        aliased_socket[3] = stream.as_fd().try_clone_to_owned().expect("dup");
        assert!(super::reject_aliased_descriptors(&aliased_socket, &stream).is_err());
    }

    #[test]
    fn inode_fallback_rejects_the_same_aliases() {
        use std::os::fd::{AsFd, BorrowedFd};
        use std::os::unix::net::UnixStream;

        let (stream, _peer) = UnixStream::pair().expect("socket pair");
        let bundle = bundle();
        fn files<'a>(
            bundle: &'a [std::os::fd::OwnedFd],
            stream: &'a UnixStream,
        ) -> Vec<BorrowedFd<'a>> {
            bundle
                .iter()
                .map(AsFd::as_fd)
                .chain([stream.as_fd()])
                .collect()
        }
        super::reject_aliased_inodes(&files(&bundle, &stream)).expect("distinct bundle");

        let mut aliased_doorbell = bundle;
        aliased_doorbell[4] = aliased_doorbell[1].try_clone().expect("dup");
        assert!(super::reject_aliased_inodes(&files(&aliased_doorbell, &stream)).is_err());

        let mut aliased_mapping = aliased_doorbell;
        aliased_mapping[4] = UnixStream::pair().expect("socketpair").0.into();
        aliased_mapping[3] = aliased_mapping[0].try_clone().expect("dup");
        assert!(super::reject_aliased_inodes(&files(&aliased_mapping, &stream)).is_err());
    }

    #[test]
    fn kcmp_separates_eventfds_that_share_an_inode() {
        use std::os::fd::AsFd;

        use rustix::event::{EventfdFlags, eventfd};

        let first = eventfd(0, EventfdFlags::CLOEXEC).expect("eventfd");
        let second = eventfd(0, EventfdFlags::CLOEXEC).expect("eventfd");
        let inode = |fd: &std::os::fd::OwnedFd| {
            let stat = rustix::fs::fstat(fd).expect("fstat");
            (stat.st_dev, stat.st_ino)
        };
        assert_eq!(
            inode(&first),
            inode(&second),
            "anonymous inodes are expected to collide"
        );
        if let Some(same) = super::same_open_file(first.as_fd(), second.as_fd()).expect("kcmp") {
            assert!(!same, "kcmp must separate distinct eventfds");
        }
        let dup = first.try_clone().expect("dup");
        if let Some(same) = super::same_open_file(first.as_fd(), dup.as_fd()).expect("kcmp") {
            assert!(same, "kcmp must identify a dup");
        }
    }

    #[test]
    fn identity_mismatch_is_distinguished_from_socket_permission_errors() {
        let mismatch = super::identity_mismatch();
        assert_eq!(mismatch.kind(), std::io::ErrorKind::PermissionDenied);
        assert!(super::is_identity_mismatch(&mismatch));
        assert_eq!(mismatch.to_string(), "shared-memory identity mismatch");

        let denied_socket = std::io::Error::from_raw_os_error(libc::EACCES);
        assert_eq!(denied_socket.kind(), std::io::ErrorKind::PermissionDenied);
        assert!(!super::is_identity_mismatch(&denied_socket));
    }

    #[test]
    fn peer_closed_reports_live_then_dropped_sentinel() {
        let (client, host) = std::os::unix::net::UnixStream::pair().expect("socket pair");
        assert!(
            !super::peer_closed(&client),
            "a held setup socket is not closed"
        );
        drop(host);
        assert!(
            super::peer_closed(&client),
            "dropping the host end must surface as closed"
        );
    }

    #[test]
    fn connect_honors_the_deadline_when_the_backlog_is_full() {
        use std::os::unix::net::UnixListener;
        use std::time::{Duration, Instant};

        use rustix::net::{AddressFamily, SocketAddrUnix, SocketFlags, SocketType};

        let dir = std::env::temp_dir().join(format!("shm-native-connect-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("scratch dir");
        let path = dir.join("setup.sock");
        // A listener that never accepts; fill its backlog with non-blocking connects,
        // which return `EAGAIN` once the queue is full instead of parking.
        let listener = UnixListener::bind(&path).expect("listener");
        let address = SocketAddrUnix::new(&path).expect("address");
        let mut pending = Vec::new();
        loop {
            let socket = rustix::net::socket_with(
                AddressFamily::UNIX,
                SocketType::STREAM,
                SocketFlags::CLOEXEC | SocketFlags::NONBLOCK,
                None,
            )
            .expect("probe socket");
            match rustix::net::connect(&socket, &address) {
                Ok(()) => pending.push(socket),
                Err(rustix::io::Errno::AGAIN) => break,
                Err(errno) => panic!("unexpected connect error: {errno}"),
            }
            assert!(pending.len() <= 65_536, "backlog never filled");
        }

        let started = Instant::now();
        let result = super::connect_until(&path, started + Duration::from_millis(200));
        let elapsed = started.elapsed();
        drop(pending);
        drop(listener);
        let _ = std::fs::remove_dir_all(&dir);

        let error = result.expect_err("full backlog must not connect");
        assert_eq!(error.kind(), std::io::ErrorKind::TimedOut);
        assert!(
            elapsed < Duration::from_secs(5),
            "connect blocked past the deadline: {elapsed:?}"
        );
    }

    #[test]
    fn connect_succeeds_against_an_accepting_listener() {
        use std::os::unix::net::UnixListener;
        use std::time::{Duration, Instant};

        let dir = std::env::temp_dir().join(format!("shm-native-accept-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("scratch dir");
        let path = dir.join("setup.sock");
        let listener = UnixListener::bind(&path).expect("listener");
        let stream =
            super::connect_until(&path, Instant::now() + Duration::from_secs(1)).expect("connect");
        let (accepted, _) = listener.accept().expect("accept");
        drop((stream, accepted, listener));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
