# Fault map

## Fault classes

| Class | Available today | How |
| --- | --- | --- |
| Process restart | yes | `Fixture::reopen` in `crates/kernel/tests/kernel_claim_facts.rs` |
| Out-of-band row corruption | yes | direct SQLite writes to `observations` and `admission_decisions`; direct inserts into `object_registry` and `decisions` (the registry forbids updates) |
| Evidence retirement | yes | `Envelope::retire_evidence` |
| Object retirement and correction | yes | `retire_observation`, `retire_decision`, `correct_decision` |
| Duplicate commit | yes | repeated `CommitIntent` |
| Power loss | no | out of scope for this crate; see `crates/kernel/tests/cas_fault_injection.rs` for the SIGKILL harness |

## Required faults per property

| Property | Required faults and states | Constructed |
| --- | --- | --- |
| `canonical-claim-fields-match-fenced-source` | later commit on the same lineage; snapshot before creation | yes |
| `canonical-provenance-survives-approved-write-read-path` | reopen | yes |
| `claim-capacity-failure-is-atomic` | over-bound request; over-bound parent list | yes |
| `claim-enablement-requires-approved-evidence` | a production caller | no caller exists |
| `claim-export-predecode-bounds` | bound below a real record | yes |
| `claim-format-rollback-preserves-canonical-state` | higher detail version | yes |
| `claim-replay-preserves-newest-canonical-state` | duplicate commit; replacement record | yes |
| `echo-classification-requires-canonical-causality` | forgery; evidence retirement; parent retirement; record retirement | yes |
| `malformed-required-field-stops-projection-progress` | corrupted enum column; unreadable registry class | yes |
| `occurrence-identity-is-not-payload-or-source-triple` | one of three representations published; detail rewritten to name other rows | yes |
| `projection-has-no-second-truth-or-policy-authority` | none | yes |
| `retrieved-content-cannot-upgrade-write-authority` | forged kind, id, and object prefixes | yes |
| `revision-domains-remain-distinct-and-supported` | higher detail version | yes |
| `served-sensitivity-and-artifact-policy-govern-egress` | none for the served half | yes |
| `supporting-authority-is-preserved-not-recomputed` | revoked supporting approval | yes |

## Coverage checks to add

- `claim-enablement-requires-approved-evidence`: a caller inventory once a
  daemon path calls the reader or writer.

## Leverage ranking

1. Out-of-band row edits: cheapest valid oracle for version, malformed, and
   conflict cases; already used.
2. Public-API refusals through `commit`: exercise the poison path with no
   internal seam.
3. Reopen: one line per test, covers durability of the roundtrip.
