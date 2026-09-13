use super::*;
use kernel::{CommitIntent, CommitReadRequest, CommitReadTarget, PageEnd};
use retrieval::retirement::{RetirementReceipt, record_receipt, verify_receipt};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RetirementEvent {
    BeforeCleanup,
    Removed,
    BeforeReceiptCommit,
    LocalReleased,
    BeforeAcknowledgement,
    Acknowledged,
}

impl SearchSelection {
    pub(super) fn predecessor(
        &self,
        intent: &LifecycleIntent,
    ) -> Result<Option<Bootstrap>, BuildError> {
        let digest = &intent.selected_generation;
        if !host_runtime::lifecycle::is_canonical_payload_digest(digest) {
            return Ok(None);
        }
        let old: Bootstrap =
            serde_json::from_slice(&certificate_bytes(&self.family_home(digest)?)?)
                .map_err(|_| BuildError::Invalid("old bootstrap corrupt"))?;
        if !matches!(old.schema, 1 | 2)
            || old.seed.stage_manifest().digest() != *digest
            || old.intent.consumer.generation_id != old.seed.generation_id
            || old.intent.consumer.consumer_id == intent.consumer.consumer_id
            || old.seed.kernel_incarnation_id != intent.kernel_incarnation_id
        {
            return Err(BuildError::Invalid("old consumer binding mismatch"));
        }
        Ok(Some(old))
    }

    /// The fixed selection certificate bounds retirement; retries cannot acknowledge a later tip.
    pub fn retire(
        &self,
        kernel: &KernelStore,
        gate: &HookGate,
        spec: &super::super::ReplacementSpec,
        budget: &EvalBudget,
        observer: &mut dyn FnMut(RetirementEvent),
    ) -> Result<(), BuildError> {
        let transaction = LifecycleTransactionLock::acquire_exclusive(Some(&self.data_home))?;
        self.reopen_locked(kernel, gate, budget, &transaction)?;
        let family = self
            .selected
            .load_full()
            .ok_or(BuildError::Invalid("search unavailable"))?;
        let certificate = &family.certificate;
        let Some(old) = &certificate.retiring else {
            return Err(BuildError::Invalid("no durable old consumer binding"));
        };
        let grants = spec.admit(gate, &certificate.intent)?;
        let remaining = certificate
            .intent
            .episodes
            .deadline
            .checked_sub(super::super::wall_ms()?)
            .and_then(|n| u64::try_from(n).ok())
            .ok_or(BuildError::Expired)?;
        if deadline(budget)?
            > std::time::Instant::now() + std::time::Duration::from_millis(remaining)
        {
            return Err(BuildError::Invalid(
                "retirement budget exceeds original deadline",
            ));
        }
        for grant in &grants {
            gate.check_limits(
                grant,
                &InvalidationIdentity::from(&self.identity),
                &[
                    ("physical_drain_ms", remaining),
                    (
                        "local_transaction_rows",
                        spec.episode.batch.persist.max_records.get() as u64 + 1,
                    ),
                    (
                        "local_transaction_bytes",
                        spec.episode
                            .max_source_encoded_bytes
                            .get()
                            .checked_add(MAX_RECORD_BYTES)
                            .ok_or(BuildError::InventoryBound)?,
                    ),
                ],
            )?;
        }
        let check = || -> Result<(), BuildError> {
            deadline(budget)?;
            if super::super::wall_ms()? >= certificate.intent.episodes.deadline
                || grants.iter().any(|grant| grant.invalidated.is_cancelled())
            {
                return Err(BuildError::Expired);
            }
            family.check_kernel(kernel, budget)
        };
        check()?;
        spec.identity.require_compatible(&self.identity)?;
        let target = CommitReadTarget {
            through_commit: certificate.seed.checkpoint_commit_seq,
            incarnation: family.incarnation,
        };
        let old_digest = old.seed.stage_manifest().digest();
        if old_digest != certificate.intent.selected_generation
            || old.consumer.generation_id != old.seed.generation_id
            || old.consumer.consumer_id == certificate.intent.consumer.consumer_id
            || old.seed.kernel_incarnation_id != certificate.seed.kernel_incarnation_id
        {
            return Err(BuildError::Invalid("retirement binding mismatch"));
        }
        let obligations = kernel.consumer_obligations_within_budget(
            budget,
            &old.consumer.consumer_id,
            target,
            spec.episode.batch.persist.max_records,
            spec.episode.max_source_encoded_bytes,
        )?;
        let receipt = RetirementReceipt {
            old_consumer: &old.consumer.consumer_id,
            old_generation: &old.consumer.generation_id,
            old_family: &old_digest,
            selected_family: &family._seed_pin.digest,
            kernel_incarnation: &certificate.seed.kernel_incarnation_id,
            through: target.through_commit,
        };
        let recovered = family.projection.read_within(deadline(budget)?, |conn| {
            verify_receipt(conn, &receipt, &obligations)
        })?;
        let checkpoint =
            kernel.outbox_consumer_checkpoint_within_budget(budget, receipt.old_consumer)?;
        if recovered {
            self.remove_retiring_family(old, &transaction)?;
        } else {
            let mut after =
                checkpoint.ok_or(BuildError::Invalid("old consumer missing without receipt"))?;
            if after > target.through_commit {
                return Err(BuildError::Invalid(
                    "old checkpoint exceeds retirement target",
                ));
            }
            for _ in 0..spec.episode.max_source_pages.get() {
                check()?;
                let page = kernel
                    .read_complete_commits_within_budget(
                        budget,
                        &CommitReadRequest {
                            consumer_id: receipt.old_consumer.to_owned(),
                            incarnation: target.incarnation,
                            after_commit: after,
                            through_commit: target.through_commit,
                        },
                        spec.episode.commits,
                    )
                    .map_err(|error| BuildError::Blocked(super::super::Blocked::Read(error)))?;
                after = page.through;
                match page.end {
                    PageEnd::Exhausted => {
                        after = target.through_commit;
                        break;
                    }
                    PageEnd::Oversized { .. } => return Err(BuildError::InventoryBound),
                    PageEnd::Deferred { .. } => {}
                }
            }
            if after != target.through_commit {
                return Err(BuildError::InventoryBound);
            }
            observer(RetirementEvent::BeforeCleanup);
            check()?;
            self.remove_retiring_family(old, &transaction)?;
            observer(RetirementEvent::Removed);
            check()?;
            family.projection.write_within(deadline(budget)?, |conn| {
                record_receipt(
                    conn,
                    &receipt,
                    &obligations,
                    super::super::wall_ms().map_err(|_| ProjectionError::MutationConflict)?,
                )?;
                observer(RetirementEvent::BeforeReceiptCommit);
                check().map_err(|_| ProjectionError::MutationConflict)
            })?;
        }
        observer(RetirementEvent::LocalReleased);
        check()?;
        if checkpoint.is_none() {
            return Ok(());
        }
        observer(RetirementEvent::BeforeAcknowledgement);
        check()?;
        kernel.acknowledge_outbox_within_budget(
            budget,
            receipt.old_consumer,
            receipt.through,
            super::super::wall_ms()?,
        )?;
        observer(RetirementEvent::Acknowledged);
        check()?;
        kernel.commit_within_budget(
            budget,
            CommitIntent {
                producer: "search-retirement".to_owned(),
                operation_key: old_digest.clone(),
                request_digest: family._seed_pin.digest.clone(),
                actor: "daemon".to_owned(),
                cause: "certified consumer retirement".to_owned(),
            },
            |envelope| {
                envelope.deregister_outbox_consumer(
                    receipt.old_consumer,
                    super::super::wall_ms().map_err(|_| kernel::KernelError::InvalidInput)?,
                )?;
                Ok(String::new())
            },
        )?;
        Ok(())
    }

    fn remove_retiring_family(
        &self,
        old: &RetiringFamily,
        transaction: &LifecycleTransactionLock,
    ) -> Result<(), BuildError> {
        let digest = old.seed.stage_manifest().digest();
        let home = self.family_home(&digest)?;
        let store = GenerationStore::open(Some(&self.data_home))?;
        match store.read_search_current()? {
            CurrentProfile::Quarantined => {
                return Err(BuildError::Invalid("unknown selector references"));
            }
            CurrentProfile::Current(current) if current == digest => {
                return Err(BuildError::Invalid("old family still selected"));
            }
            _ => {}
        }
        if home.try_exists()? {
            open_directory(&home)?;
            if home.join(CERTIFICATE).try_exists()? {
                let found: Bootstrap = serde_json::from_slice(&certificate_bytes(&home)?)
                    .map_err(|_| BuildError::Invalid("old bootstrap corrupt"))?;
                if !matches!(found.schema, 1 | 2)
                    || found.seed != old.seed
                    || found.intent.consumer != old.consumer
                {
                    return Err(BuildError::Invalid("old cleanup binding mismatch"));
                }
            }
            if home.join("search").try_exists()? {
                open_directory(&home.join("search"))?;
            }
        }
        let descriptor = search_descriptor(&home)
            .map_err(crate::search_projection::SearchProjectionError::from)?;
        storage::delete_sqlite_family(&descriptor)
            .map_err(crate::search_projection::SearchProjectionError::from)?;
        storage::verify_sqlite_family_removed(&descriptor)
            .map_err(crate::search_projection::SearchProjectionError::from)?;
        store.discard_unselected(
            &old.seed.stage_manifest(),
            transaction,
            &ProjectionLifecycle::protected_generations(&self.data_home, transaction)?,
        )?;
        if home.try_exists()? {
            if home.join(CERTIFICATE).try_exists()? {
                fs::remove_file(home.join(CERTIFICATE))?;
            }
            for entry in fs::read_dir(&home)? {
                if entry?.file_name() != "search" {
                    return Err(BuildError::Invalid("unreconciled family residue"));
                }
            }
            open_directory(&home)?.sync_all()?;
        }
        open_directory(&self.data_home.join(FAMILIES))?.sync_all()?;
        Ok(())
    }
}
