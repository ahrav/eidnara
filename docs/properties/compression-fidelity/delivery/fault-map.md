# Delivery fault and situation map

Scope: `ahrav/eidnara` compression-fidelity delivery, 2026-09-19,
HEAD `99f68bd37516ca5351f8aadb8b0b51cb0f13dcc8`.
The supplied plan and local history are leads as recorded in
[catalog.md](catalog.md#scope-and-provenance). No incident logs or related
repositories were supplied; that is provenance, not a restriction on external
sources. Every situation below is **not yet exercised**.

Faults include deliberate input omissions and resource pressure, not only
crashes. No crash or network-partition campaign is needed to establish these
local serving distinctions. Test form belongs to `/testing:test-strategy`.

## Availability and requirements

| Fault or enabling state | Available mechanism and missing evidence | Properties |
| --- | --- | --- |
| Later publication over an existing baseline | `compose_m1` selects rows after the folded sequence; producer tests and seeded transform tests exist separately. Their joined provider witness is missing. | tier, credit, situations |
| Natural age/importance selection | Existing renderer and core decay math support P1-P5. Hand-reviewed source obligations and independent scenario expectations are missing. | tier, omission, situations |
| Positive hard-budget pressure | Oldest-first guard is implemented. Use pressure that affects a high-importance row, not just ordinary rows; no production anchor overlap is supplied. | omission, credit, situations |
| Missing P2/P3; empty P4 | Parser fills missing intermediate tiers and accepts empty P4. Retain authored output to distinguish repaired from authored bodies. | tier, situations |
| Persisted legacy/non-tiered row | Renderer supports flat/truncated content, while tierless producer XML rejects. Construct storage compatibility separately; do not claim v2 semantics. | tier, credit, situations |
| Hint selection and fragment truncation | Default-on history lookup needs a new eligible tail, two matching terms, and a discriminating term in fewer than half the candidate rows. Add distractors rather than lower gates. | hints, omission, situations |
| Whole hint fragment dropped after compression | `transform.rs:8716-8727` drops a formatted line when `line.len() <= 2` bytes. With the `- ` prefix, this means empty normalized content, not every one- or two-character snippet. If every line drops, no hint is returned. | hints, omission, situations |
| Memory eligibility and budget negatives | Pinned canonical read and route tests support candidate, scope, quarantine, withholding, and budget controls. Add the separate explicitly rejected admission pair. | memory, omission, situations |
| Invalid recipe, base, or boundary | Plugin tests retain raw input. Capture actual fallback and record unexercised compression rather than infer success from the answer. | credit, situations |
| Missing or unrelated capture | Helpers can return `[]` or zero. Reject these before absence/size assertions; retain raw request captures before helper reset. | credit, situations |
| Warm replay | Existing m0/m1 byte-replay tests exist. Join a warm provider request to the same published case and retained semantics. | tier, memory, hints, credit, situations |
| Missing or mismatched fixture receipt | Consume `cf-fixture-script-qualification` from evaluation; even a valid initial receipt cannot qualify a later raw fallback or unrelated capture. | credit, situations |
| Serving-work or budget regression | Audit added production calls and replay body-budget, wrapper-retry, query-gate, memory, and warm-cache boundaries. A 105% wrapper trigger is not a hard-budget allowance. | resources |

Aliases above refer to the seven full slugs in the catalog:
`tier` = `cf-tier-transitions-preserve-qualified-meaning`,
`omission` = `cf-pressure-omission-discloses-evidence-loss`,
`memory` = `cf-memory-credit-requires-admitted-visible-content`,
`hints` = `cf-hints-do-not-strengthen-source-claims`,
`credit` = `cf-delivery-credit-requires-published-folded-capture`,
`situations` = `cf-delivery-scenarios-reach-qualified-invocations`, and
`resources` = `cf-serving-resource-boundaries`.

## Independent coverage checks to add

Each name below is a constant marker. Do not generate marker names from case
IDs. Store case/scenario identity as observation data and require the declared
case coverage separately. Apply `sometimes` to each of the 14 markers separately,
with an individual witnessed/missing outcome and evidence reference. Where a
marker has required case or subcase variants, report those outcomes separately
too; one variant cannot satisfy another. The umbrella record does not replace
these outcomes with a single aggregate boolean. Every predicate can hold on a
correct implementation; none asks for lost meaning or a false success label.

| Constant marker | Independent situation predicate |
| --- | --- |
| `cf-delivery-m1-published-input` | A prior m0 exists; a later accepted range is published; the next eligible pass extends replacement coverage through that range and a nonempty correlated provider request is captured. Observe m1 placement separately. |
| `cf-delivery-m0-tier-inputs` | A qualified invocation exists for the specified P1, P2, P3, or P4 age/importance input using the accepted case and independent expected selection. Report each required case/tier outcome separately. A manually selected tier call alone does not satisfy it. |
| `cf-delivery-natural-archive-input` | A published row has independently chosen age/importance inputs beyond natural archival; its covered native source is outside the live tail, and the corresponding request is captured. |
| `cf-delivery-high-importance-pressure-input` | A published high-importance row is renderable under curve selection at the same positive budget; that budget is below the independently charged nonarchived representations and a correlated replacement request is captured. A single-row history isolates this case because its curve age is zero. Check guard demotion and omission separately. |
| `cf-delivery-parser-fallback-input` | Authored XML omits P2/P3 but has valid P1; that accepted range reaches a captured invocation requesting a sparser representation. Do not require that meaning be lost. |
| `cf-delivery-empty-p4-input` | Accepted XML has empty P4 and a source-qualified title; an independently chosen P4 scenario reaches the provider after replacement. Title safety is the separate invariant. |
| `cf-delivery-legacy-input` | A known legacy or non-tiered stored row reaches a correlated replacement invocation, with its compatibility representation recorded rather than labeled authored v2. |
| `cf-delivery-hint-fragment-input` | Report two subcases independently: the selected candidate's compressed normalized fragment exceeds 80 UTF-16 units, or its formatted line is only `- ` and meets the `line.len() <= 2` filter. Both require independently observed selection/tail preconditions and a correlated request. The drop subcase does not require a delivered hint; neither subcase requires semantic loss. |
| `cf-delivery-hints-disabled-input` | A matched omission scenario uses `auto_search_enabled=false` and reaches a nonempty correlated request. No hint is required for this marker. |
| `cf-delivery-memory-positive-input` | The correct project's independently admitted positive row is eligible, available, within budget, and the associated omitted-history request is captured. Check retained meaning separately. |
| `cf-delivery-memory-negative-inputs` | The specified candidate-only, explicitly rejected, wrong-project, withheld, or budget-excluded state is independently observed before its corresponding captured invocation. Report each required negative subcase separately. No unauthorized rendering is required. |
| `cf-delivery-raw-fallback-input` | A deliberate invalid recipe/base/boundary is returned for input containing the case source and a correlated provider request is captured. The credit check must reject compression credit. |
| `cf-delivery-missing-capture-input` | A harness-level control supplies no matching request observation to qualification. Successful rejection satisfies safety; the situation does not require false credit. |
| `cf-delivery-warm-repeat-input` | The same accepted source set reaches a later captured request without new publication, after an earlier qualified request. Warm cache behavior and semantic review are separate checks. |

The campaign retains all 14 individual outcomes, including missing ones. For
C1-C6, require P1-P4 serving and at least one pressure/omission scenario per
case, with focused extras for the remaining mechanisms. No pairwise reduction
or Cartesian expansion is needed. The fragment-drop subcase is a distinct
absence path, not evidence that its source was retained elsewhere.

## Qualification order and windows

1. U1 supplies reviewed synthetic cases and fixed source identities. Keep answer
   keys out of the producer/provider input and candidate outputs separate.
2. U2 runs valid XML through the real producer/validator/publication path. Record
   authored, parsed/healed, and accepted coverage. The direct backend's literal
   `fixture-success` is not valid XML; completion counters cannot qualify it.
   Keep fact-set extraction rejection separate: valid history can publish with
   no extracted facts, which supplies no alternate-memory preservation credit.
3. U3 consumes the successful, matching
   [fixture qualification receipt](../evaluation/catalog.md#cf-fixture-script-qualification).
   Do not rebuild its script validator here. For each invocation, materialize
   the accepted range into m1 or m0 and verify host application.
   An empty bootstrap HARD is not the m1 situation. For m1, establish an older
   folded segment before publishing the case segment.
4. Capture the nonempty final provider body with explicit correlation to the
   case, session, follow-up, and replacement. A global last-main helper alone is
   insufficient. Check raw source occurrences in all live-tail blocks, not only
   whether the summary marker is present.
5. Supply alternate-surface and omission observations to the recovery owner's
   [canonical disposition predicate](../recovery/catalog.md#cf-unavailable-evidence-no-credit)
   and consume its result. Keep required-preservation loss separate from safe
   abstention. A scenario may explicitly permit unavailable evidence and
   abstention without awarding useful-preservation or recovery credit.

Freeze attempts and deadlines before execution. The existing harness provides
bounded waits, but this catalog does not invent a latency SLA or authorize
raising a timeout until a case passes. If qualification fails, keep the row
unexercised and report its last reached stage. Do not skip it as successful CI.

## Budget and scope constraints

- For every positive effective history budget, require the unwrapped history
  body to fit or be empty. The separate m0 check measures the wrapped
  session-history slice and retries at most three extra times above 105% of the
  requested budget. It returns the last render even if still above the trigger.
  Each retry tightens the effective body budget; the empty wrapper remains for
  cache stability. This is no global 105% allowance.
- Preserve OpenCode's existing final-array policy: its heuristic adds 25%
  headroom and declines a known-limit candidate only when over limit and growing
  in bytes. `limit_unknown` and `shrinks` can accept. The verified
  [charge/validation rules](../../../../packages/opencode-plugin/src/hooks/context/invocation-budget.ts#L44-L71)
  are not an absolute whole-context cap. Measure body, wrapped slice, and full
  provider invocation separately. Do not assume additive token estimates or
  combine Rust and TypeScript token counts. Full-invocation measurement and
  raw-fallback observations remain unexercised, not statically proven results.
- `cf-serving-resource-boundaries` also requires no feature-added per-transform
  semantic judge, native-source reread, or storage query, and unchanged query
  gates/limits, cache reuse, and admission for matched inputs. Do not add a
  production instrumentation API to check this; use source audit and existing
  replay seams. Evaluation owns serving size, estimator identity, generation,
  recovery, and cold/warm cost records; delivery supplies observations once.
- Preserve the 100-candidate, three-result hint limits, score/query gate, and
  frozen decisions. The current maximum wrapped hint is 470 UTF-16 units, below
  the 800-unit total cap. Require fragment-cap reachability, not total-cap firing.
- Do not add semantic callbacks, per-tier policy flags, automatic memory
  promotion, new source access, or nonzero anchor overlap to satisfy a test.
- Explicit search semantics, exact native recovery, Pi history folding (which
  this corpus does not replay), and evaluation/forwarder controls remain with
  their separate owners.

## Leverage ranking

1. After U1/U2, consume initial fixture qualification and independently check
   each invocation's replacement and nonempty provider capture. This removes
   false success from later oracles without duplicating the fixture validator.
2. Retain authored/parsed/effective forms and exercise fallback/empty P4. This
   prevents mislabeled tier coverage before expensive semantic review.
3. Pair positive memory with exclusion states through existing canonical reads.
   This exposes unsupported alternate-visibility claims without changing policy.
4. Drive natural and guard-induced omission, then hint truncation and complete
   fragment drop with valid ranking inputs. Reuse the resource checks and one
   source obligation across matched variants; report each situation separately.
5. Review semantics on qualified captures, including warm repeats. A failing
   qualifier or unavailable required constraint blocks fidelity acceptance even
   when the harness is complete and the invocation is smaller.
