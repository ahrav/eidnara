//! `kernel.eligibility.batch`: the kernel judges one verdict per candidate; the daemon caches each verdict by store lease epoch, tip, and classification generation.

use std::collections::{HashMap, VecDeque};

use context_core::canonical_json::is_lower_hex;
use host_runtime::RouteHandle;
use kernel::{
    ArtifactDestination, EgressSnapshot, EligibilityBatch, EligibilityCandidate,
    EligibilityVerdict, KernelError, KernelStore, MAX_ELIGIBILITY_CANDIDATES,
    MAX_ELIGIBILITY_OBJECT_ID_BYTES,
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use super::project::ProjectBinding;
use super::{KernelOpenCoordinator, KernelOutcome, blocking, kernel_response, state_only};
use crate::Handler;
use crate::dispatch::PreparedOutcome;

const OPERATION: &str = "kernel.eligibility.batch";
/// Entries held before the oldest is evicted.
const CACHE_CAPACITY: usize = 4096;
/// Every candidate becomes a cache key whether or not the object exists, so
/// the id length is what bounds the bytes an entry retains.
pub const MAX_OBJECT_ID_BYTES: usize = MAX_ELIGIBILITY_OBJECT_ID_BYTES;
/// Bytes one entry may retain: the key's strings, held once in the map and
/// once in the eviction order, plus the fixed-size fields and node overhead.
const ENTRY_BYTES_MAX: u64 = 2 * (MAX_OBJECT_ID_BYTES as u64 + 64 + 128) + 256;
/// Resident bytes the verdict cache may hold at capacity.
pub const CACHE_BUDGET_BYTES: u64 = CACHE_CAPACITY as u64 * ENTRY_BYTES_MAX;

/// The wire spelling. The daemon owns wire literals, so the kernel verdict is
/// mapped here rather than serialized directly; the exhaustive match breaks
/// the build when the kernel adds a verdict the wire does not name.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Verdict {
    Ok,
    Retracted,
    Superseded,
    Stale,
    WrongScope,
    /// The object is live but `kernel.read` hides it on every surface.
    Hidden,
    ProviderSensitive,
}

impl From<EligibilityVerdict> for Verdict {
    fn from(verdict: EligibilityVerdict) -> Self {
        match verdict {
            EligibilityVerdict::Ok => Verdict::Ok,
            EligibilityVerdict::Retracted => Verdict::Retracted,
            EligibilityVerdict::Superseded => Verdict::Superseded,
            EligibilityVerdict::Stale => Verdict::Stale,
            EligibilityVerdict::WrongScope => Verdict::WrongScope,
            EligibilityVerdict::Hidden => Verdict::Hidden,
            EligibilityVerdict::ProviderSensitive => Verdict::ProviderSensitive,
        }
    }
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct BatchRequest {
    destination: String,
    candidates: Vec<Candidate>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
struct Candidate {
    object_id: String,
    source_revision: i64,
    #[serde(default)]
    artifact_digest: Option<String>,
}

impl Candidate {
    fn to_kernel(&self) -> EligibilityCandidate {
        EligibilityCandidate {
            object_id: self.object_id.clone(),
            source_revision: self.source_revision,
            artifact_digest: self.artifact_digest.clone(),
        }
    }
}

/// A verdict is a function of these inputs and nothing else, so an entry is
/// valid exactly as long as every part of its key still names the same state.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct CacheKey {
    lease_epoch: u64,
    tip: i64,
    classification_generation: u64,
    object_id: String,
    source_revision: i64,
    artifact_digest: Option<String>,
    destination: ArtifactDestination,
    project_scope_id: String,
}

#[derive(Debug, Default)]
pub(crate) struct VerdictCache {
    entries: HashMap<CacheKey, EligibilityVerdict>,
    order: VecDeque<CacheKey>,
}

impl VerdictCache {
    fn get(&self, key: &CacheKey) -> Option<EligibilityVerdict> {
        self.entries.get(key).copied()
    }

    fn insert(&mut self, key: CacheKey, verdict: EligibilityVerdict) {
        if self.entries.insert(key.clone(), verdict).is_some() {
            return;
        }
        self.order.push_back(key);
        while self.order.len() > CACHE_CAPACITY {
            if let Some(oldest) = self.order.pop_front() {
                self.entries.remove(&oldest);
            }
        }
    }

    pub(crate) fn clear(&mut self) {
        self.entries.clear();
        self.order.clear();
    }

    #[cfg(any(test, feature = "test-support"))]
    pub(crate) fn len(&self) -> usize {
        self.entries.len()
    }
}

fn parse_destination(value: &str) -> Option<ArtifactDestination> {
    match value {
        "local" => Some(ArtifactDestination::Local),
        "remote" => Some(ArtifactDestination::Remote),
        _ => None,
    }
}

struct BatchResponse {
    known_as_of: i64,
    verdicts: Vec<(String, Verdict)>,
    cache_hits: usize,
}

/// `None` when the snapshot has no cache identity: nothing is looked up and
/// nothing is stored.
fn cache_keys(
    store: &KernelStore,
    project: &ProjectBinding,
    destination: ArtifactDestination,
    candidates: &[Candidate],
    snapshot: EgressSnapshot,
) -> Option<Vec<CacheKey>> {
    let generation = snapshot.classification_generation?;
    let lease_epoch = store.lease_epoch();
    let project_scope_id = project.scope_id();
    Some(
        candidates
            .iter()
            .map(|candidate| CacheKey {
                lease_epoch,
                tip: snapshot.tip,
                classification_generation: generation,
                object_id: candidate.object_id.clone(),
                source_revision: candidate.source_revision,
                artifact_digest: candidate.artifact_digest.clone(),
                destination,
                project_scope_id: project_scope_id.clone(),
            })
            .collect(),
    )
}

/// The cache guard is held only for the lookups and later for the inserts,
/// never across the kernel judgement.
fn lookup(
    coordinator: &KernelOpenCoordinator,
    keys: Option<&[CacheKey]>,
    len: usize,
) -> Vec<Option<EligibilityVerdict>> {
    match keys {
        Some(keys) => {
            let cache = coordinator.eligibility_cache();
            keys.iter().map(|key| cache.get(key)).collect()
        }
        None => vec![None; len],
    }
}

fn evaluate(
    store: &KernelStore,
    coordinator: &KernelOpenCoordinator,
    project: &ProjectBinding,
    destination: ArtifactDestination,
    candidates: &[Candidate],
) -> Result<BatchResponse, KernelError> {
    evaluate_with(
        store,
        coordinator,
        project,
        destination,
        candidates,
        |candidates| store.judge_eligibility(project.scope(), destination, candidates),
    )
}

/// Cached verdicts fill the hits and `judged` supplies one verdict per miss,
/// in candidate order.
fn splice(
    cached: &[Option<EligibilityVerdict>],
    judged: impl IntoIterator<Item = EligibilityVerdict>,
) -> Vec<EligibilityVerdict> {
    let mut judged = judged.into_iter();
    cached
        .iter()
        .map(|hit| hit.unwrap_or_else(|| judged.next().expect("one verdict per miss")))
        .collect()
}

/// Looks the cache up before asking the kernel, so a batch whose verdicts are
/// all cached costs one snapshot read. The misses are judged at whatever state
/// the store has by then; when a commit or a classification merge moved it in
/// between, the hits were judged against an older state than the misses, so
/// the whole batch is judged again at one snapshot and any entry the cache
/// already holds for that snapshot is served from the cache.
///
/// `judge` lets tests move the store between the snapshot and the judgement.
fn evaluate_with(
    store: &KernelStore,
    coordinator: &KernelOpenCoordinator,
    project: &ProjectBinding,
    destination: ArtifactDestination,
    candidates: &[Candidate],
    mut judge: impl FnMut(&[EligibilityCandidate]) -> Result<EligibilityBatch, KernelError>,
) -> Result<BatchResponse, KernelError> {
    let mut snapshot = store.egress_snapshot()?;
    let mut keys = cache_keys(store, project, destination, candidates, snapshot);
    let mut cached = lookup(coordinator, keys.as_deref(), candidates.len());
    let misses: Vec<EligibilityCandidate> = candidates
        .iter()
        .zip(&cached)
        .filter(|(_, hit)| hit.is_none())
        .map(|(candidate, _)| candidate.to_kernel())
        .collect();
    let verdicts = if misses.is_empty() {
        splice(&cached, [])
    } else {
        let batch = judge(&misses)?;
        if batch.snapshot == snapshot {
            splice(&cached, batch.verdicts)
        } else {
            let batch = if misses.len() == candidates.len() {
                batch
            } else {
                let all: Vec<EligibilityCandidate> =
                    candidates.iter().map(Candidate::to_kernel).collect();
                judge(&all)?
            };
            snapshot = batch.snapshot;
            keys = cache_keys(store, project, destination, candidates, snapshot);
            cached = lookup(coordinator, keys.as_deref(), candidates.len());
            let misses = batch
                .verdicts
                .into_iter()
                .zip(&cached)
                .filter(|(_, hit)| hit.is_none())
                .map(|(verdict, _)| verdict);
            splice(&cached, misses)
        }
    };
    let cache_hits = cached.iter().filter(|hit| hit.is_some()).count();
    if let Some(keys) = keys {
        let fresh: Vec<(CacheKey, EligibilityVerdict)> = keys
            .into_iter()
            .zip(&cached)
            .zip(&verdicts)
            .filter(|((_, hit), _)| hit.is_none())
            .map(|((key, _), verdict)| (key, *verdict))
            .collect();
        if !fresh.is_empty() {
            let mut cache = coordinator.eligibility_cache();
            for (key, verdict) in fresh {
                cache.insert(key, verdict);
            }
        }
    }
    Ok(BatchResponse {
        known_as_of: snapshot.tip,
        verdicts: candidates
            .iter()
            .zip(verdicts)
            .map(|(candidate, verdict)| (candidate.object_id.clone(), Verdict::from(verdict)))
            .collect(),
        cache_hits,
    })
}

impl Handler {
    pub(crate) async fn handle_kernel_eligibility_batch(
        &self,
        channel: RouteHandle,
        request: Value,
    ) -> PreparedOutcome {
        let (scope, parsed) = match self.kernel_request::<BatchRequest>(channel, request, OPERATION)
        {
            Ok(bound) => bound,
            Err(outcome) => return outcome,
        };
        let Some(destination) = parse_destination(&parsed.destination) else {
            return crate::invalid_params_error(format!(
                "{OPERATION} destination must be local or remote"
            ));
        };
        if parsed.candidates.len() > MAX_ELIGIBILITY_CANDIDATES {
            return crate::invalid_params_error(format!(
                "{OPERATION} carries at most {MAX_ELIGIBILITY_CANDIDATES} candidates"
            ));
        }
        if parsed.candidates.iter().any(|candidate| {
            candidate.object_id.is_empty() || candidate.object_id.len() > MAX_OBJECT_ID_BYTES
        }) {
            return crate::invalid_params_error(format!(
                "{OPERATION} object_id must be 1..={MAX_OBJECT_ID_BYTES} bytes"
            ));
        }
        if parsed.candidates.iter().any(|candidate| {
            candidate
                .artifact_digest
                .as_deref()
                .is_some_and(|digest| !is_lower_hex(digest, 64))
        }) {
            return crate::invalid_params_error(format!(
                "{OPERATION} artifact_digest must be lowercase sha256 hex"
            ));
        }
        let store = scope.store;
        let project = scope.project;
        let coordinator = self.kernel.clone();
        let candidates = parsed.candidates;
        let result =
            blocking(move || evaluate(&store, &coordinator, &project, destination, &candidates))
                .await;
        let response = match result {
            Ok(Ok(response)) => response,
            Ok(Err(error)) => return state_only(KernelOutcome::from(error)),
            Err(outcome) => return state_only(outcome),
        };
        let verdicts: Vec<Value> = response
            .verdicts
            .iter()
            .map(|(object_id, verdict)| json!({"object_id": object_id, "verdict": verdict}))
            .collect();
        kernel_response(
            &KernelOutcome::Available,
            json!({
                "known_as_of": response.known_as_of,
                "verdicts": verdicts,
                "cache_hits": response.cache_hits,
            }),
        )
    }
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;

    use kernel::CommitIntent;

    use super::*;

    fn key(index: usize) -> CacheKey {
        CacheKey {
            lease_epoch: 1,
            tip: 1,
            classification_generation: 0,
            object_id: format!("object-{index}"),
            source_revision: 1,
            artifact_digest: None,
            destination: ArtifactDestination::Local,
            project_scope_id: "project:x".to_string(),
        }
    }

    struct Fixture {
        _directory: tempfile::TempDir,
        store: KernelStore,
        coordinator: KernelOpenCoordinator,
        project: ProjectBinding,
    }

    fn fixture() -> Fixture {
        let directory = tempfile::tempdir().unwrap();
        let store = KernelStore::open(directory.path()).unwrap();
        let project = ProjectBinding::new(directory.path());
        Fixture {
            _directory: directory,
            store,
            coordinator: KernelOpenCoordinator::new(),
            project,
        }
    }

    fn candidates(count: usize) -> Vec<Candidate> {
        (0..count)
            .map(|index| Candidate {
                object_id: format!("object-{index}"),
                source_revision: 1,
                artifact_digest: None,
            })
            .collect()
    }

    /// An empty commit still appends to the commit log, so the tip moves.
    fn move_tip(store: &KernelStore) {
        store
            .commit(
                CommitIntent {
                    producer: "eligibility-test".to_string(),
                    operation_key: "move".to_string(),
                    request_digest: "0".repeat(64),
                    actor: "test".to_string(),
                    cause: "race".to_string(),
                },
                |_| Ok(String::new()),
            )
            .unwrap();
    }

    fn evaluate_recording(
        fixture: &Fixture,
        candidates: &[Candidate],
        move_on_first_read: bool,
    ) -> (BatchResponse, Vec<Vec<String>>) {
        let reads: RefCell<Vec<Vec<String>>> = RefCell::new(Vec::new());
        let response = evaluate_with(
            &fixture.store,
            &fixture.coordinator,
            &fixture.project,
            ArtifactDestination::Local,
            candidates,
            |requested| {
                let mut reads = reads.borrow_mut();
                reads.push(
                    requested
                        .iter()
                        .map(|candidate| candidate.object_id.clone())
                        .collect(),
                );
                if move_on_first_read && reads.len() == 1 {
                    move_tip(&fixture.store);
                }
                fixture.store.judge_eligibility(
                    fixture.project.scope(),
                    ArtifactDestination::Local,
                    requested,
                )
            },
        )
        .unwrap();
        (response, reads.into_inner())
    }

    #[test]
    fn the_cache_is_bounded_and_evicts_its_oldest_entry_first() {
        let mut cache = VerdictCache::default();
        for index in 0..=CACHE_CAPACITY {
            cache.insert(key(index), EligibilityVerdict::Ok);
        }
        assert_eq!(cache.len(), CACHE_CAPACITY);
        assert_eq!(cache.get(&key(0)), None);
        assert_eq!(
            cache.get(&key(CACHE_CAPACITY)),
            Some(EligibilityVerdict::Ok)
        );
        // Re-inserting an existing key replaces its verdict without growing the order.
        cache.insert(key(1), EligibilityVerdict::Stale);
        assert_eq!(cache.get(&key(1)), Some(EligibilityVerdict::Stale));
        assert_eq!(cache.order.len(), CACHE_CAPACITY);
        cache.clear();
        assert_eq!(cache.len(), 0);
    }

    #[test]
    fn only_the_misses_are_judged_when_the_snapshot_holds() {
        let fixture = fixture();
        let candidates = candidates(2);
        let warm = evaluate(
            &fixture.store,
            &fixture.coordinator,
            &fixture.project,
            ArtifactDestination::Local,
            &candidates[..1],
        )
        .unwrap();
        assert_eq!(warm.cache_hits, 0);

        let (response, reads) = evaluate_recording(&fixture, &candidates, false);
        assert_eq!(reads, vec![vec!["object-1".to_string()]]);
        assert_eq!(response.cache_hits, 1);
        assert_eq!(response.known_as_of, warm.known_as_of);
        assert_eq!(fixture.coordinator.eligibility_cache().len(), 2);
    }

    #[test]
    fn a_hit_from_an_older_snapshot_is_discarded_when_the_misses_see_a_newer_one() {
        let fixture = fixture();
        let candidates = candidates(2);
        let warm = evaluate(
            &fixture.store,
            &fixture.coordinator,
            &fixture.project,
            ArtifactDestination::Local,
            &candidates[..1],
        )
        .unwrap();

        let (response, reads) = evaluate_recording(&fixture, &candidates, true);
        // The miss saw the moved store, so the batch is judged again in full.
        assert_eq!(
            reads,
            vec![
                vec!["object-1".to_string()],
                vec!["object-0".to_string(), "object-1".to_string()],
            ]
        );
        assert_eq!(response.cache_hits, 0);
        assert_eq!(response.known_as_of, warm.known_as_of + 1);
        assert!(
            response
                .verdicts
                .iter()
                .all(|(_, verdict)| *verdict == Verdict::Retracted)
        );
        // The stale entry stays; both candidates are stored at the new tip.
        assert_eq!(fixture.coordinator.eligibility_cache().len(), 3);
        let (again, reads) = evaluate_recording(&fixture, &candidates, false);
        assert!(reads.is_empty());
        assert_eq!(again.cache_hits, 2);
        assert_eq!(again.known_as_of, response.known_as_of);
    }

    #[test]
    fn a_batch_with_no_hits_is_not_judged_again_when_the_snapshot_moves() {
        let fixture = fixture();
        let candidates = candidates(2);
        let before = fixture.store.egress_snapshot().unwrap();

        let (response, reads) = evaluate_recording(&fixture, &candidates, true);
        assert_eq!(
            reads,
            vec![vec!["object-0".to_string(), "object-1".to_string()]]
        );
        assert_eq!(response.cache_hits, 0);
        assert_eq!(response.known_as_of, before.tip + 1);
        assert_eq!(fixture.coordinator.eligibility_cache().len(), 2);
        let (again, reads) = evaluate_recording(&fixture, &candidates, false);
        assert!(reads.is_empty());
        assert_eq!(again.cache_hits, 2);
        assert_eq!(again.known_as_of, response.known_as_of);
    }
}
