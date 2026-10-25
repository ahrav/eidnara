use context_core::canonical_json::is_lower_hex;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::blank;

/// `transfer` bounds retrieval of another task's evidence; `project_knowledge`
/// bounds retrieval of the target task's own project knowledge.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CeilingKind {
    Transfer,
    ProjectKnowledge,
}

/// An oracle ceiling bounds retrieval for one `source_task` and
/// `target_task` pair at `revision_sha`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CeilingDeclaration {
    pub kind: CeilingKind,
    pub source_task: String,
    pub target_task: String,
    pub revision_sha: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CeilingError {
    Shape(String),
    BlankTask {
        field: &'static str,
    },
    /// `revision_sha` is not 40 lowercase hex characters, the form the run
    /// identity's `code_sha` takes.
    MalformedRevision {
        revision_sha: String,
    },
    TransferWithinOneTask {
        task: String,
    },
    ProjectKnowledgeAcrossTasks {
        source_task: String,
        target_task: String,
    },
}

debug_display!(CeilingError);

impl CeilingDeclaration {
    pub fn validate(&self) -> Result<(), CeilingError> {
        for (field, task) in [
            ("source_task", &self.source_task),
            ("target_task", &self.target_task),
        ] {
            if blank(task) {
                return Err(CeilingError::BlankTask { field });
            }
        }
        if !is_lower_hex(&self.revision_sha, 40) {
            return Err(CeilingError::MalformedRevision {
                revision_sha: self.revision_sha.clone(),
            });
        }
        match self.kind {
            CeilingKind::Transfer if self.source_task == self.target_task => {
                Err(CeilingError::TransferWithinOneTask {
                    task: self.source_task.clone(),
                })
            }
            CeilingKind::ProjectKnowledge if self.source_task != self.target_task => {
                Err(CeilingError::ProjectKnowledgeAcrossTasks {
                    source_task: self.source_task.clone(),
                    target_task: self.target_task.clone(),
                })
            }
            _ => Ok(()),
        }
    }
}

pub fn parse_ceiling_declaration(value: &Value) -> Result<CeilingDeclaration, CeilingError> {
    let declaration: CeilingDeclaration = serde_json::from_value(value.clone())
        .map_err(|error| CeilingError::Shape(error.to_string()))?;
    declaration.validate()?;
    Ok(declaration)
}
