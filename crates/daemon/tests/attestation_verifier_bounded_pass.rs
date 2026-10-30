//! Seam 3: the verifier's bounded pass over a fixture store, bare repositories as project roots,
//! a controlled clock, and a switchable flag.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, mpsc};
use std::time::{Duration, Instant};

use daemon::attestation_verifier::attest::{CITATION_EVENT_KIND, Identity, Outcome, attest};
use daemon::attestation_verifier::bounded_pass::{
    COUNTERS, Counters, IDLE_INTERVAL, MAX_VERDICTS, PAGE_ROWS, PASS_DEADLINE, PassClock, Poll,
    Project, RETRY_INTERVAL, Root, Verifier,
};
use daemon::attestation_verifier::classify::{BoundRoot, classify};
use daemon::attestation_verifier::{MAX_BLOB_BYTES, Stage, verify};
use daemon::verifier_task::run;
use kernel::applicability::EvalBudget;
use kernel::source_identity::{Occurrence, OccurrenceClass};
use kernel::{
    AdmissionEvent, AdmissionRequest, AnchorSpec, AnchorValue, ArtifactIngestRequest, CommitIntent,
    DecisionEventPayload, DecisionEventSpec, DecisionPayload, DecisionSpec, DomainSpec, EventKind,
    KernelError, KernelStore, ProviderEgress, ScopeSpec, Sensitivity, SourceClass,
    SourceDescriptorPolicy, SourceDescriptorRequest, TaintClass,
};
use sha2::{Digest as _, Sha256};

const N: usize = MAX_VERDICTS as usize;

fn intent(key: &str) -> CommitIntent {
    CommitIntent {
        producer: "attestation-verifier-pass-test".into(),
        operation_key: key.into(),
        request_digest: format!("{:x}", Sha256::digest(key.as_bytes())),
        actor: "test".into(),
        cause: "proof".into(),
    }
}

fn git(dir: &Path, args: &[&str]) -> String {
    let output = Command::new("git")
        .args(["-c", "core.autocrlf=false"])
        .args(args)
        .current_dir(dir)
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_AUTHOR_NAME", "fixture")
        .env("GIT_AUTHOR_EMAIL", "fixture@example.invalid")
        .env("GIT_COMMITTER_NAME", "fixture")
        .env("GIT_COMMITTER_EMAIL", "fixture@example.invalid")
        .output()
        .unwrap();
    assert!(output.status.success(), "git {args:?}");
    String::from_utf8(output.stdout).unwrap().trim().to_owned()
}

/// A bound project whose root is a bare repository with one commit.
struct Repo {
    _dir: tempfile::TempDir,
    work: PathBuf,
    bare: PathBuf,
    head: String,
    digest: String,
}

impl Repo {
    fn new(name: &str, files: &[(String, String)]) -> Self {
        let dir = tempfile::tempdir().unwrap();
        let work = dir.path().join("work");
        std::fs::create_dir(&work).unwrap();
        git(&work, &["init", "--quiet", "--initial-branch=main"]);
        for (path, text) in files {
            let file = work.join(path);
            std::fs::create_dir_all(file.parent().unwrap()).unwrap();
            std::fs::write(file, text).unwrap();
        }
        git(&work, &["add", "--all"]);
        git(&work, &["commit", "--quiet", "--allow-empty", "-m", name]);
        let head = git(&work, &["rev-parse", "HEAD"]);
        git(
            dir.path(),
            &["clone", "--quiet", "--bare", "work", "bare.git"],
        );
        Self {
            bare: dir.path().join("bare.git"),
            work,
            _dir: dir,
            head,
            digest: format!("{:x}", Sha256::digest(name.as_bytes())),
        }
    }

    fn scope_id(&self) -> String {
        format!("project:{}", self.digest)
    }

    fn project(&self) -> Project {
        self.project_at(&self.bare)
    }

    fn project_at(&self, root: &Path) -> Project {
        Project {
            scope_id: self.scope_id(),
            roots: vec![Root {
                path: root.to_path_buf(),
                digest: self.digest.clone(),
            }],
        }
    }

    fn bound(&self) -> BoundRoot<'_> {
        BoundRoot {
            root: &self.bare,
            repository_id: &self.digest,
        }
    }
}

struct Store {
    root: tempfile::TempDir,
    kernel: KernelStore,
}

impl Store {
    fn open() -> Self {
        let root = tempfile::tempdir().unwrap();
        let kernel = KernelStore::open(root.path()).unwrap();
        kernel
            .commit(intent("domains"), |envelope| {
                for domain in ["memory", "source"] {
                    envelope.insert_domain(DomainSpec {
                        domain_id: domain.into(),
                        object_id: format!("{domain}-domain"),
                        name: domain.into(),
                        source_kind: "test".into(),
                        source_id: domain.into(),
                        source_revision: 1,
                        sensitivity: Sensitivity::Normal,
                    })?;
                }
                Ok(String::new())
            })
            .unwrap();
        Self { root, kernel }
    }

    fn reopen(self) -> Self {
        let Self { root, kernel } = self;
        drop(kernel);
        let kernel = KernelStore::open(root.path()).unwrap();
        Self { root, kernel }
    }

    fn sqlite(&self) -> rusqlite::Connection {
        rusqlite::Connection::open(self.root.path().join("kernel.sqlite")).unwrap()
    }

    fn count(&self, sql: &str) -> i64 {
        self.sqlite().query_row(sql, [], |row| row.get(0)).unwrap()
    }

    fn verifier_rows(&self) -> i64 {
        self.count("SELECT COUNT(*) FROM object_registry WHERE source_kind='verifier'")
    }

    fn scope(&self, repo: &Repo) {
        let scope_id = repo.scope_id();
        self.kernel
            .commit(intent(&format!("scope-{scope_id}")), |envelope| {
                envelope.insert_scope(ScopeSpec {
                    object_id: scope_id.clone(),
                    source_id: scope_id.clone(),
                    scope_id: scope_id.clone(),
                    domain_id: "memory".into(),
                    source_kind: "kernel_route".into(),
                    source_revision: 1,
                    sensitivity: Sensitivity::Normal,
                    terms: Vec::new(),
                })?;
                Ok(String::new())
            })
            .unwrap();
    }

    /// A model decision in `repo`'s project scope, anchored at its head, citing a document at
    /// `path` whose retained bytes are `text` under `repository_id`.
    fn decision_under(
        &self,
        repo: &Repo,
        repository_id: &str,
        name: &str,
        path: &str,
        text: &str,
    ) -> String {
        let handle = self
            .kernel
            .ingest_exact_artifact(ArtifactIngestRequest {
                intent: intent(&format!("artifact-{name}")),
                payload: text.as_bytes().to_vec(),
                evidence_id: format!("srcev:{name}"),
                object_id: format!("srcev-object:{name}"),
                object_kind: "evidence".into(),
                domain_id: "source".into(),
                source_kind: "test".into(),
                source_id: format!("source/{name}"),
                source_revision: 1,
                media_type: "text/plain".into(),
                retention_class: "canonical".into(),
                retain_until: None,
                asserted_sensitivity: Sensitivity::Sensitive,
                provider_egress: ProviderEgress::LocalOnly,
                provenance: None,
            })
            .unwrap();
        let identity = [
            ("project_id", repo.digest.as_str()),
            ("repository_id", repository_id),
            ("relative_path", path),
        ];
        self.kernel
            .commit(intent(&format!("descriptor-{name}")), |envelope| {
                envelope
                    .publish_source_descriptor(&SourceDescriptorRequest {
                        occurrence: Occurrence {
                            class: OccurrenceClass::RepositoryDocuments.code(),
                            identity: &identity,
                            revision: "1",
                            representation: "text",
                            span: None,
                        },
                        source_policy: SourceDescriptorPolicy::Native,
                        domain_id: "source",
                        scope_id: None,
                        evidence_id: &handle.evidence_id,
                        artifact_digest: &handle.digest,
                        buffer: text,
                        sensitivity: Sensitivity::Sensitive,
                        observed_at: 1,
                        expected_head: None,
                        outcome: None,
                    })
                    .map_err(|_| KernelError::InvalidInput)?;
                Ok(String::new())
            })
            .unwrap();
        let anchor = AnchorValue::ReachableFrom {
            oid: repo.head.clone(),
            captures: Vec::new(),
        };
        self.model_decision(repo, name, path, Some(anchor), Some(&handle.evidence_id))
    }

    /// A model decision in `repo`'s project scope with `anchor`, citing `evidence_id`.
    fn model_decision(
        &self,
        repo: &Repo,
        name: &str,
        path: &str,
        anchor: Option<AnchorValue>,
        evidence_id: Option<&str>,
    ) -> String {
        let object_id = format!("mem_{name}");
        self.kernel
            .commit(intent(&format!("decision-{name}")), |envelope| {
                let anchor_id = format!("anchor_{name}");
                if let Some(value) = anchor.clone() {
                    envelope.insert_anchor(AnchorSpec {
                        anchor_id: anchor_id.clone(),
                        object_id: format!("anchorobj_{name}"),
                        domain_id: "memory".into(),
                        source_kind: "assistant".into(),
                        source_id: "memory_capture".into(),
                        source_revision: 1,
                        sensitivity: Sensitivity::Normal,
                        value,
                    })?;
                }
                envelope.insert_decision(DecisionSpec {
                    decision_id: format!("dec_{name}"),
                    object_id: object_id.clone(),
                    domain_id: "memory".into(),
                    proposition_id: None,
                    scope_id: Some(repo.scope_id()),
                    anchor_id: anchor.is_some().then_some(anchor_id),
                    evidence_id: None,
                    decision_kind: "ARCHITECTURE".into(),
                    payload: DecisionPayload {
                        summary: format!("The document {path} holds its text ({name})."),
                        rationale: "{}".into(),
                    },
                    source_kind: "assistant".into(),
                    source_id: "memory_capture".into(),
                    source_revision: 1,
                    sensitivity: Sensitivity::Normal,
                })?;
                if let Some(evidence_id) = evidence_id {
                    envelope.append_decision_event(
                        &format!("dec_{name}"),
                        DecisionEventSpec {
                            event_kind: "memory_capture_citation".into(),
                            payload: DecisionEventPayload {
                                summary: "cites the document".into(),
                            },
                            evidence_id: Some(evidence_id.to_owned()),
                            recorded_at: 1,
                        },
                    )?;
                }
                envelope.record_admission(AdmissionRequest {
                    candidate_id: None,
                    subject_object_id: Some(object_id.clone()),
                    source_class: Some(SourceClass::ModelInference),
                    taint_class: Some(TaintClass::AssistantInference),
                    event: AdmissionEvent {
                        kind: EventKind::Other,
                        trigger_object_id: None,
                        approval_object_id: None,
                        evidence_id: None,
                        reason: "fixture".into(),
                    },
                })?;
                Ok(String::new())
            })
            .unwrap();
        object_id
    }

    fn decision(&self, repo: &Repo, name: &str, path: &str, text: &str) -> String {
        self.decision_under(repo, &repo.digest, name, path, text)
    }
}

type Hook<'a> = Box<dyn Fn(Poll, usize, &EvalBudget) -> bool + Send + Sync + 'a>;

/// A clock that records every poll and asks `hook` with the poll, the repository opens polled so
/// far, this one included, and the pass budget the hook may cancel.
struct Clock<'a> {
    budget: EvalBudget,
    polls: Mutex<Vec<Poll>>,
    hook: Hook<'a>,
}

impl<'a> Clock<'a> {
    fn with(hook: impl Fn(Poll, usize) -> bool + Send + Sync + 'a) -> Self {
        Self::with_budget(move |poll, opens, _| hook(poll, opens))
    }

    fn with_budget(hook: impl Fn(Poll, usize, &EvalBudget) -> bool + Send + Sync + 'a) -> Self {
        Self {
            budget: EvalBudget::unbounded(),
            polls: Mutex::default(),
            hook: Box::new(hook),
        }
    }

    fn open() -> Self {
        Self::with(|_, _| false)
    }

    fn opens(&self) -> usize {
        self.polled(Poll::Read(Stage::Open))
    }

    fn polled(&self, poll: Poll) -> usize {
        self.polls
            .lock()
            .unwrap()
            .iter()
            .filter(|polled| **polled == poll)
            .count()
    }
}

impl PassClock for Clock<'_> {
    fn passed(&self, poll: Poll) -> bool {
        let opens = {
            let mut polls = self.polls.lock().unwrap();
            polls.push(poll);
            polls
                .iter()
                .filter(|poll| **poll == Poll::Read(Stage::Open))
                .count()
        };
        (self.hook)(poll, opens, &self.budget)
    }

    fn budget(&self) -> &EvalBudget {
        &self.budget
    }
}

fn on(_: &Path) -> bool {
    true
}

/// AC12's accounting at a pass end: the verdict and unverifiable buckets sum to `considered`, and
/// no more attestations committed than matched.
fn assert_accounting(counters: &Counters) {
    let buckets: u64 = COUNTERS
        .iter()
        .filter(|name| {
            matches!(**name, "match" | "mismatch")
                || (name.starts_with("unavailable_") && **name != "unavailable_skipped")
                || name.starts_with("unverifiable_")
        })
        .map(|name| counters.get(name))
        .sum();
    assert_eq!(buckets, counters.get("considered"));
    assert!(counters.get("attestation_committed") <= counters.get("match"));
}

fn text(index: usize, bytes: usize) -> String {
    let line = format!("document {index} line\n");
    line.repeat(bytes / line.len() + 1)[..bytes].to_owned()
}

fn attestation(object_id: &str, repo: &Repo) -> String {
    Identity::of(object_id, &repo.head).object_id
}

fn exists(store: &Store, object_id: &str) -> bool {
    store.count(&format!(
        "SELECT COUNT(*) FROM object_registry WHERE object_id='{object_id}'"
    )) == 1
}

/// AC10, F12: with the flag off no pass body runs, no repository opens, and no verifier row is
/// written; a positive control with the flag on opens the repository and attests. A reload to
/// false between passes (AC13) stops the next pass before any open.
#[test]
fn a_flag_off_pass_runs_nothing_and_a_reload_to_false_stops_the_next_pass() {
    let store = Store::open();
    let files: Vec<_> = (0..N + 2)
        .map(|i| (format!("doc{i}.md"), text(i, 1024)))
        .collect();
    let repo = Repo::new("flag", &files);
    store.scope(&repo);
    for (i, (path, body)) in files.iter().enumerate() {
        store.decision(&repo, &format!("flag{i}"), path, body);
    }
    let counters = Counters::default();
    let mut verifier = Verifier::default();
    let projects = [repo.project()];

    let flag = AtomicBool::new(false);
    let enabled = |_: &Path| flag.load(Ordering::SeqCst);
    let clock = Clock::open();
    verifier.pass(&store.kernel, &projects, &enabled, &counters, &clock);
    assert_eq!(counters.get("passes"), 0);
    assert!(clock.polls.lock().unwrap().is_empty());
    assert_eq!(counters.to_json(), Counters::default().to_json());
    assert_eq!(store.verifier_rows(), 0);

    flag.store(true, Ordering::SeqCst);
    let clock = Clock::open();
    verifier.pass(&store.kernel, &projects, &enabled, &counters, &clock);
    assert_eq!(clock.opens(), N);
    assert_eq!(counters.get("attestation_committed"), MAX_VERDICTS);
    assert_eq!(counters.get("passes"), 1);
    let rows = store.verifier_rows();
    assert!(rows > 0);

    flag.store(false, Ordering::SeqCst);
    let clock = Clock::open();
    verifier.pass(&store.kernel, &projects, &enabled, &counters, &clock);
    assert!(clock.polls.lock().unwrap().is_empty());
    assert_eq!(counters.get("passes"), 1);
    assert_eq!(store.verifier_rows(), rows);
}

/// AC11, F13: N + 2 verifiable 200 KiB decisions take two passes, N then 2, with the cursor after
/// the N-th; per-project budgets keep a second project's decision behind 3N `path_absent`
/// decisions from starving, and a terminal reason is never read twice.
#[test]
fn each_pass_verifies_at_most_n_per_project_and_no_project_starves() {
    let store = Store::open();
    let files: Vec<_> = (0..N + 2)
        .map(|i| (format!("doc{i}.md"), text(i, 200 * 1024)))
        .collect();
    let a = Repo::new("a", &files);
    store.scope(&a);
    let decisions: Vec<_> = files
        .iter()
        .enumerate()
        .map(|(i, (path, body))| store.decision(&a, &format!("a{i}"), path, body))
        .collect();
    let counters = Counters::default();
    let mut verifier = Verifier::default();

    let clock = Clock::open();
    verifier.pass(&store.kernel, &[a.project()], &on, &counters, &clock);
    assert_accounting(&counters);
    assert_eq!(counters.get("match"), MAX_VERDICTS);
    assert_eq!(counters.get("attestation_committed"), MAX_VERDICTS);
    assert!(counters.get("bytes_read") <= MAX_VERDICTS * MAX_BLOB_BYTES);
    assert_eq!(counters.get("bytes_read"), MAX_VERDICTS * 200 * 1024);
    let cursor = verifier.cursor(&a.scope_id()).unwrap();
    assert_eq!(cursor.1, decisions[N - 1]);
    assert!(
        decisions[..N]
            .iter()
            .all(|id| exists(&store, &attestation(id, &a)))
    );
    assert!(!exists(&store, &attestation(&decisions[N], &a)));

    let clock = Clock::open();
    verifier.pass(&store.kernel, &[a.project()], &on, &counters, &clock);
    assert_accounting(&counters);
    assert_eq!(clock.opens(), 2);
    assert_eq!(counters.get("attestation_committed"), MAX_VERDICTS + 2);

    // The second scenario: project `b`'s head holds 3N absent paths, `c` one verifiable document.
    let b = Repo::new("b", &[]);
    let c = Repo::new("c", &[("doc.md".into(), text(0, 1024))]);
    store.scope(&b);
    store.scope(&c);
    for i in 0..3 * N {
        store.decision(&b, &format!("b{i}"), &format!("absent{i}.md"), "gone\n");
    }
    let lone = store.decision(&c, "c0", "doc.md", &text(0, 1024));
    let projects = [b.project(), c.project()];
    let counters = Counters::default();
    let mut verifier = Verifier::default();
    for pass in 1..=3 {
        let clock = Clock::open();
        verifier.pass(&store.kernel, &projects, &on, &counters, &clock);
        assert_accounting(&counters);
        assert_eq!(counters.get("unavailable_path_absent"), (pass * N) as u64);
        assert!(exists(&store, &attestation(&lone, &c)), "pass {pass}");
        // Pass 1 opens `c` once; later passes start past its cursor.
        assert_eq!(clock.opens(), N + usize::from(pass == 1));
    }
    let clock = Clock::open();
    verifier.pass(&store.kernel, &projects, &on, &counters, &clock);
    assert_eq!(clock.opens(), 0);
    assert_eq!(counters.get("unavailable_path_absent"), (3 * N) as u64);
}

/// AC4's cursor clause, F7: a deadline between decisions k and k + 1 leaves exactly k verdicts and
/// the cursor after k; a deadline during a read leaves the decision unresolved and unwritten.
#[test]
fn a_deadline_between_decisions_leaves_exactly_k_verdicts_and_the_cursor_before_the_next() {
    let store = Store::open();
    let files: Vec<_> = (0..5)
        .map(|i| (format!("doc{i}.md"), text(i, 1024)))
        .collect();
    let repo = Repo::new("deadline", &files);
    store.scope(&repo);
    let decisions: Vec<_> = files
        .iter()
        .enumerate()
        .map(|(i, (path, body))| store.decision(&repo, &format!("d{i}"), path, body))
        .collect();
    let counters = Counters::default();
    let mut verifier = Verifier::default();
    let k = 3;

    let clock = Clock::with(|poll, opens| poll == Poll::Decision && opens >= k);
    verifier.pass(&store.kernel, &[repo.project()], &on, &counters, &clock);
    assert_accounting(&counters);
    assert_eq!(counters.get("considered"), k as u64);
    assert_eq!(counters.get("match"), k as u64);
    assert_eq!(
        verifier.cursor(&repo.scope_id()).unwrap().1,
        decisions[k - 1]
    );
    assert!(!exists(&store, &attestation(&decisions[k], &repo)));

    let clock = Clock::with(|poll, _| poll == Poll::Read(Stage::Blob));
    verifier.pass(&store.kernel, &[repo.project()], &on, &counters, &clock);
    assert_accounting(&counters);
    assert_eq!(counters.get("unavailable_deadline"), 1);
    assert_eq!(
        verifier.cursor(&repo.scope_id()).unwrap().1,
        decisions[k - 1]
    );
    assert!(!exists(&store, &attestation(&decisions[k], &repo)));

    let clock = Clock::open();
    verifier.pass(&store.kernel, &[repo.project()], &on, &counters, &clock);
    assert_accounting(&counters);
    assert_eq!(counters.get("attestation_committed"), 5);
    assert_eq!(verifier.cursor(&repo.scope_id()).unwrap().1, decisions[4]);
}

/// AC12, F10, F11: a rollback after the admission counts the match and the refusal and no
/// commit, and the next pass commits; a pass whose key another writer committed first counts the
/// replay and leaves one attestation.
#[test]
fn a_rolled_back_commit_counts_its_refusal_and_a_replayed_commit_counts_its_replay() {
    let store = Store::open();
    let files = vec![
        ("one.md".to_owned(), text(1, 1024)),
        ("two.md".to_owned(), text(2, 1024)),
    ];
    let repo = Repo::new("replay", &files);
    store.scope(&repo);
    let first = store.decision(&repo, "r1", &files[0].0, &files[0].1);
    let counters = Counters::default();
    let mut verifier = Verifier::default();
    store
        .sqlite()
        .execute_batch(&format!(
            "CREATE TRIGGER fail_citation BEFORE INSERT ON decision_events
               WHEN NEW.event_kind='{CITATION_EVENT_KIND}'
             BEGIN SELECT RAISE(ABORT,'fault after the admission'); END;"
        ))
        .unwrap();
    verifier.pass(
        &store.kernel,
        &[repo.project()],
        &on,
        &counters,
        &Clock::open(),
    );
    assert_accounting(&counters);
    assert_eq!(counters.get("match"), 1);
    assert_eq!(counters.get("attestation_committed"), 0);
    let refused: Vec<_> = COUNTERS
        .iter()
        .filter(|name| name.starts_with("commit_refused_") && counters.get(name) > 0)
        .collect();
    assert_eq!(refused.len(), 1, "{refused:?}");
    assert_eq!(counters.get(refused[0]), 1);
    assert_eq!(store.verifier_rows(), 0);
    assert_eq!(verifier.cursor(&repo.scope_id()), None);
    store
        .sqlite()
        .execute_batch("DROP TRIGGER fail_citation;")
        .unwrap();
    verifier.pass(
        &store.kernel,
        &[repo.project()],
        &on,
        &counters,
        &Clock::open(),
    );
    assert_accounting(&counters);
    assert_eq!(counters.get("attestation_committed"), 1);
    assert!(exists(&store, &attestation(&first, &repo)));

    // Another writer commits the second decision's write set between the pass's skip check and
    // its commit.
    let second = store.decision(&repo, "r2", &files[1].0, &files[1].1);
    let kernel = &store.kernel;
    let raced = AtomicBool::new(false);
    let clock = Clock::with(|poll, _| {
        if poll == Poll::Commit && !raced.swap(true, Ordering::SeqCst) {
            let budget = EvalBudget::unbounded();
            let target = kernel.capture_commit_read_target().unwrap();
            let classification =
                classify(kernel, &second, target, &[repo.bound()], &budget).unwrap();
            let daemon::attestation_verifier::classify::Class::Verifiable(input) =
                &classification.class
            else {
                panic!("the second decision is verifiable");
            };
            let evaluation = verify(&repo.bare, &input.claim, &budget);
            let outcome = attest(kernel, &classification, &evaluation, &budget);
            assert!(matches!(outcome, Outcome::Committed(_)), "{outcome:?}");
        }
        false
    });
    verifier.pass(kernel, &[repo.project()], &on, &counters, &clock);
    assert_accounting(&counters);
    assert_eq!(counters.get("attestation_replayed"), 1);
    assert_eq!(counters.get("attestation_committed"), 1);
    assert_eq!(
        store.count(&format!(
            "SELECT COUNT(*) FROM object_registry WHERE object_id='{}'",
            attestation(&second, &repo)
        )),
        1
    );
}

/// AC13: a concurrent `kernel.commit` completes within its deadline while a verifier read is
/// stalled, because the pass verifies outside the Kernel writer.
#[test]
fn a_concurrent_commit_completes_while_a_verifier_read_is_stalled() {
    let store = Store::open();
    let repo = Repo::new("stall", &[("doc.md".into(), text(0, 1024))]);
    store.scope(&repo);
    store.decision(&repo, "s0", "doc.md", &text(0, 1024));
    let counters = Counters::default();
    let (stalled_tx, stalled_rx) = mpsc::channel();
    let (release_tx, release_rx) = mpsc::channel::<()>();
    let release_rx = Mutex::new(release_rx);
    let clock = Clock::with(move |poll, _| {
        if poll == Poll::Read(Stage::Blob) {
            stalled_tx.send(()).unwrap();
            release_rx
                .lock()
                .unwrap()
                .recv_timeout(Duration::from_secs(30))
                .unwrap();
        }
        false
    });
    std::thread::scope(|scope| {
        let pass = scope.spawn(|| {
            Verifier::default().pass(&store.kernel, &[repo.project()], &on, &counters, &clock)
        });
        stalled_rx.recv_timeout(Duration::from_secs(30)).unwrap();
        let deadline = Instant::now() + Duration::from_secs(2);
        let committed = store
            .kernel
            .commit_before(deadline, intent("concurrent"), |envelope| {
                envelope.insert_domain(DomainSpec {
                    domain_id: "concurrent".into(),
                    object_id: "concurrent-domain".into(),
                    name: "concurrent".into(),
                    source_kind: "test".into(),
                    source_id: "concurrent".into(),
                    source_revision: 1,
                    sensitivity: Sensitivity::Normal,
                })?;
                Ok(String::new())
            });
        let within = Instant::now() < deadline;
        release_tx.send(()).unwrap();
        pass.join().unwrap();
        assert!(committed.is_ok(), "{committed:?}");
        assert!(within);
    });
    assert_eq!(counters.get("attestation_committed"), 1);
}

/// F13, the transient head: an unbound head decision is retried K passes, then counts
/// `unavailable_skipped` and the cursor passes it and the decision resolved behind it; a
/// terminal reason is read once.
#[test]
fn a_transient_head_resolves_after_k_passes_and_a_terminal_reason_is_read_once() {
    let store = Store::open();
    let repo = Repo::new("head", &[("doc.md".into(), text(0, 1024))]);
    store.scope(&repo);
    store.decision_under(&repo, "git:unbound", "h0", "doc.md", &text(0, 1024));
    store.decision(&repo, "h1", "absent.md", "gone\n");
    let verified = store.decision(&repo, "h2", "doc.md", &text(0, 1024));
    let counters = Counters::default();
    let mut verifier = Verifier::default();

    let clock = Clock::open();
    let report = verifier.pass(&store.kernel, &[repo.project()], &on, &counters, &clock);
    assert!(report.failed && report.progressed);
    assert_accounting(&counters);
    assert_eq!(clock.opens(), 2);
    assert_eq!(counters.get("unavailable_repository_unbound"), 1);
    assert_eq!(counters.get("unavailable_path_absent"), 1);
    assert_eq!(counters.get("attestation_committed"), 1);
    assert_eq!(verifier.cursor(&repo.scope_id()), None);

    for pass in 2..=3 {
        let clock = Clock::open();
        verifier.pass(&store.kernel, &[repo.project()], &on, &counters, &clock);
        assert_accounting(&counters);
        assert_eq!(clock.opens(), 0, "pass {pass}");
        assert_eq!(counters.get("unavailable_repository_unbound"), pass);
        assert_eq!(counters.get("unavailable_path_absent"), 1);
    }
    assert_eq!(counters.get("unavailable_skipped"), 1);
    assert_eq!(verifier.cursor(&repo.scope_id()).unwrap().1, verified);

    let clock = Clock::open();
    let report = verifier.pass(&store.kernel, &[repo.project()], &on, &counters, &clock);
    assert!(!report.progressed && !report.failed);
    assert_eq!(counters.get("considered"), 5);
}

/// The restart rescan: a fresh verifier over a reopened store writes nothing for an attested or
/// `Stale` decision and counts them under the skip buckets.
#[test]
fn a_restart_rescan_writes_nothing_and_counts_skips() {
    let store = Store::open();
    let repo = Repo::new(
        "restart",
        &[
            ("same.md".into(), text(0, 1024)),
            ("changed.md".into(), text(1, 1024)),
        ],
    );
    store.scope(&repo);
    let attested = store.decision(&repo, "m0", "same.md", &text(0, 1024));
    store.decision(&repo, "m1", "changed.md", &text(2, 1024));
    let counters = Counters::default();
    Verifier::default().pass(
        &store.kernel,
        &[repo.project()],
        &on,
        &counters,
        &Clock::open(),
    );
    assert_eq!(counters.get("attestation_committed"), 1);
    assert_eq!(counters.get("stale_committed"), 1);
    assert!(exists(&store, &attestation(&attested, &repo)));

    let store = store.reopen();
    let tip = store.kernel.tip().unwrap();
    let counters = Counters::default();
    let clock = Clock::open();
    Verifier::default().pass(&store.kernel, &[repo.project()], &on, &counters, &clock);
    assert_accounting(&counters);
    assert_eq!(store.kernel.tip().unwrap(), tip);
    assert_eq!(clock.opens(), 0);
    assert_eq!(counters.get("skipped_attested"), 1);
    assert_eq!(counters.get("skipped_stale"), 1);
    assert_eq!(counters.get("considered"), 0);
}

/// The task runs no pass body while the flag is off, runs one once it is on, and returns when
/// cancelled.
#[tokio::test(flavor = "multi_thread")]
async fn the_task_steps_only_under_the_flag_and_stops_on_cancellation() {
    let store = Store::open();
    let repo = Repo::new("task", &[("doc.md".into(), text(0, 1024))]);
    store.scope(&repo);
    store.decision(&repo, "t0", "doc.md", &text(0, 1024));
    let Store { root, kernel } = store;
    let kernel = Arc::new(kernel);
    let counters = Arc::new(Counters::default());
    let flag = Arc::new(AtomicBool::new(false));
    let project = repo.project();
    let start = |flag: Arc<AtomicBool>| {
        let cancel = tokio_util::sync::CancellationToken::new();
        let kernel = Arc::clone(&kernel);
        let project = project.clone();
        let task = tokio::spawn(run(
            Arc::clone(&counters),
            Arc::new(move || Some(Arc::clone(&kernel))),
            Arc::new(move || vec![project.clone()]),
            Arc::new(move |_: &Path| flag.load(Ordering::SeqCst)),
            cancel.clone(),
        ));
        (task, cancel)
    };
    let (task, cancel) = start(Arc::clone(&flag));
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert_eq!(counters.get("passes"), 0);
    cancel.cancel();
    tokio::time::timeout(Duration::from_secs(5), task)
        .await
        .unwrap()
        .unwrap();

    flag.store(true, Ordering::SeqCst);
    let (task, cancel) = start(flag);
    let deadline = Instant::now() + Duration::from_secs(20);
    while counters.get("passes") == 0 && Instant::now() < deadline {
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    assert_eq!(counters.get("passes"), 1);
    assert_eq!(counters.get("attestation_committed"), 1);
    cancel.cancel();
    tokio::time::timeout(Duration::from_secs(5), task)
        .await
        .unwrap()
        .unwrap();
    drop(root);
}

/// The health block names exactly the counters the wire document's `metrics.verifier` table names.
#[test]
fn the_health_block_names_exactly_the_documented_counters() {
    let block = Counters::default().to_json();
    let reported: std::collections::BTreeSet<&str> = block
        .as_object()
        .unwrap()
        .keys()
        .map(String::as_str)
        .collect();
    let doc = include_str!("../../../docs/host-wire-protocol.md");
    let lines: Vec<&str> = doc.lines().collect();
    let first = lines
        .iter()
        .position(|line| line.starts_with("| `considered`"))
        .expect("the verifier table");
    let documented: std::collections::BTreeSet<&str> = lines[first..]
        .iter()
        .take_while(|line| line.starts_with('|'))
        .flat_map(|line| {
            line.split(" | ")
                .next()
                .unwrap_or_default()
                .split('`')
                .skip(1)
                .step_by(2)
        })
        .collect();
    assert_eq!(reported, documented);
    assert!(block.as_object().unwrap().values().all(|value| value == 0));
}

/// OQ6-11: a decision whose witness degrades every pass counts `witness_degraded` K times, then
/// `witness_degraded_skipped`, and the cursor passes it while the task continues.
#[test]
fn a_witness_degraded_k_passes_is_skipped_and_the_pass_continues() {
    let store = Store::open();
    let repo = Repo::new(
        "degraded",
        &[
            ("one.md".into(), text(1, 1024)),
            ("two.md".into(), text(2, 1024)),
        ],
    );
    store.scope(&repo);
    let degraded = store.decision(&repo, "w0", "one.md", &text(1, 1024));
    store
        .sqlite()
        .execute_batch(&format!(
            "CREATE TRIGGER degrade_witness AFTER INSERT ON observations
               WHEN NEW.object_id='{}'
             BEGIN
               UPDATE observations SET observation_kind='code_absent'
                WHERE object_id=NEW.object_id;
             END;",
            Identity::of(&degraded, &repo.head).witness_object_id
        ))
        .unwrap();
    let counters = Counters::default();
    let mut verifier = Verifier::default();
    for pass in 1..=3 {
        if pass == 2 {
            store.decision(&repo, "w1", "two.md", &text(2, 1024));
        }
        verifier.pass(
            &store.kernel,
            &[repo.project()],
            &on,
            &counters,
            &Clock::open(),
        );
        assert_accounting(&counters);
        assert_eq!(counters.get("witness_degraded"), pass);
        assert!(!exists(&store, &attestation(&degraded, &repo)));
    }
    assert_eq!(counters.get("witness_degraded_skipped"), 1);
    assert_eq!(counters.get("attestation_committed"), 1);
    let cursor = verifier.cursor(&repo.scope_id()).unwrap();
    assert_eq!(cursor.1, "mem_w1");
}

/// Commits `object_id`'s write set outside any pass, as another writer would.
fn attest_now(kernel: &KernelStore, repo: &Repo, object_id: &str) {
    let budget = EvalBudget::unbounded();
    let target = kernel.capture_commit_read_target().unwrap();
    let classification = classify(kernel, object_id, target, &[repo.bound()], &budget).unwrap();
    let daemon::attestation_verifier::classify::Class::Verifiable(input) = &classification.class
    else {
        panic!("{object_id} is verifiable");
    };
    let evaluation = verify(&repo.bare, &input.claim, &budget);
    let outcome = attest(kernel, &classification, &evaluation, &budget);
    assert!(matches!(outcome, Outcome::Committed(_)), "{outcome:?}");
}

/// A head decision whose every attempt ends at the deadline, at its commit or during its read,
/// counts `unavailable_skipped` once after K passes, the cursor passes it, and no later pass reads
/// it.
#[test]
fn a_head_decision_that_meets_the_deadline_k_passes_is_skipped_and_read_no_more() {
    for deadline in [Poll::Commit, Poll::Read(Stage::Blob)] {
        let store = Store::open();
        let repo = Repo::new("late", &[("doc.md".into(), text(0, 1024))]);
        store.scope(&repo);
        let head = store.decision(&repo, "l0", "doc.md", &text(0, 1024));
        let counters = Counters::default();
        let mut verifier = Verifier::default();
        for pass in 1..=3 {
            let clock = Clock::with(move |poll, _| poll == deadline);
            verifier.pass(&store.kernel, &[repo.project()], &on, &counters, &clock);
            assert_accounting(&counters);
            assert_eq!(counters.get("considered"), pass, "{deadline:?}");
        }
        assert_eq!(counters.get("unavailable_skipped"), 1, "{deadline:?}");
        assert_eq!(verifier.cursor(&repo.scope_id()).unwrap().1, head);
        assert_eq!(store.verifier_rows(), 0);

        let clock = Clock::open();
        verifier.pass(&store.kernel, &[repo.project()], &on, &counters, &clock);
        assert_eq!(clock.opens(), 0);
        assert_eq!(counters.get("considered"), 3);
        assert_eq!(counters.get("unavailable_skipped"), 1);
    }
}

/// F7 at the commit poll: the verdict counts, nothing commits, the cursor stays, and the next pass
/// commits.
#[test]
fn a_deadline_at_the_commit_leaves_the_decision_unwritten_and_the_next_pass_commits() {
    let store = Store::open();
    let repo = Repo::new("commit", &[("doc.md".into(), text(0, 1024))]);
    store.scope(&repo);
    let decision = store.decision(&repo, "c0", "doc.md", &text(0, 1024));
    let counters = Counters::default();
    let mut verifier = Verifier::default();
    let clock = Clock::with(|poll, _| poll == Poll::Commit);
    verifier.pass(&store.kernel, &[repo.project()], &on, &counters, &clock);
    assert_accounting(&counters);
    assert_eq!(counters.get("match"), 1);
    assert_eq!(counters.get("attestation_committed"), 0);
    assert_eq!(store.verifier_rows(), 0);
    assert_eq!(verifier.cursor(&repo.scope_id()), None);

    verifier.pass(
        &store.kernel,
        &[repo.project()],
        &on,
        &counters,
        &Clock::open(),
    );
    assert_eq!(counters.get("attestation_committed"), 1);
    assert_eq!(verifier.cursor(&repo.scope_id()).unwrap().1, decision);
}

/// AC12's accounting with skips and unverifiable decisions ahead of N + 1 verifiable ones: they
/// count in their buckets and outside N, and the cursor stops at the N-th verifiable decision.
#[test]
fn unverifiable_and_attested_decisions_count_outside_n() {
    let store = Store::open();
    let files: Vec<_> = (0..N + 3)
        .map(|i| (format!("doc{i}.md"), text(i, 1024)))
        .collect();
    let repo = Repo::new("mixed", &files);
    store.scope(&repo);
    let reachable = AnchorValue::ReachableFrom {
        oid: repo.head.clone(),
        captures: Vec::new(),
    };
    store.model_decision(&repo, "u0", "", Some(reachable), None);
    let exact = AnchorValue::Exact { value: "v1".into() };
    store.model_decision(&repo, "u1", "", Some(exact), None);
    store.model_decision(&repo, "u2", "", None, None);
    let decisions: Vec<_> = files
        .iter()
        .enumerate()
        .map(|(i, (path, body))| store.decision(&repo, &format!("m{i}"), path, body))
        .collect();
    for attested in &decisions[..2] {
        attest_now(&store.kernel, &repo, attested);
    }
    let counters = Counters::default();
    let mut verifier = Verifier::default();
    let clock = Clock::open();
    verifier.pass(&store.kernel, &[repo.project()], &on, &counters, &clock);
    assert_accounting(&counters);
    assert_eq!(counters.get("match"), MAX_VERDICTS);
    assert_eq!(counters.get("unverifiable_no_citation"), 1);
    assert_eq!(counters.get("unverifiable_anchor_not_git"), 1);
    assert_eq!(counters.get("unverifiable_no_anchor"), 1);
    assert_eq!(counters.get("skipped_attested"), 2);
    assert_eq!(counters.get("considered"), MAX_VERDICTS + 3);
    assert_eq!(clock.opens(), N);
    assert_eq!(
        verifier.cursor(&repo.scope_id()).unwrap().1,
        decisions[N + 1]
    );
}

/// A worktree root takes its repository's `git:` identity, so a document published under it
/// verifies; a root nested inside the worktree binds no repository.
#[test]
fn a_worktree_root_verifies_under_its_git_identity_and_a_nested_root_binds_none() {
    let store = Store::open();
    let top = Repo::new("top", &[("doc.md".into(), text(0, 1024))]);
    let nested = Repo::new("nested", &[("sub/doc.md".into(), text(1, 1024))]);
    store.scope(&top);
    store.scope(&nested);
    let attested = store.decision_under(
        &top,
        &format!("git:{}", top.head),
        "t0",
        "doc.md",
        &text(0, 1024),
    );
    store.decision_under(
        &nested,
        &format!("git:{}", nested.head),
        "n0",
        "doc.md",
        &text(1, 1024),
    );
    let projects = [
        top.project_at(&top.work),
        nested.project_at(&nested.work.join("sub")),
    ];
    let counters = Counters::default();
    Verifier::default().pass(&store.kernel, &projects, &on, &counters, &Clock::open());
    assert_accounting(&counters);
    assert_eq!(counters.get("match"), 1);
    assert_eq!(counters.get("unavailable_repository_unbound"), 1);
    assert!(exists(&store, &attestation(&attested, &top)));
}

/// Points `repo`'s worktree `.git` at a missing directory, so its root's derivation fails.
fn break_gitfile(repo: &Repo) {
    let git = repo.work.join(".git");
    std::fs::rename(&git, repo.work.join("hidden.git")).unwrap();
    std::fs::write(&git, "gitdir: /nonexistent/eidnara-repository\n").unwrap();
}

/// A root whose derivation the environment fails binds no repository for the pass and is derived
/// again next pass: its decision counts `unavailable_repository_unbound` K passes, then
/// `unavailable_skipped`, and the cursor passes it, while another project proceeds.
#[test]
fn a_failed_derivation_leaves_the_root_unbound_and_its_decision_is_skipped_after_k_passes() {
    let store = Store::open();
    let broken = Repo::new("broken", &[("doc.md".into(), text(0, 1024))]);
    let healthy = Repo::new("healthy", &[("doc.md".into(), text(1, 1024))]);
    store.scope(&broken);
    store.scope(&healthy);
    let waiting = store.decision_under(
        &broken,
        &format!("git:{}", broken.head),
        "b0",
        "doc.md",
        &text(0, 1024),
    );
    store.decision(&healthy, "h0", "doc.md", &text(1, 1024));
    break_gitfile(&broken);
    let projects = [broken.project_at(&broken.work), healthy.project()];
    let counters = Counters::default();
    let mut verifier = Verifier::default();
    for pass in 1..=3 {
        let clock = Clock::open();
        let report = verifier.pass(&store.kernel, &projects, &on, &counters, &clock);
        assert!(report.failed, "pass {pass}");
        assert_accounting(&counters);
        assert_eq!(counters.get("unavailable_repository_unbound"), pass);
        assert_eq!(clock.polled(Poll::Derive), if pass == 1 { 2 } else { 1 });
    }
    assert_eq!(counters.get("attestation_committed"), 1);
    assert_eq!(counters.get("unavailable_skipped"), 1);
    assert_eq!(verifier.cursor(&broken.scope_id()).unwrap().1, waiting);
}

/// A broken root beside a healthy one in the same project leaves the healthy root bound, so a
/// decision published under it verifies.
#[test]
fn a_broken_root_leaves_the_projects_healthy_root_bound() {
    let store = Store::open();
    let project = Repo::new("duo", &[("doc.md".into(), text(0, 1024))]);
    let broken = Repo::new("duo-broken", &[]);
    store.scope(&project);
    let verified = store.decision(&project, "d0", "doc.md", &text(0, 1024));
    break_gitfile(&broken);
    let projects = [Project {
        scope_id: project.scope_id(),
        roots: vec![
            Root {
                path: broken.work.clone(),
                digest: project.digest.clone(),
            },
            Root {
                path: project.bare.clone(),
                digest: project.digest.clone(),
            },
        ],
    }];
    let counters = Counters::default();
    let mut verifier = Verifier::default();
    verifier.pass(&store.kernel, &projects, &on, &counters, &Clock::open());
    assert_accounting(&counters);
    assert_eq!(counters.get("attestation_committed"), 1);
    assert!(exists(&store, &attestation(&verified, &project)));
}

/// A deadline at the derivation poll passes over the project before any decision is read and
/// caches no identity; the next open pass derives and verifies.
#[test]
fn a_deadline_at_the_derivation_passes_over_the_project_and_the_next_pass_verifies() {
    let store = Store::open();
    let repo = Repo::new("derive", &[("doc.md".into(), text(0, 1024))]);
    store.scope(&repo);
    let decision = store.decision(&repo, "v0", "doc.md", &text(0, 1024));
    let counters = Counters::default();
    let mut verifier = Verifier::default();
    let clock = Clock::with(|poll, _| poll == Poll::Derive);
    let report = verifier.pass(&store.kernel, &[repo.project()], &on, &counters, &clock);
    assert!(report.failed);
    assert_eq!(counters.get("considered"), 0);
    assert_eq!(clock.opens(), 0);
    assert_eq!(verifier.cursor(&repo.scope_id()), None);

    let clock = Clock::open();
    verifier.pass(&store.kernel, &[repo.project()], &on, &counters, &clock);
    assert_eq!(clock.polled(Poll::Derive), 1);
    assert_eq!(counters.get("attestation_committed"), 1);
    assert_eq!(verifier.cursor(&repo.scope_id()).unwrap().1, decision);
}

/// A pass budget cancelled at the derivation poll, which still lets the derivation run, cuts the
/// walk short with the pass budget exhausted: the project is passed over before any decision is
/// read and no identity is cached, so the next open pass derives and attests.
#[test]
fn a_pass_budget_exhausted_during_the_derivation_passes_over_the_project() {
    let store = Store::open();
    let repo = Repo::new("walk", &[("doc.md".into(), text(0, 1024))]);
    store.scope(&repo);
    let identity = format!("git:{}", repo.head);
    let decision = store.decision_under(&repo, &identity, "w0", "doc.md", &text(0, 1024));
    // The worktree root's derivation walks its history; a bare root falls back before any walk.
    // The second root shows the cut passes over the project's remaining roots.
    let mut project = repo.project_at(&repo.work);
    project.roots.push(Root {
        path: repo.bare.clone(),
        digest: repo.digest.clone(),
    });
    let projects = [project];
    let counters = Counters::default();
    let mut verifier = Verifier::default();
    let clock = Clock::with_budget(|poll, _, budget| {
        if poll == Poll::Derive {
            budget.cancel();
        }
        false
    });
    let report = verifier.pass(&store.kernel, &projects, &on, &counters, &clock);
    assert!(report.failed);
    assert_eq!(clock.polled(Poll::Derive), 1);
    assert_eq!(counters.get("considered"), 0);
    assert_eq!(clock.opens(), 0);
    assert_eq!(verifier.cursor(&repo.scope_id()), None);

    let clock = Clock::open();
    verifier.pass(&store.kernel, &projects, &on, &counters, &clock);
    assert_eq!(clock.polled(Poll::Derive), 2);
    assert_eq!(counters.get("attestation_committed"), 1);
    assert_eq!(verifier.cursor(&repo.scope_id()).unwrap().1, decision);
}

/// A decision whose classification meets the deadline, the pass budget cancelled at the decision
/// poll right before it is classified, spends an attempt each pass; after K passes it counts
/// `unavailable_skipped` once and the cursor passes it.
#[test]
fn a_classification_deadline_k_passes_is_skipped_and_the_cursor_passes_it() {
    let store = Store::open();
    let repo = Repo::new("classify", &[("doc.md".into(), text(0, 1024))]);
    store.scope(&repo);
    let head = store.decision(&repo, "k0", "doc.md", &text(0, 1024));
    // A decision after the head shows the deadline stops the pass instead of charging it.
    store.decision(&repo, "k1", "other.md", &text(1, 1024));
    let counters = Counters::default();
    let mut verifier = Verifier::default();
    for _ in 0..3 {
        let decisions = std::sync::atomic::AtomicUsize::new(0);
        // The first decision poll comes before the project, the second before the decision.
        let clock = Clock::with_budget(move |poll, _, budget| {
            if poll == Poll::Decision && decisions.fetch_add(1, Ordering::SeqCst) == 1 {
                budget.cancel();
            }
            false
        });
        let report = verifier.pass(&store.kernel, &[repo.project()], &on, &counters, &clock);
        assert!(report.failed);
        assert_eq!(clock.opens(), 0);
        assert_accounting(&counters);
    }
    assert_eq!(counters.get("considered"), 0);
    assert_eq!(counters.get("unavailable_skipped"), 1);
    assert_eq!(verifier.cursor(&repo.scope_id()).unwrap().1, head);
    assert_eq!(store.verifier_rows(), 0);
}

/// A decision resolves only against its own project's roots: a document whose repository is bound
/// only under another project counts `unavailable_repository_unbound` K passes, then
/// `unavailable_skipped`.
#[test]
fn a_decision_resolves_only_against_its_own_projects_roots() {
    let store = Store::open();
    let a = Repo::new("own-a", &[("doc.md".into(), text(0, 1024))]);
    let b = Repo::new("own-b", &[("doc.md".into(), text(0, 1024))]);
    store.scope(&a);
    store.scope(&b);
    let foreign = store.decision_under(&a, &b.digest, "f0", "doc.md", &text(0, 1024));
    let projects = [a.project(), b.project()];
    let counters = Counters::default();
    let mut verifier = Verifier::default();
    for pass in 1..=3 {
        let clock = Clock::open();
        verifier.pass(&store.kernel, &projects, &on, &counters, &clock);
        assert_accounting(&counters);
        assert_eq!(clock.opens(), 0);
        assert_eq!(counters.get("unavailable_repository_unbound"), pass);
    }
    assert_eq!(counters.get("unavailable_skipped"), 1);
    assert_eq!(counters.get("match"), 0);
    assert_eq!(verifier.cursor(&a.scope_id()).unwrap().1, foreign);
}

/// A project dropped from the bound set and bound again keeps its terminal reason: the third pass
/// opens nothing and counts nothing.
#[test]
fn a_terminal_reason_survives_a_project_unbound_and_bound_again() {
    let store = Store::open();
    let repo = Repo::new("prune", &[]);
    let other = Repo::new("prune-other", &[]);
    store.scope(&repo);
    store.decision(&repo, "a0", "absent.md", "gone\n");
    let counters = Counters::default();
    let mut verifier = Verifier::default();
    let clock = Clock::open();
    verifier.pass(&store.kernel, &[repo.project()], &on, &counters, &clock);
    assert_eq!(clock.opens(), 1);
    assert_eq!(counters.get("unavailable_path_absent"), 1);

    verifier.pass(
        &store.kernel,
        &[other.project()],
        &on,
        &counters,
        &Clock::open(),
    );
    assert_eq!(verifier.cursor(&repo.scope_id()), None);

    let clock = Clock::open();
    verifier.pass(&store.kernel, &[repo.project()], &on, &counters, &clock);
    assert_eq!(clock.opens(), 0);
    assert_eq!(counters.get("considered"), 1);
}

/// With one project's flag on and another's off, the off project's scope gets no cursor, no
/// attestation, and no repository identity derivation.
#[test]
fn a_flag_off_project_beside_an_enabled_one_is_left_untouched() {
    let store = Store::open();
    let enabled = Repo::new("enabled", &[("doc.md".into(), text(0, 1024))]);
    let disabled = Repo::new("disabled", &[("doc.md".into(), text(1, 1024))]);
    store.scope(&enabled);
    store.scope(&disabled);
    store.decision(&enabled, "e0", "doc.md", &text(0, 1024));
    let untouched = store.decision(&disabled, "d0", "doc.md", &text(1, 1024));
    let projects = [disabled.project(), enabled.project()];
    let flag = |root: &Path| root == enabled.bare;
    let counters = Counters::default();
    let mut verifier = Verifier::default();
    let clock = Clock::open();
    verifier.pass(&store.kernel, &projects, &flag, &counters, &clock);
    assert_eq!(clock.polled(Poll::Derive), 1);
    assert_eq!(counters.get("attestation_committed"), 1);
    assert_eq!(verifier.cursor(&disabled.scope_id()), None);
    assert!(!exists(&store, &attestation(&untouched, &disabled)));

    let clock = Clock::open();
    verifier.pass(&store.kernel, &projects, &flag, &counters, &clock);
    assert_eq!(clock.polled(Poll::Derive), 0);
    assert_eq!(verifier.cursor(&disabled.scope_id()), None);
}

/// A transient head ahead of more than one page of resolvable decisions: each is counted once,
/// and after K passes the cursor lands on the last.
#[test]
fn a_transient_head_ahead_of_a_page_boundary_counts_each_decision_once() {
    let store = Store::open();
    let repo = Repo::new("pages", &[("doc.md".into(), text(0, 1024))]);
    store.scope(&repo);
    store.decision_under(&repo, "git:unbound", "p0", "doc.md", &text(0, 1024));
    let behind = PAGE_ROWS.get() + 1;
    let last = (0..behind)
        .map(|i| {
            let anchor = AnchorValue::ReachableFrom {
                oid: repo.head.clone(),
                captures: Vec::new(),
            };
            store.model_decision(&repo, &format!("q{i}"), "", Some(anchor), None)
        })
        .last()
        .unwrap();
    let counters = Counters::default();
    let mut verifier = Verifier::default();
    for pass in 1..=3 {
        verifier.pass(
            &store.kernel,
            &[repo.project()],
            &on,
            &counters,
            &Clock::open(),
        );
        assert_accounting(&counters);
        assert_eq!(counters.get("unverifiable_no_citation"), behind as u64);
        assert_eq!(counters.get("unavailable_repository_unbound"), pass);
    }
    assert_eq!(counters.get("unavailable_skipped"), 1);
    assert_eq!(verifier.cursor(&repo.scope_id()).unwrap().1, last);
}

/// A terminal reason survives a change of the store's incarnation: the rescan from the reset
/// cursor does not read the decision again.
#[test]
fn a_terminal_reason_is_not_read_again_after_an_incarnation_change() {
    let store = Store::open();
    let repo = Repo::new("incarnation", &[]);
    store.scope(&repo);
    let absent = store.decision(&repo, "i0", "absent.md", "gone\n");
    let counters = Counters::default();
    let mut verifier = Verifier::default();
    let clock = Clock::open();
    verifier.pass(&store.kernel, &[repo.project()], &on, &counters, &clock);
    assert_eq!(clock.opens(), 1);
    assert_eq!(counters.get("unavailable_path_absent"), 1);

    let store = store.reopen();
    let clock = Clock::open();
    verifier.pass(&store.kernel, &[repo.project()], &on, &counters, &clock);
    assert_eq!(clock.polled(Poll::Decision), 2);
    assert_eq!(clock.opens(), 0);
    assert_eq!(counters.get("considered"), 1);
    assert_eq!(verifier.cursor(&repo.scope_id()).unwrap().1, absent);
}

/// The task waits [`RETRY_INTERVAL`] after a pass that progressed and [`IDLE_INTERVAL`] after an
/// idle one.
#[tokio::test(start_paused = true)]
async fn the_task_paces_passes_by_their_progress() {
    let store = Store::open();
    let repo = Repo::new("pace", &[("doc.md".into(), text(0, 1024))]);
    store.scope(&repo);
    store.decision(&repo, "p0", "doc.md", &text(0, 1024));
    let Store { root, kernel } = store;
    let kernel = Arc::new(kernel);
    let starts = Arc::new(Mutex::new(Vec::new()));
    let cancel = tokio_util::sync::CancellationToken::new();
    let project = repo.project();
    let task = tokio::spawn(run(
        Arc::new(Counters::default()),
        Arc::new({
            let starts = Arc::clone(&starts);
            move || {
                starts.lock().unwrap().push(tokio::time::Instant::now());
                Some(Arc::clone(&kernel))
            }
        }),
        Arc::new(move || vec![project.clone()]),
        Arc::new(|_: &Path| true),
        cancel.clone(),
    ));
    while starts.lock().unwrap().len() < 3 {
        tokio::time::sleep(Duration::from_secs(1)).await;
    }
    cancel.cancel();
    task.await.unwrap();
    let starts = starts.lock().unwrap();
    assert_eq!(starts[1] - starts[0], RETRY_INTERVAL);
    assert_eq!(starts[2] - starts[1], IDLE_INTERVAL);
    drop(root);
}

/// Cancelling the task while a pass is in flight raises the pass's budget, so the pass stops at its
/// next poll and the task returns well before [`PASS_DEADLINE`].
#[tokio::test(flavor = "multi_thread")]
async fn cancelling_an_in_flight_pass_returns_before_its_deadline() {
    let store = Store::open();
    let repo = Repo::new("cancel", &[("doc.md".into(), text(0, 1024))]);
    store.scope(&repo);
    store.decision(&repo, "x0", "doc.md", &text(0, 1024));
    let Store { root, kernel } = store;
    let kernel = Arc::new(kernel);
    let counters = Arc::new(Counters::default());
    let cancel = tokio_util::sync::CancellationToken::new();
    let (entered_tx, entered_rx) = tokio::sync::oneshot::channel();
    let entered_tx = Mutex::new(Some(entered_tx));
    let project = repo.project();
    let task = tokio::spawn(run(
        Arc::clone(&counters),
        Arc::new(move || Some(Arc::clone(&kernel))),
        Arc::new(move || vec![project.clone()]),
        Arc::new({
            let cancel = cancel.clone();
            move |_: &Path| {
                if let Some(entered) = entered_tx.lock().unwrap().take() {
                    entered.send(()).unwrap();
                }
                let stop = Instant::now() + Duration::from_secs(30);
                while !cancel.is_cancelled() && Instant::now() < stop {
                    std::thread::sleep(Duration::from_millis(1));
                }
                true
            }
        }),
        cancel.clone(),
    ));
    entered_rx.await.unwrap();
    let cancelled = Instant::now();
    cancel.cancel();
    tokio::time::timeout(Duration::from_secs(30), task)
        .await
        .unwrap()
        .unwrap();
    assert!(cancelled.elapsed() < PASS_DEADLINE / 2);
    assert_eq!(counters.get("passes"), 1);
    assert_eq!(counters.get("considered"), 0);
    drop(root);
}
