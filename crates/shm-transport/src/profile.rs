#[cfg(target_os = "linux")]
use std::collections::HashSet;
use std::fmt;
use std::sync::{Arc, Mutex};

use crate::arena::MIN_ARENA_BYTES;
use crate::descriptor::{
    HardwareProfileId, MAX_SPANS, SETUP_DOORBELL_COUNT, SETUP_MAPPING_COUNT, TransportDescriptor,
};

/// Which thread publishes and receives on a ring. Decides the `workers` charge: zero when
/// the caller drives both directions, one per dedicated worker otherwise.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WorkerTopology {
    /// The calling thread publishes and receives; no worker is charged.
    CallerThread,
    /// One worker per direction; two workers charged.
    SplitDirection,
    /// One worker drives both directions; one worker charged.
    Fused,
}

/// What one admitted connection costs the host. Every field but `spans_per_frame` is a sum
/// across admissions; `spans_per_frame` is a maximum, so `AdmissionController` tracks it
/// separately instead of subtracting on release.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ResourceCharges {
    /// Descriptor slots, both directions.
    pub descriptors: u64,
    /// Arena bytes, both directions.
    pub arena_bytes: u64,
    /// Largest span count any admitted frame may carry.
    pub spans_per_frame: u64,
    /// Outstanding receive leases, both directions.
    pub leases: u64,
    /// Shared-memory mappings.
    pub mappings: u64,
    /// File descriptors kept open for the mappings and doorbells.
    pub file_descriptors: u64,
    /// Dedicated endpoint workers, per `WorkerTopology`.
    pub workers: u64,
    /// Process-level client instances; one per admission.
    pub client_instances: u64,
    /// Workers pinned to physical cores. Every shipped profile charges zero.
    pub pinned_workers: u64,
}

impl ResourceCharges {
    /// No charges.
    pub const ZERO: Self = Self {
        descriptors: 0,
        arena_bytes: 0,
        spans_per_frame: 0,
        leases: 0,
        mappings: 0,
        file_descriptors: 0,
        workers: 0,
        client_instances: 0,
        pinned_workers: 0,
    };

    fn checked_add(self, other: Self) -> Option<Self> {
        Some(Self {
            descriptors: self.descriptors.checked_add(other.descriptors)?,
            arena_bytes: self.arena_bytes.checked_add(other.arena_bytes)?,
            spans_per_frame: self.spans_per_frame.max(other.spans_per_frame),
            leases: self.leases.checked_add(other.leases)?,
            mappings: self.mappings.checked_add(other.mappings)?,
            file_descriptors: self.file_descriptors.checked_add(other.file_descriptors)?,
            workers: self.workers.checked_add(other.workers)?,
            client_instances: self.client_instances.checked_add(other.client_instances)?,
            pinned_workers: self.pinned_workers.checked_add(other.pinned_workers)?,
        })
    }

    fn checked_sub(self, other: Self) -> Option<Self> {
        Some(Self {
            descriptors: self.descriptors.checked_sub(other.descriptors)?,
            arena_bytes: self.arena_bytes.checked_sub(other.arena_bytes)?,
            // A maximum, not a sum: release paths recompute it from the
            // per-admission span counts in `Accounting`.
            spans_per_frame: self.spans_per_frame,
            leases: self.leases.checked_sub(other.leases)?,
            mappings: self.mappings.checked_sub(other.mappings)?,
            file_descriptors: self.file_descriptors.checked_sub(other.file_descriptors)?,
            workers: self.workers.checked_sub(other.workers)?,
            client_instances: self.client_instances.checked_sub(other.client_instances)?,
            pinned_workers: self.pinned_workers.checked_sub(other.pinned_workers)?,
        })
    }
}

/// Requested ring geometry. `TargetProfile::new` checks it and computes the charges.
pub struct ProfileConfig {
    /// Schema version and hardware profile id the grant will carry.
    pub descriptor: TransportDescriptor,
    /// Descriptor slots per direction; also bounds `max_leases`.
    pub descriptor_depth: usize,
    /// Arena bytes per direction, at least `MIN_ARENA_BYTES`.
    pub arena_bytes: usize,
    /// Spans one frame may occupy: 1 forbids wrapping, 2 allows it.
    pub max_spans: usize,
    /// Receive leases outstanding at once per direction, 1 to `descriptor_depth`.
    pub max_leases: usize,
    /// Mappings charged; at least `SETUP_MAPPING_COUNT`, one per direction.
    pub mappings: usize,
    /// Pinned workers charged; must be 0.
    pub pinned_workers: usize,
    /// Which thread drives each direction.
    pub worker_topology: WorkerTopology,
}

/// A `ProfileConfig` that passed validation, with its host charges precomputed.
pub struct TargetProfile {
    descriptor: TransportDescriptor,
    descriptor_depth: usize,
    arena_bytes: usize,
    max_spans: usize,
    max_leases: usize,
    worker_topology: WorkerTopology,
    charges: ResourceCharges,
}

impl TargetProfile {
    /// Validates `config` and derives the charges. Per-direction values are doubled; the file
    /// descriptor charge is `mappings + SETUP_DOORBELL_COUNT`, the same descriptors a grant
    /// transfers. Fails before any mapping or worker exists, so a rejected profile costs
    /// nothing.
    pub fn new(config: ProfileConfig) -> Result<Self, ProfileError> {
        if config.descriptor.schema_version() != crate::descriptor::DESCRIPTOR_SCHEMA_VERSION {
            return Err(ProfileError::UnsupportedSchema);
        }
        if config.descriptor_depth == 0 {
            return Err(ProfileError::ZeroDescriptorDepth);
        }
        if config.arena_bytes < MIN_ARENA_BYTES {
            return Err(ProfileError::ArenaBelowMinimum);
        }
        if !(1..=MAX_SPANS).contains(&config.max_spans) {
            return Err(ProfileError::InvalidSpanLimit);
        }
        if config.max_leases == 0 || config.max_leases > config.descriptor_depth {
            return Err(ProfileError::InvalidLeaseLimit);
        }
        if config.mappings < SETUP_MAPPING_COUNT {
            return Err(ProfileError::InvalidMappingCharge);
        }
        if config.pinned_workers != 0 {
            return Err(ProfileError::InvalidWorkerCharge);
        }
        let descriptors = u64::try_from(config.descriptor_depth)
            .ok()
            .and_then(|value| value.checked_mul(2))
            .ok_or(ProfileError::ChargeOverflow)?;
        let arena_bytes = u64::try_from(config.arena_bytes)
            .ok()
            .and_then(|value| value.checked_mul(2))
            .ok_or(ProfileError::ChargeOverflow)?;
        let leases = u64::try_from(config.max_leases)
            .ok()
            .and_then(|value| value.checked_mul(2))
            .ok_or(ProfileError::ChargeOverflow)?;
        let charges = ResourceCharges {
            descriptors,
            arena_bytes,
            spans_per_frame: config.max_spans as u64,
            leases,
            mappings: config.mappings as u64,
            file_descriptors: (config.mappings as u64)
                .checked_add(SETUP_DOORBELL_COUNT as u64)
                .ok_or(ProfileError::ChargeOverflow)?,
            workers: match config.worker_topology {
                WorkerTopology::CallerThread => 0,
                WorkerTopology::SplitDirection => 2,
                WorkerTopology::Fused => 1,
            },
            client_instances: 1,
            pinned_workers: config.pinned_workers as u64,
        };

        Ok(Self {
            descriptor: config.descriptor,
            descriptor_depth: config.descriptor_depth,
            arena_bytes: config.arena_bytes,
            max_spans: config.max_spans,
            max_leases: config.max_leases,
            worker_topology: config.worker_topology,
            charges,
        })
    }

    /// Schema version and hardware profile id the grant carries.
    pub const fn descriptor(&self) -> &TransportDescriptor {
        &self.descriptor
    }

    /// Descriptor slots per direction.
    pub const fn descriptor_depth(&self) -> usize {
        self.descriptor_depth
    }

    /// Arena bytes per direction.
    pub const fn arena_bytes(&self) -> usize {
        self.arena_bytes
    }

    /// Spans one frame may occupy, 1 or 2.
    pub const fn max_spans(&self) -> usize {
        self.max_spans
    }

    /// Receive leases outstanding at once per direction.
    pub const fn max_leases(&self) -> usize {
        self.max_leases
    }

    /// Which thread drives each direction.
    pub const fn worker_topology(&self) -> WorkerTopology {
        self.worker_topology
    }

    /// What admitting this profile costs the host.
    pub const fn charges(&self) -> ResourceCharges {
        self.charges
    }
}

crate::redacted_debug!(TargetProfile);

/// Process-wide ceilings. Quarantined charges count against every limit except `workers`
/// and `pinned_workers`, because a quarantined ring keeps its memory but its threads exit.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct HostLimits {
    /// Descriptor slots, active plus quarantined.
    pub descriptors: u64,
    /// Arena bytes, active plus quarantined.
    pub arena_bytes: u64,
    /// Receive leases, active plus quarantined.
    pub leases: u64,
    /// Mappings, active plus quarantined.
    pub mappings: u64,
    /// File descriptors, active plus quarantined.
    pub file_descriptors: u64,
    /// Endpoint workers, active only.
    pub workers: u64,
    /// Client instances, active plus quarantined.
    pub client_instances: u64,
    /// Pinned workers, active only; also capped by `VerifiedPhysicalCores` when supplied.
    /// `TargetProfile::new` rejects any nonzero pinned charge, so no profile this crate can
    /// build ever counts against this limit and it cannot refuse an admission today.
    pub pinned_workers: u64,
}

/// Number of distinct physical cores this process may run on, read from Linux topology
/// files rather than trusted from configuration. Only bounds `HostLimits::pinned_workers`,
/// which no profile this crate can build charges, so supplying it does not change any
/// admission outcome until a pinned profile exists.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct VerifiedPhysicalCores(u64);

impl VerifiedPhysicalCores {
    /// Counts unique `(physical_package_id, core_id)` pairs over `Cpus_allowed_list`.
    /// Returns `None` if any sysfs file is missing or unparsable, so a partial answer is
    /// never mistaken for a verified one.
    #[cfg(target_os = "linux")]
    pub fn detect() -> Option<Self> {
        let allowed = allowed_linux_cpus()?;
        let mut physical = HashSet::new();
        for cpu in allowed {
            let root = format!("/sys/devices/system/cpu/cpu{cpu}/topology");
            let package: u64 = std::fs::read_to_string(format!("{root}/physical_package_id"))
                .ok()?
                .trim()
                .parse()
                .ok()?;
            let core: u64 = std::fs::read_to_string(format!("{root}/core_id"))
                .ok()?
                .trim()
                .parse()
                .ok()?;
            physical.insert((package, core));
        }
        (!physical.is_empty()).then_some(Self(physical.len() as u64))
    }

    /// Always `None` off Linux: no topology source is trusted there, so pinned-worker
    /// admission falls back to `HostLimits::pinned_workers` alone.
    #[cfg(not(target_os = "linux"))]
    pub const fn detect() -> Option<Self> {
        None
    }

    /// The verified count.
    pub const fn get(self) -> u64 {
        self.0
    }
}

#[cfg(target_os = "linux")]
fn allowed_linux_cpus() -> Option<Vec<u32>> {
    let status = std::fs::read_to_string("/proc/self/status").ok()?;
    let spec = status
        .lines()
        .find_map(|line| line.strip_prefix("Cpus_allowed_list:\t"))?;
    let mut cpus = Vec::new();
    for item in spec.split(',') {
        if let Some((start, end)) = item.split_once('-') {
            let start: u32 = start.parse().ok()?;
            let end: u32 = end.parse().ok()?;
            cpus.extend(start..=end);
        } else {
            cpus.push(item.parse().ok()?);
        }
    }
    Some(cpus)
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct Accounting {
    active: ResourceCharges,
    quarantined: ResourceCharges,
    // Active admissions per span charge; slot `i` counts admissions
    // charging `i + 1` spans. `active.spans_per_frame` is the maximum over
    // active admissions, so releasing one must recompute it from these
    // counts instead of subtracting.
    active_span_counts: [u64; MAX_SPANS],
}

impl Accounting {
    fn span_slot(spans: u64) -> Option<usize> {
        usize::try_from(spans)
            .ok()
            .and_then(|spans| spans.checked_sub(1))
            .filter(|slot| *slot < MAX_SPANS)
    }

    fn charge_spans(&mut self, spans: u64) {
        if let Some(slot) = Self::span_slot(spans) {
            self.active_span_counts[slot] = self.active_span_counts[slot].saturating_add(1);
        }
    }

    fn release_spans(&mut self, spans: u64) {
        if let Some(slot) = Self::span_slot(spans) {
            self.active_span_counts[slot] = self.active_span_counts[slot].saturating_sub(1);
        }
        self.active.spans_per_frame = self
            .active_span_counts
            .iter()
            .rposition(|count| *count > 0)
            .map_or(0, |slot| slot as u64 + 1);
    }
}

/// Aggregate charges without any per-connection identity, safe to log.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct AccountingSnapshot {
    /// Charges of live admissions.
    pub active: ResourceCharges,
    /// Charges retained by quarantined admissions. `workers` and `pinned_workers` are zero.
    /// This total only grows; alarm on it, because once it approaches `HostLimits` every
    /// `admit` fails and only a process restart recovers the capacity.
    pub quarantined: ResourceCharges,
}

/// Admits connections against `HostLimits`. One instance per process; `admit` charges,
/// `Admission` refunds on drop or moves the charge to quarantine.
///
/// Quarantine is a one-way ratchet: a quarantined ring's memory may still be mapped by its
/// peer, so nothing here can prove it unmapped and reclaim the charge. A peer that keeps
/// triggering quarantine therefore consumes host capacity permanently. Callers wiring this
/// into an accept path must export `snapshot().quarantined`, alarm on it, and treat process
/// restart as the recovery.
pub struct AdmissionController {
    limits: HostLimits,
    accounting: Mutex<Accounting>,
}

impl AdmissionController {
    /// Starts with nothing admitted.
    pub const fn new(limits: HostLimits) -> Self {
        Self {
            limits,
            accounting: Mutex::new(Accounting {
                active: ResourceCharges::ZERO,
                quarantined: ResourceCharges::ZERO,
                active_span_counts: [0; MAX_SPANS],
            }),
        }
    }

    /// Same checks as `admit`, without charging. Two `can_admit` calls that both pass do not
    /// guarantee two `admit` calls will.
    pub fn can_admit(
        &self,
        profile: &TargetProfile,
        physical_cores: Option<VerifiedPhysicalCores>,
    ) -> Result<(), AdmissionError> {
        let accounting = self
            .accounting
            .lock()
            .map_err(|_| AdmissionError::AccountingUnavailable)?;
        self.check_admission(*accounting, profile, physical_cores)
            .map(|_| ())
    }

    /// Charges `profile` if every limit holds. Call before creating mappings or workers so a
    /// rejected connection never touches the kernel. Limits are checked in field order and the
    /// first exceeded one is returned.
    pub fn admit(
        self: &Arc<Self>,
        profile: &TargetProfile,
        physical_cores: Option<VerifiedPhysicalCores>,
    ) -> Result<Admission, AdmissionError> {
        let mut accounting = self
            .accounting
            .lock()
            .map_err(|_| AdmissionError::AccountingUnavailable)?;
        let active = self.check_admission(*accounting, profile, physical_cores)?;
        let charges = profile.charges();
        accounting.active = active;
        accounting.charge_spans(charges.spans_per_frame);
        Ok(Admission {
            controller: Arc::clone(self),
            charges,
            state: AdmissionState::Active,
        })
    }

    fn check_admission(
        &self,
        accounting: Accounting,
        profile: &TargetProfile,
        physical_cores: Option<VerifiedPhysicalCores>,
    ) -> Result<ResourceCharges, AdmissionError> {
        let requested = profile.charges();
        let active = accounting
            .active
            .checked_add(requested)
            .ok_or(AdmissionError::ChargeOverflow)?;
        let committed = active
            .checked_add(accounting.quarantined)
            .ok_or(AdmissionError::ChargeOverflow)?;
        if committed.descriptors > self.limits.descriptors {
            return Err(AdmissionError::DescriptorLimit);
        }
        if committed.arena_bytes > self.limits.arena_bytes {
            return Err(AdmissionError::ArenaByteLimit);
        }
        if committed.leases > self.limits.leases {
            return Err(AdmissionError::LeaseLimit);
        }
        if committed.mappings > self.limits.mappings {
            return Err(AdmissionError::MappingLimit);
        }
        if committed.file_descriptors > self.limits.file_descriptors {
            return Err(AdmissionError::FileDescriptorLimit);
        }
        if active.workers > self.limits.workers {
            return Err(AdmissionError::WorkerLimit);
        }
        if committed.client_instances > self.limits.client_instances {
            return Err(AdmissionError::ClientInstanceLimit);
        }
        let core_limit = physical_cores
            .map(VerifiedPhysicalCores::get)
            .unwrap_or(self.limits.pinned_workers)
            .min(self.limits.pinned_workers);
        if active.pinned_workers > core_limit {
            return Err(AdmissionError::PhysicalCoreBudgetExceeded);
        }
        Ok(active)
    }

    /// Aggregate charges. Fails only if the accounting lock is poisoned.
    pub fn snapshot(&self) -> Result<AccountingSnapshot, AdmissionError> {
        let accounting = self
            .accounting
            .lock()
            .map_err(|_| AdmissionError::AccountingUnavailable)?;
        Ok(AccountingSnapshot {
            active: accounting.active,
            quarantined: accounting.quarantined,
        })
    }

    fn release(&self, charges: ResourceCharges) {
        let Ok(mut accounting) = self.accounting.lock() else {
            return;
        };
        if let Some(active) = accounting.active.checked_sub(charges) {
            accounting.active = active;
            accounting.release_spans(charges.spans_per_frame);
        }
    }

    fn quarantine(&self, charges: ResourceCharges) -> Result<(), AdmissionError> {
        let mut accounting = self
            .accounting
            .lock()
            .map_err(|_| AdmissionError::AccountingUnavailable)?;
        let retained = ResourceCharges {
            workers: 0,
            pinned_workers: 0,
            ..charges
        };
        // Both totals are computed before either is stored, so a failed checked
        // operation leaves `accounting` unchanged.
        let active = accounting
            .active
            .checked_sub(charges)
            .ok_or(AdmissionError::AccountingUnavailable)?;
        let quarantined = accounting
            .quarantined
            .checked_add(retained)
            .ok_or(AdmissionError::ChargeOverflow)?;
        accounting.active = active;
        accounting.quarantined = quarantined;
        accounting.release_spans(charges.spans_per_frame);
        Ok(())
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum AdmissionState {
    Active,
    Released,
    Quarantined,
}

/// Charges held by one admitted connection. Dropping it refunds the charges; `quarantine`
/// moves them to the quarantined bucket instead.
#[must_use = "admission must remain alive while candidate resources exist"]
pub struct Admission {
    controller: Arc<AdmissionController>,
    charges: ResourceCharges,
    state: AdmissionState,
}

impl Admission {
    /// Refunds every charge. Equivalent to dropping; exists so call sites can name the intent.
    pub fn release(mut self) {
        self.controller.release(self.charges);
        self.state = AdmissionState::Released;
    }

    /// Moves descriptors, arena bytes, leases, mappings, file descriptors, and the client
    /// instance to the quarantined bucket, where they stay until the process exits. Worker
    /// charges are refunded because the threads do exit.
    pub fn quarantine(mut self) -> Result<QuarantineRecord, AdmissionError> {
        self.controller.quarantine(self.charges)?;
        self.state = AdmissionState::Quarantined;
        Ok(QuarantineRecord { _private: () })
    }
}

crate::redacted_debug!(Admission);

impl Drop for Admission {
    fn drop(&mut self) {
        if self.state == AdmissionState::Active {
            self.controller.release(self.charges);
            self.state = AdmissionState::Released;
        }
    }
}

/// Proof that an `Admission` was quarantined rather than released. Has no operations; its
/// existence in a caller's state means the charges are still counted against `HostLimits`.
pub struct QuarantineRecord {
    _private: (),
}

crate::redacted_debug!(QuarantineRecord);

/// Why `TargetProfile::new` rejected a `ProfileConfig`.
#[derive(Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum ProfileError {
    /// `descriptor.schema_version()` is not `DESCRIPTOR_SCHEMA_VERSION`.
    #[error("target profile schema is unsupported")]
    UnsupportedSchema,
    /// `descriptor_depth` is zero.
    #[error("descriptor depth is zero")]
    ZeroDescriptorDepth,
    /// `arena_bytes` is below `MIN_ARENA_BYTES`.
    #[error("arena is below protocol minimum")]
    ArenaBelowMinimum,
    /// `max_spans` is not 1 or 2.
    #[error("span limit is invalid")]
    InvalidSpanLimit,
    /// `max_leases` is zero or exceeds `descriptor_depth`.
    #[error("lease limit is invalid")]
    InvalidLeaseLimit,
    /// `mappings` is below `SETUP_MAPPING_COUNT`.
    #[error("mapping charge is invalid")]
    InvalidMappingCharge,
    /// `pinned_workers` is nonzero.
    #[error("worker charge is invalid")]
    InvalidWorkerCharge,
    /// Doubling a per-direction value or adding the doorbell descriptors overflowed `u64`.
    #[error("profile resource charge overflow")]
    ChargeOverflow,
}

impl fmt::Debug for ProfileError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(self, formatter)
    }
}

/// Why `AdmissionController::admit` refused a profile.
#[derive(Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum AdmissionError {
    /// Reserved for callers that require `VerifiedPhysicalCores` and could not obtain it;
    /// the controller itself never returns this.
    #[error("physical-core topology is unverified")]
    PhysicalCoresUnverified,
    /// Active pinned workers would exceed the smaller of `HostLimits::pinned_workers` and the
    /// verified core count. Unreachable while `TargetProfile::new` rejects every nonzero
    /// pinned charge.
    #[error("physical-core budget exceeded")]
    PhysicalCoreBudgetExceeded,
    /// Descriptor commitment exceeds host limit.
    #[error("host descriptor limit exceeded")]
    DescriptorLimit,
    /// Arena-byte commitment exceeds host limit.
    #[error("host arena-byte limit exceeded")]
    ArenaByteLimit,
    /// Lease commitment exceeds host limit.
    #[error("host lease limit exceeded")]
    LeaseLimit,
    /// Mapping commitment exceeds host limit.
    #[error("host mapping limit exceeded")]
    MappingLimit,
    /// Mapping descriptor commitment exceeds host limit.
    #[error("host file-descriptor limit exceeded")]
    FileDescriptorLimit,
    /// Active endpoint workers exceed host limit.
    #[error("host worker limit exceeded")]
    WorkerLimit,
    /// Client instances exceed host limit.
    #[error("host client-instance limit exceeded")]
    ClientInstanceLimit,
    /// Adding the requested charges to the active or quarantined totals overflowed `u64`.
    #[error("host admission arithmetic overflow")]
    ChargeOverflow,
    /// The accounting mutex is poisoned; a thread panicked while holding it.
    #[error("host admission accounting unavailable")]
    AccountingUnavailable,
}

impl fmt::Debug for AdmissionError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(self, formatter)
    }
}

/// Hardware profile id the host stamps into every production grant. A grant naming any other
/// profile is rejected before mapping, so this is a wire literal, not a configuration value.
pub const HOST_TEST_RING_PROFILE: &str = "host-test-ring-v1";

/// Descriptor slots per direction for `HOST_TEST_RING_PROFILE`; also its lease bound.
pub const HOST_TEST_RING_DEPTH: usize = 8;

/// The geometry `HOST_TEST_RING_PROFILE` names, so a peer or harness that echoes that id
/// exercises the depth and topology the host creates.
pub fn host_test_ring_profile() -> Result<TargetProfile, ProfileError> {
    TargetProfile::new(ProfileConfig {
        descriptor: TransportDescriptor::new(
            HardwareProfileId::new(HOST_TEST_RING_PROFILE)
                .expect("static hardware profile id is valid"),
        ),
        descriptor_depth: HOST_TEST_RING_DEPTH,
        arena_bytes: MIN_ARENA_BYTES,
        max_spans: 2,
        max_leases: HOST_TEST_RING_DEPTH,
        mappings: SETUP_MAPPING_COUNT,
        pinned_workers: 0,
        worker_topology: WorkerTopology::Fused,
    })
}

/// Depth-32, caller-thread profile under an arbitrary id, for tests and local tools.
pub fn ring_profile(hardware: HardwareProfileId) -> Result<TargetProfile, ProfileError> {
    TargetProfile::new(ProfileConfig {
        descriptor: TransportDescriptor::new(hardware),
        descriptor_depth: 32,
        arena_bytes: MIN_ARENA_BYTES,
        max_spans: 2,
        max_leases: 32,
        mappings: SETUP_MAPPING_COUNT,
        pinned_workers: 0,
        worker_topology: WorkerTopology::CallerThread,
    })
}
