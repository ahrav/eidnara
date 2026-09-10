# Portfolio evaluation: export and recovery dispositions

Repository: `/local/home/ahrav/scratch/eidnara`.
Verified HEAD: `913234433ae36a80a6e22c6aac14c7f9aab74386`.
Date: 2026-09-10. External scope is supplied by the user: settled plan,
linked parent/index/research, and local repo. No incident logs are supplied.
[Source registry](catalog.md#sources) records the external files and their roles.

## Independent review provenance and status

**Independent central portfolio review is complete.** The user reports that
analyst session `ses_f7623dcccffe3Y09nW2wVoABif` ran all four lenses: harness
fit, coverage balance, implementability, and wildcard. The findings and the
domain-name qualification in the user's correction brief are the review input.

This author verifies cited code and edits the catalog as **disposition**, not
as an independent reviewer or an independent re-review of the corrected set.
The original nested assessment attempt failed at the harness depth limit;
that history remains in `_lenses/` but is no longer the evaluation status.
No tests, builds, benchmarks, runtime experiments, code edits, or tracker
operations are part of this disposition. Existing checks remain unaudited.

## Four-lens synthesis

| Lens run by the central analyst | Relevant supplied findings | Disposition |
| --- | --- | --- |
| Harness fit | G3 omitted existing process-crash machinery; R1 duplicated ack/lock ownership. | Reuse the CAS child/barrier/kill/reap pattern with concrete search hooks; this part is the single ack/lock and ack-marker owner. |
| Coverage balance | G2 lacked ordinary catch-up progress; G5 lacked authorized recovery from Disabled. | Add one two-mode bounded liveness record. Keep deletion-after-pruning as its own required situation and source sweeps with projection inventory. |
| Implementability | R2 left decoded-memory observation unspecified; R7 locator range and index/marker consistency needed correction. | Name the missing permitted live-heap observer and kernel lint boundary, correct the outbox range, normalize marker case and six-column index. |
| Wildcard | G1 exposed in-place remediation; B4 stale catalog reuse and the at-rest policy gap need explicit treatment. | Narrow remediation to verified domain-name behavior and conditional field dependency; preserve durable reuse corrections; leave at-rest policy with an owner question. |

The table groups the supplied findings for disposition. It does not claim a
second four-lens run by this author or implementation evidence for the claims.

## Finding dispositions

| Finding | Classification | Verified evidence and decision | Artifact status |
| --- | --- | --- | --- |
| G2 | gap | Complete-prefix safety allowed permanent CatchingUp. P, lines 106-110, names Current; no production driver exists. Add ordinary healthy finite-backlog progress in [rp21-catchup-and-authorized-recovery-converge](catalog.md#rp21-catchup-and-authorized-recovery-converge). | Applied; RP2.9 interval/envelope and implementation remain open. |
| G5 | gap | P, line 116, requires operator recovery after disable. The new record also covers Disabled only after explicit authorization and all prerequisite/gate acceptances. No automatic enablement is authorized. | Applied in the same new record; authority persistence remains an owner question. |
| G3 | gap | `crates/kernel/tests/cas_fault_injection.rs:1-6` excludes power-loss proof; `:1046-1094` implements child barrier, bounded wait, kill, and reap. | Inventoried unaudited as a reusable pattern; concrete search hooks/oracles remain missing. Cost text no longer proposes a broad new harness. |
| R1 | refinement | `crates/kernel/src/outbox.rs:530-570` owns kernel ack, which cannot inspect search durability. [rp21-ack-follows-local-release](catalog.md#rp21-ack-follows-local-release) exclusively owns ack/lock correctness. | Canonical ack marker definitions remain only in this part's fault map. Projection references them; its local atomicity guarantee remains separate. |
| R2 | refinement | `crates/host-runtime/examples/perf_host.rs:5-33` uses unsafe `GlobalAlloc` and cumulative request counters; it does not measure live decoded heap. Kernel has `forbid(unsafe_code)` at `crates/kernel/src/lib.rs:5`. | Missing observer/lint boundary is explicit in catalog, evidence, inventory, and fault map. Logical charge is a potential implementation approach, not physical-heap proof. No allocator/dependency/lint change is prescribed. |
| G1 | gap, narrowed | `crates/kernel/src/envelope.rs:240-242` supports domain-name remediation only. `:395-438` rewrites `domains.name` with unchanged source revision and emits `operator_remediation`. The source mapping does not establish a projected dependency. | Add the control event and mutable-S validity/abort case. Reject the broader claim of an established RP2.1 byte dependency; no revision change or occurrence generation is invented. |
| R7 | refinement | `pending_outbox_reads_unpublished_rows_in_order_with_commit_boundaries` spans `crates/kernel/tests/kernel_outbox.rs:622-698`. | Standardized all references to 622-698. |
| B4 | bias/refinement | Host staging has a supplied-payload CLI caller at `crates/daemon/src/bin/eidnara-host.rs:964-993`; the mirror source cited by invalidated facade prose is absent. | Added a durable [reuse-correction section](existing-checks.md#durable-reuse-corrections); older catalogs are not edited or treated as current reachability proof. |
| At-rest sensitivity gap | owner question | Egress gates and remediation do not establish storage policy for staged or retained derived bytes. | Recorded in [the authority record](catalog.md#rp21-recovery-preserves-canonical-authority) and its investigation log. No policy is introduced. |

## Shared ownership and applicability

- The projection/source owner retains local row/checkpoint/pending-job
  atomicity, source mapping, message cleanup, git sweeps, and source coverage:
  [projection-source-inventory-complete](../projection-coverage/catalog.md#projection-source-inventory-complete).
- This part exclusively defines ack/lock checks and
  `rp21_ack_local_commit_interrupted` / `rp21_ack_response_lost` in
  [the marker table](fault-map.md#independent-situation-markers). The first
  observes local COMMIT then terminates before ack attempt; the second observes
  durable ack COMMIT then loses the response. Projection references those
  definitions rather than introducing alternative marker names or definitions.
- [projection-remediation-invalidates-derived-bytes](../projection-coverage/catalog.md#projection-remediation-invalidates-derived-bytes)
  applies only if approved mapping consumes the affected domain-name field.
  Otherwise the event is still accounted in canonical history without a
  fabricated projected dependency. If required S bytes cannot be supplied,
  export aborts; it does not restore canonically removed plaintext.
- The projection part's first-class
  [projection-acceptance-situations-witnessed](../projection-coverage/catalog.md#projection-acceptance-situations-witnessed)
  requires every declared fault-map marker. This part preserves one definition
  site and lowercase names. Normal healthy backlog admission and an authorized
  recovery outcome are distinct witnesses; neither replaces a deadline check.

## Remaining owner and implementation questions

1. **RP2.9:** Approve finite per-mode target/backlog envelopes, recovery intervals,
   work/attempt caps, dependency service assumptions, and ack endpoints. Both
   liveness records remain RP2.9-blocked. Test-control timeouts are not limits.
2. **Kernel/source mapping:** Decide which mutable fields export consumes and
   how required S remains valid or is explicitly rejected after remediation,
   source purge, or loss of retained history.
3. **Observation owner:** Identify a permitted decoder-entry/live-heap observer
   and its attribution scope. Logical charge, allocation-request totals, and
   physical live-heap high water are different claims.
4. **Daemon lifecycle:** Define durable recovery authorization/gate acceptance,
   pending-disable reconciliation, and old-consumer/barrier handoff.
5. **Publication owner:** Define the complete SQLite publication unit and
   ambiguous-selector-error reconciliation, then reuse the existing crash
   pattern with those concrete hooks.
6. **Sensitivity policy owner:** Establish the at-rest policy for exported,
   staged, selected, and retained derived bytes. This is a question, not a
   policy selected by the catalog author.

## Corrected portfolio accounting

The catalog has ten active records: eight safety and two bounded liveness,
with ten evidence files and matching index rows. The index uses exactly
`Slug | Type | Reachability | Semantics | Status | Confidence`. Every record
is proposed-only `test-only` with a per-record absence rationale. Every record
remains unexercised. Liveness uses `always` per admitted episode and is visibly
RP2.9-blocked, not an unbounded eventual assertion.

The fault map has 23 unique lowercase marker definitions. Witnesses describe
constructible inputs, injected boundaries, or authorized Current outcomes,
never the negation of a safety invariant. Structural/link/source-reference
checks verify the artifacts; they do not verify production behavior or claim
the independent analyst re-reviewed these dispositions.
