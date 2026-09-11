# bounded-secret-scan-finds-every-whole-input-finding

Baseline: `913234433ae36a80a6e22c6aac14c7f9aab74386`, 2026-09-10.
The [scope and provenance](../catalog.md#scope-and-provenance) apply here.

## Discovery trigger

The rule-set header describes `radius` as the byte radius around an anchor
match "fed to the regex". The evaluator does not do that: it runs each
preselected rule's regex over the whole input and uses `radius` only for the
context window around a full match. A latency change that moves the code
toward the documented design changes which findings exist. The wildcard pass
recorded the property that judges such a change, and the versioning duty that
follows from it.

## Evidence trail

- [`evaluate`][eval] charges the input, calls [`preselect`][preselect] over
  the whole input, then for each selected rule either takes the
  [`KeyedForm`][keyed-form] fast path (ASCII input and one of seven
  `magic-keyed-*` rules, anchored on `=` or a key quote with
  `memchr` probes at [`:75-111`][keyed-path]) or runs
  [`rule.regex.captures_iter(bytes)` over the whole input][captures]. Findings
  are sorted at [`:130-147`][sort].
- [`preselect`][preselect] uses an [ASCII-case-insensitive Aho-Corasick
  DFA][anchor-ci] over declared anchors and returns the rules whose anchor
  occurs anywhere in the input. Its doc says `anchor_proof` checks that
  skipping a rule cannot drop one of its findings.
- `radius` bounds only the context window: [`:266-297`][radius-window]
  computes `window_start = full_span.start() - radius` and `window_end =
  full_span.end() + radius`, enlarged to `two_phase.full_radius` when a
  two-phase rule confirms, and checks `must_contain` and `keywords_any`
  inside that window. Rule validation checks
  `rule.radius <= MAX_RULE_RADIUS` and the two-phase radii at
  [`:598-602`][radius-valid]; nothing requires an anchor inside every match.
- [`MAX_MATCH_BYTES`][max-match] is 32 KiB and [`MAX_RULE_RADIUS`][max-radius]
  is 16 KiB. [`ScanLimits::DEFAULT`][limits] sets `max_candidates` to 262_144
  and `max_work_bytes` to 512 MiB; the candidate counter increments in rule
  order, so which findings a `Candidates` limit drops depends on evaluation
  order ([`:112-128`][captures]).
- [`airtable-personnal-access-token`][airtable] has anchor `airtable`, a
  regex beginning `\bpat`, and `keywords_any: [airtable]` with radius 256, so
  its anchor reaches the match only through the keyword window.
- [`redaction.rs`][edge-margin] states that at an artificial slice edge `\b`
  and `$` may match and a radius is clipped, defines
  `EDGE_MARGIN_BYTES = MAX_RULE_RADIUS + MAX_LOCAL_CONTEXT_BYTES`, and sizes
  [`WINDOW_OVERLAP_BYTES`][overlap] from the same constants.
- [`REVISION`][revision] carries `semantic_digest_version: 8`; the digest doc
  at [`rules.rs:382`][digest-doc] says evaluator semantics are bound by that
  version, and [`evaluator_constants_are_pinned`][t-pinned] trips when a
  hashed table changes. The memory store persists `semantic_digest` and
  `detector_revision` per scan batch at [`:2357-2392`][ms-digest], and every
  committing pass prepares `meta` and `core_state` through
  [`content`][ms-content] (C4).
- Contract versus code: [`default_rules.yaml:12`][rules-radius-doc] describes
  `radius` as fed to the regex; [`:112`][captures] feeds the whole input.

## Failure scenario

A bounded scan evaluates a rule's regex only within some region of an anchor
hit. A finding whose nearest anchor lies farther away than the region bound
(only the keyword window binds them for rules like the Airtable one) is
dropped; a match ending within `EDGE_MARGIN_BYTES` of a region edge changes
under `\b` and `$`; a region-ordered candidate count drops a different
finding at the `Candidates` limit. The secret persists in `meta`, and without
a version bump the old and new audit rows share one `semantic_digest`.

## Timing windows and dependencies

None in time. The difference is data-shape only, and it composes with the
redaction windows: an input over `MAX_REDACTABLE_BYTES` is already cut into
overlapping windows with edge deferral, so region bounds inside a window add a
second edge class.

## What a test must construct

A differential over rule sets, profiles, limits, and inputs: a match and its
nearest anchor separated by more than the region bound; a match ending inside
`EDGE_MARGIN_BYTES` of a region edge; an input reaching `max_candidates`; an
input over `MAX_REDACTABLE_BYTES`. Compare `findings` (spans and order),
`limits_hit`, and `semantic_digest`. The
[wildcard checks](../existing-checks.md#wildcard-and-cross-cutting) cover
preselection soundness, canaries, pinned constants, the one-case
qualification fixture, and window placement; none compares a bounded scan
to the whole-input scan.

The whole-input scan must survive the change as a reference evaluation mode
or as a frozen copy of `evaluate` at HEAD
(`crates/secret-scanner/src/evaluator.rs:35-157`); otherwise the comparison
has no second side.

## Investigation log

### Q: Is bounding wanted, given the redaction windows already cap a scan?

- Sources examined: [`WINDOW_OVERLAP_BYTES`][overlap],
  [`EDGE_MARGIN_BYTES`][edge-margin], [`evaluate`][eval].
- Findings: The windows cap the bytes one `evaluate` call sees; inside a
  window every selected rule still scans every byte. Whether that cost is
  material is a measurement question W1 governs.
- Missing evidence: A measured share of pass time in `evaluate` on
  production-shaped `meta`.
- Conclusion: needs human input.

### Q: Does a per-rule proof exist that every match is anchor-bound?

- Sources examined: [`anchor_proof.rs`][anchor-proof],
  [`preselection_cannot_drop_a_finding`][t-anchor-proof],
  [`preselection_soundness_is_current`][t-anchor-proof-slow].
- Findings: A machine-checked proof exists, contrary to the record's
  wording. Its four conditions (keyword coverage, key-word coverage,
  syntactic implication, product emptiness) establish that a rule with no
  anchor anywhere in the input has no finding. Conditions 2 to 4 place an
  anchor inside the match; condition 1 places one inside the `keywords_any`
  window of radius `max(radius, two_phase.full_radius)`. That is a locality
  corollary, but the test asserts only skip-soundness, the product-proven set
  is a recorded list re-derived by an `#[ignore]` test, and the proof is
  against pinned corpus digests.
- Missing evidence: A test that states the region bound per rule and asserts
  it.
- Conclusion: resolved with answer - skip-soundness is proven in
  `anchor_proof.rs`; the region bound a bounded scan needs is a corollary
  nobody asserts, so the record's "new rule-set test" remains to be written.

[eval]: ../../../../../crates/secret-scanner/src/evaluator.rs#L35-L157
[keyed-path]: ../../../../../crates/secret-scanner/src/evaluator.rs#L75-L111
[captures]: ../../../../../crates/secret-scanner/src/evaluator.rs#L112-L128
[sort]: ../../../../../crates/secret-scanner/src/evaluator.rs#L130-L147
[radius-window]: ../../../../../crates/secret-scanner/src/evaluator.rs#L266-L297
[keyed-form]: ../../../../../crates/secret-scanner/src/evaluator.rs#L421-L479
[t-pinned]: ../../../../../crates/secret-scanner/src/evaluator.rs#L1660-L1683
[preselect]: ../../../../../crates/secret-scanner/src/rules.rs#L353-L370
[digest-doc]: ../../../../../crates/secret-scanner/src/rules.rs#L382
[anchor-ci]: ../../../../../crates/secret-scanner/src/rules.rs#L449-L473
[radius-valid]: ../../../../../crates/secret-scanner/src/rules.rs#L598-L602
[anchor-proof]: ../../../../../crates/secret-scanner/src/anchor_proof.rs#L1-L15
[t-anchor-proof]: ../../../../../crates/secret-scanner/src/anchor_proof.rs#L282-L299
[t-anchor-proof-slow]: ../../../../../crates/secret-scanner/src/anchor_proof.rs#L341-L365
[max-match]: ../../../../../crates/secret-scanner/src/api.rs#L16
[max-radius]: ../../../../../crates/secret-scanner/src/api.rs#L20
[limits]: ../../../../../crates/secret-scanner/src/api.rs#L219-L226
[revision]: ../../../../../crates/secret-scanner/src/api.rs#L276-L280
[rules-radius-doc]: ../../../../../crates/secret-scanner/default_rules.yaml#L12
[airtable]: ../../../../../crates/secret-scanner/default_rules.yaml#L297-L309
[overlap]: ../../../../../crates/context-core/src/redaction.rs#L365-L377
[edge-margin]: ../../../../../crates/context-core/src/redaction.rs#L380-L385
[ms-content]: ../../../../../crates/memory-store/src/lib.rs#L2070-L2078
[ms-digest]: ../../../../../crates/memory-store/src/lib.rs#L2357-L2392
