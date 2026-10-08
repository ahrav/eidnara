# cf-memory-credit-requires-admitted-visible-content

## Discovery trigger

R3 permits independently admitted memory that is actually included to retain
meaning lost from history. State, security, resource, and replay lenses expose
the needed qualification: a private candidate or excluded row is not visible
evidence. Inspected 2026-09-19 at
`99f68bd37516ca5351f8aadb8b0b51cb0f13dcc8` in `ahrav/eidnara`.

## Evidence trail

- [lib.rs:5372-5388,9257-9271](../../../../../crates/daemon/src/lib.rs#L5372-L5388)
  reads canonical memory only when enabled and pins one read for the pass.
  The later [pass assembly](../../../../../crates/daemon/src/lib.rs#L9257-L9271)
  shares it with composition and producer preparation.
- [canonical_memory.rs:147-174](../../../../../crates/daemon/src/canonical_memory.rs#L147-L174)
  captures the kernel tip, evaluates freshness, and reads the bound project's
  `auto_inject` memory-domain decisions. Failure returns `Withheld`.
- [kernel_routes/read.rs:162-186](../../../../../crates/daemon/src/kernel_routes/read.rs#L162-L186)
  filters by the bound project and scope before applying row limits.
- [canonical_memory.rs:195-211](../../../../../crates/daemon/src/canonical_memory.rs#L195-L211)
  keeps visible positive decisions, then trims to the memory budget.
- [admission.rs:3667-3677](../../../../../crates/kernel/src/admission.rs#L3667-L3677)
  distinguishes verified active automatic visibility from candidate/labeled,
  rejected review-only, and quarantined audit-only states.
- [m0_compose.rs:71-109,234-237](../../../../../crates/daemon/src/m0_compose.rs#L71-L109)
  uses skip-and-continue with wrapper charges; the
  [append](../../../../../crates/daemon/src/m0_compose.rs#L234-L237) renders the
  already selected memory set.
- [memory_render.rs:79-88](../../../../../crates/daemon/src/memory_render.rs#L79-L88)
  caps each content string at 64 KiB before XML escaping. Object presence alone
  does not prove that a trailing qualifier reached the provider.
- [history_summarizer_validate.rs:689-707](../../../../../crates/daemon/src/history_summarizer_validate.rs#L689-L707)
  can reject extracted facts without rejecting valid history. Successful history
  publication is therefore neither fact acceptance nor canonical admission.
- [crates/daemon/tests/transform_canonical_memory.rs:48-217](../../../../../crates/daemon/tests/transform_canonical_memory.rs#L48-L217)
  checks candidate, scope, category, lifecycle exclusion, and warm reuse.
  [canonical_memory.rs:412-474](../../../../../crates/daemon/src/canonical_memory.rs#L412-L474)
  checks budget exclusion and withheld-versus-empty metadata. Status: unaudited.

Reachability is **default-production**: memory defaults enabled at
[config.rs:145](../../../../../crates/daemon/src/config.rs#L145). The pinned
reader is called by normal transform execution, not only a test helper.

## Failure scenario

A replay omits the history row containing a task constraint. Its fixture also
stores a matching candidate, and an internal helper finds that candidate. The
report awards memory preservation although the automatic surface withholds it.
The same error occurs for wrong-project, rejected, stale/withheld, or
budget-excluded content. Conversely, a selected memory line can retain a name
but truncate away the limitation that makes it safe.

Do not confuse a `REJECTED_APPROACH` category with an explicit admission
rejection. A positive-category candidate explicitly rejected by admission is a
different negative control from a category the renderer never emits.

## Timing windows and dependencies

Use the pass's pinned snapshot, not a later read from the kernel. A mutation
after that snapshot affects the next pass. Warm m0 must be interpreted through
its actual rendered memory and revision, not assumed to contain the newest row.
Read availability is distinct from an available empty set. Budget exclusion is
distinct from admission refusal; neither receives alternate-visibility credit.

## What a test must construct

1. Keep one source obligation and follow-up fixed with the decisive raw history
   removed. Admit a matching positive memory independently of the replay result.
2. Pair it with candidate-only, `ExplicitReject`, wrong-project, withheld, and
   budget-excluded variants. Record each state before the transform.
3. Capture the actual request and identify both the object and its visible
   qualified text. Do not infer a preserved qualifier from row identity.
4. Check memory-disabled and warm-revision behavior where relevant, without
   changing admission or automatically promoting a session fact.
5. Review whether the included wording preserves the original task scope.

Situation checks assert the positive and negative input states, not rendering
of a forbidden row. All exercise remains **not yet**.

## Investigation log

### Q: Can “stored,” “read successfully,” and “included” be used interchangeably?
- Sources examined: pinned reader, project filter, memory trimmer, renderer.
- Findings: each boundary can exclude a row; `Withheld` exposes no rows and
  remains distinguishable from available emptiness. Content also has a cap.
- Missing evidence: provider-level omission/memory pairs for this corpus.
- Conclusion: resolved with answer: preservation needs an included qualified span.

### Q: Which memory fixture is independently admissible without scope inflation?
- Sources examined: supplied plan R3/U3 and canonical route tests above.
- Findings: tests can construct verified scoped rows, but this does not approve
  turning a task-only prohibition into a permanent project rule.
- Missing evidence: human approval of the memory-backed case and its scope.
- Conclusion: needs human input. Preserve admission rather than force a pass.
