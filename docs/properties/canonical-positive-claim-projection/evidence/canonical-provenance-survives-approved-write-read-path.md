# canonical-provenance-survives-approved-write-read-path

## Discovery trigger

Ticket #458: prove write/redact/read/reopen preservation using real stores. Specification open question on canonical-format owner: define a validated write/reopen roundtrip including redaction.

## Evidence trail

- `crates/kernel/src/claim_causality.rs`: `record_causality_inner` builds the detail only from ids that passed `identity`, digests that passed `is_artifact_digest`, and integers, so no redactable text can enter a stored record.
- `crates/kernel/src/claim_causality.rs`: `causal_class_at` decodes the stored detail and reports `producer` from `commit_log`, never from the detail.
- `crates/kernel/tests/kernel_claim_facts.rs`: `facts_survive_reopen` and `crates/kernel/tests/kernel_claim_causality.rs`: `oversized_payloads_read_as_unknown_and_records_survive_reopen` drop the store, reopen the directory, and assert equality.

## Failure scenario

A record whose detail the redactor rewrote would fail to decode after write, or decode to a different subject or evidence, and the class would silently become `Unknown` or name the wrong evidence.

## Timing windows and dependencies

Process restart between write and read.

## What a test must construct

Write a decision, descriptor, and `DirectObservation` record; read facts; reopen; read again; compare whole structs.

## Investigation log

No open questions at authoring time.
