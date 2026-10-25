//! The capture declaration's refusals live on the types that read it:
//! `ManifestError::MalformedCapture` and
//! `ManifestError::CaptureOnWithoutFrozenFamily` for the manifest's copy, and
//! `StatisticsError::CaptureDeclarationMismatch` at paired analysis.

use serde::{Deserialize, Serialize};

/// The `run_identity.config` key holding the run's copy of the declaration.
pub const CAPTURE_CONFIG_KEY: &str = "capture";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CaptureSwitch {
    On,
    Off,
}

/// `task_arm` keys a memory write by the writing sample's task and arm.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WriteKeyScheme {
    TaskArm,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CaptureDeclaration {
    pub source_capture: CaptureSwitch,
    pub memory_capture: CaptureSwitch,
    pub write_key: WriteKeyScheme,
}

impl CaptureDeclaration {
    /// Both switches `off`, the declaration an undeclared run agrees with.
    pub const OFF: Self = Self {
        source_capture: CaptureSwitch::Off,
        memory_capture: CaptureSwitch::Off,
        write_key: WriteKeyScheme::TaskArm,
    };

    /// `write_key` is irrelevant here: only the two switches capture.
    pub fn any_on(&self) -> bool {
        self.source_capture == CaptureSwitch::On || self.memory_capture == CaptureSwitch::On
    }
}
