//! The repository a project's captured documents belong to. Two checkouts of one repository bind
//! two project digests, so the digest alone splits a document's lineage per checkout; the
//! smallest root commit reachable from HEAD is shared by every checkout of the history and is the
//! value the plugin's project-identity resolver renders (`git rev-list --max-parents=0 HEAD`,
//! sorted first). A repository whose roots this walk cannot settle from its own state (no
//! repository, unborn HEAD, a shallow history, an object format this build does not read, or a
//! history past [`MAX_ROOT_WALK_COMMITS`]) takes the project digest, and the answer depends on that
//! state alone, so it is stable across passes.
//!
//! The daemon reads the repository without user configuration and with replacement refs ignored,
//! so a repository whose `refs/replace` entries or grafts substitute a root derives the root its
//! objects record, where the plugin's `git` invocation renders the substitute.

use std::num::NonZeroU64;
use std::path::Path;

use kernel::applicability::EvalBudget;

/// The commits one derivation visits before the history counts as past the cap, the default
/// #1105 OQ1 sets for the capture job budget.
pub(crate) const MAX_ROOT_WALK_COMMITS: usize = 1 << 18;

/// gix's allocation limit for the walk: a commit or a delta base above it fails the derivation
/// instead of being inflated.
const MAX_ROOT_WALK_OBJECT_BYTES: NonZeroU64 = NonZeroU64::new(16 << 20).unwrap();

/// A derivation outcome fixed by the repository's state; capture memoizes it for the daemon
/// incarnation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum RepositoryIdentity {
    /// `git:` and the lowercase hex id of the byte-order-smallest root commit reachable from HEAD.
    Git(String),
    /// The project digest stands in for the repository.
    Fallback,
}

impl RepositoryIdentity {
    /// The `repository_id` identity value under the project `project_id`.
    pub(crate) fn value<'a>(&'a self, project_id: &'a str) -> &'a str {
        match self {
            Self::Git(value) => value,
            Self::Fallback => project_id,
        }
    }
}

/// A derivation the environment cut short; the next pass derives again.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum DerivationFailure {
    /// The repository or an object it names could not be read.
    Unreadable,
    /// The job's budget ended before the walk did.
    Budget,
}

/// Derives the identity of the repository holding `root`, visiting at most `cap` commits within
/// `budget`.
///
/// Discovery searches `root` and its parents, as the plugin's resolver does, and opens the
/// repository without user, system, or installation configuration and with replacement refs
/// ignored, so no configuration or `refs/replace` entry substitutes another root.
pub(crate) fn derive(
    root: &Path,
    budget: &EvalBudget,
    cap: usize,
) -> Result<RepositoryIdentity, DerivationFailure> {
    use gix::discover::upwards::Error as Upwards;
    let options = crate::git_sources::isolated_options(MAX_ROOT_WALK_OBJECT_BYTES);
    let dot_git = root.join(".git");
    // Upward discovery skips a `.git` entry it cannot read. A root that holds one opens it directly,
    // so a broken entry at the root fails the derivation; above the root, an unusable candidate is
    // skipped.
    let opened = if std::fs::symlink_metadata(&dot_git).is_ok() {
        gix::ThreadSafeRepository::open_opts(dot_git, options).map_err(gix::discover::Error::Open)
    } else {
        let trust = gix::sec::trust::Mapping {
            full: options.clone(),
            reduced: options,
        };
        gix::ThreadSafeRepository::discover_opts(root, Default::default(), trust)
    };
    let mut repo = match opened {
        Ok(repo) => repo.to_thread_local(),
        Err(gix::discover::Error::Discover(
            Upwards::NoGitRepository { .. }
            | Upwards::NoGitRepositoryWithinCeiling { .. }
            | Upwards::NoGitRepositoryWithinFs { .. },
        )) => {
            return Ok(RepositoryIdentity::Fallback);
        }
        Err(gix::discover::Error::Open(error))
            if crate::git_sources::declares_unsupported_object_format(&error) =>
        {
            return Ok(RepositoryIdentity::Fallback);
        }
        Err(_) => return Err(DerivationFailure::Unreadable),
    };
    repo.objects.ignore_replacements = true;
    // A bare repository falls back, as the plugin's resolver does.
    if repo.is_bare() || repo.is_shallow() {
        return Ok(RepositoryIdentity::Fallback);
    }
    let head = repo.head().map_err(|_| DerivationFailure::Unreadable)?;
    let Some(head) = head.id() else {
        return Ok(RepositoryIdentity::Fallback);
    };
    let walk = repo
        .rev_walk([head.detach()])
        .all()
        .map_err(|_| DerivationFailure::Unreadable)?;
    let mut smallest: Option<String> = None;
    for (visited, info) in walk.enumerate() {
        if visited == cap {
            return Ok(RepositoryIdentity::Fallback);
        }
        if budget.is_exhausted() {
            return Err(DerivationFailure::Budget);
        }
        let info = info.map_err(|_| DerivationFailure::Unreadable)?;
        if info.parent_ids.is_empty() {
            let hex = info.id.to_hex().to_string();
            if smallest.as_ref().is_none_or(|current| hex < *current) {
                smallest = Some(hex);
            }
        }
    }
    // A completed walk that met no parentless commit is a property of the history, so it falls back.
    Ok(smallest.map_or(RepositoryIdentity::Fallback, |hex| {
        RepositoryIdentity::Git(format!("git:{hex}"))
    }))
}
