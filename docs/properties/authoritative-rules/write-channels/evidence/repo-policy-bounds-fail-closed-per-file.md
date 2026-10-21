# repo-policy-bounds-fail-closed-per-file

## Discovery trigger

Plan U5 step 2: "bounds: file <= 64 KiB, <= 32 files, <= 64 units per file,
unit <= 512 bytes after whitespace normalization; non-UTF-8 or over-bound
files yield no units and no observation". Test scenario "over-bound file
yields zero units and no observation". Serves R2 and KTD2; the brief names
"bounds and their fail-closed effect per file".

## Evidence trail

Verified at HEAD:

- `crates/kernel/src/cas/ingest.rs:73-75`: artifact payloads above
  `MAX_PAYLOAD_BYTES` are refused; `:141-180`: a non-UTF-8 payload is not
  inspected and is classified at least Sensitive (`:197-199`), a payload with
  a detected secret becomes Secret (`:195-196`).
- `crates/kernel/src/admission.rs:1604-1605, 1631`: trigger sensitivity folds
  into the candidate sensitivity of the admitted decision, so a Sensitive or
  Secret artifact behind the observation hides every rule it witnesses.
- `crates/kernel/src/slice/write.rs:150-152, 190-192`:
  `fold_cited_evidence_class` raises a decision's or observation's class to
  its cited evidence's class.
- `crates/daemon/src/kernel_routes/commit.rs:947-968`: the commit route
  bounds operations, tokens, and dependencies per request; a reconcile that
  calls the store directly inherits none of these and must bound itself.

Plan-only: every numeric bound and the "no observation" effect.

## Failure scenario

Legal witness: tree has `AGENTS.md` (3 KiB, 12 units) and
`crates/big/AGENTS.md` (70 KiB). The pass writes observation and 12 decisions
for the root lineage and nothing for `crates/big/AGENTS.md`.

Smallest violating traces:

1. A 65 KiB file is read, segmented, and ingested: the Kernel's own payload
   cap is larger (the plan's 64 KiB is a policy bound), so the Kernel accepts
   it; the rule count and pass latency are unbounded by the daemon's policy.
2. A non-UTF-8 file is ingested: the artifact is Sensitive; the observation
   citing it is Sensitive; every rule admitted under it is Hidden. Fail-closed
   on the read side, but the lineage now carries a Sensitive observation at
   rev N+1 that supersedes nothing and a later clean revision's units are
   admitted with the clean trigger; harmless, but the record requires no
   write.
3. 33 files: the pass picks 32 by hash-map iteration order; two passes on two
   hosts pick different sets; parity (R13) breaks.
4. A 600-byte paragraph is admitted as a unit: injection cost per rule is
   unbounded by the plan's reserve arithmetic.

## Timing windows and dependencies

None. Bounds are judged on the bytes read at the resolved oid.

## What a test must construct

- Fixtures: exactly 65536 bytes (admitted), 65537 bytes (refused), a file
  with an invalid UTF-8 sequence, a file with 65 top-level list items, a tree
  with 33 `AGENTS.md` files, a unit of 513 bytes after normalization beside a
  512-byte one.
- Assertions per refused file: no artifact whose provenance names the oid and
  the file's path, no `config_present` observation on the file's lineage with
  evidence at that oid, no decision at the lineage's next revision.
- Assertion for the in-bound sibling in the same pass: observation and
  decisions present at the new oid.
- Assertion for the 33-file tree: the chosen 32 are the first 32 in a stated
  deterministic order and two runs choose the same set.

## Investigation log

### Q: What happens to rev N rules of a file that becomes over-bound at rev N+1?
- Sources examined: plan U5 steps 2 and 4.
- Findings: step 2 says no units and no observation; step 4's reconciliation
  runs per file "by content digest"; with zero units every rev N unit is
  "removed" and would be retired, but step 2's "no observation" suggests the
  file is skipped entirely, leaving rev N live. The two readings differ in
  whether an older HEAD's rules stay served (F6).
- Missing evidence: a design statement.
- Conclusion: needs human input.

### Q: Is whitespace normalization defined?
- Sources examined: plan U5 step 2 ("after whitespace normalization").
- Findings: not defined (collapse runs, trim, CRLF).
- Missing evidence: the segmentation spec.
- Conclusion: unresolved, resolvable at implementation; must be fixed before
  golden fixtures.
