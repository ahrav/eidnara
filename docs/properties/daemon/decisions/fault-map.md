# Part 4f fault-to-property map

For each property, what must actually occur for a test to be non-vacuous, and
whether the harness can produce it today.

Same rules as Parts 1 through 4e: safety checks must hold *while* their faults are
active; liveness checks need a bounded fault-free window; rare branches need
deterministic injection to be reachable at all; and coverage checks assert
independent preconditions, never the violation.

Config-derived rows, config coverage markers, and CI execution facts use
`74044960ee91641dec95c8552f15282844a18b13`, as does
[existing-checks.md](existing-checks.md). The source catalog's `e447c927` and
`76cd6f41` references describe its historical tree, not this revision. Unrelated
codec and decision-unit assessments retain that source-catalog provenance; this
config refresh does not present them as newly verified findings. Historical
evaluation is separated in [portfolio-evaluation.md](portfolio-evaluation.md#historical-evaluation).

Five framing points specific to this part.

**First, this is the cheapest part in the catalog so far, and the reason is
structural rather than lucky.** 4f is made of pure decision units and two pure
decoders. `boundary.rs:5-9`, `selection.rs:4-7` and `scheduler.rs:1-4` all claim
no I/O, no clock, no store and no ambient state, and the check inventory confirmed
those claims structurally. Both decoders are pure functions over one immutable
input array (`codec/opencode.rs:23-25`, `codec/pi.rs:19-21`). **So most of this
part needs no harness at all: a struct literal, a JSON string, or a
`Vec<serde_json::Value>` is the entire enabling state.** There is no clock to
pause, no second process to spawn, no store to corrupt, and no two-pass sequence
to arrange. Contrast 4e, where eleven of 24 records needed seeded frozen units and
three needed a second render.

**Test execution is available.** The workspace nextest job includes daemon library
and integration targets (`.github/workflows/ci.yml:413-417`). The user-budget
reader integration check reaches the config loader and canonical-memory reader
(`crates/daemon/tests/transform_canonical_memory.rs:448-539`). The source-catalog
`F0` blocker is retired; execution does not establish test adequacy.

**Third, one capability is a build flag rather than a fault, and in this part it
buys less than it did in 4e.** All three 4f assertion sites are `debug_assert!`
(`codec/opencode.rs:251`, `:252`, `:466`) and **no `cfg(not(debug_assertions))`
exists anywhere in 4f**, verified across all eleven files. So unlike 4e's
two-armed belt there is no release arm whose distinct behaviour a release run
observes. What `F4` buys instead is the *absence*: that `:466` enforces nothing,
that `:252`'s violation is silent because `take` at `:265` saturates, and that
`:251`'s is not silent because the slice at `:258` panics on the same condition in
every profile.

**The `smart_drops` differential is a historical investigation lead, not a CI
blocker.** The flag defaults to `false` (`config.rs:128`) and is project-allowed
(`config.rs:673-681`), with the shared parser at `config.rs:890-894`. The source
catalog proposed comparing disabled-feature output against an age-only baseline.
A valid comparison needs that baseline and a workload, not merely two config
resolutions. This refresh adds no property and makes no whole-pipeline adequacy
claim from the flag's parser.

**Fifth, this part has no `sometimes` and no liveness record, which changes what
the coverage-check section has to do.** The 27 records are 26 `safety` and one
`reachability`; the catalog's own header already reports the semantics finding on
that single `reachable` record and declines to apply it. So there is no
`sometimes` marker to audit for the forbidden pairing, and every marker proposed
below is new. **The zero-liveness position was challenged by an independent
evaluation and upheld**, on the ground that `scheduler::decide` is an immediate
pure transition with no progress obligation to bound and the paging loop that
would carry one belongs to another sub-part; the reasoning is in
[portfolio-evaluation.md](portfolio-evaluation.md) and is recorded there rather
than here because it is a verdict on this file's framing, not a fact about a
fault class. The forbidden pairing is still the dominant hazard here, because in
this part the defect is almost always easier to name than its precondition.

## Fault classes required

`F0` is an available execution capability, not a fault or an outstanding workflow
change. Execution and oracle adequacy are separate questions.

| Class | Description | Available today |
| --- | --- | --- |
| **F0** test execution in CI | A workspace job that runs daemon library and integration targets | **Yes.** `.github/workflows/ci.yml:413-417` runs workspace nextest with all targets and features across two partitions. No CI change is required for the config checks. The budget-reader integration check is listed in [existing-checks.md](existing-checks.md#config-reader-integration-check) with status `unaudited` |
| **F1** arbitrary input to each decoder | A `Vec<serde_json::Value>` of arbitrary shape handed to `decode_opencode` or `decode_pi` | **Yes, and it needs no fault. This is the cheapest capability in the part.** Both decoders return `DecodedHarnessMessages` with **no error type at all** (`codec/opencode.rs:23-25`, `codec/pi.rs:19-21`), so totality is free and the interesting question inverts from "does it reject" to "what does it silently accept". An arbitrary `Vec<Value>` is the whole enabling state; the interesting members are a bare string or number as an array element, a `parts` value that is an object rather than an array, and a part whose `type` is absent. The only decoder inputs anywhere today are the two goldens' single cases plus well-formed hand-built fixtures across `codec/opencode.rs:1322-2186` (17 tests) and `codec/pi.rs:1078-1499` (14 tests) |
| **F2** configuration values at and beyond documented bounds | A user-tier value exercises parsing and range behavior; a project-tier value exercises only keys permitted by the tier policy | **Yes.** Resolve fixtures through `merge_tiers_with_warnings` (`config.rs:710-755`) and inspect the effective config and warning vector. In particular, supply `memory.injection_budget_tokens` above `20000` or below `500` in the user tier with no project tier: its parser applies only `.max(1.0)` (`:847-851`). A project value is rejected before parsing (`:729-732`) and cannot exercise this range property. The Group A rows specify the per-property enabling state |
| **F3** a malformed configuration file | An `eidnara.jsonc` whose syntax error survives `strip_jsonc`, for example an unterminated string | **Yes, with an existing check.** `read_tier_cached` records parse/read warnings (`config.rs:362-393`), and `effective_with_warnings` returns them (`config.rs:262-282`). The malformed/unreadable/missing fixture at `config.rs:2133-2182` asserts path-specific warnings, repeated warnings, and same-mtime repair; status `unaudited`. The silent-failure defect premise is invalidated |
| **F4** building and running in release | The same suite compiled with `debug_assertions` off | **Yes, and it is a build flag rather than a fault, but it buys less here than in 4e.** `cargo test -p daemon --lib --release` drops all three `debug_assert` sites (`codec/opencode.rs:251`, `:252`, `:466`) and stops compiling the one test gated `#[cfg(debug_assertions)]` at `:2077`. **Verified: no `cfg(not(debug_assertions))` exists anywhere in 4f**, so unlike 4e there is no release arm with distinct behaviour to execute. What `F4` establishes is three absences: `:466` enforces nothing while `duplicate_tool_use_locations` at `:465` still runs and its result is discarded; `:252`'s violation is silent because `take` at `:265` saturates; and `:251`'s violation is **not** silent, because `&messages[replace_from..]` at `:258` panics on the same condition in every profile. Cost: one extra invocation |
| **F5** harness input carrying unknown or omitted types | One session entry or message part whose `type` the decoder does not recognise, or a required class the goldens omit | **Yes, and it needs no fault.** One hand-built element. Pi: an entry with an unrecognised `type` and no `role` key, for example `{"type": "tool_use_v2", "data": {}}`, or the degenerate `{"type": "message"}` with no `message` key, which the decode loop drops from `decoded` and from the sidecar alike (`codec/pi.rs:41-50`, `:661-669`, `:681-686`). OpenCode: a part in `{snapshot, patch, agent, retry}`, which is preserved as raw for re-encode (`codec/opencode.rs:194-204`) and omitted from `content` (`:193`). Plus the two classes the goldens declare missing: an OpenCode `subtask` part and a Pi assistant `thinking` part carrying `redacted: true`. Verified at `HEAD` that `opencode-golden.json` covers 11 of 12 required classes with `subtask` declared missing, and `pi-golden.json` covers 12 of 13 with `redacted_thinking` declared missing, and that `assert_coverage_or_recorded_missing` (`codec/mod.rs:254-271`) passes on both |
| **F6** caller-supplied block identity | A wire ingress block whose `provider_extras` already carries a `_eidnara_codec` stamp the decoder did not write, or two byte-identical native parts in one message | **Yes, and it needs no fault.** `TransformRequest.messages` is `Vec<IngressMessage>` (`transform.rs:781`) and `WireBlock`'s `Deserialize` (`memory-store/src/lib.rs:207-221`) reads `provider_extras` verbatim, so a caller can supply plausible `blockIndex`, `nativeIndex` and `decodedFingerprint` values under the string key `_eidnara_codec` (`codec/sidecar.rs:148`). `stamped_block_identity` (`:196-203`) returns `Some` for any three well-formed values, and `stamp_block_identity` (`:177-194`) is the only writer **by convention, not by encapsulation**. The collision half needs only one OpenCode message with two byte-identical parts. The file's three direct tests (`:487-557`) exercise alignment pairing only, so both halves are unexercised in either direction |
| **F7** cross-implementation differential | The same inputs and an independently defined comparison implementation or baseline | **Available for direct Rust comparisons; broader parity remains a separate investigation.** The `smart_drops` parser is shared across permitted tiers (`config.rs:717-733`, `config.rs:890-894`), but that alone is not a byte-equality oracle. The Rust TTL fixture test at `config.rs:1133-1160` is included in workspace nextest (`.github/workflows/ci.yml:413-417`). It consumes five frozen routing vectors, not a live TypeScript implementation. The source-catalog TypeScript consumer is not present at its cited path; the claim that only its leg runs is retired |

Three availability caveats that cut across classes.

- **F3 uses a file; F2 can use the in-memory merge.** `ConfigCache` reads the
  filesystem and caches on mtime
  (`config.rs:362-393`). A successfully cached tier needs a changed mtime to
  observe changed bytes, as `config.rs:2093-2130` checks. A failed tier bypasses
  that fast path and can recover without an mtime change (`config.rs:2162-2175`).
  In-memory F2 merge checks do not require filesystem state.
- **`F6`'s trust half proves the module's behaviour on a hand-built ingress
  message without establishing that a production route supplies one.** Whether a
  non-module actor can choose `provider_extras` on a production route is a route
  question this pass cannot answer, and it is the same shape as 4e's open question
  about who can choose a tool-call id.
- **Two records are `test-only` by their own reachability label**, and that is
  orthogonal to non-vacuity. `codec-b-pi-decoder-drops-unrecognised-entry-types-without-a-record`
  and `codec-b-pi-encoder-can-return-a-shorter-array-than-it-was-given` are both
  constructible today and neither has a production caller on the Pi encode path.

## Map

All 27 records, grouped as the catalog groups them, meaning by the thing a single
test fixture would have to build. "Non-vacuous today" means a developer can
construct the required state with the current harness, not that an existing
check proves the property. The two invalidated config premises are retained as
regression contracts and excluded from open-defect totals.

One reachability precondition is stated once rather than per row. No decision unit
or codec in scope sits behind a Cargo feature gate, no unit in scope reads a clock
or a store, and the only profile-dependent code in the part is the three
`debug_assert` sites in `codec/opencode.rs`. The `explicit-config-only` records are
seven among all retained records, including the two invalidated config premises,
and five among the active records.

### Group A: the configuration contract as a defect surface

| Property | Required faults and enabling state | Non-vacuous today |
| --- | --- | --- |
| dec-a-execute-threshold-lower-bound-is-documented-20-and-enforced-1 | A user-tier threshold below `20`, such as `5`, with no project tier (F2) | **Yes.** `config.rs:750-752` clamps to `[1, 90]` without a range warning. `project_threshold_may_only_raise` (`config.rs:1297-1302`) covers the upper clamp only; status `unaudited`. A project-only low value is rejected by tier policy, so it cannot stand in for the user-range check. The historical `20-90` contract and its unresolved authority are recorded in the [evidence](evidence/dec-a-execute-threshold-lower-bound-is-documented-20-and-enforced-1.md) |
| dec-a-memory-injection-budget-documented-range-has-no-implementing-code | A user-tier `eidnara.jsonc` with `memory.injection_budget_tokens` above `20000` or below `500`, with the project tier absent (F2) | **Yes.** The standard key and deprecated fallback apply `.max(1.0)` without the source-catalog `500-20000` range (`config.rs:847-864`). Both keys are user-only (`:664-672`). Assert the range or a key-specific warning from the user-only resolution. A separate project-rejection fixture must not substitute for that assertion: its ignored-key warning would pass without exercising the range. See the [budget evidence](evidence/dec-a-memory-injection-budget-documented-range-has-no-implementing-code.md) |
| dec-a-commit-cluster-trigger-config-is-inert-in-this-crate | A nondefault user `commit_cluster_trigger` value and a workload that distinguishes it from the defaults | **Partial.** The production context uses `true` and `3` from constants (`lib.rs:640-641`, `lib.rs:4983-5005`), not parsed config. Default-valued input cannot distinguish those mechanisms. Observe the constructed context or use a workload whose commit-cluster trigger differs under the requested value (`boundary.rs:814-819`). The default-constant check (`boundary.rs:2011-2015`) is not a config-wiring check; status `unaudited` |
| dec-a-project-tier-can-write-leaves-outside-the-documented-allow-list | Distinct valid user and project values for every consumed key, both boolean directions, and both threshold directions (F2) | **Yes.** Compare effective values and warning keys against the [catalog's explicit policy](catalog.md#dec-a-project-tier-can-write-leaves-outside-the-documented-allow-list), not the implementation's classification. The merge warns for all supplied user-only keys and rejected weakening candidates (`config.rs:723-747`). The hostile fixture (`config.rs:1705-1815`) asserts selected outputs but derives its ignored-key count from `tier_class`; exercise is partial, although the full independent check is constructible |
| dec-a-config-value-clamps-and-zero-rejection-are-invisible-to-the-caller | One user-tier leaf: score threshold `0.99`, minimum prompt characters `0`, or caveman minimum characters `50`, with no project tier (F2) | **Yes.** `apply_key` clamps the score, prompt minimum, and caveman minimum at `config.rs:827-846`. `positive_usize_at` rejects zero (`config.rs:955-961`), so a user-only prompt minimum of zero leaves the default `20`. The merge returns warnings but these branches emit no range warning. Assert the altered value and a key-specific range diagnostic, not an unrelated tier-rejection or deprecation warning |
| dec-a-malformed-config-silently-resolves-to-defaults-and-stops-the-historian | A malformed user file, an unreadable project path, and missing-file controls (F3) | **Invalidated premise; retained regression contract.** `config.rs:362-393` stores read/parse warnings, `config.rs:262-282` collects them, and `config.rs:250-257` plus `config.rs:402-406` emit them. The existing fixture at `config.rs:2133-2182` observes the warnings without changing any signature; status `unaudited`. Fallback to defaults does not imply silent failure |

### Group B: model-chain resolution

| Property | Required faults and enabling state | Non-vacuous today |
| --- | --- | --- |
| dec-a-model-key-lookup-walk-has-two-implementations-that-disagree | Equivalent maps with distinct wildcard and default values, resolved for the same qualified model key | **Yes by direct call.** The TTL walk includes `provider/*` (`config.rs:151-207`); the scheduler walk does not (`scheduler.rs:779-830`). The daemon adapter builds only a scalar threshold (`transform.rs:5447-5454`), but the scheduler golden deserializes map variants and exercises them (`scheduler.rs:923-932`, `scheduler.rs:1011-1013`, `scheduler.rs:1087-1096`). That separate test and the TTL vectors (`config.rs:1133-1160`) do not compare both walks on one wildcard-only map |
| dec-a-model-chain-dedup-is-adjacent-only | User primary `a` and fallbacks `[b, a]` (F2) | **Invalidated premise; retained regression contract.** The merge calls `dedup_preserving_order` (`config.rs:750-754`), whose `HashSet` plus `retain` keeps the first occurrence (`config.rs:950-953`). `[a, b, a]` becomes `[a, b]`. The non-adjacent-repeat test at `config.rs:2185-2194` asserts ordering too; status `unaudited` |

### Group C: totality, determinism, and the one clamp with a bypass

These seven records use direct calls on a
pure function with a hand-written argument, and **four of them are guards that
hold rather than defects**, which is why they are recorded: they fix the boundary
so a later change that drops a guard is visible.

| Property | Required faults and enabling state | Non-vacuous today |
| --- | --- | --- |
| dec-a-cache-ttl-parse-is-total-over-arbitrary-strings | Direct string inputs, or user-only TTL strings such as `"0"`, `"5S"`, a long digit run, and a multibyte suffix (`config.rs:924-945`) | **Yes.** `parse_cache_ttl` returns a `Result` and saturates oversized results (`scheduler.rs:365-398`). The `never` tests at `scheduler.rs:1378-1405` remain `unaudited`. Zero hard expiry requires positive prior time and elapsed time (`scheduler.rs:405-407`), and later scheduler gates apply (`scheduler.rs:689-732`). Invalid strings use the default without a parse diagnostic (`scheduler.rs:771-773`) |
| dec-a-boundary-budget-derivation-is-total-over-non-finite-input | A `BoundaryContext` whose `context_limit`, `execute_threshold_percentage` or `usage_percentage` is `f64::INFINITY` or `f64::NAN`. No fault | **Yes by direct call; production reachability is a separate and narrower question.** A struct literal with `f64::NAN` is the whole enabling state, and no test targets non-finite input today. Reaching it *from production* needs a host-supplied usage reading, since `lib.rs:4950-4959` builds the context from request and store values. **This is the guarded analogue of Part 3's decay totality defect and, over the three fields it validates, the guard holds**: `boundary.rs:339-341` returns `TRIGGER_BUDGET_MIN` for non-finite and non-positive input, which `CONFIGURATION.md:238` (source-catalog path, not present at HEAD) does not mention. The `trigger_budget` passthrough that this cell used to fold in as "the one place a caller could still inject a non-finite value" is no longer part of this record; it is the row below |
| dec-a-caller-supplied-trigger-budget-is-the-one-unvalidated-float-and-reaches-a-diagnostic | A `BoundaryContext` with `trigger_budget: Some(f64::NAN)` and a non-empty message set. No fault | **Yes, and it is the cheapest falsifying oracle in the part.** One struct literal and one call: `BoundaryContext.trigger_budget` is a `pub` field and both read sites are reachable in-crate. `boundary.rs:377-379` and `:756-761` read it through `unwrap_or_else` with no `is_finite` gate on the `Some` arm, unlike the three neighbouring fields. `derive_protected_tail_token_target`'s own postcondition survives, because `f64::min` at `:383` returns the non-NaN operand and `n` stays finite, but `:399` stores the raw NaN into the returned struct and `:802`'s `tail_size_bar: trigger_budget * TAIL_SIZE_TRIGGER_MULTIPLIER` is a bare multiply with nothing to absorb it. So `TriggerProgress.tail_size_bar` is NaN, and that struct is carried into the transform response at `lib.rs:4982` and divided at `:5002`. **Unlike every other row in this table, this oracle fails on the current build**, and the evidence was already written: the budget record's evidence file lists this exact case as test-plan item 4 and states it fails today |
| dec-a-derive-historian-chunk-tokens-is-total-at-both-integer-extremes | Direct calls with zero and `usize::MAX`; a configured zero is rejected by `positive_usize_at` (`config.rs:955-961`) | **Yes by direct call.** The final integer clamp enforces `[8000, 50000]` (`config.rs:28-29`, `config.rs:39-46`). The maximum input reaches that clamp after quartering, without needing cast saturation. `historian_budget_derivation_clamps_at_both_bounds` (`config.rs:1449-1455`) covers both clamp arms and an interior value but omits the two extremes; status `unaudited` |
| dec-a-escalation-bands-stay-ordered-for-every-threshold | **None.** A threshold of `f64::NAN`, a negative threshold, or a threshold above `90` are the interesting inputs | **Yes.** One call per threshold. `scheduler.rs:1238` and the golden constant assertions at `boundary.rs:2226-2227` are the existing checks. The consequence the record pins is precise: if a threshold could push the force band to or past `95`, the `Force85` arm at `scheduler.rs:525` would become unreachable and the emergency arm would absorb the whole force band, changing which passes bypass mid-turn deferral. The cap makes that impossible |
| dec-a-selection-decision-order-is-total-under-hashmap-iteration | **None for the property.** Refuting it needs an input where one `target_id` receives two same-rank decisions with different payloads, which requires duplicate `SelItem` ids mapped to different `arc_id`s | **Yes, and the cheap form is a repeat-call equality plus a postcondition scan.** Both conjuncts are directly assertable: repeated calls on identical inputs return equal `Vec<ReductionDecision>`, and no two distinct arcs emit a decision for the same `target_id`. `selection.rs:2836` `drop_wins_over_edit_marker` plus the differential golden `selection.rs:32-33` names as the arbiter are the existing checks. **The cross-process form is also cheap** (two `cargo test` invocations give two `HashMap` seeds) and is the form that would catch a genuine iteration-order dependence, since both hash-iterating loops (`selection.rs:1305`, `:1397-1405`) are made order-insensitive downstream today. The header stakes the cache invariant on this: if it fails, a defer pass replays different bytes than the freeze produced |
| dec-a-region-hint-clamp-bypassed-by-sentinel-suffix | `smart_drops: true`, off by default (`config.rs:128`) and allowed in either tier (`config.rs:673-681`, `config.rs:890-894`), plus a superseded edit/write value ending with `...[truncated]` | **Yes.** The suffix arm returns the input unchanged (`selection.rs:536-549`). The UTF-16 boundary check at `selection.rs:2424-2436` does not supply a sentinel-suffixed input; status `unaudited`. A direct oversized suffix input distinguishes that arm from the ordinary length clamp |

### Group D: decoder acceptance with no rejection channel

| Property | Required faults and enabling state | Non-vacuous today |
| --- | --- | --- |
| codec-b-harness-decoders-accept-every-input-with-no-rejection-channel | **None.** An arbitrary `Vec<Value>` is the whole enabling state (F1). The interesting members are a bare string or number as an array element, a `parts` value that is an object rather than an array, and a part whose `type` is absent | **Partial: the return and consistency clauses are the cheapest codec oracle in the part; the allocation clause is not observable at all.** One call per input for the first two clauses. The third clause, "allocation is bounded by a constant multiple of input size", cannot be witnessed by a decode call: both decoders return `DecodedHarnessMessages` and expose no allocation accounting, so proving a multiple of input size needs a counting `#[global_allocator]`, a `dhat`-style profiler, or a `Vec::capacity` sweep over the returned structure, and the tree has none of the three. That clause is discharged by reading — the largest allocations are `raw_message.clone()` at `codec/opencode.rs:232` and `raw_entry.clone()` at `codec/pi.rs:114`, one per input message — and must not be counted as an oracle a call satisfies. `codec/mod.rs:78-89` and `:201-212` assert decode determinism over the goldens, which pins purity and not totality; all 31 hand-built decoder tests use well-formed fixtures. **This record differs from Part 1's equivalent in a way worth carrying**: Part 1's `decoder-totality-over-arbitrary-bytes` could say the property holds and is under-evidenced, whereas this one **is violated by design**, because there is no error variant to fall back on. The failure mode is not a crash but a fabricated message: a malformed element becomes a zero-block `"user"` message that occupies an ordinal, enters the sidecar, participates in boundary selection, and is re-encoded from its retained raw |
| codec-b-pi-decoder-drops-unrecognised-entry-types-without-a-record | One Pi session entry with an unrecognised `type` and no `role` key, for example `{"type": "tool_use_v2", "data": {}}`, or the degenerate `{"type": "message"}` with no `message` key (F5) | **Yes.** One hand-built entry. No golden case and no unit test supplies one: `codec/pi.rs:1078-1499` has 14 tests, and `:1479-1483` asserts `encode_pi(...).is_empty()` for an empty-content message, which is the encoder half of a different drop. The check is that every input entry is recoverable either from a `IngressMessage`'s meta or from `sidecar.messages`; today the entry is dropped from both (`codec/pi.rs:41-50`, `:661-669`, `:681-686`). **The unrecoverable consequence is the ordinal shift**: every later entry moves down by one, so a persisted boundary ordinal or a tag keyed to an ordinal now names a different message, and because Pi has no `absolute_ordinal` input there is no way for the harness to pin the numbering against it |
| codec-b-opencode-hides-four-part-types-from-every-transform-decision | **None for the preservation direction**; one OpenCode message carrying any of `{snapshot, patch, agent, retry}` suffices (F5). For the interesting composition, that message must **also** have a decoded block deleted, so `remove_unretained_native_parts` runs with a non-empty removal set | **Yes for both halves.** The golden already supplies one `patch` part, so the preservation direction is pinned **by accident rather than by design**: `codec/mod.rs:59-76` lists `patch` as a required coverage class and the round trip covers it. `codec/mod.rs:216-252` `codec_conformance_removes_leading_native_blocks_without_reindex_drift` exercises the removal path but on a message with no immune parts, so the composition of the two is what is missing and it is one fixture away. **Correct today and fragile in one direction**: these four types are invisible to the wire view, so the transform's byte accounting, tag numbering and boundary selection never see them while the provider does |
| codec-b-provenance-recovery-on-decode-is-all-or-nothing-and-opencode-only | For the mixed-parts hole, one OpenCode message with one synthetic part and one authored part. For the role hole, a synthetic assistant or tool message that is not the todo pair. **For Pi, any input at all** (F5) | **Yes, and the Pi half needs literally nothing.** `codec/mod.rs:128-175` covers the all-synthetic path and `:290-298` asserts `message["meta"]["synthetic"] == true` on the native fixtures; **neither covers a mixed message**. The consequence is that the module's own writes can come back classified as user-authored, and `meta.synthetic` gates `meta_for_ck`'s positional fallback (`codec/sidecar.rs:446-450`), so a misclassified module-authored message becomes eligible to inherit a native envelope by position. **Pi's hardcoded `false` means the Pi leg has no provenance in either direction**, which composes with 4e's finding to leave synthetic content indistinguishable from authentic content for that harness at every layer |

### Group E: cross-stage composition and block identity

| Property | Required faults and enabling state | Non-vacuous today |
| --- | --- | --- |
| codec-b-decoder-output-can-violate-the-projector-precondition | Two independent shapes, both harness-controlled. One OpenCode message with `info.id` containing `#`, or one Pi entry with such an `id` or `responseId`, which the decoders copy verbatim into the mid; **or** a Pi `toolResult` entry whose preceding `toolCall` entry was dropped by the mechanism above, yielding a `ToolResult` block with no pending call (F1 + F5) | **Yes, and what is missing is the composition rather than either half.** `wire.rs:1122` and `:1149` cover the projector's rejection with hand-built inputs (both verified at `HEAD` to assert `UnpairedToolResult`). **Nothing covers the mid rejection at all, and no test composes a decoder with the projector**, which is the whole point of the record. Both functions are in-crate, so `project_messages(&decode_pi(input).messages)` is one line. The rejection is correct and fail-closed; the defect is that it is detected two layers away from the layer that could have normalised it, and **a single harness-supplied id containing one `#` fails every transform pass for that session until the message leaves the window** |
| codec-b-absolute-ordinal-is-harness-supplied-and-never-validated | **None.** A window into the tail of a long session is the whole enabling state | **Yes, and the producer's contract is already verified.** `module-wire.ts:1028-1031` bases the numbering on a canonical count, so a fifteen-message window of a 500-message session carries ordinals around 501-515, and `module-wire.test.ts:180` pins `absolute_ordinal: 501` as a real value (both read at `HEAD`). `transform.rs:20278` already supplies `"absolute_ordinal": 2_414` in a fixture. **No check exists for the invariant in either language.** The check is stated over the consumer's interpretation rather than over the decoder's validation, because the producer's contract makes the verbatim pass-through correct: `boundary.rs:687-691`'s max-as-count reading is what disagrees, and it disagrees for **every** windowed session rather than for a contrived one |
| codec-b-block-identity-stamp-is-caller-writable-and-the-fingerprint-is-not-an-identity | For the collision half, one OpenCode message with two byte-identical parts. For the trust half, a wire ingress message carrying `provider_extras["_eidnara_codec"]` with plausible `blockIndex`, `nativeIndex` and `decodedFingerprint` values (F6 + F1) | **Yes for both halves, and this is the record with the least existing evidence of any in the part.** `codec/sidecar.rs` has three direct `#[test]` functions (`:487-557`), verified directly, and all three exercise `match_block_metas` and `greedy_block_metas` pairing rather than the stamp or the fingerprint. `codec/opencode.rs:1515-1582` and `codec/pi.rs:1436-1443` exercise alignment after a block deletion and an encode replay, which covers the honest path only. **The forged stamp lets a caller point a block at a native part it did not come from**, and `alignment_candidate`'s early return means the kind check that would otherwise catch the mismatch is skipped, so the encoder can write a text block's content into a reasoning part. The collision is contained today **only** because the stamp disambiguates duplicates, which makes the stamp the sole load-bearing disambiguator for a case the fingerprint cannot handle |

### Group F: release behaviour of the codec guards

| Property | Required faults and enabling state | Non-vacuous today |
| --- | --- | --- |
| codec-b-incremental-sidecar-slice-panics-behind-a-debug-assert | A caller passing `replace_from > messages.len()`. In debug the `debug_assert!` at `:251` fires first; **in release the slice at `:258` panics with "range start index out of range"** (F4 for the release half) | **Yes by direct call, and the check fails today in both profiles, which is the finding.** `decode_opencode_sidecar_incremental` is `pub(crate)`, so a test can pass an out-of-range value without a new production caller. Its only test, `incremental_sidecar_carries_pins_across_three_generations` (`codec/opencode.rs:2113`), calls it three times with `replace_from` of **1, 2 and 2** (`:2128`, `:2139`, `:2151`), all in range. Reaching it *from production* needs a new caller, or `validated_native_prefix`'s `:12561` filter changing, or `native_sidecar`'s `:12577` condition changing. **The existing test-hook asymmetry is the sharpest evidence**: `lib.rs:12452-12459` defines `CorruptSidecarForTest` and `CorruptFrontierForTest`, and `:12531-12541` deliberately perturbs the projection prefix by `+1` under `cfg(test)` then re-clamps at `:12543`, so the authors built a hook for a corrupted prefix on the projection path and **no equivalent hook exists for the sidecar slice** |
| codec-b-wire-level-tool-use-uniqueness-guard-has-no-release-behaviour | **A release build (F4)** plus an input reaching the `parts.push(render_tool_pair_as_part(block, result))` arm at `codec/opencode.rs:754` for a call id another message already emitted. The comment at `:749-757` says the arm exists because neither half matched a native index, which is the fresh-shell case | **Yes, and F4 is the whole cost.** The `always(!duplicate)` on the returned `Vec<MessageV2Json>` is assertable in either profile; F4 is what shows the guard is absent. Verified: `assert_unique_tool_use_ids` (`:462-470`) has **one** arm, so in release the function is a no-op while `duplicate_tool_use_locations(messages)` at `:465` still runs and its result is discarded, and there is **no `cfg(not(debug_assertions))` anywhere in 4f** to hold a repair. Its only test is gated `#[cfg(debug_assertions)]` at `:2077`. `transform.rs:21509` and `:21522` exercise 4e's `enforce_unique_tool_use_ids`, which is **the wrong layer**. Two further scope facts: three production callers depend on the guard (`:370`, `lib.rs:12985`, `lib.rs:21308`), and **`lib.rs:12949`'s direct call to `encode_opencode_chunks_with_transition_state` has no uniqueness check in any profile**, because the guard sits inside `encode_opencode_impl` rather than in the chunk API |

### Group G: round-trip claims and declared coverage gaps

| Property | Required faults and enabling state | Non-vacuous today |
| --- | --- | --- |
| codec-b-round-trip-identity-is-claimed-in-one-direction-on-one-case-per-harness | **None for the claimed direction.** To make the oracle meaningful, an input containing a shape the retained-raw path does not cover: an unrecognised part or entry type, or a **mutated** block, since an unmutated block short-circuits at `codec/opencode.rs:763-765` and `codec/pi.rs:463-465` and is trivially identical (F5) | **Yes, and the strengthening is a fixture change rather than a capability.** `codec/mod.rs:54-90` and `:177-213` plus the determinism assertions at `:81`, `:87`, `:204`, `:210` are **genuine oracles and not tautologies**: they compare against an independently captured input array, since `generated_from` names a real `opencode.db` and real Pi JSONL sessions, which is materially stronger than the round trip Part 1 characterised as "a tautology over accepted inputs". **The weakness is breadth and oracle fidelity, not vacuity**: one case per harness, the expected value derived from the input by `strip_opencode_compaction` / `strip_pi_compaction` at `:88` and `:211`, and the retained-raw path making identity nearly automatic for unmutated input. The other direction is provably false and already pinned: `codec/mod.rs:112-125` shows four wire messages encoding to three wire messages |
| codec-b-declared-missing-capture-classes-are-never-decoded | One OpenCode message with a `subtask` part; one Pi assistant entry with a `thinking` part carrying `redacted: true` (F5) | **Yes, and the blocker is that nobody added a case rather than that anyone cannot.** Both fixtures were read at `HEAD`: `opencode-golden.json` covers 11 of 12 required classes with `subtask` in `missing_capture_classes`, `pi-golden.json` covers 12 of 13 with `redacted_thinking`, and `assert_coverage_or_recorded_missing` (`codec/mod.rs:254-271`) passes on both because listing a required class clears it. Its own message, "codec golden neither covers nor records missing classes" (`:267-270`), is honest that it is a bookkeeping gate. **The two halves are not equally valuable**: deleting the `subtask` arm would not move the golden, since the part would fall to `:194-204` and still become an opaque block, whereas Pi's `:199-211` produces `BlockKind::RedactedReasoning` against `:212-217`'s `BlockKind::Reasoning` with a signature, and the two round-trip through different encoder arms (`:543-548` versus `:536-542`) |
| codec-b-pi-encoder-can-return-a-shorter-array-than-it-was-given | For the `codec/pi.rs:371` drop, a message whose meta role is `toolResult` but whose wire content holds no `ToolResult` block, which the transform can produce by reducing a decoded tool-result message. For the `:396-397` drop, a wire message with empty `content` whose matched meta's raw is not a Pi message | **Yes via the `:371` drop; the `:396-397` half may be unreachable by construction.** The first drop is directly constructible and refutes `encode_pi(msgs, sidecar).len() == msgs.len()`, so the check is non-vacuous. The second may be unreachable, since only `decode_opaque_entry` produces such a raw and those messages carry exactly one opaque block; that half is recorded and not resolved. `codec/pi.rs:1469-1484` pins the cleared-content drop. **Reachability is the caveat, not constructibility**: there is no production caller, so the record exists because the function is a public export (`codec/mod.rs:10`, `lib.rs:12`) whose contract differs from its OpenCode twin, and 4e's lens already notes the Pi encode path is off-route |

**Retained records: 27; active: 25; invalidated config premises: 2.** The active
rows contain 23 `Yes` and 2 `Partial` labels, counted directly rather than
subtracting from the historical headline. The partial rows are commit-trigger
configuration and decoder allocation observation. This is a label census, not
a fresh adequacy verdict for unrelated records. The invalidated rows retain
executable regression contracts.

### Config disposition

The historical 26-of-26 and 23-of-27 headlines are not the current row census.
Their investigation is retained in the historical portfolio evaluation. The
config-related dispositions are:

- **Commit-trigger configuration remains partial.** `lib.rs:5001-5002` uses
  constants rather than config. A nondefault input and a distinguishing workload
  or context observation are necessary; default-valued input cannot expose the
  wiring gap.
- **`dec-a-malformed-config-silently-resolves-to-defaults-and-stops-the-historian`
  is invalidated.** The historical demotion to `Partial` rested on an absent
  diagnostic channel. Tier warning storage and collection now make the original
  path-bearing warning directly observable (`config.rs:362-393`,
  `config.rs:262-282`). The existing test has status `unaudited`.
- **The adjacent-only model-chain premise is invalidated.** Full deduplication
  and the non-adjacent-repeat check exist (`config.rs:950-953`,
  `config.rs:2185-2194`); the regression contract remains.

### Reachability qualifications

Constructing an input is not the same as observing its oracle. Config tests can
inspect merge values and returned warnings without changing runtime interfaces.

- The model-walk differential remains `test-only` for the in-tree config route:
  `number_at` rejects an object (`config.rs:963-968`), and the transform adapter
  constructs a scalar threshold (`transform.rs:5447-5454`). The scheduler golden
  does deserialize map variants, so the old claim that no test constructs them
  is false. Separate map tests do not supply the missing differential oracle.
- `codec-b-pi-decoder-drops-unrecognised-entry-types-without-a-record` and
  `codec-b-pi-encoder-can-return-a-shorter-array-than-it-was-given` are labelled
  `test-only` in the catalog, and the second has no production caller at all.
- `codec-b-block-identity-stamp-is-caller-writable-and-the-fingerprint-is-not-an-identity`
  and `codec-b-decoder-output-can-violate-the-projector-precondition` both prove
  the module's behaviour on a hand-built input; whether a production route can
  supply that input is an unresolved route question.

`F0` is not an open blocker. Workspace CI executes the Rust test targets, and
the config-reader integration check exercises a nondefault user budget. Test
execution, property constructibility, and oracle adequacy remain separate facts.

## Coverage checks to add

Each asserts a precondition that a **correct** implementation still satisfies, so
it fires without a defect present. Names are constants, globally unique, and never
constructed dynamically. Because this part has no `sometimes` record, none of these
duplicates an existing marker.

| Coverage check | Situation it witnesses | Why it is safe |
| --- | --- | --- |
| `CONFIG_RESOLUTION_CHANGED_A_SUPPLIED_LEAF` | A resolution in which an input leaf differed from the resolved leaf, whether by a clamp, a discard, or a tier drop | The ordinary shape of every clamping resolution, and clamping is the design. It records that the campaign observed a value being altered at all, not that a warning was owed |
| `CONFIG_RESOLUTION_EMITTED_AN_IGNORED_KEY_WARNING` | The tier merge emitted a warning for a supplied user-only key or a rejected weakening candidate (`config.rs:723-747`) | Legal rejection on a correct implementation. Pair with the supplied input and effective-value observation; an ignored-project-key warning does not exercise the user-tier budget parser |
| `CONFIG_PROJECT_TIER_CHANGED_A_RESOLVED_LEAF` | A project-tier value changed a leaf of `DaemonConfig` | Legal for the documented project-writable set, so it fires on correct operation. The precondition of the allow-list record, stated as a tier-provenance fact |
| `CONFIG_FILE_READ_SUCCEEDED_AND_PARSE_FAILED` | `read_bounded_config` returned `Ok` and `serde_json::from_str` returned `Err` on the same file (`config.rs:370-379`) | A legal input-domain fact. The tier contributes no value and produces a path-bearing warning; the marker does not assert a missing warning |
| `HISTORIAN_CHAIN_WAS_ASSEMBLED_FROM_TWO_CONFIG_KEYS` | The resolved `model_chain` drew from `historian.module_model` and `historian.module_fallback_models` in one resolution | Legal input and is the documented way to configure a chain. The independent precondition of the adjacent-only dedup record, without asserting the chain contained a repeat |
| `HISTORIAN_CHAIN_DEDUP_REMOVED_AN_ELEMENT` | `dedup_preserving_order` at `config.rs:950-953` shortened the assembled chain | Legal behavior of the retained regression contract. It witnesses deduplication, not the obsolete adjacent-only defect |
| `CACHE_TTL_PARSE_RETURNED_ERR_AND_THE_DEFAULT_WAS_SUBSTITUTED` | `scheduler_ttl_ms` (`scheduler.rs:771-773`) replaced a `CacheTtlParseError` with `DEFAULT_CACHE_TTL_MS` | Legal fallback behavior. It witnesses substitution, not the absence of a diagnostic |
| `CACHE_TTL_RESOLVED_TO_ZERO_MILLISECONDS` | A configured `cache_ttl` of `"0"` parsed to `Ok(0)` | Legal input-domain fact. Pair with positive prior and elapsed timestamps to exercise hard expiry; the marker does not assert a final pass decision |
| `BOUNDARY_BUDGET_DERIVED_FROM_A_NON_FINITE_OR_NON_POSITIVE_LIMIT` | The guard at `boundary.rs:340-342` returned `TRIGGER_BUDGET_MIN` because `context_limit` was non-finite or non-positive | Legal and is the guard's purpose. **This is the positive precondition that makes the totality record meaningful**, rather than asserting that the guard's absence would be a defect |
| `ESCALATION_BANDS_DERIVED_FROM_AN_OUT_OF_RANGE_THRESHOLD` | `escalation_bands` was called with a threshold that was `NaN`, negative, or above `90` | Legal input, because the function is total over `f64`. It records that the campaign reached the extremes rather than only the default `65` |
| `SELECTION_MERGED_TWO_CANDIDATE_DECISIONS_FOR_ONE_TARGET` | The merge chose between two candidate decisions naming one `target_id` | Legal and is exactly what "drop beats edit_marker" (`selection.rs:26-27`) describes. The precondition of the determinism record, stated as a merge-provenance fact rather than as an ordering violation |
| `REGION_HINT_INPUT_ALREADY_ENDED_WITH_THE_TRUNCATION_SENTINEL` | A diff value handed to `region_hint` (`selection.rs:558-571`) already ended with the literal `...[truncated]` on entry | An input-domain fact about harness-supplied content, legal to observe, and the benign producer is a file whose text legitimately ends that way. **This is the independent precondition of the bypass and it must not be paired with a marker meaning the clamp was skipped** |
| `DECODER_ACCEPTED_AN_ELEMENT_MATCHING_NO_NAMED_SHAPE` | A decode produced a message from an input element that matched no named arm | Legal today by design, because neither decoder has a rejection channel. It records the acceptance as an input-domain fact and does not claim the message was fabricated |
| `DECODER_PRODUCED_A_ZERO_BLOCK_MESSAGE_OCCUPYING_AN_ORDINAL` | A decoded message with zero blocks was assigned an ordinal and entered the sidecar | Legal today, and the same shape an authentic empty user turn produces. The precondition of the totality record, without asserting the two are indistinguishable |
| `PI_DECODE_INPUT_CARRIED_AN_UNRECOGNISED_ENTRY_TYPE` | An input entry whose `type` matched neither a message nor one of the three named opaque types | An input-domain fact, legal to observe. It records what arrived and not what was retained, so it fires on a correct implementation that retained the entry |
| `OPENCODE_DECODE_INPUT_CARRIED_AN_IMMUNE_PART_TYPE` | An input part whose type was in `{snapshot, patch, agent, retry}` | Legal, and the golden already supplies a `patch`, so it fires today. The preservation-direction precondition |
| `OPENCODE_ENCODE_RAN_WITH_A_NON_EMPTY_NATIVE_REMOVAL_SET` | `remove_unretained_native_parts` ran with at least one block removed | Legal and is the function's purpose. **Pairing it with the marker above is the composition the existing checks miss**, since `codec/mod.rs:216-252` exercises removal on a message with no immune parts |
| `DECODED_MESSAGE_CARRIED_MIXED_SYNTHETIC_AND_AUTHORED_PARTS` | One decoded OpenCode message whose parts included both a synthetic and an authored origin | Legal input. The precondition of the provenance record, stated without asserting the resulting classification was wrong |
| `DECODED_MID_CONTAINED_A_PROJECTOR_RESERVED_CHARACTER` | A decoder copied an `info.id`, `id` or `responseId` containing `#` verbatim into a mid | An input-domain fact about harness-supplied ids, legal at the decoder because the decoder has no such precondition. It does not assert that the projector rejected anything |
| `DECODER_OUTPUT_WAS_HANDED_DIRECTLY_TO_THE_PROJECTOR` | One campaign run composed a decode with `project_messages` on the same value | A structural fact about which two stages ran in sequence, true today with fully correct behaviour. **It is the marker that distinguishes a campaign that tested the composition from one that tested each half**, which is the whole gap the record names |
| `DECODED_ARRAY_CARRIED_A_NON_ZERO_MINIMUM_ABSOLUTE_ORDINAL` | The smallest `absolute_ordinal` in a decoded array was greater than zero, meaning a windowed session | Legal and is the producer's design per `module-wire.ts:1028-1031`. The precondition of the ordinal record, stated as a numbering-provenance fact rather than as a consumer error |
| `BLOCK_ALIGNED_VIA_A_STAMP_PRESENT_ON_INGRESS` | The `_eidnara_codec` stamp used by alignment was already on the block when it arrived, rather than written by `stamp_block_identity` during this decode | Legal today, because the pass-through path preserves `provider_extras` verbatim, and the benign producer is a replay of the module's own encoded output. **The independent precondition of the trust half, and it must not be paired with a marker meaning the stamp was forged** |
| `TWO_NATIVE_PARTS_IN_ONE_MESSAGE_SHARED_A_FINGERPRINT` | `decoded_block_fingerprint` returned the same value for two parts of one message | Legal today and contained, because the stamp disambiguates. Witnessing it is what shows the stamp is load-bearing for a case the fingerprint cannot handle |
| `INCREMENTAL_SIDECAR_REPLACE_FROM_CAME_FROM_A_CALLER_FILTER` | The `replace_from` passed to `decode_opencode_sidecar_incremental` was produced by `validated_native_prefix`'s filter at `lib.rs:12561` or the guard at `:12577` | Legal and is exactly the convention the callee depends on. **It records that the callee's safety is a caller property**, which is the record's claim, without inducing an out-of-range value |
| `TOOL_USE_UNIQUENESS_GUARD_RAN_WITH_DEBUG_ASSERTIONS_OFF` | `assert_unique_tool_use_ids` (`codec/opencode.rs:462-470`) was entered with `cfg!(debug_assertions) == false` | A build fact, legal and correct in a release artifact. Pairing it with the marker below is how the no-enforcement finding becomes checkable without inducing a duplicate id in production |
| `ENCODE_EMITTED_A_TOOL_PART_FROM_THE_FRESH_SHELL_ARM` | The `parts.push(render_tool_pair_as_part(block, result))` arm at `codec/opencode.rs:754` ran | Legal and deliberate per the comment at `:749-757`: the arm exists because neither half matched a native index. The precondition of the duplicate-id record |
| `ROUND_TRIP_INPUT_CONTAINED_A_MUTATED_BLOCK` | A round-trip case included a block that did **not** short-circuit at `codec/opencode.rs:763-765` or `codec/pi.rs:463-465` | Legal, and is the only condition under which the round-trip oracle carries information. **This is the marker that measures the record's real gap**, since the retained-raw path makes identity nearly automatic for unmutated input |
| `GOLDEN_COVERAGE_GATE_CLEARED_A_CLASS_VIA_MISSING_CAPTURE_CLASSES` | The filter at `codec/mod.rs:262-266` cleared a required class because it appeared in `missing_capture_classes` rather than in `coverage` | Legal by construction and is the mechanism being reported. It records which list satisfied the gate, not that the gate is wrong |
| `PI_ENCODE_INPUT_CARRIED_A_TOOLRESULT_META_WITH_NO_TOOLRESULT_BLOCK` | A message reaching `encode_pi` whose meta role was `toolResult` while its wire content held no `ToolResult` block | An input-domain fact the transform can legitimately produce by reducing a decoded tool-result message. The precondition of the shorter-array record, without asserting the array shortened |
| `COMMIT_CLUSTER_TRIGGER_CONTEXT_BUILT_FROM_HARDWIRED_CONSTANTS` | The `TriggerContext` uses fixed values at `lib.rs:5001-5002`, defined at `lib.rs:640-641` | A structural fact, not a failed config assertion. Pair it with a nondefault supplied value and a distinguishing workload or context observation |

### The one `reachable` record, checked against METHOD.md

**This part produced no `sometimes` record and no liveness record**, so there is no
existing marker to audit for the forbidden pairing and nothing here duplicates one.
The 27 records are 26 `safety` and one `reachability`.

`codec-b-declared-missing-capture-classes-are-never-decoded` uses `reachable`, and
**the semantics are correct as written.** METHOD.md distinguishes location coverage
from situation coverage, and the obligation here is location coverage in the strict
sense: two decode arms exist (`codec/opencode.rs:171-181` and
`codec/pi.rs:199-211`), both are named as required by the golden's own manifest,
and both are provably never entered by the suite that claims to cover them. There
is no separate operational state to reach beyond executing them, which is the
condition under which METHOD.md's second rule would force `sometimes`. The
catalog's header already reports the one qualification and declines to apply it: the
`subtask` half is location coverage over a path with no distinguishable outcome,
because deleting the arm would leave the part falling to `:194-204` and still
becoming an opaque block, so the golden would not move. The redacted-thinking half
is the load-bearing one. **That record supplies no marker constant**, which is the
same compliance gap 4e recorded for both of its `sometimes` records. Give it one of
the same shape as the table above, for example
`REDACTED_THINKING_DECODE_ARM_EXECUTED`, so the assertion stops being anonymous.

### Anti-patterns to avoid in this part specifically

Seven pairings are forbidden by METHOD.md's rule, and each is tempting here because
in this part the defect is almost always easier to name than its precondition.

- Do not pair `always(config_changes_are_reported)` with
  `sometimes(a_clamp_went_unreported)`. **Every clamp goes unreported today**, so
  the marker fires on the first resolution and proves nothing.
  `CONFIG_RESOLUTION_CHANGED_A_SUPPLIED_LEAF` and
  `CONFIG_RESOLUTION_EMITTED_AN_IGNORED_KEY_WARNING` are two independent legal
  facts whose conjunction is the asymmetry.
- Do not pair `always(region_hint_output_is_within_the_cap)` with
  `sometimes(the_sentinel_bypassed_the_clamp)`. That marker can fire only by
  producing the oversized hint. Assert
  `REGION_HINT_INPUT_ALREADY_ENDED_WITH_THE_TRUNCATION_SENTINEL` instead, which is
  a fact about harness-supplied input, and keep the `always` on the payload.
- Do not pair `always(no_duplicate_tool_use_ids_in_the_encoded_array)` with
  `sometimes(the_release_guard_missed_a_duplicate)`. Assert
  `TOOL_USE_UNIQUENESS_GUARD_RAN_WITH_DEBUG_ASSERTIONS_OFF` and
  `ENCODE_EMITTED_A_TOOL_PART_FROM_THE_FRESH_SHELL_ARM` instead, and keep the
  `always` on the returned `Vec`. The `sometimes` form would also be
  profile-dependent, which makes a silent marker indistinguishable from a debug run.
- Do not pair `always(every_input_entry_is_recoverable)` with
  `sometimes(pi_dropped_an_entry)`. Assert
  `PI_DECODE_INPUT_CARRIED_AN_UNRECOGNISED_ENTRY_TYPE` instead, which records what
  arrived rather than what was lost, and keep the `always` on the recoverability
  comparison. The drop marker can only fire by observing the defect.
- Do not pair `always(project_messages_accepts_decoder_output)` with
  `sometimes(the_projector_rejected_a_decoded_set)`. The second is the violation.
  Assert `DECODED_MID_CONTAINED_A_PROJECTOR_RESERVED_CHARACTER` and
  `DECODER_OUTPUT_WAS_HANDED_DIRECTLY_TO_THE_PROJECTOR`: one input-domain fact and
  one structural fact, both legal, whose conjunction is the composition gap.
- Do not pair `always(every_aligning_stamp_was_written_by_this_decode)` with
  `sometimes(a_forged_stamp_was_trusted)`. "Forged" is not observable from inside
  the decoder, which is the record's whole point.
  `BLOCK_ALIGNED_VIA_A_STAMP_PRESENT_ON_INGRESS` is the honest form, because it
  records the stamp's provenance without judging the writer's intent.
- Do not pair `always(the_golden_covers_every_required_class)` with
  `sometimes(a_required_class_was_uncovered)`. **Two required classes are uncovered
  today**, so the marker fires immediately and proves nothing.
  `GOLDEN_COVERAGE_GATE_CLEARED_A_CLASS_VIA_MISSING_CAPTURE_CLASSES` is a
  structural fact about which list satisfied `codec/mod.rs:262-266`, and is the
  honest form.

### Placement constraints on markers in this part

Six, and they differ from 4e's because this part's boundary is a returned value
rather than a served byte array.

1. **A marker on a `debug_assert!` line does not exist in a release artifact, and
   in 4f there is no release arm to put a counterpart in.** All three assertion
   sites are `debug_assert` (`codec/opencode.rs:251`, `:252`, `:466`) and
   **verified: no `cfg(not(debug_assertions))` exists anywhere in 4f**. A marker
   placed beside one inherits its `cfg`, so a silent campaign under `--release`
   would be indistinguishable from a passing one. Markers about profile-dependent
   behaviour must be unconditional and must record `cfg!(debug_assertions)` as
   data.
2. **A marker inside `assert_unique_tool_use_ids` cannot mean "the wire was
   checked".** In release the whole body vanishes, while
   `duplicate_tool_use_locations(messages)` at `:465` still runs and its result is
   discarded. **The honest placement is on that computed value**, not beside the
   assertion, because the computation survives the profile and the assertion does
   not.
3. **A marker meaning "these are the encoded wire bytes" must not sit inside
   `encode_opencode_impl`.** The guard is applied there rather than in the chunk
   API, so `lib.rs:12949`'s direct call to
   `encode_opencode_chunks_with_transition_state` bypasses it in **every** profile.
   A marker inside the impl would fire on the guarded path and stay silent on the
   unguarded one, which inverts the signal.
4. **A marker meaning "block identity was established by this decode" must sit in
   `stamp_block_identity` (`codec/sidecar.rs:177`), not at the alignment read.**
   `stamped_block_identity` (`:196-203`) returns `Some` for any three well-formed
   values under `_eidnara_codec` (`:131`) regardless of writer, so a marker at
   the read cannot distinguish a stamp this decode wrote from one that arrived on
   ingress.
5. **A marker meaning "the golden covered this class" is false as stated.**
   `assert_coverage_or_recorded_missing` (`codec/mod.rs:254-271`) is satisfied by a
   class appearing in `missing_capture_classes`, so a coverage marker must name the
   `coverage` array as its subject or it will be read as evidence about a class the
   fixture explicitly declares absent.
6. **A marker on the stamp or fingerprint path in `codec/sidecar.rs` fires only
   transitively.** The three direct tests (`:487-557`) call `match_block_metas` and
   `greedy_block_metas` only, so for the stamp and fingerprint the only reachers are
   `codec/opencode.rs:553-554`, `:742`, `:763` and `codec/pi.rs:303-304`, `:372`,
   `:463`, which means the two one-case goldens. Such a marker records the goldens'
   path, not direct exercise of the block-identity stamper.

## Leverage ranking, by cheapest valid oracle

Ranked by the cost of the cheapest oracle that yields a valid result, not by
records unblocked per capability. **Every item on this list is cheap, which is the
distinguishing fact about this part**, so the ranking turns on value rather than on
effort once `F0` is answered.

1. **`F0` is satisfied by workspace CI.** The nextest job at
   `.github/workflows/ci.yml:413-417` includes daemon library and integration
   targets. Retain those checks; no additional test-execution lane is required
   for the config properties.
2. **The `smart_drops` comparison remains an investigation lead.** Its flag
   defaults to false (`config.rs:128`) and both permitted tiers use the same
   parser (`config.rs:890-894`). Comparing disabled-feature output requires an
   independently defined age-only baseline and a workload. The historical lead
   is preserved without claiming that a flag flip alone proves byte equality.
3. **`F2`, configuration values at and beyond documented bounds. The widest
   capability per fixture.** In-memory config resolution makes **four active**
   records constructible:
   `dec-a-execute-threshold-lower-bound-is-documented-20-and-enforced-1`,
   `dec-a-memory-injection-budget-documented-range-has-no-implementing-code`,
   `dec-a-project-tier-can-write-leaves-outside-the-documented-allow-list`, and
   `dec-a-config-value-clamps-and-zero-rejection-are-invisible-to-the-caller`.
   The invalidated model-chain premise retains an existing
   regression check instead of an open campaign. The oracle in every case is the
   resolved struct plus the returned warning vector, both of which the merge path
   already materialises, so no new plumbing is needed. **The same fixture answers a
   documentation question the register raised**: eight of the thirteen `NOT FOUND`
   claims would be discharged by one schema-diff check plus one key-set diff check,
   and 4b already proposed the key-set check for its four keys
   (`../transform/existing-checks.md:571-574`). **This map proposes both.**
4. **`F1`, arbitrary input to each decoder. The cheapest single oracle in the
   part.** A `Vec<serde_json::Value>` and one call. It makes
   `codec-b-harness-decoders-accept-every-input-with-no-rejection-channel` valid
   and supplies half of `codec-b-decoder-output-can-violate-the-projector-precondition`
   and half of the fingerprint-collision record. **The reason it is not higher is
   that the property it checks is violated by design rather than under-evidenced**:
   both decoders return `DecodedHarnessMessages` with no error variant
   (`codec/opencode.rs:23-25`, `codec/pi.rs:19-21`), so a test written here
   documents a decision rather than catching a regression. That is still worth
   having, because the failure mode is a fabricated zero-block message that
   occupies an ordinal and is indistinguishable downstream from an authentic empty
   turn.
5. **`F5`, harness input carrying unknown or omitted types. One hand-built element
   per record, four records.** It makes
   `codec-b-pi-decoder-drops-unrecognised-entry-types-without-a-record`,
   `codec-b-opencode-hides-four-part-types-from-every-transform-decision`,
   `codec-b-provenance-recovery-on-decode-is-all-or-nothing-and-opencode-only` and
   `codec-b-declared-missing-capture-classes-are-never-decoded` valid, and it is
   the capability that turns
   `codec-b-round-trip-identity-is-claimed-in-one-direction-on-one-case-per-harness`
   from a near-tautology into an informative check. **Two of its members are
   already named by the code itself**: `subtask` and `redacted_thinking` are
   declared required and declared missing in the same fixture, so the work is adding
   one part and one entry to two generators, not designing a fault.
6. **`F4`, running the suite in release as well as debug. A build flag, and it buys
   less here than the same flag bought 4e.** One extra invocation,
   `cargo test -p daemon --lib --release`. It makes
   `codec-b-wire-level-tool-use-uniqueness-guard-has-no-release-behaviour` valid and
   completes `codec-b-incremental-sidecar-slice-panics-behind-a-debug-assert`.
   **What it establishes is three absences rather than a second behaviour**, because
   there is no `cfg(not(debug_assertions))` anywhere in 4f: `:466` enforces nothing
   while `:465` still computes and discards; `:252`'s violation is silent because
   `take` at `:265` saturates; and `:251`'s is not silent because the slice at
   `:258` panics on the same condition in every profile. It also compiles the one
   test currently gated `#[cfg(debug_assertions)]` at `:2077` out of existence,
   which is the sharper half of the finding: **whichever profile ships, that
   guard's shipped behaviour has no test.**
7. **`F6`, caller-supplied block identity. One forged `provider_extras` value, and
   it targets the file with zero tests.** It makes
   `codec-b-block-identity-stamp-is-caller-writable-and-the-fingerprint-is-not-an-identity`
   valid in both halves. It ranks here rather than higher for two reasons: the
   trust half proves the module's behaviour on a hand-built ingress message without
   establishing that a production route supplies one, and the collision half is
   contained today. **But it is the only capability that reaches the block
   identity in `codec/sidecar.rs`**: the file's three direct tests (`:487-557`) own
   alignment pairing, and nothing owns the stamp everything downstream keys on, so
   its per-line value is the highest on the list.
8. **`F3` has an existing regression check, not an open silent-failure defect.**
   `config.rs:2133-2182` checks malformed, unreadable, repeated, repaired, and
   missing tiers through `effective_with_warnings`. Its status is `unaudited`.
9. **The TTL fixture's Rust leg runs in workspace CI.**
   `cache_ttl_resolution_matches_shared_typescript_vectors`
   (`config.rs:1133-1160`) checks five frozen cases. It does not run TypeScript,
   and no literal reference to that fixture's name is found under `packages/`. The
   historical claim that only the TypeScript leg executes is false; a live
   cross-language comparison remains separate from the existing frozen-vector
   check. Status of the existing check: `unaudited`.

### Config oracle boundaries

The reader's comments tie several defaults to TypeScript (`config.rs:20-25`),
but matching a constant or a frozen fixture is not a live cross-language proof.
Use the explicit tier policy, value parser, warning kind, and actual consumer
route to define each config oracle. Do not infer a product-wide parity result
from the absence of a key in this reader.

## Records that need a product decision rather than a harness

This list retains source-catalog questions outside the bounded config refresh.
The explicit config and CI dispositions below supersede their historical
premises; unrelated questions are not new HEAD findings.

- **Which build profile does a particular distributed artifact use?** The CI
  payload smoke builds without `--release` (`.github/workflows/ci.yml:625-629`),
  but that does not determine every distributed artifact's profile. The release
  pipeline is the evidence source for that separate question.
- **Should a `debug_assert!` whose condition is independently enforced by the
  language in release be catalogued differently from one that is not?**
  `codec/opencode.rs:251` is re-checked by the slice at `:258`; `:252` is consumed
  by `take` at `:265`, which saturates. The two sit on adjacent lines with opposite
  release semantics. (needs human input)
- **Are `subtask` and `redacted_thinking` absent from the codec goldens because the
  generator cannot produce them, or because nobody has added a case?** The
  `missing_capture_classes` mechanism records the fact and not the reason.
  Unresolved, needs the intent behind `testdata/codec/gen-opencode-golden.ts` and
  `gen-pi-golden.ts`.
- **Is the encode-direction oracle at `codec/mod.rs:88` and `:211` intended to be
  self-referential, or was an independent expected output intended?** Each fixture
  carries a `projection_oracle` field that nothing deserializes, which suggests the
  latter. Unresolved.
- **The nine/thirteen count debate is historical.** The current parser census is
  25 registered keys across 22 parsed display rows, plus nine unparsed display
  rows in the comparison table. It is not a product-wide defect count.
- **`output_reserve` remains a cross-component lead.** It is absent from the
  consumed-key table (`config.rs:588-618`). Its behavior elsewhere is outside
  this reader audit; absence here does not establish a product defect.
- **Is `PARITY.md` a claim source for `daemon` at all?** It is titled "Pi to
  OpenCode: Intentional Divergences" and describes two TypeScript plugins. If its
  scope is TypeScript only, four register claims move from "contradicted" or "NOT
  FOUND" to "out of scope", **and the Rust codecs are left without a stated divergence
  contract**, which is a worse position: verified at `HEAD` that `codec/mod.rs:1`,
  `codec/opencode.rs:1`, `codec/pi.rs:1` and `codec/sidecar.rs:1` each carry a
  `//!` header describing round-trip and alignment behaviour, and none of the four
  states which divergences from the TypeScript plugins are intentional. (needs
  human input)
- **Do the four undocumented but effective keys belong in `CONFIGURATION.md` (source-catalog path, not present at HEAD)?**
  `memory.user_profile_budget_tokens`, `historian.module_model` with
  `module_fallback_models`, `historian.context_limit_tokens`, and
  `prompt_surface.guidance_override_text`. These keys are part of the consumed
  pointer table (`config.rs:588-618`), but the historical configuration document
  is absent here. Which user-facing document should own them? (needs human input)
- **The historical request to execute `release_contract_conformance.rs` is no
  longer a CI gap.** Workspace nextest selects integration targets
  (`.github/workflows/ci.yml:413-417`). Its ownership in a property catalog is
  separate from whether CI selects the target.
- **Does `caveman.rs` belong to 4e or 4f?** The scope map says 4e (`:590`), 4e's
  inventory counts its single test, and the brief assigns the file here. One test
  and 651 production lines are currently double-counted. (needs human input)
- **Which of 4b's five buckets hold the 9 tests 4f attributes and neither sibling
  did?** Without 4b's per-test bucket assignment the union of the three parts over
  `transform.rs` can only be bracketed at 253 to 262 of 280, and the orphan
  remainder at 18 to 27. Unresolved, needs 4b's enumeration.
- **The blanket absent-CI premise is retired.** The
  [current inventory](existing-checks.md#ci-execution) identifies workspace test
  selection. Execution profile, constructed case, and oracle adequacy are
  separate facts; every existing check remains `unaudited`.
