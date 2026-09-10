//! The project a route is bound to, and how kernel rows are filtered by it.
//!
//! One kernel root serves every project on the host; a project is a scope
//! term on the `project` dimension whose exact value is the digest of the
//! bound root. A row serves to a route only when its scope names that digest.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use kernel::{
    CommitIntent, Dimension, KernelError, KernelStore, ProjectScope, ScopeSpec, ScopeTermFilter,
    ScopeTermSpec, Sensitivity,
};
use memory_store::canonical_root;
use serde::Deserialize;
use sha2::{Digest, Sha256};

/// Source kind stamped on the scope object the route materializes per project.
const PROJECT_SCOPE_SOURCE_KIND: &str = "kernel_route";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProjectBinding {
    /// The bound root after canonicalization, compared and hashed as the raw
    /// path bytes so two roots that differ only in bytes a lossy string would
    /// fold together stay distinct.
    root: PathBuf,
    /// `sha256(root)`: the scope term's exact value, and the handle the scope
    /// id and the per-project operation-key prefix are built from. A path can
    /// match a secret detector and be stored as a placeholder, which would
    /// leave the route unable to match its own rows; the digest cannot.
    digest: String,
    /// The kernel's view of the same project: the scope every eligibility and
    /// read decision is judged against.
    scope: ProjectScope,
}

impl ProjectBinding {
    pub(crate) fn new(root: &Path) -> Self {
        let root = canonical_root(root);
        let digest = format!("{:x}", Sha256::digest(identity_bytes(&root)));
        let scope = ProjectScope::new(&digest).expect("a sha256 hex digest is a project term");
        Self {
            root,
            digest,
            scope,
        }
    }

    pub(crate) fn scope(&self) -> &ProjectScope {
        &self.scope
    }

    /// A request's `project_root` is compared after the same canonicalization
    /// the binding went through, so a symlinked spelling of the bound root passes.
    pub(crate) fn accepts(&self, requested: &Path) -> bool {
        canonical_root(requested) == self.root
    }

    pub(crate) fn scope_id(&self) -> String {
        format!("project:{}", self.digest)
    }

    /// The receipt key the store sees. `(producer, operation_key)` is unique
    /// kernel-wide: the project digest keeps two projects apart, and `family`
    /// keeps one route family's idempotency receipt from answering another's
    /// request. A blank key is refused here because the prefix would otherwise
    /// hide it from the kernel's emptiness check.
    pub(crate) fn operation_key(&self, family: &str, operation_key: &str) -> Option<String> {
        if operation_key.trim().is_empty() {
            return None;
        }
        Some(format!("{}:{family}:{operation_key}", self.digest))
    }

    /// The scope object every route-written row names.
    pub(crate) fn scope_spec(&self, domain_id: &str) -> ScopeSpec {
        let scope_id = self.scope_id();
        ScopeSpec {
            object_id: scope_id.clone(),
            source_id: scope_id.clone(),
            scope_id,
            domain_id: domain_id.to_string(),
            source_kind: PROJECT_SCOPE_SOURCE_KIND.to_string(),
            source_revision: 1,
            sensitivity: Sensitivity::Normal,
            terms: vec![ScopeTermSpec {
                dimension: Dimension::Project.as_str().to_string(),
                operator: "exact".to_string(),
                exact_value: Some(self.digest.clone()),
                ..ScopeTermSpec::default()
            }],
        }
    }

    /// The term a stored scope must carry to name this project.
    pub(crate) fn scope_term(&self) -> ScopeTermFilter<'_> {
        ScopeTermFilter {
            dimension: Dimension::Project,
            value: &self.digest,
        }
    }
}

/// The bytes a root is hashed over. The digest is persisted in scope ids, so
/// it is taken over the platform's own path representation, which is stable
/// across Rust releases, rather than `OsStr::as_encoded_bytes`, whose
/// encoding the standard library leaves unspecified.
#[cfg(unix)]
fn identity_bytes(root: &Path) -> std::borrow::Cow<'_, [u8]> {
    use std::os::unix::ffi::OsStrExt as _;
    std::borrow::Cow::Borrowed(root.as_os_str().as_bytes())
}

/// UTF-16 code units in little-endian byte order.
#[cfg(windows)]
fn identity_bytes(root: &Path) -> std::borrow::Cow<'_, [u8]> {
    use std::os::windows::ffi::OsStrExt as _;
    std::borrow::Cow::Owned(
        root.as_os_str()
            .encode_wide()
            .flat_map(u16::to_le_bytes)
            .collect(),
    )
}

/// A commit intent as the wire carries it; `kernel.commit` and artifact
/// ingestion accept the same shape.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct IntentRequest {
    pub(crate) producer: String,
    pub(crate) operation_key: String,
    pub(crate) request_digest: String,
    pub(crate) actor: String,
    pub(crate) cause: String,
}

impl IntentRequest {
    /// The intent the store receives, its key namespaced to `project` and
    /// `family`; `None` when the caller's key is blank.
    pub(crate) fn into_intent(
        self,
        project: &ProjectBinding,
        family: &str,
    ) -> Option<CommitIntent> {
        let operation_key = project.operation_key(family, &self.operation_key)?;
        Some(CommitIntent {
            producer: self.producer,
            operation_key,
            request_digest: self.request_digest,
            actor: self.actor,
            cause: self.cause,
        })
    }
}

/// The stored terms of one scope, or `None` when no scope row exists.
pub(crate) type ScopeTerms<'a> =
    dyn FnMut(&str) -> Result<Option<Vec<ScopeTermSpec>>, KernelError> + 'a;

/// Loads scope terms from the store's own read snapshot.
pub(crate) fn stored_terms(
    store: &KernelStore,
) -> impl FnMut(&str) -> Result<Option<Vec<ScopeTermSpec>>, KernelError> + '_ {
    move |scope_id| match store.scope_terms(scope_id) {
        Ok(terms) => Ok(Some(terms)),
        Err(KernelError::NotFound) => Ok(None),
        Err(error) => Err(error),
    }
}

/// Answers whether a row's scope names the bound project, remembering each
/// scope's verdict for the duration of one request. The decision itself is
/// the kernel's [`ProjectScope::names_project`], so a route and the kernel's
/// own eligibility judgement cannot disagree about a scope.
pub(crate) struct ScopeFilter {
    project: ProjectScope,
    verdicts: HashMap<String, bool>,
}

impl ScopeFilter {
    pub(crate) fn new(project: &ProjectBinding) -> Self {
        Self {
            project: project.scope.clone(),
            verdicts: HashMap::new(),
        }
    }

    pub(crate) fn matches(
        &mut self,
        scope_id: Option<&str>,
        terms: &mut ScopeTerms<'_>,
    ) -> Result<bool, KernelError> {
        let Some(scope_id) = scope_id else {
            return Ok(false);
        };
        if let Some(verdict) = self.verdicts.get(scope_id) {
            return Ok(*verdict);
        }
        let verdict = self.project.names_project(terms(scope_id)?.as_deref());
        self.verdicts.insert(scope_id.to_string(), verdict);
        Ok(verdict)
    }
}
