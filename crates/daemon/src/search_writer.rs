//! The quarantine every daemon writer into `search.sqlite` enters when the projection's contents fall into doubt.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum QuarantineKind {
    /// Stored rows disagree with what was written or with their own digests.
    Integrity,
    /// The projection store failed while running or reading back a transaction, so its durable contents cannot be trusted from this side.
    Storage,
}

/// The reason a writer stopped trusting the projection; every later call on that writer returns it unchanged.
/// `detail` is unredacted backend error text for the operator and must not be forwarded to untrusted sinks.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Quarantine {
    pub kind: QuarantineKind,
    pub detail: String,
}

impl Quarantine {
    pub(crate) fn new(kind: QuarantineKind, error: &dyn std::fmt::Display) -> Self {
        Self {
            kind,
            detail: error.to_string(),
        }
    }
}
