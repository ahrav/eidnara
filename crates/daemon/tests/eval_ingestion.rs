//! Rendered worlds through the real ingestion adapters: identity and
//! accounting round trips, the observation-lead boundary, git units under the
//! evaluator's time projection, observation-time inertness, and the composed
//! hold-embedding / correct / release / query lifecycle.

#![cfg(feature = "test-support")]

mod support;

use std::collections::{BTreeMap, BTreeSet};
use std::num::{NonZeroU64, NonZeroUsize};
use std::path::Path;
use std::time::{Duration, Instant};

use daemon::embedding_publication::{
    EmbeddingPublisher, ObsoleteCause, Publication, VectorPublication,
};
use daemon::git_sources::{GitReadBounds, read_selection};
use daemon::harness_sources::{
    AdapterRefusal, Harness, MAX_REVISION_LEAD_MS, PublishError, Published, SessionIdentity,
    SourcePublisher, SourceUnit, opencode_units,
};
use daemon::query_route::{DenseLane, execute};
use daemon::search_catchup::{CatchUpConsumer, EpisodeBounds, SearchCatchUp};
use daemon::search_projection::SearchProjection;
use eval_core::{
    Coverage, EventId, MARKERS, Mode, Payload, RenderConfig, Rendering, RepositorySpec,
    SessionSpec, World, WorldConfig, check_accounting, generate_all, git_identity, render,
};
use kernel::{
    ArtifactDestination, CommitPageBounds, CurrentInputDescriptor, EligibilityBinding,
    EligibilityCandidate, EligibilityVerdict, KernelStore, ProjectScope, ProviderEgress,
    Sensitivity, SourceRow, StaleInput, Surface,
};
use retrieval::eligibility::Authority;
use rusqlite::{Connection, OpenFlags};
use serde_json::json;
use support::embedding_fixtures::{
    Corpus, DAY_MS, GENERATION, PROJECT, SCOPE, TestEngine, batch_bounds, generation,
    hold_admission, inspect, kernel_incarnation_id, source_page_bounds,
};
use support::git_repo::Repo;
use support::projection_gate::open_gate;
use support::query_route::{limits, request_budget};

const SEED: u64 = 0x01DE_1D1A;
const EPOCH_MS: i64 = 1_700_000_000_000;
const REPOSITORY: &str = "repo-0";

fn config() -> WorldConfig {
    WorldConfig {
        sessions: vec![SessionSpec {
            messages: 5,
            tool_span_every: 2,
            correction_every: 3,
            invalidation_every: 0,
        }],
        repositories: vec![RepositorySpec {
            commits: 3,
            rename_every: 2,
        }],
        epoch_ms: EPOCH_MS,
        tick_ms: 1_000,
        max_events_per_log: 64,
    }
}

fn render_config() -> RenderConfig {
    RenderConfig {
        project_id: PROJECT.to_string(),
        repository_id: REPOSITORY.to_string(),
        object_format: "sha1".to_string(),
    }
}

fn world() -> World {
    generate_all(SEED, &config(), Mode::Generate).unwrap()
}

fn rendering(world: &World) -> Rendering {
    render(&world.log, &render_config()).unwrap()
}

fn session(session_id: &str) -> SessionIdentity {
    SessionIdentity {
        project_id: PROJECT.to_string(),
        harness: Harness::OpenCode,
        session_id: session_id.to_string(),
    }
}

fn adapter(kernel: &KernelStore) -> SourcePublisher<'_> {
    SourcePublisher {
        kernel,
        domain_id: "domain",
        scope_id: Some(SCOPE),
        egress: ProviderEgress::LocalOnly,
        sensitivity: Sensitivity::Normal,
    }
}

/// The two refusal kinds one unit can meet on its way into the store.
#[derive(Debug)]
enum Refusal {
    Adapter(AdapterRefusal),
    Publish(PublishError),
}

/// One typed outcome per expected unit, keyed by the identity the renderer expects.
#[derive(Default, Debug)]
struct Outcomes {
    published: BTreeMap<String, Published>,
    refused: BTreeMap<String, Refusal>,
}

impl Outcomes {
    fn published_ids(&self) -> BTreeSet<String> {
        self.published.keys().cloned().collect()
    }

    fn refused_ids(&self) -> BTreeSet<String> {
        self.refused.keys().cloned().collect()
    }
}

/// Publishes every rendered message in log order; a message the adapter
/// refuses refuses every unit the renderer expected from it.
fn ingest(publisher: &SourcePublisher<'_>, rendering: &Rendering, lag_ms: i64) -> Outcomes {
    let mut outcomes = Outcomes::default();
    for message in &rendering.messages {
        let expected_ids: Vec<&str> = message
            .expected
            .iter()
            .map(|unit| unit.identity.occurrence_id.as_str())
            .collect();
        match opencode_units(&session(&message.session_id), &message.message) {
            Err(refusal) => {
                for id in expected_ids {
                    outcomes
                        .refused
                        .insert(id.to_string(), Refusal::Adapter(refusal.clone()));
                }
            }
            Ok(units) => {
                assert_eq!(units.len(), expected_ids.len(), "{}", message.message);
                for (unit, id) in units.iter().zip(expected_ids) {
                    match publisher.publish(unit, message.observation_time_ms + lag_ms) {
                        Ok(published) => {
                            assert_eq!(published.occurrence_id, id, "{unit:?}");
                            outcomes.published.insert(id.to_string(), published);
                        }
                        Err(error) => {
                            outcomes
                                .refused
                                .insert(id.to_string(), Refusal::Publish(error));
                        }
                    }
                }
            }
        }
    }
    outcomes
}

fn expected_ids(rendering: &Rendering) -> BTreeSet<String> {
    rendering
        .messages
        .iter()
        .flat_map(|m| m.expected.iter().map(|u| u.identity.occurrence_id.clone()))
        .collect()
}

fn seeded(root: &Path) -> Corpus {
    let corpus = Corpus::open(root);
    corpus.seed();
    corpus
}

fn rendered_worlds_round_trip_through_the_opencode_adapter_with_exact_accounting(
    coverage: &mut Coverage,
) {
    let world = world();
    let rendering = rendering(&world);
    assert_eq!(rendering.commits.len(), 3);
    assert_eq!(
        rendering.excluded_by_rule.get("rename_is_commit_metadata"),
        Some(&1)
    );
    assert!(
        rendering.messages.iter().any(|m| m.expected.len() > 1),
        "a message with a tool span"
    );
    let corrections: Vec<&EventId> = world
        .log
        .events
        .iter()
        .filter(|e| matches!(e.payload, Payload::Correction { .. }))
        .map(|e| &e.id)
        .collect();
    assert!(!corrections.is_empty());

    let dir = tempfile::tempdir().unwrap();
    let corpus = seeded(dir.path());
    let publisher = adapter(&corpus.kernel);
    let expected = expected_ids(&rendering);
    let outcomes = ingest(&publisher, &rendering, 0);
    let publish_refusals: Vec<&PublishError> = outcomes
        .refused
        .values()
        .filter_map(|r| match r {
            Refusal::Publish(error) => Some(error),
            Refusal::Adapter(_) => None,
        })
        .collect();
    assert!(publish_refusals.is_empty(), "{publish_refusals:?}");
    assert!(outcomes.refused.is_empty(), "{:?}", outcomes.refused);
    assert_eq!(outcomes.published_ids(), expected);
    check_accounting(
        &expected,
        &outcomes.published_ids(),
        &outcomes.refused_ids(),
    )
    .unwrap();

    // A correction is a new occurrence of the same lineage that replaces the
    // predecessor; every first publication replaces nothing.
    let mut object_by_lineage: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for message in &rendering.messages {
        for unit in &message.expected {
            let published = &outcomes.published[&unit.identity.occurrence_id];
            let chain = object_by_lineage
                .entry(unit.identity.lineage_id.clone())
                .or_default();
            if corrections.contains(&&message.event_id) {
                assert_eq!(
                    published.replaced_object_id.as_deref(),
                    chain.last().map(String::as_str),
                    "the correction replaces the latest predecessor"
                );
            } else {
                assert_eq!(published.replaced_object_id, None, "{unit:?}");
            }
            assert!(!published.replayed);
            chain.push(published.object_id.clone());
        }
    }
    assert!(object_by_lineage.values().any(|chain| chain.len() == 2));

    // Republishing replays every receipt without minting anything.
    let replay = ingest(&publisher, &rendering, 0);
    assert_eq!(replay.published_ids(), expected);
    assert!(replay.published.values().all(|p| p.replayed));
    coverage
        .record("ing_adapter_round_trip_expected_equals_published_plus_refused")
        .unwrap();

    // The adapter is a tolerant reader: parts it does not read shift no
    // identity the renderer expects, and a part it cannot identify refuses the
    // whole message. Accounting stays exact under both.
    let mut tolerant = rendering.clone();
    let first = &mut tolerant.messages[0];
    let parts = first.message["parts"].as_array_mut().unwrap();
    parts.push(json!({"type": "step-start"}));
    parts.push(json!({"type": "text", "text": "ignored", "ignored": true}));
    parts.push(json!({"type": "tool", "callID": "running", "tool": "bash", "state": {"status": "running", "input": {}}}));
    let last = tolerant.messages.len() - 1;
    let broken = &mut tolerant.messages[last];
    broken.message["parts"]
        .as_array_mut()
        .unwrap()
        .push(json!({"type": "tool", "tool": "bash", "state": {"status": "completed", "input": {}, "output": "x", "time": {"end": 1}}}));
    let broken_ids: BTreeSet<String> = broken
        .expected
        .iter()
        .map(|u| u.identity.occurrence_id.clone())
        .collect();
    let dir = tempfile::tempdir().unwrap();
    let corpus = seeded(dir.path());
    let tolerant_publisher = adapter(&corpus.kernel);
    let outcomes = ingest(&tolerant_publisher, &tolerant, 0);
    assert_eq!(outcomes.refused_ids(), broken_ids);
    assert!(
        outcomes.refused.values().all(|refusal| matches!(
            refusal,
            Refusal::Adapter(AdapterRefusal::MissingIdentity(_))
        )),
        "{:?}",
        outcomes.refused
    );
    check_accounting(
        &expected,
        &outcomes.published_ids(),
        &outcomes.refused_ids(),
    )
    .unwrap();
    assert_eq!(
        outcomes.published_ids().len() + outcomes.refused.len(),
        expected.len()
    );

    // Silent loss is caught: dropping one outcome breaks the equation.
    let mut lost = outcomes.published_ids();
    let dropped = lost.pop_first().unwrap();
    assert_eq!(
        check_accounting(&expected, &lost, &outcomes.refused_ids()),
        Err(eval_core::AccountingError::Missing(BTreeSet::from([
            dropped.clone()
        ])))
    );
    let mut double = outcomes.refused_ids();
    double.insert(dropped.clone());
    assert!(matches!(
        check_accounting(&expected, &outcomes.published_ids(), &double),
        Err(eval_core::AccountingError::PublishedAndRefused(_))
    ));
}

fn generated_observations_never_lead_and_a_boundary_fixture_refuses(coverage: &mut Coverage) {
    let world = world();
    let rendering = rendering(&world);
    for message in &rendering.messages {
        for unit in &message.expected {
            let revision: i64 = unit.revision.parse().unwrap();
            assert!(
                revision <= message.observation_time_ms,
                "generated fixtures never lead"
            );
            assert!((0..=eval_core::MAX_VALID_TIME_MS).contains(&revision));
        }
    }
    let dir = tempfile::tempdir().unwrap();
    let corpus = seeded(dir.path());
    let publisher = adapter(&corpus.kernel);
    let outcomes = ingest(&publisher, &rendering, 0);
    assert!(outcomes.refused.is_empty());
    coverage
        .record("ing_generated_observations_never_lead")
        .unwrap();

    // The boundary: one hour of lead publishes, one millisecond more refuses.
    let message = &rendering.messages[0];
    let unit = &opencode_units(&session(&message.session_id), &message.message).unwrap()[0];
    let revision: i64 = unit.revision.parse().unwrap();
    let tip = corpus.kernel.tip().unwrap();
    let at_bound = publisher
        .publish(unit, revision - MAX_REVISION_LEAD_MS)
        .unwrap();
    assert!(
        at_bound.replayed,
        "the unit was published above; the lead alone is judged"
    );
    let mut ahead = unit.clone();
    ahead.revision = (revision + MAX_REVISION_LEAD_MS + 1).to_string();
    let refusal = publisher.publish(&ahead, revision).unwrap_err();
    assert!(
        matches!(
            refusal,
            PublishError::RevisionAhead { revision: ahead_revision, observed_at }
                if ahead_revision == revision + MAX_REVISION_LEAD_MS + 1 && observed_at == revision
        ),
        "{refusal:?}"
    );
    assert_eq!(corpus.kernel.tip().unwrap(), tip, "a refusal moves no tip");
}

fn git_bounds() -> GitReadBounds {
    GitReadBounds {
        max_commits: NonZeroUsize::new(8).unwrap(),
        max_object_bytes: NonZeroU64::new(4096).unwrap(),
        max_total_object_bytes: NonZeroU64::new(1 << 16).unwrap(),
    }
}

fn git_units_keep_revision_one_and_take_valid_time_from_the_projection(coverage: &mut Coverage) {
    let world = world();
    let rendering = rendering(&world);
    let dir = tempfile::tempdir().unwrap();
    let repo = Repo::init(&dir.path().join("repo"));
    // The evaluator owns `oid -> valid_time_ms`; git ingestion reads no time.
    let mut projection: BTreeMap<String, i64> = BTreeMap::new();
    let mut observations: BTreeMap<String, i64> = BTreeMap::new();
    let mut oids = Vec::new();
    for commit in &rendering.commits {
        let oid = repo.commit(&commit.message, commit.valid_time_ms / 1_000);
        projection.insert(oid.clone(), commit.valid_time_ms);
        observations.insert(oid.clone(), commit.observation_time_ms);
        oids.push(oid);
    }
    // Two commits that differ only in committer time are two identities.
    let twin_a = repo.commit("same words", 1_700_000_000);
    let twin_b = repo.commit("same words", 1_700_000_001);
    assert_ne!(twin_a, twin_b);
    projection.insert(twin_a.clone(), 1_700_000_000_000);
    projection.insert(twin_b.clone(), 1_700_000_001_000);
    oids.extend([twin_a.clone(), twin_b.clone()]);

    let selection =
        read_selection(&open_gate(), &repo.binding(REPOSITORY), &oids, git_bounds()).unwrap();
    assert_eq!(
        selection.units.len(),
        oids.len(),
        "{:?}",
        selection.dispositions
    );
    let corpus = seeded(dir.path());
    let publisher = adapter(&corpus.kernel);
    let mut published_ids = BTreeSet::new();
    for (unit, oid) in selection.units.iter().zip(&oids) {
        // Destructuring pins the unit's field set: there is no time field.
        let SourceUnit {
            class,
            identity,
            revision,
            representation,
            text,
            role,
        } = unit;
        assert_eq!(class.code(), "git_commits");
        let names: Vec<&str> = identity.iter().map(|(name, _)| *name).collect();
        assert_eq!(names, ["repository_id", "object_format", "oid"]);
        assert_eq!(identity[2].1, *oid);
        assert_eq!(revision, "1");
        assert_eq!(representation.as_str(), "commit_message");
        assert_eq!(role, "commit");
        assert!(!text.is_empty());
        let expected = git_identity(&render_config(), oid).unwrap();
        // The projection is the only time source, so it must agree with what
        // git recorded for the object it names.
        assert_eq!(
            projection[oid] / 1_000,
            repo.committer_seconds(oid),
            "{oid}"
        );
        let observation = observations
            .get(oid)
            .copied()
            .unwrap_or(projection[oid] + DAY_MS);
        let published = publisher.publish(unit, observation).unwrap();
        assert_eq!(published.occurrence_id, expected.occurrence_id);
        published_ids.insert(published.occurrence_id);
    }
    assert_eq!(published_ids.len(), oids.len());
    let twins: Vec<&SourceUnit> = selection
        .units
        .iter()
        .filter(|u| u.text == "same words")
        .collect();
    assert_eq!(twins.len(), 2);
    assert_eq!(twins[0].revision, twins[1].revision);
    assert_ne!(twins[0].identity, twins[1].identity);
    coverage
        .record("ing_git_units_fixed_revision_time_from_projection")
        .unwrap();
}

fn eligibility_of(
    kernel: &KernelStore,
    outcomes: &Outcomes,
    rendering: &Rendering,
) -> Vec<(String, EligibilityVerdict)> {
    let project = ProjectScope::new(PROJECT).unwrap();
    let candidates: Vec<EligibilityCandidate> = rendering
        .messages
        .iter()
        .flat_map(|m| m.expected.iter())
        .map(|unit| EligibilityCandidate {
            object_id: outcomes.published[&unit.identity.occurrence_id]
                .object_id
                .clone(),
            source_revision: unit.revision.parse().unwrap(),
            artifact_digest: None,
        })
        .collect();
    let batch = kernel
        .judge_surface_eligibility(
            &project,
            ArtifactDestination::Local,
            Surface::ExplicitSearch,
            &candidates,
        )
        .unwrap();
    candidates
        .iter()
        .zip(&batch.verdicts)
        .map(|(c, v)| (c.object_id.clone(), v.verdict))
        .collect()
}

fn observation_time_is_inert_for_identity_and_eligibility(coverage: &mut Coverage) {
    let world = world();
    let rendering = rendering(&world);
    let ten_years_ms = 10 * 365 * DAY_MS;
    let dir_a = tempfile::tempdir().unwrap();
    let dir_b = tempfile::tempdir().unwrap();
    let (a, b) = (seeded(dir_a.path()), seeded(dir_b.path()));
    let outcomes_a = ingest(&adapter(&a.kernel), &rendering, 0);
    let outcomes_b = ingest(&adapter(&b.kernel), &rendering, ten_years_ms);
    assert_eq!(outcomes_a.published_ids(), outcomes_b.published_ids());
    assert_eq!(outcomes_a.published_ids(), expected_ids(&rendering));
    let object_ids = |o: &Outcomes| -> Vec<String> {
        o.published.values().map(|p| p.object_id.clone()).collect()
    };
    assert_eq!(object_ids(&outcomes_a), object_ids(&outcomes_b));
    let verdicts_a = eligibility_of(&a.kernel, &outcomes_a, &rendering);
    let verdicts_b = eligibility_of(&b.kernel, &outcomes_b, &rendering);
    assert_eq!(verdicts_a, verdicts_b);
    assert!(verdicts_a.iter().any(|(_, v)| *v == EligibilityVerdict::Ok));
    assert!(
        verdicts_a
            .iter()
            .any(|(_, v)| *v == EligibilityVerdict::Superseded)
    );

    // Republishing at a later observation time replays and leaves the stored
    // observation time alone.
    let message = &rendering.messages[0];
    let unit = &opencode_units(&session(&message.session_id), &message.message).unwrap()[0];
    let first = &outcomes_a.published[&message.expected[0].identity.occurrence_id];
    let again = adapter(&a.kernel)
        .publish(unit, message.observation_time_ms + 1_000_000)
        .unwrap();
    assert!(again.replayed);
    assert_eq!(again.object_id, first.object_id);
    let stored: i64 = Connection::open_with_flags(
        dir_a.path().join("kernel").join("kernel.sqlite"),
        OpenFlags::SQLITE_OPEN_READ_ONLY,
    )
    .unwrap()
    .query_row(
        "SELECT observed_at FROM observations WHERE object_id=?1",
        [&first.object_id],
        |row| row.get(0),
    )
    .unwrap();
    assert_eq!(stored, message.observation_time_ms);
    coverage.record("ing_observation_time_inert").unwrap();
}

fn episode_bounds() -> EpisodeBounds {
    EpisodeBounds {
        commits: CommitPageBounds {
            max_commits: NonZeroUsize::new(64).unwrap(),
            max_rows: NonZeroUsize::new(64).unwrap(),
            max_payload_bytes: NonZeroU64::new(1 << 20).unwrap(),
        },
        hold_admission: hold_admission(),
        source_page: source_page_bounds(),
        max_source_pages: NonZeroUsize::new(8).unwrap(),
        max_source_encoded_bytes: NonZeroU64::new(1 << 20).unwrap(),
        batch: batch_bounds(),
    }
}

/// The catch-up consumer's baseline: a hold at S, the snapshot applied, S acknowledged.
fn bootstrap(
    corpus: &Corpus,
    data_home: &Path,
) -> (SearchProjection, CatchUpConsumer, Vec<SourceRow>) {
    let (projection, hold, rows) = corpus.bootstrap_with_hold(data_home);
    let binding = corpus.binding();
    corpus
        .kernel
        .acknowledge_through_source_hold(&binding, &hold.hold_id, hold.snapshot, 2)
        .unwrap();
    let consumer = CatchUpConsumer {
        binding,
        hold_id: hold.hold_id,
        kernel_incarnation_id: kernel_incarnation_id(data_home),
        generation_id: Some(GENERATION.to_string()),
    };
    (projection, consumer, rows)
}

fn publish_outbox(kernel: &KernelStore) {
    let pending = kernel.pending_outbox(256).unwrap();
    if let Some(last) = pending.iter().rev().find(|entry| entry.commit_boundary) {
        kernel
            .mark_outbox_published_through(last.outbox_position, 1)
            .unwrap();
    }
}

fn descriptor_for(row: &SourceRow) -> CurrentInputDescriptor {
    CurrentInputDescriptor {
        object_id: row.object_id.clone(),
        source_revision: row.revision,
        detail: row.detail.clone(),
        domain_id: row.domain_id.clone(),
        sensitivity: row.sensitivity,
        created_commit_seq: row.created_commit_seq,
    }
}

fn embed(
    publisher: &mut EmbeddingPublisher<'_>,
    row: &SourceRow,
    project: &ProjectScope,
) -> Publication {
    let generation = generation();
    let vector = TestEngine::vector_for(row.text.as_deref().unwrap_or_default());
    publisher
        .publish(
            &VectorPublication {
                input: descriptor_for(row),
                generation: &generation,
                vector: &vector,
                input_bytes: row.text.as_ref().map_or(0, |t| t.len() as u64),
                input_tokens: 3,
            },
            EligibilityBinding {
                project,
                destination: ArtifactDestination::Local,
            },
            Instant::now() + Duration::from_secs(10),
            3,
            &mut |_| {},
        )
        .unwrap()
}

fn query_ids(
    projection: &SearchProjection,
    kernel: &KernelStore,
    project: &ProjectScope,
    query: &str,
) -> Vec<String> {
    let (_token, budget) = request_budget(10_000);
    let outcome = execute(
        projection,
        kernel,
        Authority {
            project,
            destination: ArtifactDestination::Local,
        },
        &limits(),
        budget.shared(),
        query,
        DenseLane::Undeclared,
        |_| {},
    )
    .unwrap();
    assert_eq!(outcome.body["kind"], "fused", "{}", outcome.body);
    support::query_route::entry_ids(&outcome.body)
}

/// The four seams composed: the predecessor's embedding is held (pending,
/// unpublished), the correction commits through the adapter, the hold releases
/// into a stale publication, and the route serves the successor only.
fn hold_embedding_commit_correction_release_query_makes_the_predecessor_obsolete(
    coverage: &mut Coverage,
) {
    let dir = tempfile::tempdir().unwrap();
    let corpus = seeded(dir.path());
    let project = ProjectScope::new(PROJECT).unwrap();
    let publisher = adapter(&corpus.kernel);
    let session_id = "session-lifecycle";
    let message = |text: &str, created: i64| {
        json!({
            "info": {"id": "msg-corrected", "sessionID": session_id, "role": "user", "time": {"created": created}},
            "parts": [{"type": "text", "text": text}],
        })
    };
    let predecessor_units = opencode_units(
        &session(session_id),
        &message("keep the barrier explicit", EPOCH_MS),
    )
    .unwrap();
    let predecessor = publisher.publish(&predecessor_units[0], EPOCH_MS).unwrap();
    assert_eq!(predecessor.replaced_object_id, None);
    publish_outbox(&corpus.kernel);

    // Seam 1: the projection holds the predecessor's embedding job pending.
    let (projection, consumer, rows) = bootstrap(&corpus, dir.path());
    let predecessor_row = rows
        .iter()
        .find(|row| row.object_id == predecessor.object_id)
        .unwrap();
    let pending_before: i64 = inspect(dir.path())
        .query_row(
            "SELECT COUNT(*) FROM embedding_jobs WHERE state='pending'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert!(pending_before >= 1, "the predecessor's embedding is held");

    // Seam 2: the correction commits through the adapter as a new revision of
    // the same lineage; a non-advancing revision is the broken fixture.
    let same_time = opencode_units(
        &session(session_id),
        &message("keep the barrier explicit, edited", EPOCH_MS),
    )
    .unwrap();
    let reused = publisher.publish(&same_time[0], EPOCH_MS + 1).unwrap_err();
    assert!(matches!(reused, PublishError::IdentityReused), "{reused:?}");
    let backdated = opencode_units(
        &session(session_id),
        &message("keep the barrier explicit, edited", EPOCH_MS - 1_000),
    )
    .unwrap();
    let not_advanced = publisher.publish(&backdated[0], EPOCH_MS + 1).unwrap_err();
    assert!(
        matches!(not_advanced, PublishError::Descriptor { .. }),
        "{not_advanced:?}"
    );
    let successor_units = opencode_units(
        &session(session_id),
        &message("keep the barrier explicit, edited", EPOCH_MS + 5_000),
    )
    .unwrap();
    let successor = publisher
        .publish(&successor_units[0], EPOCH_MS + 5_000)
        .unwrap();
    assert_eq!(
        successor.replaced_object_id.as_deref(),
        Some(predecessor.object_id.as_str())
    );
    assert_ne!(successor.occurrence_id, predecessor.occurrence_id);
    publish_outbox(&corpus.kernel);
    let mut catch_up = SearchCatchUp::new(&corpus.kernel, &projection);
    let report = catch_up
        .run_episode(&consumer, &episode_bounds(), 3, &mut |_| {})
        .unwrap();
    assert!(report.commits_consumed > 0, "{report:?}");
    coverage
        .record("ing_four_seam_hold_correct_release_query")
        .unwrap();

    // Seam 3: releasing the held embedding into the corrected world makes the
    // predecessor's publication obsolete, and the successor's lands.
    let mut embedder = EmbeddingPublisher::new(&corpus.kernel, &projection);
    let late = embed(&mut embedder, predecessor_row, &project);
    assert_eq!(
        late,
        Publication::Obsolete(ObsoleteCause::Canonical(StaleInput::Superseded))
    );
    let successor_row = corpus
        .export()
        .into_iter()
        .find(|row| row.object_id == successor.object_id)
        .unwrap();
    assert_eq!(
        embed(&mut embedder, &successor_row, &project),
        Publication::Embedded
    );

    // Seam 4: the route serves the successor and never the predecessor.
    let ids = query_ids(
        &projection,
        &corpus.kernel,
        &project,
        "barrier explicit edited",
    );
    assert!(ids.contains(&successor.occurrence_id), "{ids:?}");
    assert!(!ids.contains(&predecessor.occurrence_id), "{ids:?}");

    // Republishing the successor is receipt-idempotent.
    let again = publisher
        .publish(&successor_units[0], EPOCH_MS + 6_000)
        .unwrap();
    assert!(again.replayed);
    assert_eq!(again.object_id, successor.object_id);
    assert_eq!(
        query_ids(
            &projection,
            &corpus.kernel,
            &project,
            "barrier explicit edited"
        ),
        ids
    );
}

const SUITE: &str = "crates/daemon/tests/eval_ingestion.rs::";

type Scenario = fn(&mut Coverage);

fn scenarios() -> [(&'static str, Scenario); 5] {
    [
        (
            "rendered_worlds_round_trip_through_the_opencode_adapter_with_exact_accounting",
            rendered_worlds_round_trip_through_the_opencode_adapter_with_exact_accounting,
        ),
        (
            "generated_observations_never_lead_and_a_boundary_fixture_refuses",
            generated_observations_never_lead_and_a_boundary_fixture_refuses,
        ),
        (
            "git_units_keep_revision_one_and_take_valid_time_from_the_projection",
            git_units_keep_revision_one_and_take_valid_time_from_the_projection,
        ),
        (
            "observation_time_is_inert_for_identity_and_eligibility",
            observation_time_is_inert_for_identity_and_eligibility,
        ),
        (
            "hold_embedding_commit_correction_release_query_makes_the_predecessor_obsolete",
            hold_embedding_commit_correction_release_query_makes_the_predecessor_obsolete,
        ),
    ]
}

fn run(name: &str) {
    let (_, scenario) = scenarios().into_iter().find(|(n, _)| *n == name).unwrap();
    let mut coverage = Coverage::default();
    scenario(&mut coverage);
    let marker = MARKERS
        .iter()
        .find(|m| m.test == format!("{SUITE}{name}"))
        .unwrap();
    assert!(
        coverage.fired().contains(marker.name),
        "{name} records its marker"
    );
}

#[test]
fn adapter_round_trip() {
    run("rendered_worlds_round_trip_through_the_opencode_adapter_with_exact_accounting");
}

#[test]
fn observation_lead() {
    run("generated_observations_never_lead_and_a_boundary_fixture_refuses");
}

#[test]
fn git_units() {
    run("git_units_keep_revision_one_and_take_valid_time_from_the_projection");
}

#[test]
fn observation_time_inert() {
    run("observation_time_is_inert_for_identity_and_eligibility");
}

#[test]
fn four_seam_lifecycle() {
    run("hold_embedding_commit_correction_release_query_makes_the_predecessor_obsolete");
}

#[test]
fn coverage_markers_are_unique_and_each_names_a_scenario_here() {
    let names: BTreeSet<&str> = MARKERS.iter().map(|m| m.name).collect();
    assert_eq!(
        names.len(),
        MARKERS.len(),
        "marker names are globally unique"
    );
    let scenario_names: BTreeSet<&str> = scenarios().iter().map(|(n, _)| *n).collect();
    for marker in MARKERS.iter().filter(|m| m.name.starts_with("ing_")) {
        let test = marker
            .test
            .strip_prefix(SUITE)
            .unwrap_or_else(|| panic!("{}", marker.test));
        assert!(
            scenario_names.contains(test),
            "{test} is a scenario of this suite"
        );
    }
    let mut coverage = Coverage::default();
    assert_eq!(
        coverage.record("ing_not_registered"),
        Err(eval_core::CoverageError::Unregistered(
            "ing_not_registered".to_string()
        ))
    );
    coverage.record(MARKERS[0].name).unwrap();
    assert!(matches!(
        coverage.complete(SUITE),
        Err(eval_core::CoverageError::Incomplete { .. })
    ));
}

/// The completeness proof: one run of every scenario fires every marker this
/// suite owns, so a marker whose scenario stops recording it fails here.
#[test]
fn every_registered_marker_fires_across_the_scenarios() {
    let mut coverage = Coverage::default();
    for (_, scenario) in scenarios() {
        scenario(&mut coverage);
    }
    coverage.complete(SUITE).unwrap();
}

fn adapter_ids(session: &SessionIdentity, message: &serde_json::Value) -> Vec<String> {
    opencode_units(session, message)
        .unwrap()
        .iter()
        .map(|unit| {
            let identity: Vec<(&str, &str)> = unit
                .identity
                .iter()
                .map(|(name, value)| (*name, value.as_str()))
                .collect();
            eval_core::encode(&eval_core::Occurrence {
                class: unit.class.code(),
                identity: &identity,
                revision: &unit.revision,
                representation: unit.representation.as_str(),
                span: None,
            })
            .unwrap()
            .occurrence_id
        })
        .collect()
}

/// The identity rule flipped one adapter field at a time on a rendered
/// assistant message with a text and a tool part: each identity or revision
/// field moves the ids it enters; payload fields move none.
#[test]
fn each_adapter_identity_field_flips_its_ids_and_payload_fields_flip_none() {
    let world = world();
    let rendering = rendering(&world);
    let message = rendering
        .messages
        .iter()
        .find(|m| m.expected.len() == 2)
        .unwrap();
    let base_session = session(&message.session_id);
    let base = adapter_ids(&base_session, &message.message);
    let expected: Vec<String> = message
        .expected
        .iter()
        .map(|u| u.identity.occurrence_id.clone())
        .collect();
    assert_eq!(base, expected, "[text unit, tool unit]");

    type Flip = fn(&mut serde_json::Value, &mut SessionIdentity);
    let matrix: [(&str, Flip, [bool; 2]); 10] = [
        (
            "info.id",
            |m, _| m["info"]["id"] = json!("other-message"),
            [true, true],
        ),
        (
            "sessionID",
            |m, s| {
                m["info"]["sessionID"] = json!("other-session");
                s.session_id = "other-session".to_string();
            },
            [true, true],
        ),
        (
            "project_id",
            |_, s| s.project_id = format!("{PROJECT}x"),
            [true, true],
        ),
        (
            "time.completed",
            |m, _| m["info"]["time"]["completed"] = json!(EPOCH_MS + 999_000),
            [true, false],
        ),
        (
            "part position",
            |m, _| {
                let parts = m["parts"].as_array_mut().unwrap();
                parts.insert(0, json!({"type": "step-start"}));
            },
            [true, false],
        ),
        (
            "callID",
            |m, _| m["parts"][1]["callID"] = json!("other-call"),
            [false, true],
        ),
        (
            "state.time.end",
            |m, _| m["parts"][1]["state"]["time"]["end"] = json!(EPOCH_MS + 999_000),
            [false, true],
        ),
        (
            "text",
            |m, _| m["parts"][0]["text"] = json!("a different sentence"),
            [false, false],
        ),
        (
            "role",
            |m, _| m["info"]["role"] = json!("user"),
            [false, false],
        ),
        (
            "tool output",
            |m, _| m["parts"][1]["state"]["output"] = json!("other output"),
            [false, false],
        ),
    ];
    for (name, flip, moves) in matrix {
        let mut flipped_message = message.message.clone();
        let mut flipped_session = base_session.clone();
        flip(&mut flipped_message, &mut flipped_session);
        let flipped = adapter_ids(&flipped_session, &flipped_message);
        assert_eq!(flipped.len(), 2, "{name}");
        for (slot, unit) in ["text unit", "tool unit"].iter().enumerate() {
            assert_eq!(
                flipped[slot] != base[slot],
                moves[slot],
                "{name} moves the {unit}: {}",
                moves[slot]
            );
        }
    }
}
