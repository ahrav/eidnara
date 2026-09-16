mod support;

use std::collections::BTreeSet;
use std::fs;
use std::num::NonZeroUsize;
use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;

use daemon::projection_gates::{Admission, EntryPoint, HookGate, ProjectionHook};
use daemon::vector_composition::{
    COMPOSITION_FILE, Composition, CompositionRefusal, CompositionSpec, Progress, Reconciled,
    SelectorState, Unavailable, compose, publish, reconcile, recover, verify_composition,
};
use daemon::vector_generation::{
    CODES_FILE, ExpectedVectors, Staging, VerifiedVectors, build, stage, verify,
};
use host_runtime::generation::{
    CurrentProfile, GENERATIONS_DIR_NAME, GenerationError, GenerationStore, MEMBERS_FILE_NAME,
    ProfileEvent, SEARCH_PROFILE_NAME, SourceSpec, StageMeta, VECTOR_PROFILE_NAME,
    VECTOR_SELECTION_TARGET,
};
use host_runtime::lifecycle::LifecycleTransactionLock;
use retrieval::ProjectionIdentity;
use retrieval::batch::{ProjectionCheckpoint, VectorGeneration};
use retrieval::dense::codec::Metric;
use retrieval::dense::export::{ExportedRow, LiveRows};
use retrieval::dense::scalar::ScalarRecipe;
use sha2::Digest;
use support::projection_gate::{identity, passing_evaluator};

const DIMENSION: u32 = 8;
const TOLERANCE: f64 = 1e-3;
const KERNEL: &str = "test-incarnation";

fn generation() -> VectorGeneration {
    let identity = identity(KERNEL, DIMENSION);
    VectorGeneration {
        generation_id: "gen-vectors-1".to_owned(),
        embedding_model: identity.embedding_model,
        tokenizer_fingerprint: identity.tokenizer_fingerprint,
        vector_dimension: identity.vector_dimension,
        generation_epoch: identity.generation_epoch,
    }
}

fn unit(raw: [f32; 8]) -> Vec<f32> {
    let norm = raw
        .iter()
        .map(|value| f64::from(*value) * f64::from(*value))
        .sum::<f64>()
        .sqrt();
    raw.iter()
        .map(|value| (f64::from(*value) / norm) as f32)
        .collect()
}

fn rows(seed: u8) -> Vec<ExportedRow> {
    let mut rows: Vec<(String, Vec<f32>)> = (0..4u8)
        .map(|i| {
            let mut raw = [0.05f32; 8];
            raw[usize::from(i)] = 0.9;
            raw[7] = f32::from(seed) / 100.0;
            (format!("{:02x}", i + seed).repeat(32), unit(raw))
        })
        .collect();
    rows.sort_by(|a, b| a.0.cmp(&b.0));
    rows.into_iter()
        .map(|(occurrence_id, vector)| ExportedRow {
            occurrence_id,
            vector,
        })
        .collect()
}

fn export(seed: u8, checkpoint: i64) -> LiveRows {
    LiveRows {
        generation: generation(),
        kernel_incarnation_id: KERNEL.to_owned(),
        checkpoint: ProjectionCheckpoint {
            snapshot_commit_seq: checkpoint - 1,
            checkpoint_commit_seq: checkpoint,
            hold_id: "hold-7".to_owned(),
        },
        rows: rows(seed),
        tombstones: Vec::new(),
    }
}

struct Fixture {
    root: tempfile::TempDir,
    store: GenerationStore,
    tx: LifecycleTransactionLock,
    gate: HookGate,
    admission: Admission,
    identity: ProjectionIdentity,
    generation: VectorGeneration,
    protected: BTreeSet<String>,
    work_dirs: std::cell::Cell<usize>,
}

impl Fixture {
    fn new() -> Self {
        let root = tempfile::tempdir().unwrap();
        let store = GenerationStore::open(Some(root.path())).unwrap();
        let tx = LifecycleTransactionLock::acquire_exclusive(Some(root.path())).unwrap();
        let identity = identity(KERNEL, DIMENSION);
        let gate = HookGate::closed();
        gate.install(passing_evaluator(&identity, 0, &ProjectionHook::ALL));
        let admission = gate
            .admit(ProjectionHook::EmbeddingBootstrap, EntryPoint::Explicit)
            .unwrap();
        let generation = generation();
        Self {
            root,
            store,
            tx,
            gate,
            admission,
            identity,
            generation,
            protected: BTreeSet::new(),
            work_dirs: std::cell::Cell::new(0),
        }
    }

    fn expected(&self) -> ExpectedVectors<'_> {
        ExpectedVectors {
            generation: &self.generation,
            kernel_incarnation_id: &self.identity.kernel_incarnation_id,
            metric: Metric::InnerProduct,
            unit_norm_tolerance: TOLERANCE,
            recipe: ScalarRecipe::SymmetricInt8V1,
            checkpoint: None,
        }
    }

    fn work_dir(&self) -> PathBuf {
        let index = self.work_dirs.get();
        self.work_dirs.set(index + 1);
        let dir = self.root.path().join(format!("work-{index}"));
        fs::create_dir(&dir).unwrap();
        dir
    }

    fn staging(&self) -> Staging<'_> {
        Staging {
            store: &self.store,
            transaction: &self.tx,
            gate: &self.gate,
            admission: &self.admission,
            identity: &self.identity,
            protected: &self.protected,
        }
    }

    /// Builds, stages, and verifies one layer.
    fn layer(&self, seed: u8, checkpoint: i64) -> VerifiedVectors {
        self.layer_masking(seed, checkpoint, &[])
    }

    fn layer_masking(&self, seed: u8, checkpoint: i64, tombstones: &[String]) -> VerifiedVectors {
        let mut export = export(seed, checkpoint);
        export.tombstones = tombstones.to_vec();
        let built = build(&self.expected(), &export, &self.work_dir()).unwrap();
        let digest = stage(&built, &self.staging()).unwrap();
        verify(&self.store, &digest, &self.expected()).unwrap()
    }

    fn compose(
        &self,
        sequence: u64,
        base: &VerifiedVectors,
        deltas: &[VerifiedVectors],
    ) -> Result<Composition, CompositionRefusal> {
        let expected = self.expected();
        compose(&CompositionSpec {
            expected: &expected,
            sequence,
            base,
            deltas,
            max_deltas: NonZeroUsize::new(4).unwrap(),
        })
    }

    fn publish(&self, composition: &Composition) -> Result<String, CompositionRefusal> {
        publish(composition, &self.staging(), &self.work_dir(), &mut |_| {
            Ok(())
        })
        .map_err(|failure| failure.refusal)
    }

    fn verify_composition(&self, digest: &str) -> Result<Vec<String>, CompositionRefusal> {
        verify_composition(
            &self.store,
            digest,
            &self.expected(),
            NonZeroUsize::new(4).unwrap(),
        )
        .map(|verified| verified.composition.members())
    }

    fn lifecycle_dir(&self) -> PathBuf {
        self.root.path().join("eidnara").join("lifecycle")
    }

    fn generation_dir(&self, digest: &str) -> PathBuf {
        self.lifecycle_dir().join(GENERATIONS_DIR_NAME).join(digest)
    }

    fn generations(&self) -> BTreeSet<String> {
        fs::read_dir(self.lifecycle_dir().join(GENERATIONS_DIR_NAME))
            .unwrap()
            .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
            .collect()
    }

    fn selector_bytes(&self, name: &str) -> Option<Vec<u8>> {
        fs::read(self.lifecycle_dir().join(name)).ok()
    }

    fn write_selector(&self, name: &str, bytes: &[u8]) {
        let path = self.lifecycle_dir().join(name);
        let _ = fs::remove_file(&path);
        fs::write(&path, bytes).unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).unwrap();
    }

    fn corrupt(&self, digest: &str, file: &str) {
        let path = self.generation_dir(digest).join(file);
        let mut bytes = fs::read(&path).unwrap();
        bytes[0] ^= 0xff;
        fs::write(&path, bytes).unwrap();
    }

    fn stage_search_seed(&self) -> String {
        let dir = self.work_dir();
        let path = dir.join("search.sqlite");
        fs::write(&path, b"not really a database").unwrap();
        let meta = StageMeta {
            target: "search-projection-seed".to_owned(),
            release_contract_sha256: "a".repeat(64),
            inputs_lock_sha256: "b".repeat(64),
            source_payload_manifest_sha256: "c".repeat(64),
        };
        self.store
            .stage(
                &[SourceSpec {
                    rel_path: "search.sqlite".to_owned(),
                    source: path,
                    executable: false,
                    expected_size: None,
                    expected_sha256: None,
                }],
                &meta,
                &BTreeSet::new(),
            )
            .unwrap()
    }

    fn recover(&self) -> Result<(String, SelectorState), Unavailable> {
        recover(
            &self.store,
            &self.tx,
            &self.expected(),
            NonZeroUsize::new(4).unwrap(),
            NonZeroUsize::new(8).unwrap(),
        )
        .map(|recovered| (recovered.composition.digest, recovered.selector))
    }

    /// Stages a record another build or a foreign stager could have written, bypassing `publish`'s canonical encoding.
    fn stage_record(&self, record: &[u8], template: &Composition) -> String {
        let dir = self.work_dir();
        fs::write(dir.join(COMPOSITION_FILE), record).unwrap();
        fs::write(
            dir.join(MEMBERS_FILE_NAME),
            serde_json::to_vec(&host_runtime::generation::WireMembers {
                schema: 1,
                members: template.members(),
            })
            .unwrap(),
        )
        .unwrap();
        let sources: Vec<SourceSpec> = [COMPOSITION_FILE, MEMBERS_FILE_NAME]
            .into_iter()
            .map(|name| SourceSpec {
                rel_path: name.to_owned(),
                source: dir.join(name),
                executable: false,
                expected_size: None,
                expected_sha256: None,
            })
            .collect();
        self.store
            .stage(&sources, &template.stage_meta(), &BTreeSet::new())
            .unwrap()
    }
}

#[test]
fn a_base_publishes_as_one_complete_selection_whose_members_stay_protected_without_readers() {
    let fixture = Fixture::new();
    let seed = fixture.stage_search_seed();
    fixture
        .store
        .select_search(&seed, &fixture.tx, &mut |_| Ok(()))
        .unwrap();
    let search_before = fixture.selector_bytes(SEARCH_PROFILE_NAME).unwrap();

    let base = fixture.layer(1, 10);
    let base_digest = base.digest.clone();
    let composition = fixture.compose(1, &base, &[]).unwrap();
    assert_eq!(composition.members(), vec![base_digest.clone()]);
    drop(base);

    let published = fixture.publish(&composition).unwrap();
    assert_eq!(published, composition.digest());
    assert_eq!(
        fixture.store.read_vector_current().unwrap(),
        CurrentProfile::Current(composition.digest())
    );
    assert_eq!(
        fixture.selector_bytes(SEARCH_PROFILE_NAME).unwrap(),
        search_before,
        "the search selector is byte-for-byte unchanged"
    );
    assert_eq!(
        fixture.store.read_current().unwrap(),
        CurrentProfile::Absent
    );

    // No reader pins anything, yet prune keeps the composition and its member.
    let report = fixture.store.prune(&BTreeSet::new()).unwrap();
    assert_eq!(report.removed_generations, 0);
    assert!(fixture.generations().contains(&base_digest));
    assert!(fixture.generations().contains(&composition.digest()));
    let base_manifest = verify(&fixture.store, &base_digest, &fixture.expected())
        .unwrap()
        .sidecar
        .stage_manifest();
    assert!(
        fixture
            .store
            .discard_unselected(&base_manifest, &fixture.tx, &BTreeSet::new())
            .is_err(),
        "a selected composition's member cannot be discarded"
    );

    // An identical retry is refused by sequence, not by a second record: the selection already names this composition.
    assert_eq!(
        fixture.publish(&composition).unwrap_err(),
        CompositionRefusal::Sequence {
            sequence: 1,
            selected: 1
        }
    );
    assert_eq!(fixture.generations().len(), 3);

    let (digest, selector) = fixture.recover().unwrap();
    assert_eq!(digest, composition.digest());
    assert_eq!(selector, SelectorState::Current);
}

#[test]
fn a_delta_update_publishes_a_new_complete_target_and_releases_the_old_composition_only() {
    let fixture = Fixture::new();
    let base = fixture.layer(1, 10);
    let first = fixture.compose(1, &base, &[]).unwrap();
    fixture.publish(&first).unwrap();

    let delta = fixture.layer(5, 12);
    let second = fixture
        .compose(2, &base, std::slice::from_ref(&delta))
        .unwrap();
    assert_eq!(
        second.members(),
        vec![base.digest.clone(), delta.digest.clone()]
    );
    fixture.publish(&second).unwrap();
    assert_eq!(
        fixture.store.read_vector_current().unwrap(),
        CurrentProfile::Current(second.digest())
    );

    let report = fixture.store.prune(&BTreeSet::new()).unwrap();
    assert_eq!(
        report.removed_generations, 1,
        "only the superseded composition record is reclaimable"
    );
    let remaining = fixture.generations();
    assert!(remaining.contains(&base.digest));
    assert!(remaining.contains(&delta.digest));
    assert!(remaining.contains(&second.digest()));
    assert!(!remaining.contains(&first.digest()));

    assert_eq!(
        fixture.verify_composition(&second.digest()).unwrap(),
        vec![base.digest.clone(), delta.digest.clone()]
    );
    // A stale sequence can no longer be published over the selection.
    assert_eq!(
        fixture.publish(&first).unwrap_err(),
        CompositionRefusal::Sequence {
            sequence: 1,
            selected: 2
        }
    );
}

#[test]
fn topology_and_identity_checks_refuse_a_composition_before_anything_is_staged() {
    let fixture = Fixture::new();
    let base = fixture.layer(1, 10);
    let delta = fixture.layer(5, 12);
    let before = fixture.generations();

    let earlier = fixture.layer(6, 9);
    assert_eq!(
        fixture.compose(1, &base, &[earlier]).unwrap_err(),
        CompositionRefusal::CheckpointOrder { index: 0 }
    );
    let duplicate = verify(&fixture.store, &delta.digest, &fixture.expected()).unwrap();
    assert_eq!(
        fixture.compose(1, &base, &[delta, duplicate]).unwrap_err(),
        CompositionRefusal::DuplicateMember { index: 1 }
    );
    let many: Vec<VerifiedVectors> = (0..5u8)
        .map(|i| fixture.layer(20 + i, 20 + i64::from(i)))
        .collect();
    assert_eq!(
        fixture.compose(1, &base, &many).unwrap_err(),
        CompositionRefusal::DeltasOverBound { count: 5, max: 4 }
    );
    let masking = fixture.layer_masking(7, 13, &["ff".repeat(32)]);
    assert_eq!(masking.sidecar.tombstones, 1);
    assert_eq!(
        fixture.compose(1, &masking, &[]).unwrap_err(),
        CompositionRefusal::BaseTombstones { tombstones: 1 }
    );
    assert!(
        fixture.compose(1, &base, &[masking]).is_ok(),
        "a delta may mask"
    );

    let mut other = fixture.generation.clone();
    other.embedding_model = "another-model".to_owned();
    let foreign = ExpectedVectors {
        generation: &other,
        ..fixture.expected()
    };
    assert_eq!(
        compose(&CompositionSpec {
            expected: &foreign,
            sequence: 1,
            base: &base,
            deltas: &[],
            max_deltas: NonZeroUsize::new(1).unwrap(),
        })
        .unwrap_err(),
        CompositionRefusal::MemberIdentity {
            digest: base.digest.clone(),
            field: "embedding_model"
        }
    );
    assert_eq!(
        fixture.generations().len(),
        before.len() + 7,
        "only the layers themselves were staged; no composition record exists"
    );
    assert_eq!(
        fixture.store.read_vector_current().unwrap(),
        CurrentProfile::Absent
    );
}

#[test]
fn a_composition_with_a_missing_or_unverified_member_is_never_selected() {
    let fixture = Fixture::new();
    let base = fixture.layer(1, 10);
    let composition = fixture.compose(1, &base, &[]).unwrap();
    // The member is reclaimed between composing and publishing.
    let member_manifest = base.sidecar.stage_manifest();
    drop(base);
    fixture
        .store
        .discard_unselected(&member_manifest, &fixture.tx, &BTreeSet::new())
        .unwrap();
    let failure = publish(
        &composition,
        &fixture.staging(),
        &fixture.work_dir(),
        &mut |_| Ok(()),
    )
    .unwrap_err();
    assert_eq!(failure.progress, Progress::Staged);
    assert!(matches!(failure.refusal, CompositionRefusal::Store(_)));
    assert_eq!(
        fixture.store.read_vector_current().unwrap(),
        CurrentProfile::Absent
    );
    assert_eq!(
        reconcile(&fixture.store, &fixture.tx, &composition.digest()).unwrap(),
        Reconciled::Other(None)
    );

    // A composition whose member is present but rehashed to another meaning verifies as a store object and fails composition verification.
    let fixture = Fixture::new();
    let base = fixture.layer(1, 10);
    let composition = fixture.compose(1, &base, &[]).unwrap();
    fixture.publish(&composition).unwrap();
    fixture.corrupt(&base.digest, CODES_FILE);
    let refused = fixture
        .verify_composition(&composition.digest())
        .unwrap_err();
    assert!(
        matches!(refused, CompositionRefusal::Member { .. }),
        "{refused:?}"
    );
    assert_eq!(
        fixture.recover().unwrap_err(),
        Unavailable::NoCompatibleTarget { examined: 1 }
    );
    // The store still retains the selected composition's member: its record is readable, so its members are known.
    assert!(fixture.store.prune(&BTreeSet::new()).is_ok());
    assert!(fixture.generations().contains(&base.digest));
}

#[test]
fn every_selector_cut_leaves_the_old_or_the_new_complete_selection_and_reconciles_by_digest() {
    for cut in [
        ProfileEvent::BeforeRename,
        ProfileEvent::AfterRename,
        ProfileEvent::BeforeDirectorySync,
        ProfileEvent::AfterDirectorySync,
    ] {
        let fixture = Fixture::new();
        let base = fixture.layer(1, 10);
        let old = fixture.compose(1, &base, &[]).unwrap();
        fixture.publish(&old).unwrap();
        let old_bytes = fixture.selector_bytes(VECTOR_PROFILE_NAME).unwrap();

        let delta = fixture.layer(5, 12);
        let new = fixture.compose(2, &base, &[delta]).unwrap();
        let mut seen = Vec::new();
        let failure = publish(
            &new,
            &fixture.staging(),
            &fixture.work_dir(),
            &mut |event| {
                seen.push(event);
                if event == cut {
                    Err(GenerationError::NativePayloadInvalid { detail: "cut" })
                } else {
                    Ok(())
                }
            },
        )
        .unwrap_err();
        assert_eq!(*seen.last().unwrap(), cut);
        let expected_progress = match cut {
            ProfileEvent::BeforeRename => Progress::Staged,
            ProfileEvent::AfterRename | ProfileEvent::BeforeDirectorySync => Progress::Acknowledged,
            ProfileEvent::AfterDirectorySync => Progress::Durable,
        };
        assert_eq!(failure.progress, expected_progress, "{cut:?}");

        let (expected_current, expected_reconciled) = if cut == ProfileEvent::BeforeRename {
            (old.digest(), Reconciled::Other(Some(old.digest())))
        } else {
            (new.digest(), Reconciled::Published)
        };
        assert_eq!(
            fixture.store.read_vector_current().unwrap(),
            CurrentProfile::Current(expected_current.clone()),
            "{cut:?}: the selection is the old complete one or the new complete one"
        );
        assert_eq!(
            reconcile(&fixture.store, &fixture.tx, &new.digest()).unwrap(),
            expected_reconciled,
            "{cut:?}"
        );
        if cut == ProfileEvent::BeforeRename {
            assert_eq!(
                fixture.selector_bytes(VECTOR_PROFILE_NAME).unwrap(),
                old_bytes
            );
        }
        // Both compositions are complete and verified; recovery names the selected one and reports it current.
        let (recovered, selector) = fixture.recover().unwrap();
        assert_eq!(recovered, expected_current, "{cut:?}");
        assert_eq!(selector, SelectorState::Current, "{cut:?}");

        // A retry of the same publication converges on the new selection without a second composition record;
        // once the new selection is acknowledged, the retry is refused by sequence and changes nothing.
        let records_before = fixture.generations().len();
        match fixture.publish(&new) {
            Ok(retry) => assert_eq!(retry, new.digest(), "{cut:?}"),
            Err(CompositionRefusal::Sequence {
                sequence: 2,
                selected: 2,
            }) => {
                assert_ne!(cut, ProfileEvent::BeforeRename, "{cut:?}");
            }
            Err(other) => panic!("{cut:?}: {other:?}"),
        }
        assert_eq!(fixture.generations().len(), records_before, "{cut:?}");
        assert_eq!(
            fixture.store.read_vector_current().unwrap(),
            CurrentProfile::Current(new.digest())
        );
        assert_eq!(
            fixture.verify_composition(&new.digest()).unwrap(),
            new.members()
        );
    }
}

#[test]
fn recovery_takes_the_newest_verified_composition_and_reports_a_stale_or_absent_selector() {
    let fixture = Fixture::new();
    assert_eq!(
        fixture.recover().unwrap_err(),
        Unavailable::NoCompatibleTarget { examined: 0 }
    );

    let base = fixture.layer(1, 10);
    let first = fixture.compose(1, &base, &[]).unwrap();
    fixture.publish(&first).unwrap();
    let delta = fixture.layer(5, 12);
    let second = fixture.compose(2, &base, &[delta]).unwrap();
    fixture.publish(&second).unwrap();

    // The selector names the older composition, as a cut before the rename of a later publication would leave it:
    // the acknowledged selection stands; the newer staged composition does not displace it.
    fixture
        .store
        .select_vector(&first.digest(), &fixture.tx, &mut |_| Ok(()))
        .unwrap();
    let (digest, selector) = fixture.recover().unwrap();
    assert_eq!(digest, first.digest());
    assert_eq!(selector, SelectorState::Current);

    // The selected composition is corrupt: recovery falls back to the newest other verified one and says the selector is stale.
    fixture
        .store
        .select_vector(&second.digest(), &fixture.tx, &mut |_| Ok(()))
        .unwrap();
    fixture.corrupt(&second.digest(), COMPOSITION_FILE);
    let (digest, selector) = fixture.recover().unwrap();
    assert_eq!(digest, first.digest());
    assert_eq!(selector, SelectorState::Stale(second.digest()));
    assert_eq!(
        fixture.store.read_vector_current().unwrap(),
        CurrentProfile::Current(second.digest()),
        "recovery reports; it does not repoint"
    );

    // No selector at all: the newest verified composition is still found.
    fs::remove_file(fixture.lifecycle_dir().join(VECTOR_PROFILE_NAME)).unwrap();
    let (digest, selector) = fixture.recover().unwrap();
    assert_eq!(digest, first.digest());
    assert_eq!(selector, SelectorState::Absent);

    // A quarantined selector stops recovery and every mutation.
    fixture.write_selector(VECTOR_PROFILE_NAME, br#"{"schema":9,"current":"zz"}"#);
    assert_eq!(
        fixture.recover().unwrap_err(),
        Unavailable::QuarantinedSelector
    );
    assert!(matches!(
        fixture.publish(&first).unwrap_err(),
        CompositionRefusal::Quarantined
    ));
    assert_eq!(
        reconcile(&fixture.store, &fixture.tx, &first.digest()).unwrap(),
        Reconciled::Quarantined
    );

    // Under another expectation nothing verifies.
    fs::remove_file(fixture.lifecycle_dir().join(VECTOR_PROFILE_NAME)).unwrap();
    let mut other = fixture.generation.clone();
    other.generation_epoch = 2;
    let foreign = ExpectedVectors {
        generation: &other,
        ..fixture.expected()
    };
    assert_eq!(
        recover(
            &fixture.store,
            &fixture.tx,
            &foreign,
            NonZeroUsize::new(4).unwrap(),
            NonZeroUsize::new(8).unwrap()
        )
        .unwrap_err(),
        Unavailable::NoCompatibleTarget { examined: 1 }
    );
    // With no selector the corrupt record is not a candidate, so a bound of one still reaches the verified one.
    assert!(
        recover(
            &fixture.store,
            &fixture.tx,
            &fixture.expected(),
            NonZeroUsize::new(4).unwrap(),
            NonZeroUsize::new(1).unwrap()
        )
        .is_ok()
    );
    // A delta bound below the recovered composition's deltas refuses it: admission limits hold through recovery.
    assert_eq!(
        recover(
            &fixture.store,
            &fixture.tx,
            &fixture.expected(),
            NonZeroUsize::new(1).unwrap(),
            NonZeroUsize::new(8).unwrap()
        )
        .map(|r| r.composition.digest),
        Ok(first.digest()),
        "the base-only composition still verifies under a one-delta bound"
    );
}

#[test]
fn recovery_counts_full_validation_failures_against_its_candidate_bound() {
    let fixture = Fixture::new();
    let base = fixture.layer(1, 10);
    let first = fixture.compose(1, &base, &[]).unwrap();
    fixture.publish(&first).unwrap();
    let second = fixture.compose(2, &base, &[]).unwrap();
    fixture.publish(&second).unwrap();
    fs::remove_file(fixture.lifecycle_dir().join(VECTOR_PROFILE_NAME)).unwrap();
    fs::write(
        fixture.generation_dir(&second.digest()).join("unlisted"),
        b"bad",
    )
    .unwrap();

    assert_eq!(
        recover(
            &fixture.store,
            &fixture.tx,
            &fixture.expected(),
            NonZeroUsize::new(4).unwrap(),
            NonZeroUsize::new(1).unwrap(),
        )
        .unwrap_err(),
        Unavailable::NoCompatibleTarget { examined: 1 },
        "inventory validation belongs inside the bound, not discovery"
    );
    assert_eq!(
        fixture.recover().unwrap(),
        (first.digest(), SelectorState::Absent)
    );
}

#[test]
fn recovery_does_not_retain_discovery_descriptors() {
    const CHILD: &str = "EIDNARA_COMPOSITION_FD_BOUND_CHILD";
    if std::env::var_os(CHILD).is_none() {
        let output = std::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "recovery_does_not_retain_discovery_descriptors",
                "--nocapture",
            ])
            .env(CHILD, "1")
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        return;
    }
    // Only this child lowers its FD limit; parallel tests keep the parent's limits.
    rustix::process::setrlimit(
        rustix::process::Resource::Nofile,
        rustix::process::Rlimit {
            current: Some(64),
            maximum: Some(64),
        },
    )
    .unwrap();
    let fixture = Fixture::new();
    let base = fixture.layer(1, 10);
    let mut newest = String::new();
    for sequence in 1..=80 {
        newest = fixture
            .publish(&fixture.compose(sequence, &base, &[]).unwrap())
            .unwrap();
    }
    fs::remove_file(fixture.lifecycle_dir().join(VECTOR_PROFILE_NAME)).unwrap();
    let recovered = recover(
        &fixture.store,
        &fixture.tx,
        &fixture.expected(),
        NonZeroUsize::new(4).unwrap(),
        NonZeroUsize::new(1).unwrap(),
    )
    .unwrap();
    assert_eq!(recovered.composition.digest, newest);
    assert_eq!(recovered.selector, SelectorState::Absent);
}

#[test]
fn the_vector_selector_refuses_layers_and_other_owners_and_a_composition_record_binds_its_members()
{
    let fixture = Fixture::new();
    let seed = fixture.stage_search_seed();
    assert!(
        fixture
            .store
            .select_vector(&seed, &fixture.tx, &mut |_| Ok(()))
            .is_err()
    );
    let base = fixture.layer(1, 10);
    assert!(
        fixture
            .store
            .select_vector(&base.digest, &fixture.tx, &mut |_| Ok(()))
            .is_err(),
        "a layer is not a composition"
    );
    assert_eq!(
        fixture.store.read_vector_current().unwrap(),
        CurrentProfile::Absent
    );

    let composition = fixture.compose(1, &base, &[]).unwrap();
    fixture.publish(&composition).unwrap();
    let dir = fixture.generation_dir(&composition.digest());
    let manifest_target: serde_json::Value =
        serde_json::from_slice(&fs::read(dir.join("manifest.json")).unwrap()).unwrap();
    assert_eq!(manifest_target["target"], VECTOR_SELECTION_TARGET);
    let members: serde_json::Value =
        serde_json::from_slice(&fs::read(dir.join(MEMBERS_FILE_NAME)).unwrap()).unwrap();
    assert_eq!(members["members"], serde_json::json!([base.digest]));

    // A record whose members file disagrees with its record is not a composition, even with every hash rewritten.
    let mut forged = composition.clone();
    forged.deltas.push("ab".repeat(32));
    let forged_dir = fixture.work_dir();
    fs::write(forged_dir.join(COMPOSITION_FILE), forged.canonical_bytes()).unwrap();
    fs::write(
        forged_dir.join(MEMBERS_FILE_NAME),
        serde_json::to_vec(&host_runtime::generation::WireMembers {
            schema: 1,
            members: vec![base.digest.clone()],
        })
        .unwrap(),
    )
    .unwrap();
    let mut manifest = forged.stage_manifest();
    let members_bytes = fs::read(forged_dir.join(MEMBERS_FILE_NAME)).unwrap();
    for file in &mut manifest.files {
        if file.path == MEMBERS_FILE_NAME {
            file.size = members_bytes.len() as u64;
            file.sha256 = format!("{:x}", sha2::Sha256::digest(&members_bytes));
        }
    }
    let sources: Vec<SourceSpec> = manifest
        .files
        .iter()
        .map(|file| SourceSpec {
            rel_path: file.path.clone(),
            source: forged_dir.join(&file.path),
            executable: false,
            expected_size: Some(file.size),
            expected_sha256: Some(file.sha256.clone()),
        })
        .collect();
    let forged_digest = fixture
        .store
        .stage(&sources, &forged.stage_meta(), &BTreeSet::new())
        .unwrap();
    assert!(fixture.store.validate(&forged_digest).is_ok());
    assert_eq!(
        fixture.verify_composition(&forged_digest).unwrap_err(),
        CompositionRefusal::NotComposition("manifest binding")
    );
}

#[test]
fn a_reader_pinning_a_superseded_composition_keeps_its_members_through_prune() {
    let fixture = Fixture::new();
    let base = fixture.layer(1, 10);
    let first = fixture.compose(1, &base, &[]).unwrap();
    fixture.publish(&first).unwrap();
    // The reader pins the composition it opened, as the daemon's handoff will, before the selector moves on.
    let pinned = fixture.store.validate(&first.digest()).unwrap();
    pinned.pin().unwrap();

    let other_base = fixture.layer(7, 11);
    let second = fixture.compose(2, &other_base, &[]).unwrap();
    fixture.publish(&second).unwrap();

    let report = fixture.store.prune(&BTreeSet::new()).unwrap();
    assert_eq!(
        report.removed_generations, 0,
        "the pinned composition and its member both stay"
    );
    assert!(fixture.generations().contains(&base.digest));
    assert!(
        fixture
            .store
            .discard_unselected(
                &base.sidecar.stage_manifest(),
                &fixture.tx,
                &BTreeSet::new()
            )
            .is_err(),
        "a member of a pinned composition cannot be discarded"
    );
    drop(pinned);
    // A member outlives every record that names it by one pass: the record goes first, the member next.
    let report = fixture.store.prune(&BTreeSet::new()).unwrap();
    assert_eq!(
        report.removed_generations, 1,
        "released, the old composition record is reclaimed"
    );
    assert!(fixture.generations().contains(&base.digest));
    let report = fixture.store.prune(&BTreeSet::new()).unwrap();
    assert_eq!(
        report.removed_generations, 1,
        "no record names the old member any more"
    );
    assert!(!fixture.generations().contains(&base.digest));
    assert!(fixture.generations().contains(&other_base.digest));
}

#[test]
fn a_pinned_composition_whose_members_file_is_unreadable_quarantines_pruning() {
    let fixture = Fixture::new();
    let base = fixture.layer(1, 10);
    let first = fixture.compose(1, &base, &[]).unwrap();
    fixture.publish(&first).unwrap();
    let pinned = fixture.store.validate(&first.digest()).unwrap();
    pinned.pin().unwrap();
    let other_base = fixture.layer(7, 11);
    let second = fixture.compose(2, &other_base, &[]).unwrap();
    fixture.publish(&second).unwrap();

    // The pinned record is retained but its members cannot be read, so nothing may be reclaimed.
    fixture.corrupt(&first.digest(), MEMBERS_FILE_NAME);
    let report = fixture.store.prune(&BTreeSet::new()).unwrap();
    assert_eq!(report.removed_generations, 0);
    assert!(report.quarantined >= 1);
    assert!(fixture.generations().contains(&base.digest));
    assert!(matches!(
        fixture.store.discard_unselected(
            &base.sidecar.stage_manifest(),
            &fixture.tx,
            &BTreeSet::new()
        ),
        Err(GenerationError::UnsupportedStateSchema)
    ));
}

#[test]
fn exchange_repair_refuses_a_corrupt_member_of_a_pinned_composition() {
    let fixture = Fixture::new();
    let base = fixture.layer(1, 10);
    let first = fixture.compose(1, &base, &[]).unwrap();
    fixture.publish(&first).unwrap();
    let pinned = fixture.store.validate(&first.digest()).unwrap();
    pinned.pin().unwrap();
    let other_base = fixture.layer(7, 11);
    let second = fixture.compose(2, &other_base, &[]).unwrap();
    fixture.publish(&second).unwrap();

    fs::write(fixture.generation_dir(&base.digest).join("extra"), b"x").unwrap();
    assert!(fixture.store.validate(&base.digest).is_err());
    let rebuilt = build(&fixture.expected(), &export(1, 10), &fixture.work_dir()).unwrap();
    assert_eq!(rebuilt.digest(), base.digest);
    assert!(
        stage(&rebuilt, &fixture.staging()).is_err(),
        "a member of a pinned composition is not exchanged under its reader"
    );
    assert!(
        fixture.generation_dir(&base.digest).join("extra").exists(),
        "the occupant is left as it is"
    );
    drop(pinned);
    fixture.store.prune(&BTreeSet::new()).unwrap();
    let rebuilt = build(&fixture.expected(), &export(1, 10), &fixture.work_dir()).unwrap();
    assert_eq!(stage(&rebuilt, &fixture.staging()).unwrap(), base.digest);
}

#[test]
fn an_unreadable_selected_composition_quarantines_pruning_while_a_corrupt_member_does_not() {
    let fixture = Fixture::new();
    let base = fixture.layer(1, 10);
    let composition = fixture.compose(1, &base, &[]).unwrap();
    fixture.publish(&composition).unwrap();
    let orphan = fixture.layer(9, 30);

    // A corrupt member: the record still names it, so prune knows what to keep and reclaims only the orphan.
    fixture.corrupt(&base.digest, CODES_FILE);
    let report = fixture.store.prune(&BTreeSet::new()).unwrap();
    assert_eq!(report.removed_generations, 1);
    assert!(fixture.generations().contains(&base.digest));
    assert!(!fixture.generations().contains(&orphan.digest));
    // Exchange repair refuses to replace the selected record even when a same-digest stager arrives.
    fixture.corrupt(&composition.digest(), COMPOSITION_FILE);
    let failure = publish(
        &composition,
        &fixture.staging(),
        &fixture.work_dir(),
        &mut |_| Ok(()),
    )
    .unwrap_err();
    assert_eq!(failure.progress, Progress::NotStaged);
    assert!(
        matches!(
            failure.refusal,
            CompositionRefusal::Sequence { .. } | CompositionRefusal::Stage(_)
        ),
        "{failure:?}"
    );

    // A torn record manifest: the members are unknown, so prune quarantines and removes temps only.
    let dir = fixture.generation_dir(&composition.digest());
    let manifest = fs::read(dir.join("manifest.json")).unwrap();
    fs::write(dir.join("manifest.json"), &manifest[..manifest.len() / 2]).unwrap();
    let temp = fixture
        .lifecycle_dir()
        .join(GENERATIONS_DIR_NAME)
        .join("tmp-0123456789abcdef");
    fs::create_dir(&temp).unwrap();
    let another = fixture.layer(11, 31);
    let report = fixture.store.prune(&BTreeSet::new()).unwrap();
    assert_eq!(report.removed_temps, 1);
    assert_eq!(report.removed_generations, 0);
    assert!(report.quarantined >= 1);
    assert!(fixture.generations().contains(&another.digest));
    assert!(matches!(
        fixture.store.discard_unselected(
            &another.sidecar.stage_manifest(),
            &fixture.tx,
            &BTreeSet::new()
        ),
        Err(GenerationError::UnsupportedStateSchema)
    ));
    assert_eq!(
        fixture.recover().unwrap_err(),
        Unavailable::NoCompatibleTarget { examined: 1 }
    );
}

#[test]
fn a_rename_lost_before_its_sync_is_read_back_as_the_old_selection_and_the_retry_republishes() {
    let fixture = Fixture::new();
    let base = fixture.layer(1, 10);
    let old = fixture.compose(1, &base, &[]).unwrap();
    fixture.publish(&old).unwrap();
    let old_bytes = fixture.selector_bytes(VECTOR_PROFILE_NAME).unwrap();
    let new = fixture.compose(2, &base, &[fixture.layer(5, 12)]).unwrap();
    let failure = publish(
        &new,
        &fixture.staging(),
        &fixture.work_dir(),
        &mut |event| {
            if event == ProfileEvent::BeforeDirectorySync {
                Err(GenerationError::NativePayloadInvalid { detail: "cut" })
            } else {
                Ok(())
            }
        },
    )
    .unwrap_err();
    assert_eq!(failure.progress, Progress::Acknowledged);
    // The rename was acknowledged but never synced: a crash here can leave the old selector on disk.
    fixture.write_selector(VECTOR_PROFILE_NAME, &old_bytes);
    assert_eq!(
        reconcile(&fixture.store, &fixture.tx, &new.digest()).unwrap(),
        Reconciled::Other(Some(old.digest())),
        "an acknowledged rename is not proof of durability"
    );
    let reopened = GenerationStore::open(Some(fixture.root.path())).unwrap();
    let recovered = recover(
        &reopened,
        &fixture.tx,
        &fixture.expected(),
        NonZeroUsize::new(4).unwrap(),
        NonZeroUsize::new(8).unwrap(),
    )
    .unwrap();
    assert_eq!(recovered.composition.digest, old.digest());
    assert_eq!(recovered.selector, SelectorState::Current);
    // The retry republishes the staged record without a second copy; the old selector's bytes are gone.
    let records = fixture.generations().len();
    assert_eq!(fixture.publish(&new).unwrap(), new.digest());
    assert_eq!(fixture.generations().len(), records);
    assert_ne!(
        fixture.selector_bytes(VECTOR_PROFILE_NAME).unwrap(),
        old_bytes
    );
    let (digest, selector) = fixture.recover().unwrap();
    assert_eq!((digest, selector), (new.digest(), SelectorState::Current));
    assert_eq!(
        fixture.recover().unwrap().0,
        new.digest(),
        "recovery is repeatable"
    );
}

#[test]
fn a_competing_publication_with_the_same_sequence_and_other_members_is_refused_before_staging() {
    let fixture = Fixture::new();
    let base = fixture.layer(1, 10);
    let a = fixture.compose(2, &base, &[fixture.layer(5, 12)]).unwrap();
    fixture.publish(&a).unwrap();
    let records = fixture.generations().len();
    let b = fixture.compose(2, &base, &[fixture.layer(6, 13)]).unwrap();
    assert_ne!(a.digest(), b.digest());
    let failure =
        publish(&b, &fixture.staging(), &fixture.work_dir(), &mut |_| Ok(())).unwrap_err();
    assert_eq!(failure.progress, Progress::NotStaged);
    assert_eq!(
        failure.refusal,
        CompositionRefusal::Sequence {
            sequence: 2,
            selected: 2
        }
    );
    assert_eq!(
        fixture.generations().len(),
        records + 1,
        "only the delta layer was staged"
    );
    assert_eq!(
        fixture.verify_composition(&a.digest()).unwrap(),
        a.members()
    );
    assert_eq!(
        fixture.store.read_vector_current().unwrap(),
        CurrentProfile::Current(a.digest())
    );
}

#[test]
fn equal_sequences_without_a_selector_recover_deterministically_and_a_selector_naming_an_absent_member_is_refused()
 {
    let fixture = Fixture::new();
    let base = fixture.layer(1, 10);
    let first = fixture.compose(1, &base, &[]).unwrap();
    fixture.publish(&first).unwrap();
    fs::remove_file(fixture.lifecycle_dir().join(VECTOR_PROFILE_NAME)).unwrap();
    let second = fixture.compose(1, &base, &[fixture.layer(5, 12)]).unwrap();
    fixture.publish(&second).unwrap();
    fs::remove_file(fixture.lifecycle_dir().join(VECTOR_PROFILE_NAME)).unwrap();
    let expected = std::cmp::max(first.digest(), second.digest());
    let (digest, selector) = fixture.recover().unwrap();
    assert_eq!(
        (digest.clone(), selector),
        (expected, SelectorState::Absent)
    );
    assert_eq!(fixture.recover().unwrap().0, digest);

    // A composition whose member was removed from the store cannot be selected, whatever its record says.
    let orphaned = fixture.compose(3, &fixture.layer(7, 20), &[]).unwrap();
    let orphaned_base = orphaned.base.clone();
    let orphaned_manifest = verify(&fixture.store, &orphaned_base, &fixture.expected())
        .unwrap()
        .sidecar
        .stage_manifest();
    fixture
        .store
        .discard_unselected(&orphaned_manifest, &fixture.tx, &BTreeSet::new())
        .unwrap();
    let failure = publish(
        &orphaned,
        &fixture.staging(),
        &fixture.work_dir(),
        &mut |_| Ok(()),
    )
    .unwrap_err();
    assert_eq!(failure.progress, Progress::Staged);
    assert_eq!(
        failure.refusal,
        CompositionRefusal::Store(
            GenerationError::NativePayloadInvalid {
                detail: "composition member is not a valid generation"
            }
            .to_string()
        )
    );
    assert_eq!(
        fixture.store.read_vector_current().unwrap(),
        CurrentProfile::Absent
    );
}

#[test]
fn verification_refuses_an_oversized_record_before_opening_the_generation() {
    let fixture = Fixture::new();
    let base = fixture.layer(1, 10);
    let template = fixture.compose(1, &base, &[]).unwrap();
    let digest = fixture.stage_record(&vec![b'x'; 1024 * 1024 + 1], &template);
    assert_eq!(
        fixture.verify_composition(&digest).unwrap_err(),
        CompositionRefusal::NotComposition("record size")
    );
}

#[test]
fn verification_refuses_excess_deltas_before_opening_any_member() {
    let fixture = Fixture::new();
    let base = fixture.layer(1, 10);
    let composition = fixture
        .compose(1, &base, &[fixture.layer(5, 12), fixture.layer(6, 14)])
        .unwrap();
    fixture.publish(&composition).unwrap();
    fixture.corrupt(&base.digest, CODES_FILE);

    assert_eq!(
        verify_composition(
            &fixture.store,
            &composition.digest(),
            &fixture.expected(),
            NonZeroUsize::new(1).unwrap(),
        )
        .unwrap_err(),
        CompositionRefusal::DeltasOverBound { count: 2, max: 1 },
        "the admission bound must refuse before even the base is verified"
    );
    assert!(matches!(
        fixture.verify_composition(&composition.digest()),
        Err(CompositionRefusal::Member { digest, .. }) if digest == base.digest
    ));
}

#[test]
fn a_selection_the_verifier_rejects_gates_publication_the_same_way_recovery_treats_it() {
    let fixture = Fixture::new();
    let base = fixture.layer(1, 10);
    let first = fixture.compose(1, &base, &[]).unwrap();
    fixture.publish(&first).unwrap();

    // A hash-valid record that is not canonical: the store selects it, the verifier refuses it.
    let loose = fixture.compose(5, &base, &[]).unwrap();
    let selected = fixture.stage_record(&serde_json::to_vec_pretty(&loose).unwrap(), &loose);
    fixture
        .store
        .select_vector(&selected, &fixture.tx, &mut |_| Ok(()))
        .unwrap();
    assert_eq!(
        fixture.verify_composition(&selected).unwrap_err(),
        CompositionRefusal::NotComposition("record not canonical")
    );
    assert_eq!(
        fixture.recover().unwrap(),
        (first.digest(), SelectorState::Stale(selected.clone()))
    );
    // Recovery calls that selection stale, so its sequence must not gate the repair publication.
    let repair = fixture.compose(2, &base, &[]).unwrap();
    assert_eq!(fixture.publish(&repair).unwrap(), repair.digest());
    assert_eq!(
        fixture.store.read_vector_current().unwrap(),
        CurrentProfile::Current(repair.digest())
    );

    // A record of a schema this build does not know may be a later build's selection: nothing is published over it and nothing is concluded about it.
    let mut future = fixture.compose(3, &base, &[]).unwrap();
    future.schema = 2;
    let selected = fixture.stage_record(&future.canonical_bytes(), &future);
    fixture
        .store
        .select_vector(&selected, &fixture.tx, &mut |_| Ok(()))
        .unwrap();
    assert_eq!(
        fixture.verify_composition(&selected).unwrap_err(),
        CompositionRefusal::Quarantined
    );
    let failure = publish(
        &fixture.compose(9, &base, &[]).unwrap(),
        &fixture.staging(),
        &fixture.work_dir(),
        &mut |_| Ok(()),
    )
    .unwrap_err();
    assert_eq!(failure.progress, Progress::NotStaged);
    assert_eq!(failure.refusal, CompositionRefusal::Quarantined);
    assert_eq!(
        fixture.store.read_vector_current().unwrap(),
        CurrentProfile::Current(selected.clone())
    );
    // Recovery still serves what this build can verify without repointing the selector.
    assert_eq!(
        fixture.recover().unwrap(),
        (repair.digest(), SelectorState::Stale(selected.clone()))
    );

    // An unknown generation-manifest schema on the selected generation also refuses publication without moving the selector.
    let manifest_path = fixture.generation_dir(&selected).join("manifest.json");
    let mut manifest: serde_json::Value =
        serde_json::from_slice(&fs::read(&manifest_path).unwrap()).unwrap();
    manifest["schema"] = 7.into();
    fs::write(&manifest_path, serde_json::to_vec(&manifest).unwrap()).unwrap();
    let failure = publish(
        &fixture.compose(9, &base, &[]).unwrap(),
        &fixture.staging(),
        &fixture.work_dir(),
        &mut |_| Ok(()),
    )
    .unwrap_err();
    assert_eq!(failure.progress, Progress::NotStaged);
    assert_eq!(failure.refusal, CompositionRefusal::Quarantined);
    assert_eq!(
        fixture.store.read_vector_current().unwrap(),
        CurrentProfile::Current(selected)
    );
}

#[test]
fn an_unknown_members_schema_on_the_selected_composition_quarantines_publication() {
    let fixture = Fixture::new();
    let base = fixture.layer(1, 10);
    let future = fixture.compose(3, &base, &[]).unwrap();
    let dir = fixture.work_dir();
    fs::write(dir.join(COMPOSITION_FILE), future.canonical_bytes()).unwrap();
    fs::write(
        dir.join(MEMBERS_FILE_NAME),
        serde_json::to_vec(&host_runtime::generation::WireMembers {
            schema: 2,
            members: future.members(),
        })
        .unwrap(),
    )
    .unwrap();
    let sources: Vec<SourceSpec> = [COMPOSITION_FILE, MEMBERS_FILE_NAME]
        .into_iter()
        .map(|name| SourceSpec {
            rel_path: name.to_owned(),
            source: dir.join(name),
            executable: false,
            expected_size: None,
            expected_sha256: None,
        })
        .collect();
    let selected = fixture
        .store
        .stage(&sources, &future.stage_meta(), &BTreeSet::new())
        .unwrap();
    // This build's store refuses to select it; a later build's selector may still name it.
    assert!(
        fixture
            .store
            .select_vector(&selected, &fixture.tx, &mut |_| Ok(()))
            .is_err()
    );
    fixture.write_selector(
        VECTOR_PROFILE_NAME,
        format!(r#"{{"schema":1,"current":"{selected}"}}"#).as_bytes(),
    );

    assert_eq!(
        fixture.verify_composition(&selected).unwrap_err(),
        CompositionRefusal::Quarantined
    );
    let failure = publish(
        &fixture.compose(9, &base, &[]).unwrap(),
        &fixture.staging(),
        &fixture.work_dir(),
        &mut |_| Ok(()),
    )
    .unwrap_err();
    assert_eq!(failure.progress, Progress::NotStaged);
    assert_eq!(failure.refusal, CompositionRefusal::Quarantined);
    assert_eq!(
        fixture.store.read_vector_current().unwrap(),
        CurrentProfile::Current(selected)
    );
}

#[test]
fn a_forged_record_with_a_repeated_delta_fails_topology_at_verification() {
    let fixture = Fixture::new();
    let base = fixture.layer(1, 10);
    let delta = fixture.layer(5, 12);
    let mut forged = fixture
        .compose(1, &base, std::slice::from_ref(&delta))
        .unwrap();
    forged.deltas.push(delta.digest.clone());
    let dir = fixture.work_dir();
    fs::write(dir.join(COMPOSITION_FILE), forged.canonical_bytes()).unwrap();
    fs::write(
        dir.join(MEMBERS_FILE_NAME),
        serde_json::to_vec(&host_runtime::generation::WireMembers {
            schema: 1,
            members: forged.members(),
        })
        .unwrap(),
    )
    .unwrap();
    let manifest = forged.stage_manifest();
    let sources: Vec<SourceSpec> = manifest
        .files
        .iter()
        .map(|file| SourceSpec {
            rel_path: file.path.clone(),
            source: dir.join(&file.path),
            executable: false,
            expected_size: Some(file.size),
            expected_sha256: Some(file.sha256.clone()),
        })
        .collect();
    let digest = fixture
        .store
        .stage(&sources, &forged.stage_meta(), &BTreeSet::new())
        .unwrap();
    assert_eq!(
        fixture.verify_composition(&digest).unwrap_err(),
        CompositionRefusal::DuplicateMember { index: 1 }
    );
    // Checkpoint order: equality is allowed, one step back on the snapshot alone is not.
    let same = fixture.layer(8, 12);
    assert!(fixture.compose(4, &base, &[same]).is_ok());
    let earlier_snapshot = LiveRows {
        generation: generation(),
        kernel_incarnation_id: KERNEL.to_owned(),
        checkpoint: ProjectionCheckpoint {
            snapshot_commit_seq: 9,
            checkpoint_commit_seq: 12,
            hold_id: "hold-7".to_owned(),
        },
        rows: rows(9),
        tombstones: Vec::new(),
    };
    let built = build(&fixture.expected(), &earlier_snapshot, &fixture.work_dir()).unwrap();
    let staged = stage(&built, &fixture.staging()).unwrap();
    let layer = verify(&fixture.store, &staged, &fixture.expected()).unwrap();
    assert_eq!(
        fixture.compose(4, &base, &[layer]).unwrap_err(),
        CompositionRefusal::CheckpointOrder { index: 0 }
    );
    // A layer whose snapshot follows its own checkpoint is refused at build.
    let inverted = LiveRows {
        generation: generation(),
        kernel_incarnation_id: KERNEL.to_owned(),
        checkpoint: ProjectionCheckpoint {
            snapshot_commit_seq: 13,
            checkpoint_commit_seq: 12,
            hold_id: "hold-7".to_owned(),
        },
        rows: rows(10),
        tombstones: Vec::new(),
    };
    assert!(matches!(
        build(&fixture.expected(), &inverted, &fixture.work_dir()),
        Err(daemon::vector_generation::VectorRefusal::Identity {
            field: "checkpoint"
        })
    ));
}

#[test]
fn a_quarantined_vector_selector_stops_staging_and_pruning_and_an_unselected_corrupt_target_is_repaired()
 {
    let fixture = Fixture::new();
    let base = fixture.layer(1, 10);
    let composition = fixture.compose(1, &base, &[]).unwrap();
    fixture.publish(&composition).unwrap();
    let stray = fixture.layer(9, 30);

    fixture.write_selector(VECTOR_PROFILE_NAME, br#"{"schema":9,"current":"zz"}"#);
    assert_eq!(
        fixture.store.read_vector_current().unwrap(),
        CurrentProfile::Quarantined
    );
    let report = fixture.store.prune(&BTreeSet::new()).unwrap();
    assert_eq!(
        report.removed_generations, 0,
        "every generation stays under a quarantined owner selector"
    );
    assert!(fixture.generations().contains(&stray.digest));
    let failure = publish(
        &fixture.compose(2, &base, &[]).unwrap(),
        &fixture.staging(),
        &fixture.work_dir(),
        &mut |_| Ok(()),
    )
    .unwrap_err();
    assert_eq!(failure.progress, Progress::NotStaged);
    assert_eq!(failure.refusal, CompositionRefusal::Quarantined);

    // Unselected and unpinned, a corrupt layer is repaired by restaging the same bytes.
    fs::remove_file(fixture.lifecycle_dir().join(VECTOR_PROFILE_NAME)).unwrap();
    fixture.corrupt(&stray.digest, CODES_FILE);
    assert!(fixture.store.validate(&stray.digest).is_err());
    let rebuilt = build(&fixture.expected(), &export(9, 30), &fixture.work_dir()).unwrap();
    assert_eq!(stage(&rebuilt, &fixture.staging()).unwrap(), stray.digest);
    assert!(verify(&fixture.store, &stray.digest, &fixture.expected()).is_ok());
}

#[test]
fn a_search_seed_that_lists_members_retains_them_like_any_owner() {
    let fixture = Fixture::new();
    let layer = fixture.layer(1, 10);
    let dir = fixture.work_dir();
    fs::write(dir.join("search.sqlite"), b"not really a database").unwrap();
    fs::write(
        dir.join(MEMBERS_FILE_NAME),
        serde_json::to_vec(&host_runtime::generation::WireMembers {
            schema: 1,
            members: vec![layer.digest.clone()],
        })
        .unwrap(),
    )
    .unwrap();
    let meta = StageMeta {
        target: "search-projection-seed".to_owned(),
        release_contract_sha256: "a".repeat(64),
        inputs_lock_sha256: "b".repeat(64),
        source_payload_manifest_sha256: "c".repeat(64),
    };
    let sources: Vec<SourceSpec> = ["search.sqlite", MEMBERS_FILE_NAME]
        .into_iter()
        .map(|name| SourceSpec {
            rel_path: name.to_owned(),
            source: dir.join(name),
            executable: false,
            expected_size: None,
            expected_sha256: None,
        })
        .collect();
    let seed = fixture
        .store
        .stage(&sources, &meta, &BTreeSet::new())
        .unwrap();
    fixture
        .store
        .select_search(&seed, &fixture.tx, &mut |_| Ok(()))
        .unwrap();
    let report = fixture.store.prune(&BTreeSet::new()).unwrap();
    assert_eq!(
        report.removed_generations, 0,
        "the members rule is owner-agnostic"
    );
    assert!(fixture.generations().contains(&layer.digest));
}
