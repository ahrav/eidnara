use memory_store::{MemoryStore, ServedBlockFingerprint, TailHygieneBaseline};

use crate::tail_hygiene::BaselineParts;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct DerivedState {
    pub(crate) revert_epoch: u64,
    pub(crate) accepted_row_version: u64,
    pub(crate) served: Vec<ServedBlockFingerprint>,
    pub(crate) baseline_parts: Option<BaselineParts>,
}

impl DerivedState {
    pub(crate) fn served_for(&self, revert_epoch: u64) -> Option<&[ServedBlockFingerprint]> {
        (self.revert_epoch == revert_epoch).then_some(self.served.as_slice())
    }

    pub(crate) fn parts_for(
        &self,
        revert_epoch: u64,
        baseline: Option<&TailHygieneBaseline>,
    ) -> Option<&BaselineParts> {
        let baseline = baseline?;
        self.baseline_parts.as_ref().filter(|parts| {
            self.revert_epoch == revert_epoch
                && parts.baseline_generation == baseline.baseline_generation
        })
    }

    pub(crate) fn supersedes(&self, current: &Self) -> bool {
        (self.revert_epoch, self.accepted_row_version)
            >= (current.revert_epoch, current.accepted_row_version)
    }

    pub(crate) fn retained_bytes(&self) -> usize {
        size_of::<Self>()
            .saturating_add(
                self.served
                    .capacity()
                    .saturating_mul(size_of::<ServedBlockFingerprint>()),
            )
            .saturating_add(
                self.served
                    .iter()
                    .map(|served| {
                        served
                            .block_id
                            .capacity()
                            .saturating_add(served.content_hash.capacity())
                    })
                    .sum::<usize>(),
            )
            .saturating_add(
                self.baseline_parts
                    .as_ref()
                    .map_or(0, BaselineParts::retained_bytes),
            )
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ProposedDerivedState {
    pub(crate) revert_epoch: u64,
    pub(crate) read_row_version: Option<u64>,
    pub(crate) served: Vec<ServedBlockFingerprint>,
    pub(crate) baseline_parts: Option<BaselineParts>,
}

impl ProposedDerivedState {
    pub(crate) fn accept(
        self,
        store: &MemoryStore,
        session_id: &str,
        committed_row_version: Option<u64>,
    ) -> Option<DerivedState> {
        let accepted_row_version = match committed_row_version {
            Some(row_version) => row_version,
            None => {
                let current = store.load_fold_authority(session_id).ok()?.row_version;
                if current != self.read_row_version {
                    return None;
                }
                current.unwrap_or(0)
            }
        };
        Some(DerivedState {
            revert_epoch: self.revert_epoch,
            accepted_row_version,
            served: self.served,
            baseline_parts: self.baseline_parts,
        })
    }
}

#[cfg(test)]
mod tests {
    use cache_stability::CoreState;
    use memory_store::ModuleMeta;
    use storage::{Isolation, StorageBackend, StorageDescriptor};

    use super::*;

    fn state(
        revert_epoch: u64,
        accepted_row_version: u64,
        generation: Option<u64>,
    ) -> DerivedState {
        DerivedState {
            revert_epoch,
            accepted_row_version,
            served: Vec::new(),
            baseline_parts: generation.map(|baseline_generation| BaselineParts {
                baseline_generation,
                parts: Vec::new(),
                excluded_prefix_len: 0,
                excluded_prefix_digest: String::new(),
            }),
        }
    }

    #[test]
    fn newer_epochs_and_versions_supersede_and_keys_select_by_epoch_and_generation() {
        let current = state(2, 10, Some(3));
        assert!(state(2, 10, None).supersedes(&current));
        assert!(state(2, 11, None).supersedes(&current));
        assert!(state(3, 1, None).supersedes(&current));
        assert!(!state(2, 9, None).supersedes(&current));
        assert!(!state(1, 99, None).supersedes(&current));

        assert!(current.served_for(2).is_some());
        assert!(current.served_for(1).is_none());
        let baseline = |baseline_generation| TailHygieneBaseline {
            baseline_generation,
            ..Default::default()
        };
        assert!(current.parts_for(2, Some(&baseline(3))).is_some());
        assert!(current.parts_for(2, Some(&baseline(4))).is_none());
        assert!(current.parts_for(1, Some(&baseline(3))).is_none());
        assert!(current.parts_for(2, None).is_none());
        assert!(
            state(2, 10, None)
                .parts_for(2, Some(&baseline(3)))
                .is_none()
        );
    }

    #[test]
    fn a_no_write_proposal_is_accepted_only_while_its_read_version_is_current() {
        let dir = tempfile::tempdir().unwrap();
        let store = MemoryStore::open(&StorageDescriptor {
            module_id: "eidnara-test".to_string(),
            storage_namespace: "memory".to_string(),
            isolation: Isolation::Module,
            backend: StorageBackend::Sqlite {
                path: dir.path().join("store.db").to_string_lossy().to_string(),
            },
        })
        .unwrap();
        let proposal = |read_row_version| ProposedDerivedState {
            revert_epoch: 0,
            read_row_version,
            served: Vec::new(),
            baseline_parts: None,
        };
        let accepted = |proposal: ProposedDerivedState, committed| {
            proposal
                .accept(&store, "ses", committed)
                .map(|state| state.accepted_row_version)
        };
        assert_eq!(accepted(proposal(None), None), Some(0));
        let first = store
            .commit("ses", None, &CoreState::empty(), &ModuleMeta::default())
            .unwrap();
        assert_eq!(accepted(proposal(None), None), None);
        assert_eq!(accepted(proposal(Some(first)), None), Some(first));
        let second = store
            .commit(
                "ses",
                Some(first),
                &CoreState::empty(),
                &ModuleMeta::default(),
            )
            .unwrap();
        assert_eq!(accepted(proposal(Some(first)), None), None);
        assert_eq!(accepted(proposal(Some(first)), Some(second)), Some(second));
    }
}
