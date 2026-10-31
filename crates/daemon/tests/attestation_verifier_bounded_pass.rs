//! Seam 3: the verifier's bounded pass over a fixture store, bare repositories as project roots,
//! a controlled clock, and a switchable flag.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, mpsc};
use std::time::{Duration, Instant};

use daemon::attestation_verifier::attest::{CITATION_EVENT_KIND, Identity, Outcome, attest};
use daemon::attestation_verifier::bounded_pass::{
    COUNTERS, Counters, IDLE_INTERVAL, MAX_ATTEMPTS, MAX_VERDICTS, PAGE_ROWS, PASS_DEADLINE,
    PassClock, PassReport, Poll, Project, RETRY_INTERVAL, Root, Verifier,
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
        // Fixed dates fix each head oid, so the attestation ids and their page order replay.
        .env("GIT_AUTHOR_DATE", "@1 +0000")
        .env("GIT_COMMITTER_DATE", "@1 +0000")
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

/// `attested` in the applicability page's order: an attestation's `source_id` is the hash its
/// object id carries after the `verattest:` prefix, so object id order is page order.
fn page_order(attested: &[String]) -> Vec<String> {
    let mut ordered = attested.to_vec();
    ordered.sort();
    ordered
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
    // The trigger's abort reaches the commit as a refusal outside the named reasons.
    assert_eq!(refused, [&"commit_refused_other"]);
    assert_eq!(counters.get("commit_refused_other"), 1);
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
    // The project, the decision, and the applicability step's one page.
    assert_eq!(clock.polled(Poll::Decision), 3);
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

/// Publishes revision `revision` of the document `name` at `path` in `repo` with retained bytes
/// `text`, as source capture republishes a document whose digest changed.
fn republish(store: &Store, repo: &Repo, name: &str, path: &str, text: &str, revision: i64) {
    let handle = store
        .kernel
        .ingest_exact_artifact(ArtifactIngestRequest {
            intent: intent(&format!("artifact-{name}-r{revision}")),
            payload: text.as_bytes().to_vec(),
            evidence_id: format!("srcev:{name}:r{revision}"),
            object_id: format!("srcev-object:{name}:r{revision}"),
            object_kind: "evidence".into(),
            domain_id: "source".into(),
            source_kind: "test".into(),
            source_id: format!("source/{name}/r{revision}"),
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
        ("repository_id", repo.digest.as_str()),
        ("relative_path", path),
    ];
    let revision_text = revision.to_string();
    store
        .kernel
        .commit(
            intent(&format!("descriptor-{name}-r{revision}")),
            |envelope| {
                envelope
                    .publish_source_descriptor(&SourceDescriptorRequest {
                        occurrence: Occurrence {
                            class: OccurrenceClass::RepositoryDocuments.code(),
                            identity: &identity,
                            revision: &revision_text,
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
                        expected_head: Some(revision - 1),
                        outcome: None,
                    })
                    .map_err(|_| KernelError::InvalidInput)?;
                Ok(String::new())
            },
        )
        .unwrap();
}

fn sha256(text: &str) -> String {
    format!("{:x}", Sha256::digest(text.as_bytes()))
}

/// The `MarkStale` rows on `object_id`, as `(source_class, taint_class, evidence_id, reason)`.
fn stale_marks(store: &Store, object_id: &str) -> Vec<(String, String, Option<String>, String)> {
    let connection = store.sqlite();
    let mut statement = connection
        .prepare(
            "SELECT source_class, taint_class, evidence_id, reason FROM admission_decisions
              WHERE subject_object_id=?1 AND event_kind='mark_stale'",
        )
        .unwrap();
    statement
        .query_map([object_id], |row| {
            Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?))
        })
        .unwrap()
        .collect::<Result<_, _>>()
        .unwrap()
}

/// The model decision's registry row and admission history, to compare across passes.
fn standing_of(store: &Store, object_id: &str) -> String {
    store
        .sqlite()
        .query_row(
            "SELECT (SELECT group_concat(admission_decision_id||':'||disposition||':'||maturity, ',')
                       FROM admission_decisions WHERE subject_object_id=?1)
                    ||'|'||ifnull(o.invalidated_commit_seq, '')||'|'||o.source_revision
               FROM object_registry o WHERE o.object_id=?1",
            [object_id],
            |row| row.get(0),
        )
        .unwrap()
}

fn applicability_counts(counters: &Counters) -> [u64; 3] {
    [
        "applicability_stale",
        "applicability_current",
        "applicability_unavailable",
    ]
    .map(|name| counters.get(name))
}

/// A pass over `repo`'s project with the open clock.
fn pass(verifier: &mut Verifier, store: &Store, repo: &Repo, counters: &Counters) -> PassReport {
    let report = verifier.pass(
        &store.kernel,
        &[repo.project()],
        &on,
        counters,
        &Clock::open(),
    );
    assert_accounting(counters);
    report
}

/// AC18, `p6-new-attestation-stale-on-byte-change`: after a `Match`, a pass over an unchanged
/// document counts the attestation current; after source capture republishes the document with
/// another digest, the next pass writes exactly one `MarkStale` on the attestation under its
/// stored classes, with no evidence, naming both digests and the descriptor revision; a second
/// pass writes nothing; the model decision is unchanged throughout.
#[test]
fn a_republished_document_marks_its_attestation_stale_once_and_leaves_the_model_decision_unchanged()
{
    let store = Store::open();
    let witnessed = text(0, 1024);
    let repo = Repo::new("republish", &[("doc.md".into(), witnessed.clone())]);
    store.scope(&repo);
    let model = store.decision(&repo, "p0", "doc.md", &witnessed);
    let counters = Counters::default();
    let mut verifier = Verifier::default();
    pass(&mut verifier, &store, &repo, &counters);
    assert_eq!(counters.get("attestation_committed"), 1);
    let attested = attestation(&model, &repo);
    let model_standing = standing_of(&store, &model);

    pass(&mut verifier, &store, &repo, &counters);
    assert_eq!(applicability_counts(&counters), [0, 1, 0]);
    assert!(stale_marks(&store, &attested).is_empty());
    let stored: (String, String) = store
        .sqlite()
        .query_row(
            "SELECT source_class, taint_class FROM admission_decisions WHERE subject_object_id=?1
              ORDER BY commit_seq DESC LIMIT 1",
            [&attested],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .unwrap();

    let republished = text(1, 1024);
    republish(&store, &repo, "p0", "doc.md", &republished, 2);
    let report = pass(&mut verifier, &store, &repo, &counters);
    assert!(report.progressed && !report.failed, "{report:?}");
    assert_eq!(applicability_counts(&counters), [1, 1, 0]);
    let marks = stale_marks(&store, &attested);
    assert_eq!(marks.len(), 1, "{marks:?}");
    let (source_class, taint_class, evidence_id, reason) = &marks[0];
    assert_eq!((source_class, taint_class), (&stored.0, &stored.1));
    assert_eq!(evidence_id, &None);
    assert!(reason.contains(&sha256(&witnessed)), "{reason}");
    assert!(reason.contains(&sha256(&republished)), "{reason}");
    assert!(reason.contains("descriptor revision 2"), "{reason}");
    let key: i64 = store.count(&format!(
        "SELECT COUNT(*) FROM operation_receipts WHERE producer='verifier'
           AND operation_key='applicability:{attested}:2'"
    ));
    assert_eq!(key, 1);
    let disposition: String = store
        .sqlite()
        .query_row(
            "SELECT disposition FROM admission_decisions WHERE subject_object_id=?1
              ORDER BY commit_seq DESC LIMIT 1",
            [&attested],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(disposition, "stale");

    let rows = store.verifier_rows();
    let admissions = store.count("SELECT COUNT(*) FROM admission_decisions");
    let report = pass(&mut verifier, &store, &repo, &counters);
    assert!(!report.progressed && !report.failed, "{report:?}");
    assert_eq!(applicability_counts(&counters), [1, 1, 0]);
    assert_eq!(store.verifier_rows(), rows);
    assert_eq!(
        store.count("SELECT COUNT(*) FROM admission_decisions"),
        admissions
    );
    assert_eq!(standing_of(&store, &model), model_standing);
}

/// AC18's silent cases: a republish with an equal digest counts current, a retired descriptor, an
/// undecodable witness detail, and a witness detail at another `witness_version` count
/// unavailable, and none writes.
#[test]
fn an_equal_republish_a_retired_descriptor_and_an_undecodable_witness_write_nothing() {
    use daemon::attestation_verifier::applicability::{Applicability, evaluate};
    let store = Store::open();
    let files: Vec<_> = (0..4)
        .map(|i| (format!("doc{i}.md"), text(i, 1024)))
        .collect();
    let repo = Repo::new("silent", &files);
    store.scope(&repo);
    let models: Vec<_> = files
        .iter()
        .enumerate()
        .map(|(i, (path, body))| store.decision(&repo, &format!("s{i}"), path, body))
        .collect();
    let counters = Counters::default();
    let mut verifier = Verifier::default();
    pass(&mut verifier, &store, &repo, &counters);
    assert_eq!(counters.get("attestation_committed"), 4);

    republish(&store, &repo, "s0", &files[0].0, &files[0].1, 2);
    let descriptor: String = store
        .sqlite()
        .query_row(
            "SELECT object_id FROM observations WHERE evidence_id='srcev:s1'
               AND invalidated_commit_seq IS NULL",
            [],
            |row| row.get(0),
        )
        .unwrap();
    store
        .kernel
        .commit(intent("retire-s1"), |envelope| {
            envelope.retire_observation(&descriptor)?;
            Ok(String::new())
        })
        .unwrap();
    let witness = Identity::of(&models[2], &repo.head).witness_object_id;
    let changed = store
        .sqlite()
        .execute(
            "UPDATE observations SET observation_payload=CAST(json_set(
               CAST(observation_payload AS TEXT), '$.detail', '{}') AS BLOB) WHERE object_id=?1",
            [&witness],
        )
        .unwrap();
    assert_eq!(changed, 1);
    let witness = Identity::of(&models[3], &repo.head).witness_object_id;
    let changed = store
        .sqlite()
        .execute(
            "UPDATE observations SET observation_payload=CAST(json_set(
               CAST(observation_payload AS TEXT), '$.detail', replace(json_extract(
               CAST(observation_payload AS TEXT), '$.detail'), '\"witness_version\":1',
               '\"witness_version\":2')) AS BLOB) WHERE object_id=?1",
            [&witness],
        )
        .unwrap();
    assert_eq!(changed, 1);
    let target = store.kernel.capture_commit_read_target().unwrap();
    let standings: Vec<_> = models
        .iter()
        .map(|model| {
            evaluate(
                &store.kernel,
                &attestation(model, &repo),
                target,
                &EvalBudget::unbounded(),
            )
            .unwrap()
        })
        .collect();
    assert_eq!(
        standings,
        [
            Applicability::Current,
            Applicability::Unavailable,
            Applicability::Unavailable,
            Applicability::Unavailable
        ]
    );
    let rows = store.verifier_rows();
    let admissions = store.count("SELECT COUNT(*) FROM admission_decisions");
    pass(&mut verifier, &store, &repo, &counters);
    assert_eq!(applicability_counts(&counters), [0, 1, 3]);
    assert_eq!(store.verifier_rows(), rows);
    assert_eq!(
        store.count("SELECT COUNT(*) FROM admission_decisions"),
        admissions
    );
}

/// F2: an attestation retired between the applicability page and its commit leaves the envelope
/// `changed` with no `MarkStale`, and the next pass passes over the retired attestation.
#[test]
fn an_attestation_retired_before_its_stale_mark_commits_writes_nothing() {
    let store = Store::open();
    let witnessed = text(0, 1024);
    let repo = Repo::new("retired", &[("doc.md".into(), witnessed.clone())]);
    store.scope(&repo);
    let model = store.decision(&repo, "t0", "doc.md", &witnessed);
    let counters = Counters::default();
    let mut verifier = Verifier::default();
    pass(&mut verifier, &store, &repo, &counters);
    let attested = attestation(&model, &repo);
    republish(&store, &repo, "t0", "doc.md", &text(1, 1024), 2);

    let kernel = &store.kernel;
    let retired = AtomicBool::new(false);
    let clock = Clock::with(|poll, _| {
        if poll == Poll::Commit && !retired.swap(true, Ordering::SeqCst) {
            kernel
                .commit(intent("retire-attestation"), |envelope| {
                    envelope.retire_decision(&attested)?;
                    Ok(String::new())
                })
                .unwrap();
        }
        false
    });
    let report = verifier.pass(kernel, &[repo.project()], &on, &counters, &clock);
    assert!(report.failed, "{report:?}");
    assert!(retired.load(Ordering::SeqCst));
    assert_eq!(counters.get("commit_refused_changed"), 1);
    assert_eq!(applicability_counts(&counters), [0, 0, 0]);
    assert!(stale_marks(&store, &attested).is_empty());

    let report = pass(&mut verifier, &store, &repo, &counters);
    assert!(!report.failed, "{report:?}");
    assert_eq!(counters.get("commit_refused_changed"), 1);
    assert_eq!(applicability_counts(&counters), [0, 0, 0]);
    assert!(stale_marks(&store, &attested).is_empty());
}

/// F10: the stale mark's key replays its receipt and writes nothing new.
#[test]
fn a_replayed_stale_mark_key_writes_nothing_new() {
    use daemon::attestation_verifier::applicability::{Applicability, evaluate, mark_republished};
    let store = Store::open();
    let witnessed = text(0, 1024);
    let repo = Repo::new("replayed", &[("doc.md".into(), witnessed.clone())]);
    store.scope(&repo);
    let model = store.decision(&repo, "k0", "doc.md", &witnessed);
    let counters = Counters::default();
    let mut verifier = Verifier::default();
    pass(&mut verifier, &store, &repo, &counters);
    let attested = attestation(&model, &repo);
    republish(&store, &repo, "k0", "doc.md", &text(1, 1024), 2);

    let budget = EvalBudget::unbounded();
    let target = store.kernel.capture_commit_read_target().unwrap();
    let Applicability::Republished(republished) =
        evaluate(&store.kernel, &attested, target, &budget).unwrap()
    else {
        panic!("the document was republished with another digest");
    };
    let cancelled = EvalBudget::unbounded();
    cancelled.cancel();
    assert_eq!(
        evaluate(&store.kernel, &attested, target, &cancelled),
        Err(KernelError::Deadline)
    );
    let first = mark_republished(&store.kernel, target, &republished, &budget);
    assert!(matches!(first, Outcome::StaleCommitted(_)), "{first:?}");
    let admissions = store.count("SELECT COUNT(*) FROM admission_decisions");
    let second = mark_republished(&store.kernel, target, &republished, &budget);
    assert!(matches!(second, Outcome::Replayed(_)), "{second:?}");
    assert_eq!(
        store.count("SELECT COUNT(*) FROM admission_decisions"),
        admissions
    );
    assert_eq!(stale_marks(&store, &attested).len(), 1);

    pass(&mut verifier, &store, &repo, &counters);
    assert_eq!(applicability_counts(&counters), [0, 0, 0]);
    assert_eq!(stale_marks(&store, &attested).len(), 1);
}

/// `files` as N + 1 documents of `repo`, each cited by one model decision that two passes attest;
/// returns the attestation object ids in commit order.
fn attest_n_plus_one(
    store: &Store,
    repo: &Repo,
    files: &[(String, String)],
    verifier: &mut Verifier,
    counters: &Counters,
) -> Vec<String> {
    store.scope(repo);
    let attested: Vec<String> = files
        .iter()
        .enumerate()
        .map(|(i, (path, body))| {
            attestation(&store.decision(repo, &format!("c{i}"), path, body), repo)
        })
        .collect();
    pass(verifier, store, repo, counters);
    pass(verifier, store, repo, counters);
    assert_eq!(counters.get("attestation_committed"), MAX_VERDICTS + 1);
    attested
}

fn documents(count: usize) -> Vec<(String, String)> {
    (0..count)
        .map(|i| (format!("doc{i}.md"), text(i, 1024)))
        .collect()
}

/// The applicability step evaluates at most N attestations per project per pass, and a deadline at
/// a stale mark's commit leaves the cursor before that attestation, so the next pass evaluates it
/// first.
#[test]
fn a_commit_deadline_leaves_its_attestation_first_for_the_next_pass() {
    let store = Store::open();
    let files = documents(N + 1);
    let repo = Repo::new("cycle", &files);
    let counters = Counters::default();
    let mut verifier = Verifier::default();
    let attested = attest_n_plus_one(&store, &repo, &files, &mut verifier, &counters);
    assert_eq!(applicability_counts(&counters), [0, MAX_VERDICTS, 0]);
    assert_eq!(verifier.applicability_cursor(&repo.scope_id()), None);

    for (i, (path, body)) in files.iter().enumerate() {
        let changed = format!("{body}changed\n");
        republish(&store, &repo, &format!("c{i}"), path, &changed, 2);
    }
    let clock = Clock::with(|poll, _| poll == Poll::Commit);
    verifier.pass(&store.kernel, &[repo.project()], &on, &counters, &clock);
    assert_eq!(clock.polled(Poll::Commit), 1);
    assert_eq!(applicability_counts(&counters), [0, MAX_VERDICTS, 0]);
    assert_eq!(
        store.count("SELECT COUNT(*) FROM admission_decisions WHERE event_kind='mark_stale'"),
        0
    );
    assert_eq!(verifier.applicability_cursor(&repo.scope_id()), None);

    pass(&mut verifier, &store, &repo, &counters);
    assert_eq!(
        applicability_counts(&counters),
        [MAX_VERDICTS, MAX_VERDICTS, 0]
    );
    // The cut attestation, first in page order, is among the N the next pass marks.
    let ordered = page_order(&attested);
    for (i, attestation) in ordered.iter().enumerate() {
        assert_eq!(
            stale_marks(&store, attestation).len(),
            usize::from(i < N),
            "{i}"
        );
    }
    pass(&mut verifier, &store, &repo, &counters);
    assert_eq!(
        applicability_counts(&counters),
        [MAX_VERDICTS + 1, MAX_VERDICTS, 0]
    );
    assert_eq!(stale_marks(&store, &ordered[N]).len(), 1);
    pass(&mut verifier, &store, &repo, &counters);
    assert_eq!(
        applicability_counts(&counters),
        [MAX_VERDICTS + 1, MAX_VERDICTS, 0]
    );
}

/// A pass that evaluates N `Active` attestations stops the project with the applicability cursor
/// after the N-th, so the next pass evaluates the (N + 1)-th first.
#[test]
fn the_applicability_step_resumes_after_the_last_attestation_it_evaluated() {
    let store = Store::open();
    let files = documents(N + 1);
    let repo = Repo::new("resume", &files);
    let counters = Counters::default();
    let mut verifier = Verifier::default();
    let attested = attest_n_plus_one(&store, &repo, &files, &mut verifier, &counters);
    let [_, current, _] = applicability_counts(&counters);

    pass(&mut verifier, &store, &repo, &counters);
    assert_eq!(
        applicability_counts(&counters),
        [0, current + MAX_VERDICTS, 0]
    );
    let cursor = verifier
        .applicability_cursor(&repo.scope_id())
        .expect("the pass stopped after its N-th evaluation");
    assert!(attested.contains(&cursor), "{cursor}");
    for (i, (path, body)) in files.iter().enumerate() {
        let changed = format!("{body}changed\n");
        republish(&store, &repo, &format!("c{i}"), path, &changed, 2);
    }
    pass(&mut verifier, &store, &repo, &counters);
    assert_eq!(
        applicability_counts(&counters),
        [1, current + MAX_VERDICTS, 0]
    );
    let marked: Vec<&String> = attested
        .iter()
        .filter(|attestation| !stale_marks(&store, attestation).is_empty())
        .collect();
    assert_eq!(marked.len(), 1, "{marked:?}");
    assert_ne!(marked[0], &cursor);
    assert_eq!(verifier.applicability_cursor(&repo.scope_id()), None);
    pass(&mut verifier, &store, &repo, &counters);
    assert_eq!(
        applicability_counts(&counters),
        [MAX_VERDICTS + 1, current + MAX_VERDICTS, 0]
    );
    for attestation in &attested {
        assert_eq!(stale_marks(&store, attestation).len(), 1, "{attestation}");
    }
}

/// One model decision attested by a pass, then its document republished at revision 2 with other
/// bytes; returns the attestation object id.
fn attested_and_republished(
    store: &Store,
    repo: &Repo,
    verifier: &mut Verifier,
    counters: &Counters,
) -> String {
    store.scope(repo);
    let model = store.decision(repo, "r0", "doc.md", &text(0, 1024));
    pass(verifier, store, repo, counters);
    republish(store, repo, "r0", "doc.md", &text(1, 1024), 2);
    attestation(&model, repo)
}

/// A pass budget cancelled at the stale mark's commit poll refuses the commit at its deadline:
/// the pass counts `commit_refused_deadline` once and writes no `MarkStale`, and the next open
/// pass marks the attestation first.
#[test]
fn a_budget_cancelled_at_the_stale_mark_commit_counts_a_deadline_refusal() {
    let store = Store::open();
    let repo = Repo::new("cancelled", &[("doc.md".into(), text(0, 1024))]);
    let counters = Counters::default();
    let mut verifier = Verifier::default();
    let attested = attested_and_republished(&store, &repo, &mut verifier, &counters);
    let clock = Clock::with_budget(|poll, _, budget| {
        if poll == Poll::Commit {
            budget.cancel();
        }
        false
    });
    let report = verifier.pass(&store.kernel, &[repo.project()], &on, &counters, &clock);
    assert!(report.failed);
    assert_eq!(clock.polled(Poll::Commit), 1);
    assert_eq!(counters.get("commit_refused_deadline"), 1);
    assert_eq!(applicability_counts(&counters), [0, 0, 0]);
    assert_eq!(
        store.count("SELECT COUNT(*) FROM admission_decisions WHERE event_kind='mark_stale'"),
        0
    );

    pass(&mut verifier, &store, &repo, &counters);
    assert_eq!(applicability_counts(&counters), [1, 0, 0]);
    assert_eq!(stale_marks(&store, &attested).len(), 1);
}

/// The stale mark's envelope rereads the lineage's live descriptor: a revision published between
/// the evaluation and the commit refuses the mark as changed, and the next pass marks the
/// attestation at the newer revision.
#[test]
fn a_revision_published_before_the_stale_mark_commits_refuses_it_as_changed() {
    let store = Store::open();
    let repo = Repo::new("reread", &[("doc.md".into(), text(0, 1024))]);
    let counters = Counters::default();
    let mut verifier = Verifier::default();
    let attested = attested_and_republished(&store, &repo, &mut verifier, &counters);
    let published = AtomicBool::new(false);
    let clock = Clock::with(|poll, _| {
        if poll == Poll::Commit && !published.swap(true, Ordering::SeqCst) {
            republish(&store, &repo, "r0", "doc.md", &text(2, 1024), 3);
        }
        false
    });
    verifier.pass(&store.kernel, &[repo.project()], &on, &counters, &clock);
    assert!(published.load(Ordering::SeqCst));
    assert_eq!(counters.get("commit_refused_changed"), 1);
    assert_eq!(applicability_counts(&counters), [0, 0, 0]);
    assert!(stale_marks(&store, &attested).is_empty());

    pass(&mut verifier, &store, &repo, &counters);
    assert_eq!(applicability_counts(&counters), [1, 0, 0]);
    let marks = stale_marks(&store, &attested);
    assert_eq!(marks.len(), 1, "{marks:?}");
    assert!(marks[0].3.contains("descriptor revision 3"), "{marks:?}");
}

/// A plugin's `MarkStale` committed between the evaluation and the verifier's commit leaves the
/// plugin's one mark: the envelope finds the attestation `Stale` and counts nothing.
#[test]
fn a_plugin_stale_mark_before_the_verifiers_commit_leaves_one_mark() {
    let store = Store::open();
    let repo = Repo::new("plugin", &[("doc.md".into(), text(0, 1024))]);
    let counters = Counters::default();
    let mut verifier = Verifier::default();
    let attested = attested_and_republished(&store, &repo, &mut verifier, &counters);
    let kernel = &store.kernel;
    let marked = AtomicBool::new(false);
    let clock = Clock::with(|poll, _| {
        if poll == Poll::Commit && !marked.swap(true, Ordering::SeqCst) {
            kernel
                .commit(intent("plugin-stale"), |envelope| {
                    let (prior, _) = envelope
                        .subject_admission(&attested)?
                        .ok_or(KernelError::InvalidInput)?;
                    envelope.record_admission(AdmissionRequest {
                        candidate_id: None,
                        subject_object_id: Some(attested.clone()),
                        source_class: Some(prior.source_class),
                        taint_class: Some(prior.taint_class),
                        event: AdmissionEvent {
                            kind: EventKind::MarkStale,
                            trigger_object_id: None,
                            approval_object_id: None,
                            evidence_id: None,
                            reason: "plugin".into(),
                        },
                    })?;
                    Ok(String::new())
                })
                .unwrap();
        }
        false
    });
    verifier.pass(kernel, &[repo.project()], &on, &counters, &clock);
    assert!(marked.load(Ordering::SeqCst));
    assert_eq!(counters.get("commit_refused_changed"), 0);
    assert_eq!(applicability_counts(&counters), [0, 0, 0]);
    let marks = stale_marks(&store, &attested);
    assert_eq!(marks.len(), 1, "{marks:?}");
    assert_eq!(marks[0].3, "plugin");
}

/// A witness read the Kernel refuses for its corrupt row resolves its attestation under
/// `applicability_unavailable` with the pass healthy, so the attestation behind it is still
/// evaluated, in this pass and the next.
#[test]
fn a_witness_read_the_kernel_refuses_resolves_its_attestation_and_the_step_continues() {
    use daemon::attestation_verifier::applicability::evaluate;
    let store = Store::open();
    let files = documents(2);
    let repo = Repo::new("refused-witness", &files);
    store.scope(&repo);
    let models: Vec<_> = files
        .iter()
        .enumerate()
        .map(|(i, (path, body))| store.decision(&repo, &format!("w{i}"), path, body))
        .collect();
    let counters = Counters::default();
    let mut verifier = Verifier::default();
    pass(&mut verifier, &store, &repo, &counters);
    let attested: Vec<String> = models
        .iter()
        .map(|model| attestation(model, &repo))
        .collect();
    let first = page_order(&attested).remove(0);
    let witness = Identity::of_attestation(&first).unwrap().witness_object_id;
    let changed = store
        .sqlite()
        .execute(
            "UPDATE observations SET observation_payload=X'00' WHERE object_id=?1",
            [&witness],
        )
        .unwrap();
    assert_eq!(changed, 1);
    let target = store.kernel.capture_commit_read_target().unwrap();
    let budget = EvalBudget::unbounded();
    let refused = evaluate(&store.kernel, &first, target, &budget);
    assert!(
        matches!(refused, Err(ref error) if *error != KernelError::Deadline),
        "{refused:?}"
    );

    for passes in 1..=2 {
        let report = pass(&mut verifier, &store, &repo, &counters);
        assert!(!report.failed, "pass {passes}");
        assert_eq!(applicability_counts(&counters), [0, passes, passes]);
    }
}

/// Each attestation's standing is its own read in the page's read transaction: the first and last
/// rows of the page, whose standing reads refuse, each resolve under `applicability_unavailable`
/// with the pass healthy, and the attestation between them is still evaluated, in this pass and
/// the next.
#[test]
fn an_attestation_row_the_standing_read_refuses_resolves_and_the_step_continues() {
    let store = Store::open();
    let files = documents(3);
    let repo = Repo::new("refused-standing", &files);
    store.scope(&repo);
    let models: Vec<_> = files
        .iter()
        .enumerate()
        .map(|(i, (path, body))| store.decision(&repo, &format!("s{i}"), path, body))
        .collect();
    let counters = Counters::default();
    let mut verifier = Verifier::default();
    pass(&mut verifier, &store, &repo, &counters);
    assert_eq!(counters.get("attestation_committed"), 3);
    let attested: Vec<String> = models
        .iter()
        .map(|model| attestation(model, &repo))
        .collect();
    let ordered = page_order(&attested);
    for refused in [&ordered[0], &ordered[2]] {
        let changed = store
            .sqlite()
            .execute(
                "UPDATE admission_decisions SET disposition='unknown' WHERE subject_object_id=?1",
                [refused],
            )
            .unwrap();
        assert_eq!(changed, 1);
    }

    for passes in 1..=2 {
        let report = pass(&mut verifier, &store, &repo, &counters);
        assert!(!report.failed, "pass {passes}");
        assert_eq!(applicability_counts(&counters), [0, passes, 2 * passes]);
    }
}

/// The applicability step pages at most N × `PAGE_ROWS` rows per pass and polls the deadline once
/// per page: an `Active` attestation behind more `Stale` verifier decisions than that is reached
/// on every other pass.
#[test]
fn the_applicability_step_pages_at_most_n_pages_of_rows_per_pass() {
    let store = Store::open();
    let repo = Repo::new("paged", &[("doc.md".into(), text(0, 1024))]);
    store.scope(&repo);
    let rows = PAGE_ROWS.get();
    let stale = N * rows + rows / 2;
    for chunk in 0..stale.div_ceil(rows) {
        store
            .kernel
            .commit(intent(&format!("stale-{chunk}")), |envelope| {
                for i in chunk * rows..stale.min((chunk + 1) * rows) {
                    let object_id = format!("stale_{i}");
                    envelope.insert_decision(DecisionSpec {
                        decision_id: format!("dec_stale_{i}"),
                        object_id: object_id.clone(),
                        domain_id: "memory".into(),
                        proposition_id: None,
                        scope_id: Some(repo.scope_id()),
                        anchor_id: None,
                        evidence_id: None,
                        decision_kind: "ARCHITECTURE".into(),
                        payload: DecisionPayload {
                            summary: format!("stale attestation {i}"),
                            rationale: "{}".into(),
                        },
                        source_kind: "verifier".into(),
                        // `-` sorts before every hexadecimal digit.
                        source_id: format!("-{object_id}"),
                        source_revision: 1,
                        sensitivity: Sensitivity::Normal,
                    })?;
                    for kind in [EventKind::Other, EventKind::MarkStale] {
                        envelope.record_admission(AdmissionRequest {
                            candidate_id: None,
                            subject_object_id: Some(object_id.clone()),
                            source_class: Some(SourceClass::ModelInference),
                            taint_class: Some(TaintClass::AssistantInference),
                            event: AdmissionEvent {
                                kind,
                                trigger_object_id: None,
                                approval_object_id: None,
                                evidence_id: None,
                                reason: "fixture".into(),
                            },
                        })?;
                    }
                }
                Ok(String::new())
            })
            .unwrap();
    }
    store.decision(&repo, "m0", "doc.md", &text(0, 1024));
    let counters = Counters::default();
    let mut verifier = Verifier::default();
    pass(&mut verifier, &store, &repo, &counters);
    assert_eq!(counters.get("attestation_committed"), 1);
    // The first pass's applicability cycle pages through the commit captured before the
    // attestation, so the attestation joins the cycle the third pass starts.
    for (passes, current) in [(2, 0), (3, 0), (4, 1), (5, 1), (6, 2)] {
        let clock = Clock::open();
        verifier.pass(&store.kernel, &[repo.project()], &on, &counters, &clock);
        assert_eq!(
            applicability_counts(&counters),
            [0, current, 0],
            "pass {passes}"
        );
        if passes == 3 {
            assert_eq!(clock.polled(Poll::Decision), 1 + N, "pass {passes}");
        }
    }
}

/// A top-level worktree root binds the project digest beside its Git identity, so a document
/// published under the digest, as capture does before the repository has a derivable identity,
/// verifies against the root.
#[test]
fn a_worktree_root_verifies_a_document_published_under_the_project_digest() {
    let store = Store::open();
    let repo = Repo::new("digest", &[("doc.md".into(), text(0, 1024))]);
    store.scope(&repo);
    let decision = store.decision(&repo, "g0", "doc.md", &text(0, 1024));
    let counters = Counters::default();
    Verifier::default().pass(
        &store.kernel,
        &[repo.project_at(&repo.work)],
        &on,
        &counters,
        &Clock::open(),
    );
    assert_accounting(&counters);
    assert_eq!(counters.get("match"), 1);
    assert_eq!(counters.get("unavailable_repository_unbound"), 0);
    assert!(exists(&store, &attestation(&decision, &repo)));
}

#[test]
fn a_digest_only_root_derives_again_after_a_decision_names_an_unbound_repository() {
    let store = Store::open();
    let dir = tempfile::tempdir().unwrap();
    let work = dir.path().join("work");
    std::fs::create_dir(&work).unwrap();
    git(&work, &["init", "--quiet", "--initial-branch=main"]);
    let mut repo = Repo {
        bare: work.clone(),
        work: work.clone(),
        _dir: dir,
        head: String::new(),
        digest: format!("{:x}", Sha256::digest(b"unborn")),
    };
    store.scope(&repo);
    let project = repo.project_at(&work);
    let counters = Counters::default();
    let mut verifier = Verifier::default();
    let clock = Clock::open();
    verifier.pass(
        &store.kernel,
        std::slice::from_ref(&project),
        &on,
        &counters,
        &clock,
    );
    assert_eq!(clock.polled(Poll::Derive), 1);

    std::fs::write(work.join("doc.md"), text(0, 1024)).unwrap();
    git(&work, &["add", "--all"]);
    git(&work, &["commit", "--quiet", "-m", "first"]);
    repo.head = git(&work, &["rev-parse", "HEAD"]);
    let decision = store.decision_under(
        &repo,
        &format!("git:{}", repo.head),
        "u0",
        "doc.md",
        &text(0, 1024),
    );
    for _ in 0..MAX_ATTEMPTS {
        verifier.pass(
            &store.kernel,
            std::slice::from_ref(&project),
            &on,
            &counters,
            &Clock::open(),
        );
        assert_accounting(&counters);
    }
    assert_eq!(counters.get("unavailable_repository_unbound"), 1);
    assert_eq!(counters.get("unavailable_skipped"), 0);
    assert_eq!(counters.get("attestation_committed"), 1);
    assert!(exists(&store, &attestation(&decision, &repo)));

    let clock = Clock::open();
    verifier.pass(&store.kernel, &[project], &on, &counters, &clock);
    assert_eq!(clock.polled(Poll::Derive), 0);
}

/// A root path bound again under another project digest derives its repository identity again.
#[test]
fn a_root_path_bound_under_another_project_derives_again() {
    let store = Store::open();
    let repo = Repo::new("rebound", &[]);
    let other = Repo::new("rebound-other", &[]);
    let counters = Counters::default();
    let mut verifier = Verifier::default();
    let clock = Clock::open();
    verifier.pass(&store.kernel, &[repo.project()], &on, &counters, &clock);
    assert_eq!(clock.polled(Poll::Derive), 1);
    let clock = Clock::open();
    verifier.pass(&store.kernel, &[repo.project()], &on, &counters, &clock);
    assert_eq!(clock.polled(Poll::Derive), 0);
    let mut rebound = other.project_at(&repo.bare);
    rebound.roots[0].digest = other.digest.clone();
    let clock = Clock::open();
    verifier.pass(&store.kernel, &[rebound], &on, &counters, &clock);
    assert_eq!(clock.polled(Poll::Derive), 1);
}

/// A decision row the batched standing read refuses leaves the page to each decision's own
/// classification: the bad row spends an attempt each pass and is skipped after K, while the
/// decision behind it verifies in the first pass.
#[test]
fn a_row_the_standing_read_refuses_is_skipped_after_k_passes() {
    let store = Store::open();
    let repo = Repo::new(
        "corrupt",
        &[
            ("bad.md".into(), text(0, 1024)),
            ("good.md".into(), text(1, 1024)),
        ],
    );
    store.scope(&repo);
    let bad = store.decision(&repo, "c0", "bad.md", &text(0, 1024));
    let good = store.decision(&repo, "c1", "good.md", &text(1, 1024));
    store
        .sqlite()
        .execute(
            "UPDATE admission_decisions SET disposition='unknown' WHERE subject_object_id=?1",
            [&bad],
        )
        .unwrap();
    let counters = Counters::default();
    let mut verifier = Verifier::default();
    for pass in 1..=3 {
        let report = verifier.pass(
            &store.kernel,
            &[repo.project()],
            &on,
            &counters,
            &Clock::open(),
        );
        assert!(report.failed, "pass {pass}");
        assert_accounting(&counters);
        assert_eq!(counters.get("attestation_committed"), 1, "pass {pass}");
    }
    assert!(exists(&store, &attestation(&good, &repo)));
    assert_eq!(counters.get("unavailable_skipped"), 1);
    assert_eq!(verifier.cursor(&repo.scope_id()).unwrap().1, good);

    // The refused page standing names no anchor, so a restart skips the attested decision at the
    // check after its classification, before any repository read.
    let store = store.reopen();
    let tip = store.kernel.tip().unwrap();
    let counters = Counters::default();
    let clock = Clock::open();
    Verifier::default().pass(&store.kernel, &[repo.project()], &on, &counters, &clock);
    assert_eq!(counters.get("skipped_attested"), 1);
    assert_eq!(counters.get("attestation_replayed"), 0);
    assert_eq!(clock.opens(), 0);
    assert_eq!(store.kernel.tip().unwrap(), tip);
}

/// An anchor row the pre-check cannot read leaves its decision to classification, which refuses it
/// and spends an attempt each pass until the K-th skips it, while the decision behind it verifies
/// in the first pass.
#[test]
fn a_refused_anchor_read_leaves_the_decision_to_its_classification() {
    let store = Store::open();
    let repo = Repo::new(
        "anchor",
        &[
            ("bad.md".into(), text(0, 1024)),
            ("good.md".into(), text(1, 1024)),
        ],
    );
    store.scope(&repo);
    let bad = store.decision(&repo, "a0", "bad.md", &text(0, 1024));
    let good = store.decision(&repo, "a1", "good.md", &text(1, 1024));
    store
        .sqlite()
        .execute(
            "UPDATE anchors SET anchor_kind='bogus' WHERE anchor_id='anchor_a0'",
            [],
        )
        .unwrap();
    let counters = Counters::default();
    let mut verifier = Verifier::default();
    for pass in 1..=3 {
        let report = verifier.pass(
            &store.kernel,
            &[repo.project()],
            &on,
            &counters,
            &Clock::open(),
        );
        assert!(report.failed, "pass {pass}");
        assert_accounting(&counters);
        assert_eq!(counters.get("attestation_committed"), 1, "pass {pass}");
        let skipped = u64::from(pass == 3);
        assert_eq!(counters.get("unavailable_skipped"), skipped, "pass {pass}");
        if pass < 3 {
            assert_eq!(verifier.cursor(&repo.scope_id()), None, "pass {pass}");
        }
    }
    assert!(exists(&store, &attestation(&good, &repo)));
    assert!(!exists(&store, &attestation(&bad, &repo)));
    assert_eq!(verifier.cursor(&repo.scope_id()).unwrap().1, good);
}

/// A project whose pages ran out with every decision resolved records the page start just past the
/// target's sequence as its floor. A decision committed at the floor's sequence, decisions committed
/// behind another project's, and a decision committed during the project's own pass after its
/// target capture are each read on a later pass, and a transient decision committed past the floor
/// holds the project at its cursor until it resolves.
#[test]
fn a_drained_project_verifies_the_decisions_committed_after_its_last_page() {
    let store = Store::open();
    let files: Vec<_> = (0..5)
        .map(|i| (format!("doc{i}.md"), text(i, 1024)))
        .collect();
    let repo = Repo::new("drained", &files);
    let other = Repo::new("other", &files);
    store.scope(&repo);
    store.scope(&other);
    let first = store.decision(&repo, "d0", &files[0].0, &files[0].1);
    let counters = Counters::default();
    let mut verifier = Verifier::default();
    let both = [repo.project(), other.project()];
    let pass = |verifier: &mut Verifier, clock: &Clock<'_>| {
        let report = verifier.pass(&store.kernel, &both, &on, &counters, clock);
        assert_accounting(&counters);
        report
    };
    let floor = |verifier: &Verifier, repo: &Repo| verifier.floor(&repo.scope_id());
    let past = |seq: i64| Some((seq + 1, String::new()));
    let clock = Clock::open();
    let captured = store.kernel.tip().unwrap();
    pass(&mut verifier, &clock);
    assert_eq!(clock.opens(), 1);
    assert_eq!(verifier.cursor(&repo.scope_id()).unwrap().1, first);
    // The floor is just past the target captured before the attestation commit, which the page
    // excludes.
    assert_eq!(floor(&verifier, &repo), past(captured));
    let report = pass(&mut verifier, &Clock::open());
    assert!(!report.progressed && !report.failed);
    let drained = store.kernel.tip().unwrap();
    assert_eq!(floor(&verifier, &repo), past(drained));
    assert_eq!(floor(&verifier, &other), past(drained));

    // One commit lands at the floor's own sequence.
    let next = store.model_decision(&repo, "n0", "", None, None);
    assert_eq!(store.kernel.tip().unwrap(), drained + 1);
    let report = pass(&mut verifier, &Clock::open());
    assert!(report.progressed && !report.failed);
    assert_eq!(counters.get("unverifiable_no_anchor"), 1);
    assert_eq!(verifier.cursor(&repo.scope_id()).unwrap().1, next);
    assert_eq!(floor(&verifier, &repo), past(drained + 1));

    let later = store.decision(&repo, "d1", &files[1].0, &files[1].1);
    let unbound = store.decision_under(&repo, "git:unbound", "d2", &files[2].0, &files[2].1);
    let last = store.decision(&repo, "d3", &files[3].0, &files[3].1);
    let clock = Clock::open();
    pass(&mut verifier, &clock);
    assert_eq!(clock.opens(), 2);
    assert!(exists(&store, &attestation(&later, &repo)));
    assert!(exists(&store, &attestation(&last, &repo)));
    assert_eq!(verifier.cursor(&repo.scope_id()).unwrap().1, later);
    assert_eq!(floor(&verifier, &repo), past(drained + 1));
    for attempt in 2..=3 {
        let report = pass(&mut verifier, &Clock::open());
        assert!(report.failed);
        assert_eq!(counters.get("unavailable_repository_unbound"), attempt);
    }
    assert_eq!(counters.get("unavailable_skipped"), 1);
    assert_eq!(verifier.cursor(&repo.scope_id()).unwrap().1, last);
    assert!(!exists(&store, &attestation(&unbound, &repo)));
    let resolved = store.kernel.tip().unwrap();
    assert_eq!(floor(&verifier, &repo), past(resolved));

    // `other` pages from its own floor and verifies the decisions committed behind `repo`'s. The
    // rotation starts this pass with `repo`, whose floor then lies past them; the floors pin it.
    for (i, (path, body)) in files.iter().enumerate().take(3) {
        store.decision(&other, &format!("o{i}"), path, body);
    }
    let before = store.kernel.tip().unwrap();
    let clock = Clock::open();
    let report = pass(&mut verifier, &clock);
    assert!(report.progressed && !report.failed);
    assert_eq!(clock.opens(), 3);
    assert_eq!(counters.get("attestation_committed"), 6);
    assert_eq!(floor(&verifier, &repo), past(before));
    assert_eq!(floor(&verifier, &other), past(before));

    // A decision committed during `repo`'s own run, after its target capture and while a decision
    // there is pending, waits for the next pass: the floor is the captured target's, not the tip's.
    let pending = store.decision(&repo, "p0", &files[4].0, &files[4].1);
    let during = Mutex::new(None);
    let clock = Clock::with(|poll, opens| {
        if poll == Poll::Read(Stage::Open) && opens == 1 {
            *during.lock().unwrap() = Some(store.model_decision(&repo, "m0", "", None, None));
        }
        false
    });
    let report = pass(&mut verifier, &clock);
    assert!(report.progressed && !report.failed);
    assert_eq!(counters.get("attestation_committed"), 7);
    assert_eq!(verifier.cursor(&repo.scope_id()).unwrap().1, pending);
    let during = during.lock().unwrap().clone().unwrap();
    let report = pass(&mut verifier, &Clock::open());
    assert!(report.progressed && !report.failed);
    assert_eq!(verifier.cursor(&repo.scope_id()).unwrap().1, during);
    assert_eq!(counters.get("unverifiable_no_anchor"), 2);
    let clock = Clock::open();
    let report = pass(&mut verifier, &clock);
    assert!(!report.progressed && !report.failed);
    assert_eq!(clock.opens(), 0);
    assert_eq!(counters.get("considered"), 12);
    assert_eq!(counters.get("skipped_attested"), 0);
}

/// An attested decision skips under `skipped_attested` at the oid its anchor names before it is
/// classified, so a restart counts it skipped after its cited descriptor retires, where its
/// classification alone would count `unverifiable_descriptor_retired`.
#[test]
fn an_attested_decision_skips_before_its_classification_after_its_descriptor_retires() {
    let store = Store::open();
    let repo = Repo::new("retired", &[("doc.md".into(), text(0, 1024))]);
    store.scope(&repo);
    let decision = store.decision(&repo, "s0", "doc.md", &text(0, 1024));
    let counters = Counters::default();
    let clock = Clock::open();
    Verifier::default().pass(&store.kernel, &[repo.project()], &on, &counters, &clock);
    assert_eq!(counters.get("attestation_committed"), 1);
    let descriptor: String = store
        .sqlite()
        .query_row(
            "SELECT object_id FROM observations WHERE evidence_id='srcev:s0'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    store
        .kernel
        .commit(intent("retire-descriptor"), |envelope| {
            envelope.retire_observation(&descriptor)?;
            Ok(String::new())
        })
        .unwrap();
    let target = store.kernel.capture_commit_read_target().unwrap();
    let budget = EvalBudget::unbounded();
    let classification = classify(&store.kernel, &decision, target, &[repo.bound()], &budget);
    assert_eq!(
        classification.unwrap().class,
        daemon::attestation_verifier::classify::Class::Unverifiable(
            daemon::attestation_verifier::classify::Unverifiable::DescriptorRetired
        )
    );

    let store = store.reopen();
    let tip = store.kernel.tip().unwrap();
    let counters = Counters::default();
    let clock = Clock::open();
    Verifier::default().pass(&store.kernel, &[repo.project()], &on, &counters, &clock);
    assert_accounting(&counters);
    assert_eq!(counters.get("skipped_attested"), 1);
    assert_eq!(counters.get("considered"), 0);
    assert_eq!(counters.get("unverifiable_descriptor_retired"), 0);
    assert_eq!(clock.opens(), 0);
    assert_eq!(store.kernel.tip().unwrap(), tip);
}

/// A decision committed through a plugin route under the object id the attestation would take.
fn squat(store: &Store, object_id: &str) {
    store
        .kernel
        .commit(intent(&format!("squat-{object_id}")), |envelope| {
            envelope.insert_decision(DecisionSpec {
                decision_id: format!("squatdec:{object_id}"),
                object_id: object_id.to_owned(),
                domain_id: "memory".into(),
                proposition_id: None,
                scope_id: None,
                anchor_id: None,
                evidence_id: None,
                decision_kind: "ARCHITECTURE".into(),
                payload: DecisionPayload {
                    summary: "a plugin-route object under the attestation's id".into(),
                    rationale: "{}".into(),
                },
                source_kind: "assistant".into(),
                source_id: "memory_capture".into(),
                source_revision: 1,
                sensitivity: Sensitivity::Normal,
            })?;
            Ok(String::new())
        })
        .unwrap();
}

/// A registry object under the attestation's id written by another source is not the attestation:
/// the mismatch behind it is marked stale, and the match behind it is verified and its commit
/// refused by the occupied id, with no `skipped_attested`.
#[test]
fn an_object_another_source_wrote_under_the_attestations_id_skips_nothing() {
    let store = Store::open();
    let repo = Repo::new(
        "squat",
        &[
            ("same.md".into(), text(0, 1024)),
            ("changed.md".into(), text(1, 1024)),
        ],
    );
    store.scope(&repo);
    let matched = store.decision(&repo, "q0", "same.md", &text(0, 1024));
    let mismatched = store.decision(&repo, "q1", "changed.md", &text(2, 1024));
    squat(&store, &attestation(&matched, &repo));
    squat(&store, &attestation(&mismatched, &repo));
    let counters = Counters::default();
    let clock = Clock::open();
    Verifier::default().pass(&store.kernel, &[repo.project()], &on, &counters, &clock);
    assert_accounting(&counters);
    assert_eq!(counters.get("skipped_attested"), 0);
    assert_eq!(counters.get("mismatch"), 1);
    assert_eq!(counters.get("stale_committed"), 1);
    assert_eq!(counters.get("match"), 1);
    assert_eq!(counters.get("attestation_committed"), 0);
    assert_eq!(counters.get("commit_refused_other"), 1);
    assert_eq!(clock.opens(), 2);
    assert_eq!(store.verifier_rows(), 0);
}

/// A terminal reason names the claim's path at its oid, so a store with another history, where the
/// same decision object id cites a path the commit holds, verifies the decision.
#[test]
fn a_terminal_reason_does_not_resolve_the_same_object_id_citing_another_path() {
    let repo = Repo::new("history", &[("doc.md".into(), text(0, 1024))]);
    let store = Store::open();
    store.scope(&repo);
    store.decision(&repo, "h0", "absent.md", "gone\n");
    let counters = Counters::default();
    let mut verifier = Verifier::default();
    verifier.pass(
        &store.kernel,
        &[repo.project()],
        &on,
        &counters,
        &Clock::open(),
    );
    assert_eq!(counters.get("unavailable_path_absent"), 1);

    let other = Store::open();
    other.scope(&repo);
    let decision = other.decision(&repo, "h0", "doc.md", &text(0, 1024));
    let clock = Clock::open();
    verifier.pass(&other.kernel, &[repo.project()], &on, &counters, &clock);
    assert_eq!(clock.opens(), 1);
    assert_eq!(counters.get("match"), 1);
    assert_eq!(counters.get("attestation_committed"), 1);
    assert!(exists(&other, &attestation(&decision, &repo)));
}

/// The attempt of a transient decision retired before it resolves is dropped once the cursor or
/// the floor passes its commit, so retired decisions leave no attempt behind.
#[test]
fn a_transient_decision_retired_before_it_resolves_leaves_no_attempt() {
    let store = Store::open();
    let repo = Repo::new("retired", &[("doc.md".into(), text(0, 1024))]);
    store.scope(&repo);
    let unbound = store.decision_under(&repo, "git:unbound", "r0", "doc.md", &text(0, 1024));
    let counters = Counters::default();
    let mut verifier = Verifier::default();
    verifier.pass(
        &store.kernel,
        &[repo.project()],
        &on,
        &counters,
        &Clock::open(),
    );
    assert_eq!(counters.get("unavailable_repository_unbound"), 1);
    assert_eq!(verifier.attempts(&repo.scope_id()), 1);

    store
        .kernel
        .commit(intent("retire-r0"), |envelope| {
            envelope.retire_decision(&unbound)?;
            Ok(String::new())
        })
        .unwrap();
    verifier.pass(
        &store.kernel,
        &[repo.project()],
        &on,
        &counters,
        &Clock::open(),
    );
    assert_eq!(verifier.attempts(&repo.scope_id()), 0);

    let unbound = store.decision_under(&repo, "git:unbound", "r1", "other.md", &text(1, 1024));
    let verified = store.decision(&repo, "r2", "doc.md", &text(0, 1024));
    verifier.pass(
        &store.kernel,
        &[repo.project()],
        &on,
        &counters,
        &Clock::open(),
    );
    assert_eq!(counters.get("unavailable_repository_unbound"), 2);
    assert_eq!(counters.get("attestation_committed"), 1);
    assert_eq!(verifier.attempts(&repo.scope_id()), 1);
    store
        .kernel
        .commit(intent("retire-r1"), |envelope| {
            envelope.retire_decision(&unbound)?;
            Ok(String::new())
        })
        .unwrap();
    // A transient decision behind the verified one keeps the floor where it is, so the cursor's
    // advance to the verified decision is what drops the retired decision's attempt.
    store.decision_under(&repo, "git:unbound", "r3", "third.md", &text(2, 1024));
    let floor = verifier.floor(&repo.scope_id());
    verifier.pass(
        &store.kernel,
        &[repo.project()],
        &on,
        &counters,
        &Clock::open(),
    );
    assert_eq!(counters.get("unavailable_repository_unbound"), 3);
    assert_eq!(verifier.cursor(&repo.scope_id()).unwrap().1, verified);
    assert_eq!(verifier.floor(&repo.scope_id()), floor);
    assert_eq!(verifier.attempts(&repo.scope_id()), 1);
}

/// A resolved decision remembered behind a transient head is dropped once the floor passes it, so
/// a batch retired before its head resolves leaves no memory behind.
#[test]
fn a_resolved_decision_retired_behind_a_transient_head_is_forgotten_at_the_floor() {
    let store = Store::open();
    let repo = Repo::new("forgotten", &[("doc.md".into(), text(0, 1024))]);
    store.scope(&repo);
    let head = store.decision_under(&repo, "git:unbound", "f0", "other.md", &text(1, 1024));
    let tail = store.decision(&repo, "f1", "doc.md", &text(0, 1024));
    let counters = Counters::default();
    let mut verifier = Verifier::default();
    verifier.pass(
        &store.kernel,
        &[repo.project()],
        &on,
        &counters,
        &Clock::open(),
    );
    assert_eq!(counters.get("unavailable_repository_unbound"), 1);
    assert_eq!(counters.get("attestation_committed"), 1);
    assert_eq!(verifier.remembered(&repo.scope_id()), 1);
    assert_eq!(verifier.attempts(&repo.scope_id()), 1);

    store
        .kernel
        .commit(intent("retire-f0-f1"), |envelope| {
            envelope.retire_decision(&head)?;
            envelope.retire_decision(&tail)?;
            Ok(String::new())
        })
        .unwrap();
    verifier.pass(
        &store.kernel,
        &[repo.project()],
        &on,
        &counters,
        &Clock::open(),
    );
    assert!(verifier.floor(&repo.scope_id()).is_some());
    assert_eq!(verifier.remembered(&repo.scope_id()), 0);
    assert_eq!(verifier.attempts(&repo.scope_id()), 0);
}

/// A root whose derivation fails marks the pass failed even when the project has no decision to
/// count the failure through, so the task retries at [`RETRY_INTERVAL`] instead of idling.
#[test]
fn a_failed_derivation_without_decisions_fails_the_pass() {
    let store = Store::open();
    let broken = Repo::new("broken-idle", &[("doc.md".into(), text(0, 1024))]);
    store.scope(&broken);
    break_gitfile(&broken);
    let counters = Counters::default();
    let mut verifier = Verifier::default();
    let clock = Clock::open();
    let report = verifier.pass(
        &store.kernel,
        &[broken.project_at(&broken.work)],
        &on,
        &counters,
        &clock,
    );
    assert_eq!(clock.polled(Poll::Derive), 1);
    assert_eq!(counters.get("considered"), 0);
    assert!(report.failed);
    assert!(!report.progressed);
}

/// The applicability step runs after the decisions step stops at its verdict bound: a pass that
/// computes N verdicts for fresh decisions still marks the attestation of a republished document.
#[test]
fn the_applicability_step_runs_after_the_decisions_step_reaches_n_verdicts() {
    let store = Store::open();
    let files = documents(N + 1);
    let repo = Repo::new("backlog", &files);
    store.scope(&repo);
    let counters = Counters::default();
    let mut verifier = Verifier::default();
    let (path, body) = &files[0];
    let model = store.decision(&repo, "b0", path, body);
    pass(&mut verifier, &store, &repo, &counters);
    pass(&mut verifier, &store, &repo, &counters);
    assert_eq!(applicability_counts(&counters), [0, 1, 0]);
    republish(&store, &repo, "b0", path, &format!("{body}changed\n"), 2);
    for (i, (path, body)) in files.iter().enumerate().skip(1) {
        store.decision(&repo, &format!("b{i}"), path, body);
    }
    pass(&mut verifier, &store, &repo, &counters);
    assert_eq!(counters.get("match"), MAX_VERDICTS + 1);
    assert_eq!(applicability_counts(&counters), [1, 1, 0]);
    assert_eq!(stale_marks(&store, &attestation(&model, &repo)).len(), 1);
}

/// A deadline at the applicability page poll fails and stops the pass before the page is read,
/// with no count and no mark, and the next open pass marks the attestation.
#[test]
fn a_deadline_at_the_applicability_page_poll_leaves_the_attestation_for_the_next_pass() {
    let store = Store::open();
    let repo = Repo::new("page-poll", &[("doc.md".into(), text(0, 1024))]);
    let counters = Counters::default();
    let mut verifier = Verifier::default();
    let attested = attested_and_republished(&store, &repo, &mut verifier, &counters);
    let before = applicability_counts(&counters);
    // The first `Decision` poll precedes the project; the decisions step polls none, since its
    // cursor passed every decision, so the second precedes the first applicability page.
    let decisions = std::sync::atomic::AtomicUsize::new(0);
    let clock = Clock::with(|poll, _| {
        poll == Poll::Decision && decisions.fetch_add(1, Ordering::SeqCst) == 1
    });
    let report = verifier.pass(&store.kernel, &[repo.project()], &on, &counters, &clock);
    assert!(report.failed && !report.progressed, "{report:?}");
    assert_eq!(clock.polled(Poll::Decision), 2);
    assert_eq!(applicability_counts(&counters), before);
    assert!(stale_marks(&store, &attested).is_empty());

    pass(&mut verifier, &store, &repo, &counters);
    assert_eq!(
        applicability_counts(&counters),
        [before[0] + 1, before[1], before[2]]
    );
    assert_eq!(stale_marks(&store, &attested).len(), 1);
}

/// A stale mark the Kernel rolls back counts its refusal reason, fails the pass, and counts no
/// `applicability_stale`; the next pass that reaches the attestation marks it.
#[test]
fn a_rolled_back_stale_mark_counts_its_refusal_and_a_later_pass_marks() {
    let store = Store::open();
    let repo = Repo::new("rolled-back", &[("doc.md".into(), text(0, 1024))]);
    let counters = Counters::default();
    let mut verifier = Verifier::default();
    let attested = attested_and_republished(&store, &repo, &mut verifier, &counters);
    store
        .sqlite()
        .execute_batch(
            "CREATE TRIGGER fail_stale BEFORE INSERT ON admission_decisions
              WHEN NEW.event_kind='mark_stale'
              BEGIN SELECT RAISE(ABORT, 'fault at the stale mark'); END;",
        )
        .unwrap();
    let refusals = || {
        [
            "commit_refused_busy",
            "commit_refused_io",
            "commit_refused_other",
        ]
        .map(|name| counters.get(name))
        .iter()
        .sum::<u64>()
    };
    let report = pass(&mut verifier, &store, &repo, &counters);
    assert!(report.failed && !report.progressed, "{report:?}");
    assert_eq!(refusals(), 1);
    assert_eq!(applicability_counts(&counters), [0, 0, 0]);
    assert!(stale_marks(&store, &attested).is_empty());

    store
        .sqlite()
        .execute_batch("DROP TRIGGER fail_stale;")
        .unwrap();
    let report = pass(&mut verifier, &store, &repo, &counters);
    assert!(report.progressed && !report.failed, "{report:?}");
    assert_eq!(refusals(), 1);
    assert_eq!(applicability_counts(&counters), [1, 0, 0]);
    assert_eq!(stale_marks(&store, &attested).len(), 1);
}

/// `evaluate` reads at the target's commit: a target captured before a republication reads the
/// attestation current, one captured after reads it republished, and a target captured before the
/// lineage's live descriptor retires still reads that descriptor.
#[test]
fn evaluate_reads_the_witness_and_the_descriptor_at_the_targets_commit() {
    use daemon::attestation_verifier::applicability::{Applicability, evaluate};
    let store = Store::open();
    let repo = Repo::new("historical", &[("doc.md".into(), text(0, 1024))]);
    store.scope(&repo);
    let model = store.decision(&repo, "h0", "doc.md", &text(0, 1024));
    let counters = Counters::default();
    let mut verifier = Verifier::default();
    pass(&mut verifier, &store, &repo, &counters);
    let attested = attestation(&model, &repo);
    let budget = EvalBudget::unbounded();
    let before = store.kernel.capture_commit_read_target().unwrap();
    republish(&store, &repo, "h0", "doc.md", &text(1, 1024), 2);
    let after = store.kernel.capture_commit_read_target().unwrap();
    assert_eq!(
        evaluate(&store.kernel, &attested, before, &budget).unwrap(),
        Applicability::Current
    );
    assert!(matches!(
        evaluate(&store.kernel, &attested, after, &budget).unwrap(),
        Applicability::Republished(_)
    ));
    let descriptor: String = store
        .sqlite()
        .query_row(
            "SELECT object_id FROM observations WHERE evidence_id='srcev:h0:r2'
               AND invalidated_commit_seq IS NULL",
            [],
            |row| row.get(0),
        )
        .unwrap();
    store
        .kernel
        .commit(intent("retire-h0"), |envelope| {
            envelope.retire_observation(&descriptor)?;
            Ok(String::new())
        })
        .unwrap();
    let retired = store.kernel.capture_commit_read_target().unwrap();
    assert!(matches!(
        evaluate(&store.kernel, &attested, after, &budget).unwrap(),
        Applicability::Republished(_)
    ));
    assert_eq!(
        evaluate(&store.kernel, &attested, retired, &budget).unwrap(),
        Applicability::Unavailable
    );
}

/// The stale mark's envelope rereads the lineage: a live descriptor retired between the
/// evaluation and the commit returns `Changed` with no write.
#[test]
fn a_descriptor_retired_before_the_stale_mark_commits_refuses_it_as_changed() {
    use daemon::attestation_verifier::applicability::{Applicability, evaluate, mark_republished};
    let store = Store::open();
    let repo = Repo::new("retired-lineage", &[("doc.md".into(), text(0, 1024))]);
    let counters = Counters::default();
    let mut verifier = Verifier::default();
    let attested = attested_and_republished(&store, &repo, &mut verifier, &counters);
    let budget = EvalBudget::unbounded();
    let target = store.kernel.capture_commit_read_target().unwrap();
    let Applicability::Republished(republished) =
        evaluate(&store.kernel, &attested, target, &budget).unwrap()
    else {
        panic!("the document was republished with another digest");
    };
    let descriptor: String = store
        .sqlite()
        .query_row(
            "SELECT object_id FROM observations WHERE evidence_id='srcev:r0:r2'
               AND invalidated_commit_seq IS NULL",
            [],
            |row| row.get(0),
        )
        .unwrap();
    store
        .kernel
        .commit(intent("retire-r0"), |envelope| {
            envelope.retire_observation(&descriptor)?;
            Ok(String::new())
        })
        .unwrap();
    let tip = store.kernel.tip().unwrap();
    assert_eq!(
        mark_republished(&store.kernel, target, &republished, &budget),
        Outcome::Changed
    );
    assert_eq!(store.kernel.tip().unwrap(), tip);
    assert!(stale_marks(&store, &attested).is_empty());
}

/// An applicability cycle pages through the commit its first pass captured, so an attestation
/// retired later in the cycle is still on its page; its standing at the pass target carries the
/// retirement, and the step passes over it with no evaluation and no refused commit.
#[test]
fn an_attestation_retired_within_an_applicability_cycle_is_passed_over() {
    let store = Store::open();
    let files = documents(N + 1);
    let repo = Repo::new("retired-in-cycle", &files);
    let counters = Counters::default();
    let mut verifier = Verifier::default();
    let attested = attest_n_plus_one(&store, &repo, &files, &mut verifier, &counters);
    let [_, current, _] = applicability_counts(&counters);
    pass(&mut verifier, &store, &repo, &counters);
    assert_eq!(
        applicability_counts(&counters),
        [0, current + MAX_VERDICTS, 0]
    );
    store
        .kernel
        .commit(intent("retire-every-attestation"), |envelope| {
            for attestation in &attested {
                envelope.retire_decision(attestation)?;
            }
            Ok(String::new())
        })
        .unwrap();
    for (i, (path, body)) in files.iter().enumerate() {
        let changed = format!("{body}changed\n");
        republish(&store, &repo, &format!("c{i}"), path, &changed, 2);
    }
    let report = pass(&mut verifier, &store, &repo, &counters);
    assert!(!report.failed, "{report:?}");
    assert_eq!(counters.get("commit_refused_changed"), 0);
    assert_eq!(
        applicability_counts(&counters),
        [0, current + MAX_VERDICTS, 0]
    );
    assert_eq!(verifier.applicability_cursor(&repo.scope_id()), None);
    for attestation in &attested {
        assert!(stale_marks(&store, attestation).is_empty(), "{attestation}");
    }
}

/// Each applicability cycle evaluates every live `Active` attestation of its project once, across
/// pages, and none of another project's: with every document of two projects republished, passes
/// over one project mark each of its attestations once, at most N per pass, and leave the other
/// project's attestations `Active`.
#[test]
fn an_applicability_cycle_marks_each_of_its_projects_attestations_once_and_no_other_projects() {
    let store = Store::open();
    let files = documents(PAGE_ROWS.get() + N + 1);
    let ours = Repo::new("ours", &files);
    let theirs = Repo::new("theirs", &files);
    let mut attested = [Vec::new(), Vec::new()];
    for (side, (repo, prefix)) in [(&ours, "o"), (&theirs, "t")].into_iter().enumerate() {
        store.scope(repo);
        for (i, (path, body)) in files.iter().enumerate() {
            let model = store.decision(repo, &format!("{prefix}{i}"), path, body);
            attested[side].push(attestation(&model, repo));
        }
    }
    let counters = Counters::default();
    let mut verifier = Verifier::default();
    let both = [ours.project(), theirs.project()];
    for _ in 0..files.len() {
        verifier.pass(&store.kernel, &both, &on, &counters, &Clock::open());
    }
    assert_eq!(
        counters.get("attestation_committed"),
        2 * files.len() as u64
    );
    for (repo, prefix) in [(&ours, "o"), (&theirs, "t")] {
        for (i, (path, body)) in files.iter().enumerate() {
            let changed = format!("{body}changed\n");
            republish(&store, repo, &format!("{prefix}{i}"), path, &changed, 2);
        }
    }

    let before = applicability_counts(&counters);
    let mut marked = 0;
    for _ in 0..=files.len().div_ceil(N) {
        pass(&mut verifier, &store, &ours, &counters);
        let [stale, current, unavailable] = applicability_counts(&counters);
        assert!(stale - marked <= MAX_VERDICTS, "{stale} after {marked}");
        assert_eq!([current, unavailable], [before[1], before[2]]);
        marked = stale;
    }
    assert_eq!(marked - before[0], files.len() as u64);
    for attestation in &attested[0] {
        assert_eq!(stale_marks(&store, attestation).len(), 1, "{attestation}");
    }
    for attestation in &attested[1] {
        assert!(stale_marks(&store, attestation).is_empty(), "{attestation}");
    }
}

/// A drained project whose store committed nothing since the previous pass reads no decisions page:
/// the page would start past the target's commit. The pass takes one reader for its target and one
/// for its applicability page; a commit since the previous pass adds the decisions page's reader.
#[test]
fn an_idle_pass_over_a_drained_project_reads_no_decisions_page() {
    let store = Store::open();
    let repo = Repo::new("idle", &[("doc.md".into(), text(0, 1024))]);
    store.scope(&repo);
    store.decision(&repo, "i0", "doc.md", &text(0, 1024));
    let counters = Counters::default();
    let mut verifier = Verifier::default();
    pass(&mut verifier, &store, &repo, &counters);
    pass(&mut verifier, &store, &repo, &counters);
    assert_eq!(counters.get("attestation_committed"), 1);
    assert!(verifier.floor(&repo.scope_id()).is_some());
    let readers = |verifier: &mut Verifier| {
        let before = store.kernel.controlled_acquisitions_for_test();
        pass(verifier, &store, &repo, &counters);
        store.kernel.controlled_acquisitions_for_test() - before
    };
    let [_, current, _] = applicability_counts(&counters);
    assert_eq!(readers(&mut verifier), 2);
    store
        .kernel
        .commit(intent("unrelated"), |envelope| {
            envelope.insert_domain(DomainSpec {
                domain_id: "unrelated".into(),
                object_id: "unrelated-domain".into(),
                name: "unrelated".into(),
                source_kind: "test".into(),
                source_id: "unrelated".into(),
                source_revision: 1,
                sensitivity: Sensitivity::Normal,
            })?;
            Ok(String::new())
        })
        .unwrap();
    assert_eq!(readers(&mut verifier), 3);
    assert_eq!(readers(&mut verifier), 2);
    assert_eq!(applicability_counts(&counters), [0, current + 3, 0]);
}

/// Rewrites the stored detail of the live descriptor citing `evidence_id` to another
/// `descriptor_version`, which the descriptor row checks refuse, and leaves its evidence digest.
fn corrupt_live_descriptor(store: &Store, evidence_id: &str) {
    let changed = store
        .sqlite()
        .execute(
            "UPDATE observations SET observation_payload=CAST(json_set(
               CAST(observation_payload AS TEXT), '$.detail', replace(json_extract(
               CAST(observation_payload AS TEXT), '$.detail'), '\"descriptor_version\":1',
               '\"descriptor_version\":99')) AS BLOB)
              WHERE evidence_id=?1 AND invalidated_commit_seq IS NULL
                AND observation_kind='source_descriptor'",
            [evidence_id],
        )
        .unwrap();
    assert_eq!(changed, 1);
}

/// A live descriptor whose evidence digest is the witnessed one counts current without its row
/// checks, and one whose digest differs runs them before any stale mark: a descriptor the row
/// checks refuse counts unavailable with no mark and the pass healthy.
#[test]
fn the_descriptor_row_checks_run_only_for_a_digest_that_differs_and_before_any_mark() {
    use daemon::attestation_verifier::applicability::{Applicability, evaluate};
    let store = Store::open();
    let files = documents(2);
    let repo = Repo::new("checked", &files);
    store.scope(&repo);
    let attested: Vec<String> = files
        .iter()
        .enumerate()
        .map(|(i, (path, body))| {
            attestation(&store.decision(&repo, &format!("d{i}"), path, body), &repo)
        })
        .collect();
    let counters = Counters::default();
    let mut verifier = Verifier::default();
    pass(&mut verifier, &store, &repo, &counters);
    assert_eq!(counters.get("attestation_committed"), 2);

    corrupt_live_descriptor(&store, "srcev:d0");
    let (path, body) = &files[1];
    republish(&store, &repo, "d1", path, &format!("{body}changed\n"), 2);
    corrupt_live_descriptor(&store, "srcev:d1:r2");
    let target = store.kernel.capture_commit_read_target().unwrap();
    let budget = EvalBudget::unbounded();
    // The row checks refuse the first descriptor too, through the citing read.
    assert_eq!(
        store.kernel.source_descriptor_citing_evidence(
            OccurrenceClass::RepositoryDocuments,
            "srcev:d0",
            target.through_commit,
            &budget,
        ),
        Err(KernelError::CorruptCanonicalRow)
    );
    assert_eq!(
        evaluate(&store.kernel, &attested[0], target, &budget),
        Ok(Applicability::Current)
    );
    assert_eq!(
        evaluate(&store.kernel, &attested[1], target, &budget),
        Err(KernelError::CorruptCanonicalRow)
    );

    let before = applicability_counts(&counters);
    let report = pass(&mut verifier, &store, &repo, &counters);
    assert!(!report.failed, "{report:?}");
    assert_eq!(
        applicability_counts(&counters),
        [before[0], before[1] + 1, before[2] + 1]
    );
    for attestation in &attested {
        assert!(stale_marks(&store, attestation).is_empty(), "{attestation}");
    }
}

/// The evaluation bound N holds across the pages of one pass: with one `Active` attestation on a
/// full first page and N on the second, the pass evaluates N in all and stops at the (N + 1)-th.
#[test]
fn the_evaluation_bound_holds_across_the_pages_of_a_pass() {
    let store = Store::open();
    let files = documents(N + 1);
    let repo = Repo::new("two-pages", &files);
    let counters = Counters::default();
    let mut verifier = Verifier::default();
    let attested = attest_n_plus_one(&store, &repo, &files, &mut verifier, &counters);
    assert_eq!(verifier.applicability_cursor(&repo.scope_id()), None);
    let ordered = page_order(&attested);
    let first = Identity::of_attestation(&ordered[0]).unwrap().source_id;
    let rows = PAGE_ROWS.get();
    store
        .kernel
        .commit(intent("stale-after-first"), |envelope| {
            // These source ids extend the first attestation's, so they sort between it and the
            // second.
            for i in 0..rows - 1 {
                let object_id = format!("stale_{i}");
                envelope.insert_decision(DecisionSpec {
                    decision_id: format!("dec_stale_{i}"),
                    object_id: object_id.clone(),
                    domain_id: "memory".into(),
                    proposition_id: None,
                    scope_id: Some(repo.scope_id()),
                    anchor_id: None,
                    evidence_id: None,
                    decision_kind: "ARCHITECTURE".into(),
                    payload: DecisionPayload {
                        summary: format!("stale attestation {i}"),
                        rationale: "{}".into(),
                    },
                    source_kind: "verifier".into(),
                    source_id: format!("{first}-{i:02}"),
                    source_revision: 1,
                    sensitivity: Sensitivity::Normal,
                })?;
                for kind in [EventKind::Other, EventKind::MarkStale] {
                    envelope.record_admission(AdmissionRequest {
                        candidate_id: None,
                        subject_object_id: Some(object_id.clone()),
                        source_class: Some(SourceClass::ModelInference),
                        taint_class: Some(TaintClass::AssistantInference),
                        event: AdmissionEvent {
                            kind,
                            trigger_object_id: None,
                            approval_object_id: None,
                            evidence_id: None,
                            reason: "fixture".into(),
                        },
                    })?;
                }
            }
            Ok(String::new())
        })
        .unwrap();

    let [_, current, _] = applicability_counts(&counters);
    let clock = Clock::open();
    verifier.pass(&store.kernel, &[repo.project()], &on, &counters, &clock);
    assert_accounting(&counters);
    assert_eq!(
        applicability_counts(&counters),
        [0, current + MAX_VERDICTS, 0]
    );
    assert_eq!(clock.polled(Poll::Decision), 1 + 2);
    assert_eq!(
        verifier.applicability_cursor(&repo.scope_id()).as_ref(),
        Some(&ordered[N - 1])
    );
}

/// A restore between a page's read transaction and its stale marks refuses the first mark as
/// changed and stops the project with one refusal counted; the next pass, on the restored store,
/// marks every attestation of the page.
#[test]
fn a_restore_before_a_pages_stale_marks_stops_the_project_after_one_refusal() {
    use std::os::unix::fs::PermissionsExt;
    let store = Store::open();
    let files = documents(2);
    let repo = Repo::new("restored", &files);
    store.scope(&repo);
    let attested: Vec<String> = files
        .iter()
        .enumerate()
        .map(|(i, (path, body))| {
            attestation(&store.decision(&repo, &format!("x{i}"), path, body), &repo)
        })
        .collect();
    let counters = Counters::default();
    let mut verifier = Verifier::default();
    pass(&mut verifier, &store, &repo, &counters);
    assert_eq!(counters.get("attestation_committed"), 2);
    for (i, (path, body)) in files.iter().enumerate() {
        republish(
            &store,
            &repo,
            &format!("x{i}"),
            path,
            &format!("{body}changed\n"),
            2,
        );
    }
    let destination = tempfile::tempdir().unwrap();
    std::fs::set_permissions(destination.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
    let backup = store
        .kernel
        .backup(kernel::BackupRequest {
            destination_directory: destination.path().to_path_buf(),
            deadline: Instant::now() + Duration::from_secs(30),
            capture_pin_expires_at: None,
        })
        .unwrap();

    let kernel = &store.kernel;
    let restored = AtomicBool::new(false);
    let clock = Clock::with(|poll, _| {
        if poll == Poll::Commit && !restored.swap(true, Ordering::SeqCst) {
            kernel.restore(&backup.destination_path).unwrap();
        }
        false
    });
    let report = verifier.pass(kernel, &[repo.project()], &on, &counters, &clock);
    assert!(restored.load(Ordering::SeqCst));
    assert!(report.failed, "{report:?}");
    assert_eq!(clock.polled(Poll::Commit), 1);
    assert_eq!(counters.get("commit_refused_changed"), 1);
    assert_eq!(applicability_counts(&counters)[0], 0);

    let report = pass(&mut verifier, &store, &repo, &counters);
    assert!(report.progressed && !report.failed, "{report:?}");
    assert_eq!(applicability_counts(&counters)[0], 2);
    for attestation in &attested {
        assert_eq!(stale_marks(&store, attestation).len(), 1, "{attestation}");
    }
}

/// A stale mark refused as changed with the store's incarnation unchanged resolves only its own
/// attestation: the step goes on to mark the next republished attestation of the page in the same
/// pass.
#[test]
fn a_changed_mark_leaves_the_rest_of_its_page_to_be_marked_in_the_same_pass() {
    let store = Store::open();
    let files = documents(2);
    let repo = Repo::new("changed-then-marked", &files);
    store.scope(&repo);
    let attested: Vec<String> = files
        .iter()
        .enumerate()
        .map(|(i, (path, body))| {
            attestation(&store.decision(&repo, &format!("v{i}"), path, body), &repo)
        })
        .collect();
    let counters = Counters::default();
    let mut verifier = Verifier::default();
    pass(&mut verifier, &store, &repo, &counters);
    assert_eq!(counters.get("attestation_committed"), 2);
    for (i, (path, body)) in files.iter().enumerate() {
        republish(
            &store,
            &repo,
            &format!("v{i}"),
            path,
            &format!("{body}changed\n"),
            2,
        );
    }
    let ordered = page_order(&attested);
    let first = attested.iter().position(|id| *id == ordered[0]).unwrap();
    let (path, body) = &files[first];

    let republished = AtomicBool::new(false);
    // The second commit poll follows the first attestation's refused mark, so the refusal precedes
    // the mark this pass still makes.
    let refused_first = AtomicBool::new(false);
    let clock = Clock::with(|poll, _| {
        if poll == Poll::Commit {
            if !republished.swap(true, Ordering::SeqCst) {
                republish(
                    &store,
                    &repo,
                    &format!("v{first}"),
                    path,
                    &format!("{body}again\n"),
                    3,
                );
            } else {
                refused_first.store(
                    counters.get("commit_refused_changed") == 1,
                    Ordering::SeqCst,
                );
            }
        }
        false
    });
    let report = verifier.pass(&store.kernel, &[repo.project()], &on, &counters, &clock);
    assert!(report.failed, "{report:?}");
    assert_eq!(clock.polled(Poll::Commit), 2);
    assert!(refused_first.load(Ordering::SeqCst));
    assert_eq!(counters.get("commit_refused_changed"), 1);
    assert_eq!(applicability_counts(&counters)[0], 1);
    assert!(stale_marks(&store, &ordered[0]).is_empty());
    assert_eq!(stale_marks(&store, &ordered[1]).len(), 1);
}

/// A stale mark the Kernel rolls back resolves only its own attestation: with every mark refused,
/// one pass attempts the mark of each republished attestation of the page and counts each refusal.
#[test]
fn a_rolled_back_stale_mark_leaves_the_rest_of_its_page_to_their_own_marks() {
    let store = Store::open();
    let files = documents(2);
    let repo = Repo::new("rolled-back-page", &files);
    store.scope(&repo);
    let attested: Vec<String> = files
        .iter()
        .enumerate()
        .map(|(i, (path, body))| {
            attestation(&store.decision(&repo, &format!("q{i}"), path, body), &repo)
        })
        .collect();
    let counters = Counters::default();
    let mut verifier = Verifier::default();
    pass(&mut verifier, &store, &repo, &counters);
    assert_eq!(counters.get("attestation_committed"), 2);
    for (i, (path, body)) in files.iter().enumerate() {
        republish(
            &store,
            &repo,
            &format!("q{i}"),
            path,
            &format!("{body}changed\n"),
            2,
        );
    }
    store
        .sqlite()
        .execute_batch(
            "CREATE TRIGGER fail_stale BEFORE INSERT ON admission_decisions
              WHEN NEW.event_kind='mark_stale'
              BEGIN SELECT RAISE(ABORT, 'fault at the stale mark'); END;",
        )
        .unwrap();
    let clock = Clock::open();
    let report = verifier.pass(&store.kernel, &[repo.project()], &on, &counters, &clock);
    assert!(report.failed && !report.progressed, "{report:?}");
    assert_eq!(clock.polled(Poll::Commit), 2);
    assert_eq!(
        [
            "commit_refused_busy",
            "commit_refused_io",
            "commit_refused_other"
        ]
        .map(|name| counters.get(name))
        .iter()
        .sum::<u64>(),
        2
    );
    for attestation in &attested {
        assert!(stale_marks(&store, attestation).is_empty(), "{attestation}");
    }
}
