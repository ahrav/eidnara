use serde::{Deserialize, Serialize};

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
    pub const OFF: Self = Self {
        source_capture: CaptureSwitch::Off,
        memory_capture: CaptureSwitch::Off,
        write_key: WriteKeyScheme::TaskArm,
    };

    pub fn captures(&self) -> bool {
        self.source_capture == CaptureSwitch::On || self.memory_capture == CaptureSwitch::On
    }
}
