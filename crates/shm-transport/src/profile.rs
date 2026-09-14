#[cfg(target_os = "linux")]
use std::collections::HashSet;
use std::fmt;
use std::sync::{Arc, Mutex};

use crate::backend::retained::system_page_size;
use crate::descriptor::{
    HardwareProfileId, SETUP_DOORBELL_COUNT, SETUP_MAPPING_COUNT, TransportDescriptor,
};
use crate::pool::{MappingLayout, PoolGeometry, ledger_bytes};

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

/// What one admitted connection costs the host. Every field is a sum across admissions.
/// Physical commitment (`mapping_bytes`, `ledger_bytes`) is separate from logical occupancy
/// (`descriptors`, `leases`), so the aggregate the host exposes is a checked layout total,
/// not a residency claim.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ResourceCharges {
    /// Descriptor slots, ordinary plus reserved, both directions.
    pub descriptors: u64,
    /// Mapped bytes, both directions: block backing, padding, descriptors, completion cells,
    /// and control metadata.
    pub mapping_bytes: u64,
    /// Private heap bytes, both directions: producer ledgers, free lists, and receiver
    /// records, all allocated before activation.
    pub ledger_bytes: u64,
    /// Blocks, both directions: the bound on live leases and return records.
    pub leases: u64,
    /// Shared-memory mappings.
    pub mappings: u64,
    /// File descriptors kept open for the mappings and doorbells.
    pub file_descriptors: u64,
    /// Local wake handles an endpoint retains beyond its thread: one capacity doorbell end per
    /// direction, kept alive by outstanding leases.
    pub wake_handles: u64,
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
        mapping_bytes: 0,
        ledger_bytes: 0,
        leases: 0,
        mappings: 0,
        file_descriptors: 0,
        wake_handles: 0,
        workers: 0,
        client_instances: 0,
        pinned_workers: 0,
    };

    fn checked_add(self, other: Self) -> Option<Self> {
        Some(Self {
            descriptors: self.descriptors.checked_add(other.descriptors)?,
            mapping_bytes: self.mapping_bytes.checked_add(other.mapping_bytes)?,
            ledger_bytes: self.ledger_bytes.checked_add(other.ledger_bytes)?,
            leases: self.leases.checked_add(other.leases)?,
            mappings: self.mappings.checked_add(other.mappings)?,
            file_descriptors: self.file_descriptors.checked_add(other.file_descriptors)?,
            wake_handles: self.wake_handles.checked_add(other.wake_handles)?,
            workers: self.workers.checked_add(other.workers)?,
            client_instances: self.client_instances.checked_add(other.client_instances)?,
            pinned_workers: self.pinned_workers.checked_add(other.pinned_workers)?,
        })
    }

    fn checked_sub(self, other: Self) -> Option<Self> {
        Some(Self {
            descriptors: self.descriptors.checked_sub(other.descriptors)?,
            mapping_bytes: self.mapping_bytes.checked_sub(other.mapping_bytes)?,
            ledger_bytes: self.ledger_bytes.checked_sub(other.ledger_bytes)?,
            leases: self.leases.checked_sub(other.leases)?,
            mappings: self.mappings.checked_sub(other.mappings)?,
            file_descriptors: self.file_descriptors.checked_sub(other.file_descriptors)?,
            wake_handles: self.wake_handles.checked_sub(other.wake_handles)?,
            workers: self.workers.checked_sub(other.workers)?,
            client_instances: self.client_instances.checked_sub(other.client_instances)?,
            pinned_workers: self.pinned_workers.checked_sub(other.pinned_workers)?,
        })
    }

    /// Every physical byte this admission commits: mapping plus private ledgers.
    pub const fn committed_bytes(self) -> Option<u64> {
        self.mapping_bytes.checked_add(self.ledger_bytes)
    }

    /// The worker-thread portion, refunded when the endpoint thread exits.
    const fn worker_part(self) -> Self {
        Self {
            workers: self.workers,
            pinned_workers: self.pinned_workers,
            ..Self::ZERO
        }
    }

    /// Everything but the worker portion: backing, metadata, handles, and the instance.
    const fn backing_part(self) -> Self {
        Self {
            workers: 0,
            pinned_workers: 0,
            ..self
        }
    }
}

/// Requested pool geometry. `TargetProfile::new` checks it and computes the charges.
pub struct ProfileConfig {
    /// Schema version and hardware profile id the grant will carry.
    pub descriptor: TransportDescriptor,
    /// Per-direction geometry.
    pub geometry: PoolGeometry,
    /// Mappings charged; at least `SETUP_MAPPING_COUNT`, one per direction.
    pub mappings: usize,
    /// Pinned workers charged; must be 0.
    pub pinned_workers: usize,
    /// Which thread drives each direction.
    pub worker_topology: WorkerTopology,
}

/// A `ProfileConfig` that passed validation, with its host charges precomputed from the
/// complete mapping layout.
pub struct TargetProfile {
    descriptor: TransportDescriptor,
    geometry: PoolGeometry,
    mapping_bytes_per_direction: u64,
    worker_topology: WorkerTopology,
    charges: ResourceCharges,
}

impl TargetProfile {
    /// Validates `config` and derives the charges from the full mapping layout. Per-direction
    /// values are doubled; the file descriptor charge is `mappings + SETUP_DOORBELL_COUNT`, the
    /// same descriptors a grant transfers. Fails before any mapping or worker exists, so a
    /// rejected profile costs nothing.
    pub fn new(config: ProfileConfig) -> Result<Self, ProfileError> {
        if config.descriptor.schema_version() != crate::descriptor::DESCRIPTOR_SCHEMA_VERSION {
            return Err(ProfileError::UnsupportedSchema);
        }
        if !config.geometry.holds_maximum_frame() {
            return Err(ProfileError::LargestClassBelowMaximumFrame);
        }
        if config.mappings < SETUP_MAPPING_COUNT {
            return Err(ProfileError::InvalidMappingCharge);
        }
        if config.pinned_workers != 0 {
            return Err(ProfileError::InvalidWorkerCharge);
        }
        let layout = MappingLayout::new(&config.geometry, system_page_size())
            .map_err(|_| ProfileError::InvalidGeometry)?;
        let double = |value: u64| value.checked_mul(2).ok_or(ProfileError::ChargeOverflow);
        let mapping_bytes_per_direction = layout.total as u64;
        let charges = ResourceCharges {
            descriptors: double(u64::from(config.geometry.descriptor_depth()))?,
            mapping_bytes: double(mapping_bytes_per_direction)?,
            ledger_bytes: double(ledger_bytes(&config.geometry))?,
            leases: double(u64::from(config.geometry.block_count()))?,
            mappings: config.mappings as u64,
            file_descriptors: (config.mappings as u64)
                .checked_add(SETUP_DOORBELL_COUNT as u64)
                .ok_or(ProfileError::ChargeOverflow)?,
            wake_handles: SETUP_MAPPING_COUNT as u64,
            workers: match config.worker_topology {
                WorkerTopology::CallerThread => 0,
                WorkerTopology::SplitDirection => 2,
                WorkerTopology::Fused => 1,
            },
            client_instances: 1,
            pinned_workers: config.pinned_workers as u64,
        };
        charges
            .committed_bytes()
            .ok_or(ProfileError::ChargeOverflow)?;

        Ok(Self {
            descriptor: config.descriptor,
            geometry: config.geometry,
            mapping_bytes_per_direction,
            worker_topology: config.worker_topology,
            charges,
        })
    }

    /// Schema version and hardware profile id the grant carries.
    pub const fn descriptor(&self) -> &TransportDescriptor {
        &self.descriptor
    }

    /// Per-direction geometry.
    pub const fn geometry(&self) -> &PoolGeometry {
        &self.geometry
    }

    /// Descriptor slots per direction, ordinary plus reserved.
    pub const fn descriptor_depth(&self) -> u32 {
        self.geometry.descriptor_depth()
    }

    /// Complete mapping bytes per direction, from the checked layout.
    pub const fn mapping_bytes_per_direction(&self) -> u64 {
        self.mapping_bytes_per_direction
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
/// and `pinned_workers`, because a quarantined pool keeps its memory but its threads exit.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct HostLimits {
    /// Descriptor slots, active plus quarantined.
    pub descriptors: u64,
    /// Mapped bytes, active plus quarantined.
    pub mapping_bytes: u64,
    /// Private ledger bytes, active plus quarantined.
    pub ledger_bytes: u64,
    /// Blocks, active plus quarantined.
    pub leases: u64,
    /// Mappings, active plus quarantined.
    pub mappings: u64,
    /// File descriptors, active plus quarantined.
    pub file_descriptors: u64,
    /// Retained wake handles, active plus quarantined.
    pub wake_handles: u64,
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
    parse_cpu_list(spec)
}

/// Kernel CPU-list syntax (`0-3,8,10-11`). Returns `None` if any item is malformed, including
/// an inverted range, so a partial list is never returned as a verified one.
#[cfg(any(target_os = "linux", test))]
fn parse_cpu_list(spec: &str) -> Option<Vec<u32>> {
    let mut cpus = Vec::new();
    for item in spec.split(',') {
        if let Some((start, end)) = item.split_once('-') {
            let start: u32 = start.parse().ok()?;
            let end: u32 = end.parse().ok()?;
            // An inverted range is empty, so reject it instead of accepting a smaller CPU list.
            if start > end {
                return None;
            }
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
/// Quarantine is a one-way ratchet: a quarantined pool's memory may still be mapped by its
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
        self.check_admission(*accounting, profile.charges(), physical_cores)
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
        let charges = profile.charges();
        let active = self.check_admission(*accounting, charges, physical_cores)?;
        accounting.active = active;
        Ok(Admission {
            controller: Arc::clone(self),
            charges,
            state: AdmissionState::Active,
        })
    }

    fn check_admission(
        &self,
        accounting: Accounting,
        requested: ResourceCharges,
        physical_cores: Option<VerifiedPhysicalCores>,
    ) -> Result<ResourceCharges, AdmissionError> {
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
        if committed.mapping_bytes > self.limits.mapping_bytes {
            return Err(AdmissionError::MappingByteLimit);
        }
        if committed.ledger_bytes > self.limits.ledger_bytes {
            return Err(AdmissionError::LedgerByteLimit);
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
        if committed.wake_handles > self.limits.wake_handles {
            return Err(AdmissionError::WakeHandleLimit);
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
            .ok_or(AdmissionError::ChargeUnderflow)?;
        let quarantined = accounting
            .quarantined
            .checked_add(retained)
            .ok_or(AdmissionError::ChargeOverflow)?;
        accounting.active = active;
        accounting.quarantined = quarantined;
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
/// moves them to the quarantined bucket instead; `split` separates the worker portion, which
/// ends with the thread, from the backing portion, which ends with the last lease.
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

    /// Moves descriptors, bytes, leases, mappings, file descriptors, wake handles, and the
    /// client instance to the quarantined bucket, where they stay until the process exits.
    /// Worker charges are refunded because the threads do exit.
    pub fn quarantine(mut self) -> Result<QuarantineRecord, AdmissionError> {
        self.controller.quarantine(self.charges)?;
        self.state = AdmissionState::Quarantined;
        Ok(QuarantineRecord { _private: () })
    }

    /// Separates the charge into the worker portion and the backing portion so each is
    /// settled when its own lifetime ends.
    pub fn split(mut self) -> (WorkerAdmission, BackingAdmission) {
        self.state = AdmissionState::Released;
        (
            WorkerAdmission {
                controller: Arc::clone(&self.controller),
                charges: self.charges.worker_part(),
                released: false,
            },
            BackingAdmission {
                controller: Arc::clone(&self.controller),
                charges: self.charges.backing_part(),
                state: Mutex::new(BackingState::Active),
            },
        )
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

/// The worker-thread portion of an admission; refunded when the endpoint thread exits.
#[must_use = "worker admission must remain alive while the endpoint thread runs"]
pub struct WorkerAdmission {
    controller: Arc<AdmissionController>,
    charges: ResourceCharges,
    released: bool,
}

impl WorkerAdmission {
    /// Refunds the worker charge. Equivalent to dropping.
    pub fn release(mut self) {
        self.controller.release(self.charges);
        self.released = true;
    }
}

crate::redacted_debug!(WorkerAdmission);

impl Drop for WorkerAdmission {
    fn drop(&mut self) {
        if !self.released {
            self.controller.release(self.charges);
            self.released = true;
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum BackingState {
    Active,
    Quarantined,
    /// An accounting operation failed; the charge stays counted as active forever rather than
    /// being refunded for storage nobody proved released.
    RetainedUncertain,
}

/// The backing portion of an admission: mapping, ledgers, descriptors, leases, handles, and
/// the instance. Shared by both directions' retained backing, so it settles when the last
/// lease of either direction returns and both handles have dropped, not when the endpoint
/// thread exits.
#[must_use = "backing admission must remain alive while the mapping may be reachable"]
pub struct BackingAdmission {
    controller: Arc<AdmissionController>,
    charges: ResourceCharges,
    state: Mutex<BackingState>,
}

impl BackingAdmission {
    /// Moves the charge to the quarantined bucket. A failure leaves the charge counted as
    /// active permanently: nothing may refund storage whose release is unproved.
    pub fn quarantine(&self) -> Result<QuarantineRecord, AdmissionError> {
        let mut state = self
            .state
            .lock()
            .map_err(|_| AdmissionError::AccountingUnavailable)?;
        if *state != BackingState::Active {
            return Err(AdmissionError::ChargeUnderflow);
        }
        // The state is moved off `Active` before the accounting call so a failure below cannot
        // fall through to the refunding destructor.
        *state = BackingState::RetainedUncertain;
        self.controller.quarantine(self.charges)?;
        *state = BackingState::Quarantined;
        Ok(QuarantineRecord { _private: () })
    }

    /// Keeps the charge counted as active for the process lifetime. Used when ownership of the
    /// backing is uncertain and quarantine accounting itself is unavailable.
    pub fn retain_uncertain(&self) {
        if let Ok(mut state) = self.state.lock()
            && *state == BackingState::Active
        {
            *state = BackingState::RetainedUncertain;
        }
    }

    /// The charges this admission holds.
    pub const fn charges(&self) -> ResourceCharges {
        self.charges
    }

    /// Whether the charge is still counted as an active admission that a drop would refund.
    pub fn is_active(&self) -> bool {
        self.state
            .lock()
            .is_ok_and(|state| *state == BackingState::Active)
    }
}

crate::redacted_debug!(BackingAdmission);

impl Drop for BackingAdmission {
    fn drop(&mut self) {
        let refund = self
            .state
            .lock()
            .is_ok_and(|state| *state == BackingState::Active);
        if refund {
            self.controller.release(self.charges);
        }
    }
}

/// Proof that an admission was quarantined rather than released. Has no operations; its
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
    /// The largest ordinary class cannot hold one maximum frame.
    #[error("largest pool class is below one maximum frame")]
    LargestClassBelowMaximumFrame,
    /// The geometry could not be laid out on this page size.
    #[error("pool geometry is invalid")]
    InvalidGeometry,
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
    /// Mapped-byte commitment exceeds host limit.
    #[error("host mapping-byte limit exceeded")]
    MappingByteLimit,
    /// Ledger-byte commitment exceeds host limit.
    #[error("host ledger-byte limit exceeded")]
    LedgerByteLimit,
    /// Lease commitment exceeds host limit.
    #[error("host lease limit exceeded")]
    LeaseLimit,
    /// Mapping commitment exceeds host limit.
    #[error("host mapping limit exceeded")]
    MappingLimit,
    /// Mapping descriptor commitment exceeds host limit.
    #[error("host file-descriptor limit exceeded")]
    FileDescriptorLimit,
    /// Wake-handle commitment exceeds host limit.
    #[error("host wake-handle limit exceeded")]
    WakeHandleLimit,
    /// Active endpoint workers exceed host limit.
    #[error("host worker limit exceeded")]
    WorkerLimit,
    /// Client instances exceed host limit.
    #[error("host client-instance limit exceeded")]
    ClientInstanceLimit,
    /// Adding the requested charges to the active or quarantined totals overflowed `u64`.
    #[error("host admission arithmetic overflow")]
    ChargeOverflow,
    /// Moving an admission's charges out of the active total would take a field below zero.
    /// Every admission is charged once and released once, so this means the accounting is
    /// corrupt, not that the caller did anything wrong.
    #[error("host admission accounting underflow")]
    ChargeUnderflow,
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
pub const HOST_PAYLOAD_POOL_PROFILE: &str = "host-payload-pool-v1";

/// The geometry `HOST_PAYLOAD_POOL_PROFILE` names, so a peer or harness that echoes that id
/// exercises the inventory the host creates.
pub fn host_payload_pool_profile() -> Result<TargetProfile, ProfileError> {
    TargetProfile::new(ProfileConfig {
        descriptor: TransportDescriptor::new(
            HardwareProfileId::new(HOST_PAYLOAD_POOL_PROFILE)
                .expect("static hardware profile id is valid"),
        ),
        geometry: PoolGeometry::host_payload_pool(),
        mappings: SETUP_MAPPING_COUNT,
        pinned_workers: 0,
        worker_topology: WorkerTopology::Fused,
    })
}

/// Caller-thread profile under an arbitrary id and geometry, for tests and local tools.
pub fn pool_profile(
    hardware: HardwareProfileId,
    geometry: PoolGeometry,
) -> Result<TargetProfile, ProfileError> {
    TargetProfile::new(ProfileConfig {
        descriptor: TransportDescriptor::new(hardware),
        geometry,
        mappings: SETUP_MAPPING_COUNT,
        pinned_workers: 0,
        worker_topology: WorkerTopology::CallerThread,
    })
}

#[cfg(test)]
mod tests {
    use super::parse_cpu_list;

    #[test]
    fn cpu_list_accepts_singletons_and_ascending_ranges() {
        assert_eq!(parse_cpu_list("0"), Some(vec![0]));
        assert_eq!(
            parse_cpu_list("0-3,8,10-11"),
            Some(vec![0, 1, 2, 3, 8, 10, 11])
        );
        assert_eq!(parse_cpu_list("5-5"), Some(vec![5]));
    }

    #[test]
    fn cpu_list_rejects_every_malformed_item_rather_than_returning_a_subset() {
        for spec in ["3-1", "0-3,9-7", "", "0,", "a", "0-", "-1", "0--1", "1-2-3"] {
            assert_eq!(parse_cpu_list(spec), None, "spec {spec:?} must not parse");
        }
    }
}
