# repo-policy-unit-reconciliation-by-digest

## Discovery trigger

Plan requirement R8: "unchanged units are superseded by identical successors
at the next revision, changed units by their new text, removed units are
retired, and new units are created, all citing a fresh `config_present`
observation at that revision; an unchanged blob writes nothing". KTD3
repository rules; U5 step 4; acceptance example AE5. Forbidden outcome F6
(stale rule from an older HEAD live after reconciliation).

## Evidence trail

Verified at HEAD:

- `crates/kernel/src/admission.rs:1312-1349` `supersede_decision` with
  `refuse_barred_lineage`; `crates/kernel/src/slice/write.rs:354-452`
  `correct_decision_inner` (invalidate predecessor, insert or fold successor,
  `set_successor`, cascade); `write.rs:1271-1284` `validate_successor`
  (same domain, source_kind, source_id; revision strictly greater).
- `write.rs:543-546` `retire_decision` is `pub` and reachable from the
  daemon; `retire_decision_with_cascade` is `pub(crate)` (`:548-553`), as the
  plan's assumption states.
- `write.rs:139-162` `insert_decision`.
- `admission.rs:2176-2200` `load_prior_decision`: an object subject's prior
  is its own latest admission; a new successor object has no prior, so each
  rev N+1 decision is admitted fresh with its own `ConfigObserved` trigger.
- `crates/daemon/src/kernel_routes/commit.rs:787-848` is the daemon's
  existing supersede pattern including the `RevisionNotAdvanced` refusal and
  the sensitivity floor; the reconcile would reuse `envelope.supersede_decision`
  directly.
- Tests: `crates/kernel/tests/kernel_admission.rs:788`
  `correction_preserves_predecessor_history_and_emits_both_changes`.

Plan-only: the digest matching, the "unchanged blob writes nothing" rule, the
per-file envelope.

Plan-internal tension: the per-file operation key is `<path>:<oid>`, which is
new at every new HEAD even when the blob is unchanged; "writes nothing" must
therefore be a blob-digest comparison against the live observation's artifact
before any commit, not a receipt replay.

## Failure scenario

Legal witness (AE5): rev N at oid X has units u1, u2, u3. Oid Y rewrites u2
to u2' and deletes u3. One envelope: observation rev N+1; `supersede_decision
(rule(u1,N) -> rule(u1,N+1))` identical text; `supersede_decision(rule(u2,N)
-> rule(u2',N+1))`; `retire_decision(rule(u3,N))`; admit both successors
`ConfigObserved` with the rev N+1 observation. After commit: live decisions on
the lineage are exactly `{rule(u1,N+1), rule(u2',N+1)}`; the rev N rows are
invalidated; `superseded_by` links u1 and u2.

Smallest violating traces:

1. Matching by position instead of digest: deleting u1 shifts u2 into u1's
   slot; u2 is "changed to u1's successor" and u3 is "removed"; history
   records wrong successions and `superseded_by` lies.
2. The reconcile inserts rev N+1 rows and forgets to retire u3: a deleted rule
   stays Verified; F6.
3. The reconcile computes the live set before acquiring the writer; a
   concurrent pass already superseded rev N to N+1; this pass's
   `supersede_decision(rule(u1,N) ...)` hits an invalidated object
   (`load_live_typed_object` -> `NotFound`); the envelope poisons; the file
   does not advance in this pass. Correct but the plan must be computed inside
   the closure to avoid a permanently failing retry when HEAD is stable.
4. The reconcile treats a new oid with identical bytes as a change: every
   unit is superseded by an identical twin; history grows per commit and
   `created_commit_seq`-based ordering (N4) churns.

## Timing windows and dependencies

Plan computation versus writer acquisition (trace 3). The envelope's
same-transaction prior cache (`admission.rs:2184-2200`) lets later admissions
in the file see earlier inserts.

## What a test must construct

- A repository fixture with two commits (AE5); run reconcile at X then at Y.
- Assert after Y: live `repository_policy` decisions on the lineage all have
  `source_revision = N+1`; `rule(u1,N).superseded_by = rule(u1,N+1)` with
  equal unit digest; `rule(u2,N).superseded_by = rule(u2',N+1)`;
  `rule(u3,N).invalidated_commit_seq` set and no successor; both successors'
  latest admission `config_observed`, Verified, `trigger_object_id` = the rev
  N+1 observation.
- A third commit touching only another file: assert the lineage's max
  revision is unchanged and no new observation on it.
- A commit that only renames the heading above u1: assert whichever behavior
  the resolved OQ-N2-5 fixes.

## Investigation log

### Q: How is "unchanged blob writes nothing" keyed?
- Sources examined: plan U5 steps 1, 4, 6.
- Findings: the cache is per binding (HEAD oid), the key is per path and oid,
  neither captures "same blob at new oid"; a blob digest comparison is needed.
- Missing evidence: design statement.
- Conclusion: unresolved, resolvable at implementation.

### Q: Does retiring a rev N unit that a user directive cites cascade?
- Sources examined: `write.rs:627-665`.
- Findings: only if the unit granted authority; a `trusted_local_code` row is
  not an approval root (`approval_row_fields` requires `explicit_user`), so
  no cascade.
- Missing evidence: none.
- Conclusion: resolved with answer: no cascade from repository rules.
