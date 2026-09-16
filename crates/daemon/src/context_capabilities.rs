//! Consumer capability strings are claims; the host backend's declaration is the only authorization truth for an edit class, and it is read once at bind.

use std::sync::Arc;

use host_runtime::model_execution::backend::{
    ContextCapabilities, EditClass, Harness, LlmExecutionBackend,
};

/// Reads a harness's declaration; a harness the host has no adapter for is unreadable rather than empty, so the two failures stay distinct.
pub trait CapabilitySource: Send + Sync {
    fn declare(&self, harness: &str) -> Result<ContextCapabilities, &'static str>;
}

impl<T: LlmExecutionBackend + ?Sized> CapabilitySource for T {
    fn declare(&self, harness: &str) -> Result<ContextCapabilities, &'static str> {
        let harness = Harness::parse(harness).ok_or("unknown_harness")?;
        // An unavailable backend returns its reason instead of declaring `ContextCapabilities::NONE`.
        match self.unavailable_reason(harness) {
            Some(reason) => Err(reason),
            None => Ok(self.context_capabilities(harness)),
        }
    }
}

/// The declaration a route binding holds for its whole epoch.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LatchedCapabilities {
    Declared(ContextCapabilities),
    Unreadable(&'static str),
}

impl LatchedCapabilities {
    pub fn read(source: Option<&Arc<dyn CapabilitySource>>, harness: &str) -> Self {
        match source {
            None => Self::Unreadable("no_declaration"),
            Some(source) => match source.declare(harness) {
                Ok(capabilities) => Self::Declared(capabilities),
                Err(reason) => Self::Unreadable(reason),
            },
        }
    }

    pub fn gate(&self, class: EditClass) -> Result<(), CapabilityDenial> {
        match self {
            Self::Unreadable(reason) => Err(CapabilityDenial::Unreadable { class, reason }),
            Self::Declared(capabilities) if capabilities.allows(class) => Ok(()),
            Self::Declared(_) => Err(CapabilityDenial::Unsupported { class }),
        }
    }
}

/// Both fail closed; the reason tells an operator whether to enable a class or to repair the declaration.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CapabilityDenial {
    Unsupported {
        class: EditClass,
    },
    Unreadable {
        class: EditClass,
        reason: &'static str,
    },
}

/// The production source: each harness's availability and declaration, read from the host's backend once at construction. The backend's availability read revalidates the installed closure, so a bind is a table lookup.
pub struct BackendDeclarations {
    entries: [(Harness, Result<ContextCapabilities, &'static str>); 2],
}

impl BackendDeclarations {
    pub fn new(backend: &Arc<dyn LlmExecutionBackend>) -> Self {
        let read = |harness: Harness| (harness, backend.declare(harness.as_str()));
        Self {
            entries: [read(Harness::OpenCode), read(Harness::Pi)],
        }
    }
}

impl CapabilitySource for BackendDeclarations {
    fn declare(&self, harness: &str) -> Result<ContextCapabilities, &'static str> {
        let harness = Harness::parse(harness).ok_or("unknown_harness")?;
        self.entries
            .iter()
            .find(|(known, _)| *known == harness)
            .map(|(_, declared)| *declared)
            .unwrap_or(Err("unknown_harness"))
    }
}

/// A fixed declaration per harness name, for hosts that build no backend and for tests.
pub struct StaticDeclarations {
    entries: Vec<(String, ContextCapabilities)>,
}

impl StaticDeclarations {
    pub fn new(entries: Vec<(String, ContextCapabilities)>) -> Self {
        Self { entries }
    }
}

impl CapabilitySource for StaticDeclarations {
    fn declare(&self, harness: &str) -> Result<ContextCapabilities, &'static str> {
        self.entries
            .iter()
            .find(|(name, _)| name == harness)
            .map(|(_, capabilities)| *capabilities)
            .ok_or("unknown_harness")
    }
}
