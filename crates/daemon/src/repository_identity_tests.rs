use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use std::time::{Duration, Instant};

use kernel::applicability::EvalBudget;

use super::source_corpus::git;
use crate::repository_identity::{
    DerivationFailure, MAX_ROOT_WALK_COMMITS, RepositoryIdentity, derive,
};

fn budget() -> EvalBudget {
    EvalBudget::new(
        Some(Instant::now() + Duration::from_secs(30)),
        Arc::new(AtomicBool::new(false)),
    )
}

fn commit(dir: &Path, file: &str, text: &str) {
    std::fs::write(dir.join(file), text).unwrap();
    git(dir, &["add", file]).unwrap();
    git(dir, &["commit", "-q", "-m", text]).unwrap();
}

fn repository(dir: &Path, count: usize) {
    std::fs::create_dir_all(dir).unwrap();
    git(dir, &["init", "-q", "-b", "main"]).unwrap();
    for index in 0..count {
        commit(dir, "file.txt", &format!("commit {index}"));
    }
}

/// The plugin resolver's rendering: `git rev-list --max-parents=0 HEAD`, sorted, first.
fn plugin_identity(dir: &Path) -> String {
    let roots = git(dir, &["rev-list", "--max-parents=0", "HEAD"]).unwrap();
    let mut roots: Vec<&str> = roots.lines().map(str::trim).collect();
    roots.sort_unstable();
    format!("git:{}", roots[0])
}

fn derived(dir: &Path) -> RepositoryIdentity {
    derive(dir, &budget(), MAX_ROOT_WALK_COMMITS).unwrap()
}

#[test]
fn a_repository_derives_the_plugin_rendering_of_its_root() {
    let dir = tempfile::tempdir().unwrap();
    repository(dir.path(), 3);
    let identity = derived(dir.path());
    assert_eq!(
        identity,
        RepositoryIdentity::Git(plugin_identity(dir.path()))
    );
    let RepositoryIdentity::Git(value) = identity else {
        unreachable!()
    };
    let hex = value.strip_prefix("git:").unwrap();
    assert_eq!(hex.len(), 40);
    assert!(
        hex.bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    );
}

/// A merged orphan branch gives the history two roots; both resolvers pick the byte-order-smallest
/// whichever side of the merge HEAD sits on, so the smallest root is the first one a walk meets in
/// one case and not in the other.
#[test]
fn two_roots_derive_the_byte_order_smallest() {
    for head in ["main", "other"] {
        let dir = tempfile::tempdir().unwrap();
        repository(dir.path(), 1);
        git(dir.path(), &["checkout", "-q", "--orphan", "other"]).unwrap();
        git(dir.path(), &["rm", "-q", "-rf", "."]).unwrap();
        commit(dir.path(), "other.txt", "other root");
        let merged = if head == "main" { "other" } else { "main" };
        git(dir.path(), &["checkout", "-q", head]).unwrap();
        let merge = [
            "merge",
            "-q",
            "--allow-unrelated-histories",
            "-m",
            "merge",
            merged,
        ];
        git(dir.path(), &merge).unwrap();
        let roots = git(dir.path(), &["rev-list", "--max-parents=0", "HEAD"]).unwrap();
        assert_eq!(roots.lines().count(), 2, "{head}");
        assert_eq!(
            derived(dir.path()),
            RepositoryIdentity::Git(plugin_identity(dir.path())),
            "{head}"
        );
    }
}

/// Replacement refs are ignored: a `refs/replace` entry that swaps HEAD for a parentless commit
/// makes `git` report HEAD as the root, while the derived value stays the root the objects record.
#[test]
fn a_replacement_ref_leaves_the_recorded_root() {
    let dir = tempfile::tempdir().unwrap();
    repository(dir.path(), 2);
    let recorded = plugin_identity(dir.path());
    let substitute = git(
        dir.path(),
        &["commit-tree", "HEAD^{tree}", "-m", "substitute"],
    )
    .unwrap();
    git(dir.path(), &["replace", "HEAD", substitute.trim()]).unwrap();
    assert_ne!(plugin_identity(dir.path()), recorded);
    assert_eq!(derived(dir.path()), RepositoryIdentity::Git(recorded));
}

/// A `.git` entry that discovery cannot open is an unreadable repository, retried on the next
/// pass, never a project without one.
#[test]
fn an_unopenable_git_entry_fails_the_derivation() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join(".git"), "not a gitdir link").unwrap();
    assert_eq!(
        derive(dir.path(), &budget(), MAX_ROOT_WALK_COMMITS),
        Err(DerivationFailure::Unreadable)
    );
}

/// Discovery searches upward, as the plugin's resolver does, so a project bound below the
/// repository's top derives the repository's identity.
#[test]
fn a_subdirectory_derives_its_repository() {
    let dir = tempfile::tempdir().unwrap();
    repository(dir.path(), 2);
    let nested = dir.path().join("packages/app");
    std::fs::create_dir_all(&nested).unwrap();
    assert_eq!(derived(&nested), derived(dir.path()));
}

/// Every fallback is a function of the repository's state, so a second derivation agrees.
#[test]
fn states_without_a_settled_root_fall_back_to_the_project() {
    let plain = tempfile::tempdir().unwrap();
    let unborn = tempfile::tempdir().unwrap();
    git(unborn.path(), &["init", "-q"]).unwrap();
    let source = tempfile::tempdir().unwrap();
    repository(source.path(), 3);
    let shallow = tempfile::tempdir().unwrap();
    let url = format!("file://{}", source.path().display());
    git(
        shallow.path(),
        &["clone", "-q", "--depth", "1", &url, "clone"],
    )
    .unwrap();
    let shallow = shallow.path().join("clone");
    for dir in [plain.path(), unborn.path(), &shallow] {
        for _ in 0..2 {
            assert_eq!(
                derived(dir),
                RepositoryIdentity::Fallback,
                "{}",
                dir.display()
            );
        }
    }
    assert_eq!(
        derive(source.path(), &budget(), 2).unwrap(),
        RepositoryIdentity::Fallback,
        "a history past the cap"
    );
    assert_eq!(
        derive(source.path(), &budget(), 3).unwrap(),
        RepositoryIdentity::Git(plugin_identity(source.path())),
        "a history within the cap"
    );
}

#[test]
fn an_exhausted_budget_or_an_unreadable_object_fails_the_derivation() {
    let dir = tempfile::tempdir().unwrap();
    repository(dir.path(), 2);
    let spent = EvalBudget::new(Some(Instant::now()), Arc::new(AtomicBool::new(false)));
    assert_eq!(
        derive(dir.path(), &spent, MAX_ROOT_WALK_COMMITS),
        Err(DerivationFailure::Budget)
    );
    let root = git(dir.path(), &["rev-list", "--max-parents=0", "HEAD"]).unwrap();
    let root = root.trim();
    std::fs::remove_file(
        dir.path()
            .join(".git/objects")
            .join(&root[..2])
            .join(&root[2..]),
    )
    .unwrap();
    assert_eq!(
        derive(dir.path(), &budget(), MAX_ROOT_WALK_COMMITS),
        Err(DerivationFailure::Unreadable)
    );
}

/// The identity changes only the value of one field: the encodings and versions stay, and the
/// longest `git:` value, a SHA-256 id, is a well-formed identity value.
#[test]
fn the_derived_value_fits_the_unchanged_identity_encoding() {
    assert_eq!(kernel::source_identity::OCCURRENCE_ENCODING_VERSION, 2);
    assert_eq!(
        kernel::source_identity::OccurrenceClass::RepositoryDocuments.identity_fields(),
        ["project_id", "repository_id", "relative_path"]
    );
    assert_eq!(retrieval::SCHEMA_VERSION, 8);
    assert!(kernel::source_identity::well_formed_value(&format!(
        "git:{}",
        "f".repeat(64)
    )));
    assert_eq!(RepositoryIdentity::Fallback.value("project"), "project");
}
