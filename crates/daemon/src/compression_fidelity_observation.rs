//! Owner-attributed observations for the compression fidelity witnesses. The library test module
//! and the daemon's integration tests share this file through path-based inclusion. An atomic
//! no-replace link publishes each record from a temporary file created with mode `0600`, inside a
//! directory created with mode `0700`, when `EIDNARA_FIDELITY_OBSERVATIONS_DIR` names it.

use std::io::Write;
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt};
use std::path::{Path, PathBuf};

use serde::Serialize;
use serde_json::{Value, json};

pub(crate) const OBSERVATIONS_DIR: &str = "EIDNARA_FIDELITY_OBSERVATIONS_DIR";
pub(crate) const SCHEMA_VERSION: u32 = 1;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum Terminal {
    Published,
    Served,
    ValidationRejected,
    DiscardedCoverage,
    DriftRejected,
    InputTruncated,
    ReadExact,
    /// The run did not settle within its wait, or never started a producer.
    Unsettled,
}

#[derive(Debug, Serialize)]
pub(crate) struct Observation {
    pub schema_version: u32,
    pub owner: &'static str,
    pub corpus_sha256: &'static str,
    pub case: String,
    pub source: String,
    /// The corpus scenario this observation witnesses; `None` for a source-level stage.
    pub scenario: Option<String>,
    pub stage: String,
    pub terminal: Terminal,
    pub markers: Vec<&'static str>,
    pub detail: Value,
}

impl Observation {
    pub(crate) fn new(
        owner: &'static str,
        corpus_sha256: &'static str,
        case: &str,
        source: &str,
        stage: impl Into<String>,
        terminal: Terminal,
    ) -> Self {
        Self {
            schema_version: SCHEMA_VERSION,
            owner,
            corpus_sha256,
            case: case.to_owned(),
            source: source.to_owned(),
            scenario: None,
            stage: stage.into(),
            terminal,
            markers: Vec::new(),
            detail: json!({}),
        }
    }

    pub(crate) fn scenario(mut self, scenario: &str) -> Self {
        self.scenario = Some(scenario.to_owned());
        self
    }

    pub(crate) fn with(mut self, detail: Value) -> Self {
        self.detail = detail;
        self
    }

    pub(crate) fn mark(mut self, marker: &'static str) -> Self {
        self.markers.push(marker);
        self
    }

    fn file_name(&self) -> String {
        let label = self.scenario.as_deref().unwrap_or(&self.source);
        format!("{}.{}.{label}.{}.json", self.owner, self.case, self.stage)
    }

    pub(crate) fn emit(self) -> Self {
        if let Some(dir) = std::env::var_os(OBSERVATIONS_DIR) {
            self.emit_to(&PathBuf::from(dir));
        }
        self
    }

    /// Publishes the complete record at its final path; `emit_to` panics when that path exists,
    /// whether at the check or at publication.
    pub(crate) fn emit_to(&self, dir: &Path) -> PathBuf {
        std::fs::DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(dir)
            .unwrap_or_else(|error| panic!("{}: {error}", dir.display()));
        let path = dir.join(self.file_name());
        assert!(!path.exists(), "{} was already written", path.display());
        let temporary = dir.join(format!(".{}.tmp", self.file_name()));
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&temporary)
            .unwrap_or_else(|error| panic!("{}: {error}", temporary.display()));
        file.write_all(&serde_json::to_vec_pretty(self).unwrap())
            .and_then(|()| file.sync_all())
            .unwrap_or_else(|error| panic!("{}: {error}", temporary.display()));
        publish(&temporary, &path).unwrap_or_else(|error| panic!("{}: {error}", path.display()));
        path
    }
}

/// Links the complete temporary file to its final name; the link fails when that name exists,
/// so a writer whose existence check raced another writer keeps the first record intact.
pub(crate) fn publish(temporary: &Path, path: &Path) -> std::io::Result<()> {
    let linked = std::fs::hard_link(temporary, path);
    std::fs::remove_file(temporary)?;
    linked
}
