# Part 4f property catalog: decision units, configuration, and harness codecs

Config-derived records, their code references, and CI execution facts are
refreshed against `74044960ee91641dec95c8552f15282844a18b13`. Unrelated
decision-unit and codec investigations retain the source-catalog provenance
below; their discovery-time coordinates are not newly verified HEAD evidence.
An active record states an obligation to test, not that its historical defect
assessment has been reproduced in this tree.

Scope: sub-part 4f of `crates/daemon`, the decision layer every transform pass
consults plus the two harness codecs that own the bytes entering and leaving the
crate. `src/codec/` is 4,323 lines across four files, `src/selection.rs` is 3,365,
`src/boundary.rs` 3,053, `src/scheduler.rs` 1,449, `src/config.rs` 1,229,
`src/caveman.rs` 651, and `src/session_resolver.rs` 70. `src/wire.rs` (1,279)
is in scope where it bears on codec contracts, and `CONFIGURATION.md` (source-catalog path, not present at HEAD) (841) is
read as the documented contract rather than as evidence of behaviour. The decision
regions of
[../_lenses/scope-map-and-risk-ranking.md](../_lenses/scope-map-and-risk-ranking.md)
at `:607-649` fix the boundary.

One path correction to the task framing, because it changes what a reader greps
for: `sidecar.rs` is at `codec/sidecar.rs`, not `src/sidecar.rs`. It is the file
every other codec unit depends on for block identity, and it is the one file in
scope with no tests of its own.

Out-of-part files are cited rather than catalogued. `crates/memory-store/src/lib.rs`
owns the wire types the codecs produce (`:40-300`), the `lib.rs` call sites that
supply or consume these units are 4b, 4c and 4d code reading a 4f contract, and
`crates/daemon/src/healing.rs:10-28` defines five `SerializerProfile` variants
against two codecs, so the profile axis is larger than the codec axis and is left
to a later pass.

The source-catalog provenance is recorded in [../README.md](../../README.md).
Its `e447c927` and `76cd6f41` CI observations are historical, not the execution
contract for this tree. The config-derived records and execution inventory use
`74044960ee91641dec95c8552f15282844a18b13`; see
[existing-checks.md](existing-checks.md). The method contract is
[METHOD.md](../../METHOD.md).

### Reconstruction provenance

The initial reconstruction rebuilt this file from `_lenses/` after the working
tree was cleaned and the synthesized `catalog.md` was lost. It copied records
from `_lenses/lens-a-decision-units-and-config.md` (14 records,
`dec-a-` prefix) and `_lenses/lens-b-harness-codecs.md` (12 records, `codec-b-`
prefix), with two mechanical adjustments and no re-derivation: evidence links were
rewritten from the lens-relative `../evidence/` to the catalog-relative
`evidence/`, and field paragraphs were rewrapped to about 80 columns. Content
equality against the lens text was checked mechanically, token by token, after
rewrapping. `_lenses/lens-c1-claims-and-config.md` and
`_lenses/lens-c2-check-inventory.md` proposed no records; they supply the claims
register, the configuration contract table, the release-behaviour divergence and
the check inventory this header cites. `portfolio-evaluation.md` records the
independent evaluation and the four refinements it accepted.

That reconstruction account is historical. The current config-derived records
and evidence include the corrections identified in this refresh.

**The grouping below is mine.** The lens files produced two flat record lists, and
neither proposed group headings, so the seven groups are a synthesis choice: they
cut by mechanism, meaning the thing a single test fixture would have to build,
rather than by lens or by file. Index order follows group order.

### One semantics finding, reported and not applied

Lens B produced the part's only `reachable` record,
[codec-b-declared-missing-capture-classes-are-never-decoded](#codec-b-declared-missing-capture-classes-are-never-decoded),
and per METHOD.md rules 3 and 6 it is reproduced verbatim with the finding
recorded here rather than resolved in the record.

**The semantics are correct as written.** METHOD.md distinguishes location
coverage from situation coverage: `reachable` is "a specific code point or path
should be executed", `sometimes` is "a meaningful state must occur at least once
per campaign". The obligation here is location coverage in the strict sense. Two
decode arms exist, `codec/opencode.rs:171-181` for `subtask` and
`codec/pi.rs:199-211` for redacted thinking; both are named as required classes by
the golden's own manifest; and both are provably never entered by the suite that
claims to cover them, because listing a class in `missing_capture_classes` clears
the coverage assertion at `codec/mod.rs:262-266`. The record asks for those two
arms to execute. There is no separate operational state to reach beyond executing
them, which is the condition under which METHOD.md's second rule would force
`sometimes` instead.

One qualification a reviewer should carry. For the `subtask` arm the record's own
`Impact:` line observes that deleting the arm would not move the golden, because
the part would fall through to `:194-204` and still become an opaque block. So the
`subtask` half is location coverage over a path with no distinguishable outcome,
which is the weakest form of the check; the redacted-thinking half is the load
bearing one, since `:199-211` produces `BlockKind::RedactedReasoning` while the
non-redacted branch at `:212-217` produces `BlockKind::Reasoning` with a signature,
and the two round-trip through different encoder arms. A future pass may want to
split the record on that asymmetry. That is a strengthening, not a correction, and
it is not applied here.

## Provenance in this repository

- Source: the host repository at `eb6da6109`, catalog `part-4f-decisions`. The records,
  their evidence files, and the check inventory, fault map, and portfolio
  evaluation are that catalog's text under this repository's crate, module,
  table, and identifier names. Nothing generates or validates this file.
- The header, scope statement, line counts, region maps, identifiers, and
  commits above and below this section are the source catalog's: they
  describe the host repository's tree at `eb6da6109`, not this one.
- Line citations are the source catalog's coordinates and are not verified
  against this tree. An automated check over citations written as a
  repository-root path (`crates/...`, `packages/...`, `docs/...`,
  `.github/...`, `release/...`), as `ci.yml:NNN`, as `CONFIGURATION.md:NNN`,
  as `tests/sqlite_runtime.rs:NNN`, or as `../commons/...` (source-catalog path, not present at HEAD) marks every
  citation whose file is absent here as `(source-catalog path, not present at
  HEAD)` and every citation past the current file's length as
  `(source-catalog line, not present at HEAD)`. The check verifies path
  existence and line range only; a citation without a mark is still
  unverified, and a campaign re-verifies it before instrumenting it. Bare
  file names (`lib.rs:NNN`) are not checked. Test names are the stable
  anchors. Citations into `packages/plugin`, `packages/pi-plugin`,
  `packages/cli`, `packages/e2e-tests`, and `CONFIGURATION.md` (source-catalog path, not present at HEAD) name files
  this repository does not carry.
- Every `Type`, `Reachability`, `Status`, `Exercised`, `Check`, and
  `Confidence` value uses METHOD's enumerated form; the reconciliation moved
  each field's note behind a spaced hyphen and changed no note's content,
  except that two `Confidence` values that named two levels now carry the
  lower level with the split stated in the note.
- The `drive-fault` Cargo feature and its eight tests are gone from
  `crates/daemon` (KTD14); prose below that treats them as live describes the
  source tree.

## What this part is about

### Historical discovery framing

The discovery narrative and route counts in this subsection describe the source
catalog. They are not current product-wide defect or coverage counts. Current
config behavior is recorded in the configuration table and records below; the
current execution inventory follows this historical subsection.

4f is where the crate decides things and where it talks to the outside world. The
decision units are pure by their own headers and that claim survives inspection;
the defects are almost all on the two boundaries, the configuration contract above
and the harness wire below. Six facts frame the records.

**The configuration contract is the part's largest defect surface, and the
headline that number belongs to is route-scoped rather than product-wide.** The
table below has 31 leaves. **This section previously read "13 documented keys
either do nothing here or disagree with their own documentation" and treated that
as a product defect count. An independent evaluation refuted the framing and the
membership, and the correction is applied below rather than footnoted, because the
Rust-first scope decision makes the distinction decision-relevant rather than
pedantic.** Two things were wrong. First, `protected_tags` and
`clear_reasoning_age` are not inert: both are carried on the transform *request*
and consumed as Rust request fields, so "does nothing here" is false for them.
`rust-mode-transform.ts:1355` sends `protected_tags` and `:1398` sends
`clear_reasoning_age` on one call path, `:2031` and `:2014` send the same pair on
another, and `transform.rs:682-684` and `:693-697` declare them as
`#[serde(default = ...)]` fields on the request struct. Second, three keys were
filed as "absent everywhere", which is true of `crates/daemon/src` and false of
the workspace: `historian_timeout_ms` is read at `pi-plugin/src/index.ts:676` and
threaded through `:1297`, `:1313`, `:1332`; `history_budget_percentage` at
`pi-plugin/src/index.ts:693` and `:1229`; `output_reserve` at
`pi-plugin/src/config/index.ts:427` and `:600`. "Absent" was a statement about one
crate wearing the clothes of a statement about the product.

The replacement is a **route-aware matrix**. The axis that matters is *which
channel carries the key to the Rust reader*, because that is what the migration
has to preserve:

| Route | Count | Members | What a defect here means |
| --- | --- | --- | --- |
| **Parsed by the Rust config reader** | 24 | every row of the table below whose "Takes effect here?" is `Yes`, including the four undocumented-but-effective leaves and the deprecated `memory.budget_tokens` | `config.rs` is the authority. A bound or default that disagrees with `CONFIGURATION.md` (source-catalog path, not present at HEAD) is a real divergence in this crate, and 7 of these are divergent: `execute_threshold_percentage` scalar, `memory.injection_budget_tokens`, `memory.auto_search.min_prompt_chars`, `caveman_text_compression.min_chars`, the `review-user-memories` schedule, `historian.model` with `fallback_models`, and `cache_ttl` |
| **Request-supplied** | 2 | `protected_tags`, `clear_reasoning_age` | Not inert. The value arrives per pass on the transform request (`transform.rs:682-697`) from the TypeScript sender (`rust-mode-transform.ts:1355`, `:1398`, `:2014`, `:2031`), and `config.rs` correctly does not parse it. **These are the keys the Rust-first migration must preserve**, because the sender is the thing being replaced. A hardwired Rust constant standing in for either — `DEFAULT_PROTECTED_TAGS` at `lib.rs:603` — is a fallback for a *missing request field*, not a config gap |
| **TypeScript-only** | 6 | `execute_threshold_percentage` object form, `execute_threshold_tokens`, `commit_cluster_trigger.enabled`, `commit_cluster_trigger.min_clusters`, `historian_timeout_ms`, `history_budget_percentage`, `output_reserve` (7 leaf names, 6 documented keys, since the object form shares a key with the scalar) | The key is honoured, in TypeScript, by code the Rust reader never consults. Verified per key: the `commit_cluster_trigger` pair is parsed by `plugin/src/config/schema/eidnara.ts` and consumed by `pi-plugin/src/context-handler.ts`, while Rust hardwires `DEFAULT_COMMIT_CLUSTER_TRIGGER_ENABLED` and `DEFAULT_MIN_COMMIT_CLUSTERS` (`lib.rs:605`, `:607`) at `:4962-4963` and never reads either. **This is the class the Rust-first decision actually threatens**: a key that works today only because a TypeScript component is in the path |
| **Truly absent from both** | 0 | none | Checked per key. Every leaf in the documented table has a consumer somewhere in the workspace, in Rust, in TypeScript, or on the request. The pre-disposition "absent everywhere: 3" bucket is empty once the search leaves `crates/daemon/src` |

So the corrected headline is: **7 divergences on the Rust-parsed route, 6
documented keys honoured only in TypeScript, 2 keys carried on the request rather
than in config, and nothing absent.** The old "13" was 7 real divergences plus 6
keys misfiled as inert, of which 2 were request-supplied and 4 were
TypeScript-only. The number was not inflated by carelessness — every cited line
was correct — but it summed across three routes that a migration treats
differently, which is the shape of error that matters when the decision on the
table is which route survives. The per-route counts above supersede both the old
13 and the sibling lens's 9.

A further 9 documented keys have zero occurrences in `crates/daemon/src` **and**
describe behaviour outside the module, listed at the end of this section; those are
correctly out of scope and unaffected.

**13 of 30 claims have no implementing code.** The claims register holds 30
entries: 13 are consistent with the code at `HEAD`, 13 are `NOT FOUND`, 3 are
contradicted, and 1 is partial. **Eight of the thirteen `NOT FOUND` claims are
cross-implementation parity claims whose oracle lives in TypeScript** and is never
read from this crate. That is the register's dominant shape: the Rust module states
its obligations against a twin it does not consult.

**No harness decoder has a rejection channel.** Both decoders return
`DecodedHarnessMessages` with no error type at all (`codec/opencode.rs:23-25`,
`codec/pi.rs:19-21`), so totality is free and worthless, and the interesting
question inverts from "does it reject" to "what does it silently accept and
silently discard". The two harnesses then answer that question in opposite ways.
The Pi decoder drops an entry that is neither a message nor one of three named
opaque types, from `decoded` and from the sidecar alike, retaining nothing
(`codec/pi.rs:41-50`, `:661-669`, `:681-686`). OpenCode preserves unknowns as
opaque with the raw part cloned (`codec/opencode.rs:194-204`) but omits four named
part types from `content` (`:193`), so they exist for re-encode and are invisible
to every transform decision. The wire layer's own contract requires the pass-through
path stay `Value`-level "so harmless future wire fields are not silently dropped"
(`wire.rs:19-21`), and one of the two harness codecs violates that spirit
outright.

**There is a release-behaviour divergence, and the dangerous line is not the
obvious one.** Three `debug_assert!` sites exist in the 4f production halves, all
in `codec/opencode.rs`. The out-of-range index at `:251` is **re-checked by the
slice at `:258`**, so a violation fails loudly in every profile and the assertion
only buys a better message. **`:252` is the silent one**: same function, adjacent
line, no release equivalent, and a later `take` at `:265` saturates, so a violated
precondition becomes a silently truncated sidecar. **Neither is tested.** The third
site, `:466` inside `assert_unique_tool_use_ids`, **enforces nothing in release**:
unlike 4e's two-armed belt it has one arm, so the function becomes a no-op while
`duplicate_tool_use_locations` still runs and its result is discarded, and its only
test is debug-gated at `:2077`. Verified: no `cfg(not(debug_assertions))` exists
anywhere in 4f, so no release-arm counterpart exists for any of them.

**One totality defect was found, and this line previously said none was.** The
claim mattered because of what it was compared against: Part 3's analogue is an
infinite input producing a NaN that broke a documented invariant, and the sibling
crate carries **three** such defects. Saying 4f had zero was therefore a
substantive result about this crate, not a throwaway, which is exactly why it had
to be checked rather than inherited. Most of it survives. Every `f64` entry point
into `derive_trigger_budget` (`boundary.rs:339-345`),
`derive_protected_tail_token_target`'s own context fields (`:363-372`), and
`clamp_percentage` (`:926-931`) guards `is_finite` first, and where a NaN could
still arrive it is absorbed rather than propagated, because `f64::max` and
`f64::min` return the non-NaN operand; both expressions were executed to confirm.
**The exception is `BoundaryContext::trigger_budget`, a caller-supplied
`Option<f64>` that is read through `unwrap_or_else` at `:377-379` and again at
`:756-761` with no validation on the `Some` arm.** A `Some(f64::NAN)` therefore
reaches `:802`'s `tail_size_bar: trigger_budget * TAIL_SIZE_TRIGGER_MULTIPLIER`,
which is a bare multiply with no `max` or `min` to absorb it, and the NaN lands in
`TriggerProgress` — a struct whose own doc comment (`:322-324`) says it is
"Surfaced through the transform response's historian diagnostics so a stalled rig
drive is diagnosable per pass". It is carried out at `lib.rs:4982` and divided at
`:5002`. So the defect class is present in shape, unreachable on the guarded
derivations, and **reachable on the one unguarded passthrough**, which is a
different sentence from the one this section used to carry. Group C's four guards
still hold, and they are still worth recording for the reason given there. What
changes is that the fifth thing in that neighbourhood is a defect and is now
recorded as one. Production passes `None` (`lib.rs:4957`) and the only `Some` sites
are `lib.rs:16495` and `:16760`, both tests, so the reachability is latent rather
than default-production, and the split record says so. The nearest *reachable*
hazard remains a different one: a `cache_ttl` of `"0"` parses to 0 ms and forces
execution every pass, which no documentation mentions.

### Current config and execution evidence

The [file-local inventory](existing-checks.md#file-local-census) counts 180 tests
across the eleven scope files, including 43 in `config.rs` and three in
`codec/sidecar.rs`. This is not a transitive attribution over `transform.rs` or
`lib.rs`; the old 192-test figure measures a different, historical population.

Workspace nextest includes daemon library and integration targets
(`.github/workflows/ci.yml:413-417`). The budget-reader integration check loads
a 500-token user config, asserts row trimming, and checks revision stability
after an excluded-row edit
(`crates/daemon/tests/transform_canonical_memory.rs:448-539`). The absence of
CI or integration execution is therefore not an open config-layer gap. Every
check remains `unaudited`; source inclusion does not establish oracle adequacy.

## Decision unit table

Ten units carry a genuine decision. "Pure?" is a verdict against the file's own
purity claim, not a restatement of it.

| Unit | Decides | Inputs | Output domain | Pure? |
| --- | --- | --- | --- | --- |
| `selection::select_reductions_with_outcome` (`selection.rs:1119-1385`) | which tail blocks to reduce and with which payload | `items`, `frozen_keys`, `SelectionContext`, `SelectionConfig` | `Vec<ReductionDecision>` sorted by unique `target_id`, kinds `drop`/`skeleton`/`edit_marker`; empty on Defer | Yes. No clock, no store, no statics. Iterates `HashMap`s internally but every result is a set or is totally sorted before it escapes; see `dec-a-selection-decision-order-is-total-under-hashmap-iteration` |
| `selection::region_hint` (`:558-571`) | how far to clamp a superseded diff value | one `&str` | `String`, normally `<=40` UTF-16 units plus the sentinel | Yes, and idempotent. But the idempotence guard is also a bypass: see `dec-a-region-hint-clamp-bypassed-by-sentinel-suffix` |
| `selection::skeleton_payload` (`:648-694`) / `canonical_json` (`:597-619`) | the frozen call-skeleton bytes | one `serde_json::Value` | canonical `String` with sorted keys | Yes. Key sort makes bytes independent of map order |
| `selection::resolve_tool_tier` (`:948-958`) | emergency drop tier of a tool | tool name | `{1,2,3}`, total via the `else` arm | Yes |
| `selection::select_emergency` (`:995-1084`) | which arcs to evict under force pressure | active arcs, ctx, floor tokens | `HashSet<String>` of arc ids | Yes. Guards non-finite ceiling and usage at `:1001-1009` and refuses sub-`2000`-token reclaim at `:1018` |
| `boundary::resolve_protected_tail_boundary` (`:410-416`) | where the compactable/protected split sits | messages, `BoundaryContext` | `BoundaryResolution` with ordinals and a reason string | Yes. `HashMap` at `:1001` is lookup-only, built from a `BTreeMap` at `:1027` |
| `boundary::check_compartment_trigger*` (`:751-882`) | whether the historian fires, and why | messages, `TriggerContext`, token index, estimator | `TriggerDecision`, `reason` in a closed 4-variant enum | Yes given the caller-supplied estimator. `tokenizer` determinism is Part 3's |
| `boundary::derive_trigger_budget` (`:338-346`) + `derive_protected_tail_token_target` (`:362-401`) | the size-trigger budget and the protected-tail token target | `context_limit`, `execute_threshold_percentage`, usage, optional budget | budget always in `[5000, 50000]`; `n` always `>= 1` | Yes, and total **over the three fields it validates**: see `dec-a-boundary-budget-derivation-is-total-over-non-finite-input`. **Not total over `ctx.trigger_budget`**, which is read at `:377-379` with no `is_finite` gate: `n` stays finite because `f64::min` absorbs the NaN at `:383`, but the raw value is stored at `:399` and reaches `TriggerProgress.tail_size_bar` at `:802`. See `dec-a-caller-supplied-trigger-budget-is-the-one-unvalidated-float-and-reaches-a-diagnostic` |
| `scheduler::decide` (`:706-800`) | the pass class, band, latch, and overflow verdict | `SchedulerInputs` (config, session, usage, `now_ms`, latch, error text) | `SchedulerOutcome`; `PassDecision` in a closed 4-variant enum | Yes. `now_ms` is a parameter, not a clock read. Regexes live behind `OnceLock` but are constant |
| `scheduler::parse_cache_ttl` (`:385-419`) + `escalation_bands` (`:187-198`) | the idle TTL in ms, and the force/emergency bands | a TTL string; the effective threshold | `Result<u64, CacheTtlParseError>`; bands with force in `[85, 92]`, emergency fixed at `95` | Yes and total: `dec-a-cache-ttl-parse-is-total-over-arbitrary-strings`, `dec-a-escalation-bands-stay-ordered-for-every-threshold` |

`config.rs` itself is not pure: `ConfigCache` reads the filesystem and caches on
path and mtime, except that failures bypass the cache fast path
(`config.rs:362-393`). Its two derived helpers are pure:
`derive_historian_chunk_tokens` (`config.rs:39-46`) and
`resolve_cache_ttl_with_provenance` (`config.rs:151-207`).

`session_resolver.rs` holds no decision worth a record.
`MissingSessionResolver::resolve_session` (`:44-52`) returns `Ok(None)`
unconditionally and its one test (`:57-67`) already pins that.

### Purity verdict against the headers

`boundary.rs:5-9` claims "no I/O, wall clock, store access, or ambient cache
state here: the same inputs always produce the same boundary and trigger
decision." Confirmed for the boundary and trigger functions. The only statics are
eight `OnceLock<Regex>` caches (`:1930-1970`) holding compile-time-constant
patterns, which do not vary with input.

`selection.rs:4-7` claims "PURE, DETERMINISTIC ... same (items, frozen_keys, ctx,
cfg) -> same decisions". Confirmed structurally, with one unstated precondition
recorded below.

`scheduler.rs:1-4` claims "Pure state-transition functions; durable state enters
as parameters and exits in return values." Confirmed.

The prior-part finding that this repository uses ordered maps and explicit sorts
holds here. Both hash-map-iterating loops I found
(`selection.rs:1305` and `:1397-1405`) are made order-insensitive downstream, and
`boundary.rs`'s only `HashMap` is keyed lookup built from a `BTreeMap`. I found
no process-local timezone or locale read in 4f scope.

## Configuration contract table (key | code default | documented default | enforced bound | takes effect here?)

Thirty-one rows. Selection rule: every key `config.rs` parses, plus every
documented key whose description names behaviour `daemon` performs. "Enforced
bound" is the bound the Rust code actually applies, which is the column the
sibling table did not carry. "Takes effect here?" means the parsed value reaches
a decision inside `daemon`.

The Rust config citations in this table are verified against the classified
reader. Code columns name their source file explicitly; shorthand references
in the documented-default column refer to the historical `CONFIGURATION.md`,
which is absent at HEAD. "Undocumented" and documentation divergences refer to
that source-catalog contract, not a census of all current user-facing docs.

| Key | Code default | Documented default | Enforced bound | Takes effect here? |
| --- | --- | --- | --- | --- |
| `execute_threshold_percentage` (scalar) | `65.0` (`config.rs:21`, `config.rs:115`) | `65`, range `20-90` (`CONFIGURATION.md:167` (source-catalog path, not present at HEAD)) | `clamp(1.0, 90.0)` (`config.rs:750-752`); project tier may only raise (`config.rs:682-686`, `config.rs:734-744`) | Yes. **Divergent**: documented lower bound `20`, enforced `1` |
| `execute_threshold_percentage` (object form) | not parsed | documented, example at `:791` | none | **No.** `number_at` (`config.rs:963-968`) returns `None` for an object; 4b's `sel-per-model-and-token-thresholds-inert-in-module` |
| `execute_threshold_tokens` | not parsed | documented (`:168`, `:319-338`), doc claims clamp to `90% x context_limit` | none in the config reader | Not through this reader: the adapter sets token thresholds to `None` (`transform.rs:5447-5454`). The scheduler does implement a token cap when supplied directly (`scheduler.rs:419-427`); parser absence is not implementation absence |
| `compaction.enabled` | `true` (`config.rs:116`) | `true` (`:172`) | none; user-only (`config.rs:663-672`) | Yes, user tier (`config.rs:812-816`); project warns (`config.rs:729-732`) |
| `memory.enabled` | `true` (`config.rs:117`) | `true` (`:589`) | none | Yes, both tiers (`config.rs:817-821`, `config.rs:717-733`) |
| `memory.injection_budget_tokens` | `4000.0` (`config.rs:23`, `config.rs:123`) | `4000`, range `500-20000` (`:591`) | `.max(1.0)` only (`config.rs:847-851`) | Yes, user tier only: `UserOnly` and `privileged()` (`config.rs:627-693`); project warns (`config.rs:729-732`). **Divergent**: neither documented bound exists |
| `memory.budget_tokens` (deprecated) | falls back to the same field (`config.rs:852-864`) | absent from the documented table | `.max(1.0)` (`config.rs:856`) | Yes, user-only and privileged (`config.rs:627-693`), with a deprecation warning (`config.rs:858-863`); project warns and ignores (`config.rs:729-732`) |
| `memory.user_profile_budget_tokens` | `4000.0` (`config.rs:25`, `config.rs:124`) | **undocumented** | `.max(1.0)` (`config.rs:865-869`) | Yes, user-only (`config.rs:666-672`); project warns (`config.rs:729-732`) |
| `memory.auto_promote` | `true` (`config.rs:120`) | `true` (`:592`) | none | Yes, both tiers (`config.rs:870-874`, `config.rs:717-733`) |
| `memory.auto_search.enabled` | `true` (`config.rs:58`) | `true` (`:682`) | none | Yes, both tiers (`config.rs:822-826`, `config.rs:717-733`) |
| `memory.auto_search.score_threshold` | `0.6` (`config.rs:34`) | `0.6`, prose range `0.3-0.95` (`:683`, `:706`) | `clamp(0.3, 0.95)`, silent (`config.rs:827-831`) | Yes. Bound matches the prose; the clamp is invisible to the caller |
| `memory.auto_search.min_prompt_chars` | `20` (`config.rs:35`) | `20`, **no range documented** (`:684`, `:707`) | `clamp(5, 500)` (`config.rs:832-836`); a `0` is silently discarded by `positive_usize_at` (`config.rs:955-961`) | Yes. **Divergent**: an undocumented bound and an undocumented discard |
| `caveman_text_compression.enabled` | `false` (`config.rs:74`) | `false` (`:724`) | none | Yes, both tiers (`config.rs:837-841`, `config.rs:717-733`) |
| `caveman_text_compression.min_chars` | `500` (`config.rs:37`, `config.rs:75`) | `500`, **no range documented** (`:725`) | `clamp(100, 10_000)` (`config.rs:842-846`); a `0` discarded (`config.rs:955-961`) | Yes. **Divergent**: undocumented bound |
| `smart_drops` | `false` (`config.rs:128`) | `false` (`:752`) | none | Yes, both tiers (`config.rs:890-894`, `config.rs:717-733`) |
| `dreamer.inject_docs` | `true` (`config.rs:125`) | `true` (`:501`) | none | Yes, user-only and privileged (`config.rs:627-693`, read at `config.rs:895-899`); project warns (`config.rs:729-732`) |
| `temporal_awareness` | `true` (`config.rs:126`) | `true` (`:650`) | none | Yes, both tiers (`config.rs:900-904`, `config.rs:717-733`) |
| `dreamer.tasks.review-user-memories.schedule`, legacy `user_memories.enabled` | privacy gate defaults `false` (`config.rs:121`) | task default schedule `0 3 * * *`, i.e. on (`:527`) | none; a non-empty trimmed string reads as consent (`config.rs:875-879`); the schedule is `UserOnly` (`config.rs:667-672`), the flag is `ProjectRaiseOnly` and a project may only close the gate (`config.rs:687-692`) | Yes as a presence test. **Divergent**: module default is closed, documented default is scheduled |
| `historian.model`, `historian.fallback_models` | empty chain (`config.rs:114`) | documented with **no user-only marker** (`:448-449`) | `dedup_preserving_order` (`config.rs:750-753`, `config.rs:950-953`) | Yes, user-only and privileged (`config.rs:627-693`, read at `config.rs:788-806`); project warns (`config.rs:729-732`) |
| `historian.module_model`, `historian.module_fallback_models` | absent | **undocumented** | `dedup_preserving_order` (`config.rs:753`, `config.rs:950-953`) | Yes, user-only (`config.rs:659-672`); a non-empty module model selects the module chain instead of the plugin chain (`config.rs:774-806`) |
| `historian.context_limit_tokens` | `128_000` (`config.rs:32`, `config.rs:122`) | **undocumented** | `> 0` via `positive_usize_at` (`config.rs:955-961`), applied at `config.rs:885-889` | Yes, user-only (`config.rs:668-672`); project warns (`config.rs:729-732`) |
| `cache_ttl` (string or object) | `"5m"` (`config.rs:129`) | `"5m"` (`:163`), **no user-only marker** | parse is total; invalid falls back to `DEFAULT_CACHE_TTL_MS` (`scheduler.rs:771-773`); `"never"` maps to `u64::MAX` (`scheduler.rs:365-368`) | Yes, user-only and privileged (`config.rs:627-693`, read at `config.rs:924-945`); project warns (`config.rs:729-732`); the TypeScript strip removes it from project config (`project-security.ts:386-391`). `"0"` produces zero ms: hard expiry needs a positive prior timestamp and positive elapsed time (`scheduler.rs:400-407`), and later scheduler gates still apply (`scheduler.rs:689-732`) |
| `prompt_surface.guidance_override_path` | `None` (`config.rs:127`) | documented, user-only (`:75`, `:80-88`) | must be a readable section with exactly one marker (documented at `CONFIGURATION.md:88`) | Yes, from the user tier after merging (`config.rs:278-279`, `config.rs:408-490`); project warns (`config.rs:729-732`) |
| `prompt_surface.guidance_override_text` | `None` (`config.rs:127`) | **undocumented** | exactly one guidance marker (`config.rs:905-920`) | Yes, user-only (`config.rs:670-672`), but a configured path resets it to `None` first (`config.rs:424`); project warns (`config.rs:729-732`) |
| `commit_cluster_trigger.enabled` | not parsed | `true` (`:237`) | none from config | The daemon supplies constant `true` (`lib.rs:640`, `lib.rs:5001`). TypeScript routing in the original catalog is historical, not verified by this reader audit |
| `commit_cluster_trigger.min_clusters` | not parsed | `3`, **minimum `1`** (`:232`, `:238`) | none from config | The daemon supplies constant `3` (`lib.rs:641`, `lib.rs:5002`). The boundary consumes it at `boundary.rs:814-819` |
| `protected_tags` | not parsed by `config.rs` | `20`, range `1-100` (`:165`) | no config-reader bound; the request default is `20` (`transform.rs:797-799`) | It has a request field (`transform.rs:629-631`), so absence from config does not mean the value cannot arrive |
| `clear_reasoning_age` | not parsed by `config.rs` | `50` (`:169`) | no config-reader bound | It has a request field (`transform.rs:640-644`) and a default helper (`transform.rs:765-767`); absence from config is not product-wide absence |
| `historian_timeout_ms` | not parsed | `300_000` (`:170`) | none from this reader | Not in the consumed-key table (`config.rs:588-618`). The historical TypeScript-consumer lead is outside this reader audit |
| `history_budget_percentage` | not parsed | `0.15`, range `0.05-0.5` (`:171`) | none from this reader | Not in the consumed-key table (`config.rs:588-618`). No product-wide absence claim is made |
| `output_reserve` | not parsed | automatic; `0` disables (`:164`, `:308-315`) | none from this reader | Not in the consumed-key table (`config.rs:588-618`). Its historical cross-component routing lead is not a config-reader implementation claim |

The table has 31 display rows, not 31 distinct consumed keys. The two model
families and the schedule/legacy-flag pair each combine two keys in one row.

### Totals

The 22 parsed display rows represent all 25 registered keys
(`config.rs:560-586`). Nine display rows describe a shape or key this reader
does not parse. That is a parser census, not a product-wide defect count.

| Reader classification | Keys | Evidence |
| --- | --- | --- |
| User-only | 14 | `config.rs:659-672` |
| Project-allowed | 9 | `config.rs:673-681` |
| Project-raise-only | 2 | `config.rs:682-692` |
| Total registered | 25 | `config.rs:560-586`, `config.rs:588-618` |
| Privileged subset | 12 | `config.rs:627-640`; overlaps the tier classes above |

The source-catalog nine/thirteen-defect debate and 24-key route matrix are
historical framing, preserved in the
[portfolio evaluation](portfolio-evaluation.md#historical-evaluation). They do
not establish current product routing or current documentation coverage. The
records below state which historical contractual questions remain open without
changing runtime policy.

## Index

| Slug | Type | Confidence |
| --- | --- | --- |
| [dec-a-execute-threshold-lower-bound-is-documented-20-and-enforced-1](#dec-a-execute-threshold-lower-bound-is-documented-20-and-enforced-1) | safety | high |
| [dec-a-memory-injection-budget-documented-range-has-no-implementing-code](#dec-a-memory-injection-budget-documented-range-has-no-implementing-code) | safety | high |
| [dec-a-commit-cluster-trigger-config-is-inert-in-this-crate](#dec-a-commit-cluster-trigger-config-is-inert-in-this-crate) | safety | high |
| [dec-a-project-tier-can-write-leaves-outside-the-documented-allow-list](#dec-a-project-tier-can-write-leaves-outside-the-documented-allow-list) | safety | high |
| [dec-a-config-value-clamps-and-zero-rejection-are-invisible-to-the-caller](#dec-a-config-value-clamps-and-zero-rejection-are-invisible-to-the-caller) | safety | high |
| [dec-a-malformed-config-silently-resolves-to-defaults-and-stops-the-historian](#dec-a-malformed-config-silently-resolves-to-defaults-and-stops-the-historian) | safety | high |
| [dec-a-model-key-lookup-walk-has-two-implementations-that-disagree](#dec-a-model-key-lookup-walk-has-two-implementations-that-disagree) | safety | high |
| [dec-a-model-chain-dedup-is-adjacent-only](#dec-a-model-chain-dedup-is-adjacent-only) | safety | high |
| [dec-a-cache-ttl-parse-is-total-over-arbitrary-strings](#dec-a-cache-ttl-parse-is-total-over-arbitrary-strings) | safety | high |
| [dec-a-boundary-budget-derivation-is-total-over-non-finite-input](#dec-a-boundary-budget-derivation-is-total-over-non-finite-input) | safety | high |
| [dec-a-caller-supplied-trigger-budget-is-the-one-unvalidated-float-and-reaches-a-diagnostic](#dec-a-caller-supplied-trigger-budget-is-the-one-unvalidated-float-and-reaches-a-diagnostic) | safety | high |
| [dec-a-derive-historian-chunk-tokens-is-total-at-both-integer-extremes](#dec-a-derive-historian-chunk-tokens-is-total-at-both-integer-extremes) | safety | high |
| [dec-a-escalation-bands-stay-ordered-for-every-threshold](#dec-a-escalation-bands-stay-ordered-for-every-threshold) | safety | high |
| [dec-a-selection-decision-order-is-total-under-hashmap-iteration](#dec-a-selection-decision-order-is-total-under-hashmap-iteration) | safety | high |
| [dec-a-region-hint-clamp-bypassed-by-sentinel-suffix](#dec-a-region-hint-clamp-bypassed-by-sentinel-suffix) | safety | high |
| [codec-b-harness-decoders-accept-every-input-with-no-rejection-channel](#codec-b-harness-decoders-accept-every-input-with-no-rejection-channel) | safety | high |
| [codec-b-pi-decoder-drops-unrecognised-entry-types-without-a-record](#codec-b-pi-decoder-drops-unrecognised-entry-types-without-a-record) | safety | high |
| [codec-b-opencode-hides-four-part-types-from-every-transform-decision](#codec-b-opencode-hides-four-part-types-from-every-transform-decision) | safety | high |
| [codec-b-provenance-recovery-on-decode-is-all-or-nothing-and-opencode-only](#codec-b-provenance-recovery-on-decode-is-all-or-nothing-and-opencode-only) | safety | high |
| [codec-b-decoder-output-can-violate-the-projector-precondition](#codec-b-decoder-output-can-violate-the-projector-precondition) | safety | high |
| [codec-b-absolute-ordinal-is-harness-supplied-and-never-validated](#codec-b-absolute-ordinal-is-harness-supplied-and-never-validated) | safety | high |
| [codec-b-block-identity-stamp-is-caller-writable-and-the-fingerprint-is-not-an-identity](#codec-b-block-identity-stamp-is-caller-writable-and-the-fingerprint-is-not-an-identity) | safety | high |
| [codec-b-incremental-sidecar-slice-panics-behind-a-debug-assert](#codec-b-incremental-sidecar-slice-panics-behind-a-debug-assert) | safety | high |
| [codec-b-wire-level-tool-use-uniqueness-guard-has-no-release-behaviour](#codec-b-wire-level-tool-use-uniqueness-guard-has-no-release-behaviour) | safety | high |
| [codec-b-round-trip-identity-is-claimed-in-one-direction-on-one-case-per-harness](#codec-b-round-trip-identity-is-claimed-in-one-direction-on-one-case-per-harness) | safety | high |
| [codec-b-declared-missing-capture-classes-are-never-decoded](#codec-b-declared-missing-capture-classes-are-never-decoded) | reachability | high |
| [codec-b-pi-encoder-can-return-a-shorter-array-than-it-was-given](#codec-b-pi-encoder-can-return-a-shorter-array-than-it-was-given) | safety | high |

**Twenty-seven retained records: twenty-five active and two invalidated.** The
adjacent-only model-chain and silent malformed-config premises are invalidated
by the implementation at `74044960ee91641dec95c8552f15282844a18b13`. Their
guarantees, evidence files, and unaudited existing checks remain in the catalog.

Semantics distribution over all retained records: **twenty-six `always`, zero
`always-or-unreached`, zero `sometimes`, one `reachable`, zero `unreachable`**.
The active subset has twenty-four `always` and one `reachable`. The two retained
invalidated records use `always` for their regression contracts.

Type distribution: **twenty-six safety, one reachability, zero liveness**.
The active subset has twenty-four safety and one reachability record.

Reachability distribution: **sixteen `default-production`, seven
`explicit-config-only`, four `test-only`**, against 16/8/2 before. Two labels
moved, both away from `explicit-config-only` and both because a configuration
cannot in fact construct the state:
`dec-a-model-key-lookup-walk-has-two-implementations-that-disagree`, whose
differential needs `ExecuteThresholdConfig::ByModel` and where `number_at`
(`config.rs:963-968`) discards an object form before any enum is chosen; and the
new trigger-budget record, whose only `Some` sites are two test literals. Both
relabels are recorded at the records.

Confidence: twenty-seven high, zero medium, zero low.

The two invalidations leave the active reachability distribution at sixteen
`default-production`, five `explicit-config-only`, and four `test-only`. These
are record counts, not a claim that every historical evidence file has been
revalidated.

---

## Group A: the configuration contract as a defect surface

Six retained records cover bounds, commit-trigger configuration, explicit tier
permissions, clamp diagnostics, and malformed files. Five are active. The
malformed-file silence premise is invalidated: resolution reports read and parse
failures. Tier-policy warnings and file warnings do not settle clamp diagnostics;
each property needs its own input and warning oracle.

### dec-a-execute-threshold-lower-bound-is-documented-20-and-enforced-1

Type: safety
Reachability: explicit-config-only
Status: active
Exercised: not yet - the lower-bound reporting case is not covered.
`config.rs:1297-1302` pins the upper clamp (`91 -> 90`) and
`config.rs:1305-1308` pins the default `65`.
Guarantee: A configured `execute_threshold_percentage` that the documentation
forbids is rejected or reported, not silently accepted as the effective
threshold.
Check: `always` - after `merge_tiers_with_warnings`,
`execute_threshold_percentage >= 20.0`, or the returned warning vector names
`/execute_threshold_percentage`. These semantics because the clamp runs on
every config resolution, so there is no optional path.
Fault/timing angle: none for the merge check.
Required faults and enabling state: a user-tier `eidnara.jsonc` containing
`execute_threshold_percentage` below `20`, for example `5`, with the project
tier absent. A project-only value cannot lower the default and its rejection
warning must not substitute for a user-tier range diagnostic.
Confidence: high - [evidence](evidence/dec-a-execute-threshold-lower-bound-is-documented-20-and-enforced-1.md).
The source-catalog quotation gives `20-90`; its `CONFIGURATION.md` is absent
at HEAD. The verified implementation clamps to `[1, 90]`
(`config.rs:750-752`, ceiling at `config.rs:27`). Whether the historical lower
bound is still the intended daemon contract needs confirmation.
Existing check: `config.rs:1297-1302` `project_threshold_may_only_raise` covers
the upper bound only. Status `unaudited`.
Impact: a user can select a threshold below the historical documented range
without a range warning. Scheduler gates and workload still determine whether
a particular pass executes.
Open questions:
- Is the historical lower bound of twenty the intended daemon contract, or is
  the implemented floor of one intended? (needs human input)

### dec-a-memory-injection-budget-documented-range-has-no-implementing-code

Type: safety
Reachability: explicit-config-only
Status: active
Exercised: partial - `config.rs:1311-1314` pins the default `4000` and
`:1317-1350` pins precedence, the deprecated fallback, and project rejection.
The fallback cases at `:1885-1903` accept `128`, but no check asserts the
documented range for the standard user-tier key.
Guarantee: A configured `memory.injection_budget_tokens` outside the documented
range is rejected, clamped to the documented range, or reported.
Check: `always` - after resolving a user-tier standard-key value,
`500.0 <= memory_budget_tokens <= 20000.0`, or a warning names the key.
`always` because each resolution must enforce or report the range. Keep the
project tier absent so an ignored-project-key warning cannot satisfy this
range check without exercising the value parser.
Fault/timing angle: none.
Required faults and enabling state: a user-tier `eidnara.jsonc`
with `memory.injection_budget_tokens` set above `20000` (or below `500`).
Confidence: high - [evidence](evidence/dec-a-memory-injection-budget-documented-range-has-no-implementing-code.md).
The source-catalog contract quotes `CONFIGURATION.md:591` as `500-20000` with
default `4000`; that document is absent at HEAD. At
`74044960ee91641dec95c8552f15282844a18b13`, `config.rs:847-864` applies only
`.max(1.0)` to the standard key and legacy fallback. Both keys are user-only
(`:664-672`); project values warn without reaching those assignments
(`:723-733`). `lib.rs:4832-4845` passes the configured budget to the canonical
memory reader.
Existing check: none for standard-key range enforcement. The default,
precedence, and tier-policy checks named above remain `unaudited`; project
rejection is not evidence that a user-tier range is enforced. The reader
integration check at `crates/daemon/tests/transform_canonical_memory.rs:448-539`
loads a 500-token user value and checks trimming and revision stability; status
`unaudited`. It does not supply an out-of-range budget.
Impact: a user configuration can silently select a memory budget outside the
documented range. The budget reaches canonical memory trimming; a large value
does not remove the reader's separate row and byte caps.
Open questions: None.

### dec-a-commit-cluster-trigger-config-is-inert-in-this-crate

Type: safety
Reachability: default-production
Status: active
Exercised: partial - direct test contexts use `min_commit_clusters: 2` and
enabled and disabled flags (`lib.rs:16664-16674`, `lib.rs:16929-16940`). They
do not load those controls from config. Existing checks remain `unaudited`.
Guarantee: A configured `commit_cluster_trigger` reaches the module's trigger
decision, or the module reports that it cannot honour the key.
Check: `always` - for each supplied commit-cluster control, the production
trigger context carries the requested value or resolution reports that the
control is unsupported. `always` because every trigger evaluation constructs
the context. The context is built inline at `lib.rs:4983-5005`; use a direct
observation or a workload that distinguishes it from the requested value.
Fault/timing angle: none.
Required faults and enabling state: a nondefault user `commit_cluster_trigger`
value plus context observation or a discriminating trigger workload. Defaults
of `true` and `3` match the constants and cannot expose the wiring gap. A
behavioral check must isolate the commit-cluster arm from other fire reasons.
Confidence: high - [evidence](evidence/dec-a-commit-cluster-trigger-config-is-inert-in-this-crate.md).
The config fields and pointer table omit these controls (`config.rs:80-109`,
`config.rs:588-618`). `lib.rs:640-641` defines the constants, used at
`lib.rs:5001-5002`; `boundary.rs:814-819` consumes the context. Confidence is
in this daemon mechanism, not a product-wide TypeScript routing claim.
Existing check: none for the config wiring. The default-constant assertion at
`boundary.rs:2011-2015` and the direct contexts above remain `unaudited`.
Impact: a supplied disabling flag or higher cluster count does not change the
daemon's commit-cluster trigger. The historical configurability claim is not
implemented by this reader.
Open questions:
- Is the source-catalog configurability obligation intended for the daemon
  reader? The original configuration document is absent here. (needs human input)

### dec-a-project-tier-can-write-leaves-outside-the-documented-allow-list

Type: safety
Reachability: explicit-config-only
Status: active
Exercised: partial - the hostile fixture at `config.rs:1705-1815` supplies all
25 consumed keys and asserts selected effective values, but derives its expected
ignored-key count from `tier_class`. The pointer inventory at `:1820-1855` checks
registration, not policy. No single independent per-key oracle covers this whole
contract.
Guarantee: A project may override only `memory.enabled`,
`memory.auto_search.enabled`, `memory.auto_search.score_threshold`,
`memory.auto_search.min_prompt_chars`, `caveman_text_compression.enabled`,
`caveman_text_compression.min_chars`, `memory.auto_promote`, `smart_drops`, and
`temporal_awareness`, raise `execute_threshold_percentage`, or close the
`user_memories.enabled` gate; all other consumed keys are user-tier only.
Check: `always` - compare user-only and user-plus-project resolutions against
the fixed permissions above, not against `tier_class()` or `privileged()`.
For valid project values, assert the nine allowed leaves take the parsed project
value, the effective threshold never decreases, and a closed user-memory gate
never opens. Assert that `historian.module_model`,
`historian.module_fallback_models`, `historian.model`, `historian.fallback_models`,
`compaction.enabled`,
`memory.injection_budget_tokens`, `memory.budget_tokens`,
`memory.user_profile_budget_tokens`, `historian.context_limit_tokens`,
`dreamer.inject_docs`, `dreamer.tasks.review-user-memories.schedule`,
`prompt_surface.guidance_override_text`, `prompt_surface.guidance_override_path`,
and `cache_ttl` keep their user-only effects. Each supplied user-only key emits
one ignored-key warning; a rejected weakening emits one warning, while an
unchanged raise-only value emits none. Use distinct in-range values and both
boolean directions to distinguish rejection from a no-op. `always` applies
because every tier resolution must obey the same policy.
Fault/timing angle: none.
Required faults and enabling state: distinct valid user and project values for
each consumed key, including `user_memories.enabled: true` against a closed user
gate and a lower project threshold. The production path reads project values
from `.eidnara/eidnara.jsonc`; no injected fault is required.
Confidence: high - [evidence](evidence/dec-a-project-tier-can-write-leaves-outside-the-documented-allow-list.md).
Verified against the working tree based on
`0da79d706ea40de75e76af5423dd7fc9b088d8cf`: the classifications at
`config.rs:657-693` match the explicit policy, and `:710-755` applies the tiers
through them. The build-time assertion at `:696-708` rejects privileged keys
classified as project-allowed; it does not pin the exact permissions of every
key. The evidence separates these implementation facts from the independent
contract and retains the discovery history as historical evidence only.
Existing check: `config.rs:1297-1302` covers raising and clamping the threshold;
`:1317-1350` covers injection-budget rejection and the user-tier legacy fallback;
`:1458-1480` covers docs-injection rejection and a temporal-flag override;
`:1637-1666` covers attempts to open a closed gate and the unchanged closed gate;
`:1671-1700` pins all twelve privileged key names, including all four budget
keys; `:1705-1815` and `:1820-1855` are the hostile fixture and pointer
inventory described above. Status `unaudited` for
each check; their presence is not a full independent policy oracle.
Impact: a policy regression can let repository configuration select a model,
widen an input budget, change unattended-task controls, or replace trusted
guidance without the user's consent. Deriving the oracle from the same
classification can hide that regression.
Open questions: None.

### dec-a-config-value-clamps-and-zero-rejection-are-invisible-to-the-caller

Type: safety
Reachability: explicit-config-only
Status: active
Exercised: partial - `config.rs:1407-1446` exercises ordinary auto-search and
caveman overrides. `config.rs:1297-1302` checks the upper threshold clamp, but
neither checks a clamp-reporting warning.
Guarantee: When a configured value is altered by a clamp or discarded as out of
domain, the resolution reports which key was altered.
Check: `always` - when an isolated supplied leaf is clamped or discarded as out
of domain, the warning vector names that leaf and its range or rejection.
`always` applies to every resolution. Tier rejection and legacy-key deprecation
warnings do not substitute for a range diagnostic.
Fault/timing angle: none.
Required faults and enabling state: a user-only config with
`memory.auto_search.score_threshold: 0.99`,
`memory.auto_search.min_prompt_chars: 0`, or
`caveman_text_compression.min_chars: 50`.
Confidence: high - [evidence](evidence/dec-a-config-value-clamps-and-zero-rejection-are-invisible-to-the-caller.md).
The threshold clamp is at `config.rs:750-752`, auto-search and caveman clamps
at `config.rs:827-846`, and budget floors at `config.rs:847-869`.
`positive_usize_at` rejects zero (`config.rs:955-961`). None reports a range
warning. The warning-returning merge and file resolver are observable from tests
(`config.rs:710-755`, `config.rs:262-282`).
Existing check: none for the clamp reporting. Status `unaudited`.
Impact: a user can receive a different effective value without an explanation.
With only a user tier, `min_prompt_chars: 0` leaves the default `20`; an allowed
project zero instead preserves the earlier user value.
Open questions:
- Is the stderr line from `emit_warnings` visible in any harness the module
  runs under? The module runs as a daemon component, so stderr may be
  discarded. Unresolved, needs a look at the host's process wiring, which is
  Part 2a scope.

### dec-a-malformed-config-silently-resolves-to-defaults-and-stops-the-historian

Type: safety
Reachability: explicit-config-only
Status: invalidated
Exercised: yes - `config.rs:2133-2182` constructs malformed and unreadable
tiers and asserts path-specific warnings, repeated warnings, same-mtime repair,
and silent missing tiers. Test adequacy remains `unaudited`.
Guarantee: A configuration file that exists but cannot be parsed produces a
distinguishable signal rather than the same result as an absent file.
Check: `always` - an unusable tier produces a warning naming its path and
failure kind on each resolution; a missing tier does not. These semantics apply
to every file resolution, including repeated failures.
Fault/timing angle: failed reads bypass the mtime fast path
(`config.rs:363-366`). Successful cached values can still conceal a same-mtime
edit, which is a separate condition and not evidence of silent parse failure.
Required faults and enabling state: a user `eidnara.jsonc` with a syntax
error that `strip_jsonc` does not repair, for example an unterminated string.
Confidence: high - [evidence](evidence/dec-a-malformed-config-silently-resolves-to-defaults-and-stops-the-historian.md).
`read_tier_cached` stores both the value and warning (`config.rs:362-393`);
`effective_with_warnings` collects tier warnings (`config.rs:262-282`), and
the public resolution path emits them (`config.rs:250-257`,
`config.rs:402-406`). Defaults can still apply, but the failure is not silent.
Existing check: `unreadable_and_malformed_tiers_warn_while_missing_tiers_stay_silent`
at `config.rs:2133-2182`. Status `unaudited`.
Impact: losing this diagnostic would make an ignored broken file appear absent.
The existing fallback policy is not a last-known-good policy.
Open questions: None for the config-reader signal. Launcher-specific display of
stderr is outside this record.

Invalidated: at `74044960ee91641dec95c8552f15282844a18b13`, the read-warning
storage, collection, and emission paths disprove the silent-failure premise.
The guarantee and existing check remain recorded; this is not an open defect.

## Group B: model-chain resolution

The lookup record compares per-model configuration walks. The model-chain record
retains an invalidated adjacent-only defect premise: full deduplication and its
non-adjacent-repeat check exist. The two concerns have different oracles.

### dec-a-model-key-lookup-walk-has-two-implementations-that-disagree

Type: safety
Reachability: test-only
Status: active
Exercised: not yet - no check compares both Rust walks on the same wildcard-only
map. Config routing tests (`config.rs:1089-1115`, `config.rs:1133-1160`) and
the scheduler golden (`scheduler.rs:1087-1096`) exercise each implementation
separately. Each existing check remains `unaudited`.
Guarantee: Every consumer of a per-model configuration map resolves a given
model key through the same documented walk.
Check: `always` - for every model key and every map, `config.rs`'s walk and
`scheduler::model_key_lookup_order` select the same entry. `always` because
each walk runs on every resolution for its own consumer.
Fault/timing angle: none.
Required faults and enabling state: a per-model map keyed only by a
`provider/*` wildcard, with distinct wildcard and default values. Compare a TTL
map with a scheduler percentage map through direct calls. The daemon's config
adapter selects a scalar threshold, not the scheduler map path
(`transform.rs:5447-5454`).
Confidence: high - [evidence](evidence/dec-a-model-key-lookup-walk-has-two-implementations-that-disagree.md).
The TTL walk includes the wildcard (`config.rs:151-207`); the scheduler walk
does not (`scheduler.rs:810-830`), and its consumers fall back to `default`
(`scheduler.rs:779-808`). The original shared-walk contract comes from the
absent source-catalog `CONFIGURATION.md`, not a newly verified current document.
Existing check: `cache_ttl_resolution_matches_shared_typescript_vectors`
(`config.rs:1133-1160`) pins the TTL resolver. The scheduler golden
deserializes per-model thresholds (`scheduler.rs:923-932`,
`scheduler.rs:1011-1013`; object cases at
`crates/daemon/testdata/scheduler-golden.json:87-106`) and resolves them at
`scheduler.rs:1087-1096`. These are not a differential test. Status `unaudited`.
Impact: callers of the two map APIs can select different entries for the same
qualified key. The daemon's scalar config route does not exercise that divergence.
Open questions:
- Should the two walks be one function? They already agree on the exact, bare,
  and dash-stripped steps, which is the duplication the repository's own
  duplication policy targets. (needs human input)

The `test-only` label applies to the in-tree differential, not to the existence
of scheduler map tests. The daemon adapter constructs a scalar, while golden
deserialization constructs `ByModel`. The scheduler module is public
(`lib.rs:31`), so this label does not exclude external library callers. The
[historical investigation](evidence/dec-a-model-key-lookup-walk-has-two-implementations-that-disagree.md#historical-investigation)
retains the earlier relabeling rationale without keeping its false no-construction
claim active.

### dec-a-model-chain-dedup-is-adjacent-only

Type: safety
Reachability: explicit-config-only
Status: invalidated
Exercised: yes - `config.rs:2185-2194` supplies non-adjacent repeats and
asserts first-occurrence order. Test adequacy remains `unaudited`.
Guarantee: The resolved historian model chain contains no duplicate model id.
Check: `always` - after config resolution, `model_chain` has no repeated
element. `always` because `dedup_preserving_order` runs on every merge at
`config.rs:753`.
Fault/timing angle: none.
Required faults and enabling state: a user config with
`historian.module_model: "a"` and
`historian.module_fallback_models: ["b", "a"]`.
Confidence: high - [evidence](evidence/dec-a-model-chain-dedup-is-adjacent-only.md).
The helper uses `HashSet` insertion with `retain` (`config.rs:950-953`), so
`["a", "b", "a"]` becomes `["a", "b"]` without sorting.
Existing check: `model_chain_drops_repeats_anywhere_and_keeps_first_occurrence_order`
at `config.rs:2185-2194`. Status `unaudited`.
Impact: a regression could spend a bounded fallback attempt on a repeated model.
Open questions: None.

Invalidated: at `74044960ee91641dec95c8552f15282844a18b13`, full-chain
deduplication disproves the adjacent-only premise. The uniqueness guarantee and
existing check remain as a regression contract.

## Group C: totality, determinism, and the two exceptions

**Seven records on the pure decision units, after a disposition pass split one in
two.** Four are guards that hold: a TTL parse total over arbitrary strings, a
budget derivation total over the non-finite inputs it validates, a chunk-token
derivation total at both integer extremes, and escalation bands that stay ordered
for every threshold. They are recorded rather than assumed because each is the
guarded analogue of a Part 3 defect, so the record fixes the boundary and makes a
later change that drops a guard visible. The fifth is determinism of the selection
decision under `HashMap` iteration, which holds because every result is a set or is
totally sorted before it escapes.

**Two are exceptions rather than guards, and the group heading used to name only
one.** The first is a clamp whose idempotence guard doubles as a bypass, because a
value already carrying the sentinel suffix is passed through unclamped. The second
was carried inside the budget-derivation record as an open question and is now its
own record: `BoundaryContext::trigger_budget` is the one float read without an
`is_finite` gate, and a `Some(NaN)` reaches `TriggerProgress.tail_size_bar`
(`boundary.rs:802`) and from there the transform response's historian diagnostics.
Its evidence was already written — the budget record's own evidence file states
that this test case fails today — so promoting it is applying a finding the part
had rather than adding one. The group therefore reads: four guards, one determinism
result, and two defects.

### dec-a-cache-ttl-parse-is-total-over-arbitrary-strings

Type: safety
Reachability: default-production
Status: active
Exercised: partial - the golden parser cases (`scheduler.rs:1082-1085`) and
sentinel tests (`scheduler.rs:1378-1405`) cover ordinary TTLs, malformed input,
and `never`. They do not cover zero, an uppercase unit, an overflowing digit
run, or a multibyte trailing character. Existing checks remain `unaudited`.
Guarantee: `parse_cache_ttl` returns for every `&str` without panicking and maps
`never` or an overflowing duration to the non-expiring `u64::MAX` sentinel.
Check: `always` - for every input string, the call returns `Ok(n)` or
`Err(CacheTtlParseError)` without panicking. Non-finite or oversized computed
durations yield `u64::MAX`, which neither expiry predicate can exceed.
`always` because the scheduler parses the configured TTL on every `decide`
call through `scheduler_ttl_ms`.
Fault/timing angle: none in parsing. Hard expiry requires a positive prior
timestamp and elapsed time strictly greater than the TTL
(`scheduler.rs:405-407`); a second call alone does not establish those conditions.
Required faults and enabling state: a `cache_ttl` string. `"0"`, `"5S"`,
`"99999999999999999999h"`, and `"5\u{20ac}"` are the interesting inputs; all
are accepted from the user tier by `config.rs:924-945` as non-empty trimmed
strings. A project TTL is ignored before this parser.
Confidence: high - [evidence](evidence/dec-a-cache-ttl-parse-is-total-over-arbitrary-strings.md).
The parser returns a `Result`, slices at a character boundary, and saturates
oversized results (`scheduler.rs:365-398`). Zero parses to zero milliseconds;
uppercase units are rejected while `never` is case-insensitive.
Existing check: `parse_cache_ttl_never_returns_u64_max`
(`scheduler.rs:1378-1385`), `never_ttl_predicates_are_always_false`
(`scheduler.rs:1388-1393`), and the golden parser loop above. Status `unaudited`.
Impact: an invalid user TTL silently uses the scheduler default
(`scheduler.rs:771-773`). Zero requests expiry when its timestamp preconditions
hold; band selection and boundary deferral still govern the final pass
(`scheduler.rs:689-732`). The parser's range guarantee remains intact.
Open questions:
- Is `cache_ttl: "0"` intended as "always expire" or should it be rejected?
  `CONFIGURATION.md:163` (source-catalog path, not present at HEAD) documents neither. (needs human input)

### dec-a-boundary-budget-derivation-is-total-over-non-finite-input

Type: safety
Reachability: default-production
Status: active
Exercised: partial - `boundary.rs:2226-2227` pins the commit-cluster constant
against a golden, and the golden fixture suite drives the boundary with
realistic values. No test supplies a non-finite context limit, threshold, or
usage percentage.
Guarantee: The **guarded** boundary and trigger derivations produce a finite,
in-range result for every f64 input they validate, including infinity and NaN,
and never propagate a non-finite value from a validated field into a decision or
a serialized diagnostic. The scope word "guarded" is load-bearing and was added by
a disposition pass: the unvalidated `trigger_budget` passthrough is now
[dec-a-caller-supplied-trigger-budget-is-the-one-unvalidated-float-and-reaches-a-diagnostic](#dec-a-caller-supplied-trigger-budget-is-the-one-unvalidated-float-and-reaches-a-diagnostic),
and it is a defect rather than a guard that holds.
Check: `always` - for every `BoundaryContext`, `derive_trigger_budget` returns
a value in `[5000, 50000]`, `derive_protected_tail_token_target().n >= 1.0` and
is finite, and `clamp_percentage` returns a value in `[0, 100]`. `always`
because every trigger evaluation runs all three. Each of the three has an
`is_finite` gate on its own inputs — `boundary.rs:339-341` for `context_limit`,
`:363-372` for the context limit and threshold, `:926-931` for the percentage —
so the check is over inputs those gates cover. It does **not** cover
`ctx.trigger_budget`, and the record no longer implies that it does.
Fault/timing angle: none.
Required faults and enabling state: a `BoundaryContext` whose `context_limit`,
`execute_threshold_percentage`, or `usage_percentage` is `f64::INFINITY` or
`f64::NAN`. Reaching that from production needs a host-supplied usage reading,
since `lib.rs:4950-4959` builds the context from request and store values.
Confidence: high - [evidence](evidence/dec-a-boundary-budget-derivation-is-total-over-non-finite-input.md).
Read every guard: `boundary.rs:339-341`, `:363-372`, `:926-931`. Executed
`NAN.max(0.0) == 0.0` and `NAN.min(5.0) == 5.0` to confirm the absorption
argument, which is what makes `:342` safe against a NaN threshold. Also
confirmed that `ctx.trigger_budget` is the one unvalidated float (`:756-761`,
`:377-379`) and that production always passes `None` (`lib.rs:4957`), with
`Some` only at `lib.rs:16495` and `:16760`. The evidence file's test-plan item 4
already states that the `trigger_budget` case "fails today"
([evidence:184-187](evidence/dec-a-boundary-budget-derivation-is-total-over-non-finite-input.md)),
which is what the split acts on.
Existing check: none targeting non-finite input. Status `unaudited`.
Impact: this is the guarded analogue of Part 3's decay totality defects, and for
the three validated fields the guard holds. Recording it fixes the boundary so a
later change that drops a guard is visible.
Open questions: None. The `trigger_budget` question that stood here has been
promoted to its own record, because the evidence file already recorded that its
test case fails, which makes it a defect rather than an open question.

### dec-a-caller-supplied-trigger-budget-is-the-one-unvalidated-float-and-reaches-a-diagnostic

Type: safety
Reachability: test-only
Status: active
Exercised: not yet - and the evidence for the sibling record already says the
oracle fails. `boundary.rs`'s golden fixture suite never sets `trigger_budget` to
a non-finite value; the two `Some` sites in the tree, `lib.rs:16495` and `:16760`,
pass finite numbers.
Guarantee: `BoundaryContext::trigger_budget`, being caller-supplied and read
without validation, does not carry a non-finite value into a boundary
computation or into a serialized diagnostic. **This guarantee does not hold
today.**
Check: `always(!X)` - for every `BoundaryContext`, if
`derive_protected_tail_token_target` or `check_compartment_trigger_with_index` is
called with `trigger_budget: Some(v)` where `!v.is_finite()`, then no field of the
returned `ProtectedTailTokenTarget` or `TriggerProgress` is non-finite.
`always(!X)` over a forbidden **state** with no dedicated detection point, per
METHOD's first check-semantics rule; `unreachable` would be wrong because the
`unwrap_or_else` at `:377-379` and `:756-761` must execute on every call and only
its `Some` arm's *content* is at fault. The assertion fails on the current build,
which is the record's purpose.
Fault/timing angle: none. Pure function of one context value.
Required faults and enabling state: one direct call with
`trigger_budget: Some(f64::NAN)` and a non-empty message set.
`BoundaryContext.trigger_budget` is a `pub` field and both entry points are
reachable in-crate, so the fixture is a struct literal and one call. No harness
work.
Confidence: high - [evidence](evidence/dec-a-boundary-budget-derivation-is-total-over-non-finite-input.md).
Shares the sibling's evidence file, which already carries the trail: item 4 of its
test plan is exactly this case and states "That case fails today". Traced for this
disposition: `:377-379` reads `ctx.trigger_budget` through `unwrap_or_else` with no
`is_finite` gate on the `Some` arm, unlike every neighbouring field; `:383`'s
`(trigger_budget + reserve).min((usable * 0.5).floor())` absorbs the NaN, because
`f64::min` returns the non-NaN operand, so `headroom`, `ceiling_n`, and `n` stay
finite and `derive_protected_tail_token_target`'s own postcondition survives — but
`:399` stores the raw NaN into the returned struct's `trigger_budget` field. The
propagating path is the trigger one: `:756-761` performs the same unguarded read,
`:780-781`'s `MIN_PROACTIVE_TAIL_TOKEN_ESTIMATE.max(...)` absorbs it for
`scan_budget`, and then `:802`'s
`tail_size_bar: trigger_budget * TAIL_SIZE_TRIGGER_MULTIPLIER` is a bare multiply
with nothing to absorb it. So `TriggerProgress.tail_size_bar` is NaN.
Existing check: none. Status `unaudited`.
Impact: `TriggerProgress`'s own doc comment (`boundary.rs:322-324`) says it is
"Surfaced through the transform response's historian diagnostics so a stalled rig
drive is diagnosable per pass", and `tail_size_bar` is described at `:329-330` as
"The tail_size fire bar". It is carried out at `lib.rs:4982` and divided by 1000
and rounded at `:5002`. A NaN there is the diagnostic field going quietly wrong in
the response an operator reads to explain why the historian did not fire, and
`serde_json` renders a NaN as `null`, so the wire form is an absent number rather
than a visible error. This is the defect the sibling record's "no totality defect
was found" framing concealed, and it is the same class as Part 3's three.
Open questions:
- Should `derive_protected_tail_token_target` and
  `check_compartment_trigger_with_index` validate `ctx.trigger_budget` the way
  they validate `context_limit`, `execute_threshold_percentage`, and
  `usage_percentage`, or should the field's type make a non-finite value
  unrepresentable? The first is a two-line `is_finite` gate at each of the two
  read sites; the second is a newtype and a constructor. (needs human input)
- Production passes `None` (`lib.rs:4957`) so the reachability is `test-only`
  today. Whether a future caller may supply the budget — the field exists for
  someone — decides whether this is a latent defect or an active one. Unresolved;
  the field's purpose is not documented at its declaration (`:222-224`).

### dec-a-derive-historian-chunk-tokens-is-total-at-both-integer-extremes

Type: safety
Reachability: default-production
Status: active
Exercised: partial - `config.rs:1449-1455` covers `1`, `32_000`, `128_000`,
`200_000`, and `400_000`, so both clamp arms are hit. Neither `0` nor
`usize::MAX` is covered.
Guarantee: `derive_historian_chunk_tokens` returns a value in
`[MIN_HISTORIAN_CHUNK_TOKENS, MAX_HISTORIAN_CHUNK_TOKENS]` for every `usize`
input, without panicking.
Check: `always` - for every input, the result is in `[8000, 50000]`. `always`
because every historian firing derives the budget from the configured limit.
Fault/timing angle: none.
Required faults and enabling state: `historian.context_limit_tokens` set to `0`
is impossible, because `positive_usize_at` (`config.rs:955-961`) discards it.
Reaching the extremes needs a very large configured limit or a direct call.
Confidence: high - [evidence](evidence/dec-a-derive-historian-chunk-tokens-is-total-at-both-integer-extremes.md).
The final integer clamp (`config.rs:39-46`) enforces the ordered constants
`8000` and `50000` (`config.rs:28-29`). The maximum input reaches the upper
clamp after quartering; it does not need a saturating float-to-integer cast.
The reattach builder and firing assemblers pass the effective context limit
(`lib.rs:4708-4714`, `lib.rs:5114-5119`, `lib.rs:5270-5276`).
Existing check: `config.rs:1449-1455`
`historian_budget_derivation_clamps_at_both_bounds`. Status `unaudited`.
Impact: a regression could produce an out-of-range historian chunk budget.
The final integer clamp bounds even an extremely large configured limit.
Open questions: None.

### dec-a-escalation-bands-stay-ordered-for-every-threshold

Type: safety
Reachability: default-production
Status: active
Exercised: partial - `scheduler.rs:1238`
`escalation_bands_stay_ordered_above_execute_and_below_emergency` covers the
ordering for the thresholds it samples. Non-finite and negative thresholds are
not covered.
Guarantee: For every effective threshold, the derived force-materialization
band lies at or above `MIN_FORCE_MATERIALIZE_PERCENTAGE` and strictly below the
fixed emergency band, and is monotone non-decreasing in the threshold.
Check: `always` - for every f64 threshold,
`85.0 <= force_materialize_percentage < emergency_percentage == 95.0`, and
`t1 <= t2` implies `bands(t1).force <= bands(t2).force`. `always` because every
boundary resolution and every scheduler band derivation calls it.
Fault/timing angle: none.
Required faults and enabling state: none. A threshold of `f64::NAN`, a negative
threshold, or a threshold above `90` are the interesting inputs.
Confidence: high - [evidence](evidence/dec-a-escalation-bands-stay-ordered-for-every-threshold.md).
`scheduler.rs:187-198`. The non-finite arm substitutes `65.0` (`:191`); the
finite arm caps at `90.0` (`:190`), so `threshold + 2.0 <= 92.0` and
`force = max(85.0, threshold + 2.0)` lies in `[85, 92]`, always below the
constant `95.0` at `:21`. Confirmed the four consumers: `boundary.rs:815-816`,
`:484-485`, `:978-980`, and `scheduler.rs:522-526`.
Existing check: `scheduler.rs:1238` and, in `boundary.rs`, the golden constant
assertions at `:2226-2227`. Status `unaudited`.
Impact: if a threshold could push the force band to or past `95`, the `Force85`
arm at `scheduler.rs:525` would become unreachable and the emergency arm would
absorb the whole force band, changing which passes bypass mid-turn deferral.
The cap makes that impossible, and this record pins it.
Open questions: None.

### dec-a-selection-decision-order-is-total-under-hashmap-iteration

Type: safety
Reachability: default-production
Status: active
Exercised: partial - `selection.rs:2836` `drop_wins_over_edit_marker` covers
the rank precedence. No test asserts that two runs over identical inputs
produce identical output, which is the claim the module header makes.
Guarantee: `select_reductions` returns byte-identical decisions for identical
`(items, frozen_keys, ctx, cfg)`, independent of hash-map iteration order.
Check: `always` - for identical inputs, repeated calls return equal
`Vec<ReductionDecision>`; and the enabling precondition holds, namely that no
two distinct arcs emit a decision for the same `target_id`. `always` because
determinism is stated as the cache invariant at `selection.rs:6-7` and every
non-defer pass runs the selector.
Fault/timing angle: none in a single pass. The consequence window is across
passes: a non-deterministic decision set makes a defer-pass replay differ from
the frozen bytes.
Required faults and enabling state: none for the property. Refuting it needs an
input where one `target_id` receives two same-rank decisions with different
payloads, which requires duplicate `SelItem` ids mapped to different `arc_id`s.
Confidence: high - [evidence](evidence/dec-a-selection-decision-order-is-total-under-hashmap-iteration.md).
Traced both hash-map iterations that reach output: `selection.rs:1305` and
`dedupe_and_sort`'s `:1397-1405`. The final `out.sort_by` at `:1408` sorts on
`target_id`s that `best` has made unique, so the order is total. Also checked
every internal sort for a total tie-break: `:853-861`, `:763-770`,
`:1033-1037`, `:1071-1075`, `:447-452`, `:1290-1295` all end in an `arc_id` or
`mid` comparison.
Existing check: `selection.rs:2836` `drop_wins_over_edit_marker`, plus the
differential golden that `selection.rs:32-33` names as the arbiter. Status
`unaudited`.
Impact: the header stakes the cache invariant on this. If it fails, a defer
pass replays different bytes than the freeze produced, which busts the provider
prefix cache without any pass intending to.
Open questions:
- Can duplicate `SelItem` ids reach the selector? Ids are `mid#block_index`
  projections from `wire.rs`, which is the sibling lens's scope. Unresolved,
  needs the codec lens to confirm id uniqueness.

### dec-a-region-hint-clamp-bypassed-by-sentinel-suffix

Type: safety
Reachability: explicit-config-only
Status: active
Exercised: not yet - `selection.rs:2537-2549` covers the UTF-16 cap and the
surrogate back-off. No test supplies a value that already ends with the
sentinel.
Guarantee: An `edit_marker` payload's diff-bearing values are clamped to a
bounded region hint regardless of their content.
Check: `always` - for every diff value, the `edit_marker` payload's
corresponding value is at most `EDIT_REGION_HINT_LEN` UTF-16 units plus the
sentinel. `always` because `region_hint` runs on every diff key of every
superseded edit.
Fault/timing angle: none.
Required faults and enabling state: `smart_drops: true`, which is off by
default (`config.rs:128`) and permitted in either tier (`config.rs:680`,
`config.rs:890-894`), plus an `edit` or `write`
tool call superseded by a later edit to the same file, whose `oldString`,
`newString`, or `content` value ends with the literal `...[truncated]`.
Confidence: high - [evidence](evidence/dec-a-region-hint-clamp-bypassed-by-sentinel-suffix.md).
`selection.rs:559-561` returns the input unchanged when it ends with
`TRUNCATION_SENTINEL` (`:71`). Executed the predicate on a 5,014-character
hostile string to confirm it takes the short-circuit arm. The gate is
`cfg.smart_drops` at `:1229` and `:1236`.
Existing check: `selection.rs:2537-2549`
`edit_marker_region_hint_caps_utf16_and_backs_off_split_surrogate`, which
covers the other two arms. Status `unaudited`.
Impact: a superseded edit keeps its full diff instead of a 40-unit hint, so the
reduction reclaims nothing while the accounting believes it did. The content is
harness-supplied, so a file whose text legitimately ends with that marker is
enough; no adversary is required.
Open questions:
- Should the guard test for a well-formed hint rather than a bare suffix, for
  example a length check as well? Changing it would have to preserve
  idempotence, which the doc comment at `:557` claims. (needs human input)

## Group D: decoder acceptance with no rejection channel

Four records on what the harness codecs silently accept and silently discard.
The first is the shape: neither decoder can reject anything, so the question
inverts. The next two are the two harnesses answering it in opposite ways, Pi
dropping an unrecognised entry with nothing retained, OpenCode retaining the raw
part but hiding four named part types from every transform decision. The fourth is
provenance recovery on decode, which is all-or-nothing and exists on one harness
only. Group D is where the crate's trust boundary actually sits, and none of these
records has a rejection channel to test, so each oracle is a comparison between
what entered and what the next stage can see.

### codec-b-harness-decoders-accept-every-input-with-no-rejection-channel

Type: safety
Reachability: default-production
Status: active
Exercised: not yet - the only decoder inputs anywhere are the two goldens' 21
well-formed values (`codec/mod.rs:57`, `:180`) and the hand-built fixtures in
`codec/opencode.rs:1322-2186` and `codec/pi.rs:1078-1499`. No test supplies a
non-object array element, and there is no arbitrary-input sweep of any kind.
Guarantee: For every input array, each harness decoder returns a value whose
postcondition holds, without panicking, without unbounded allocation, and
without producing a message that silently misrepresents its input.
Check: `always` - for arbitrary input, the call returns; `decoded.len()`,
`sidecar.order.len()`, and the per-message block counts are consistent with the
input. A panic is a forbidden state with no dedicated detection point, so it is
`always(!panic)`; `unreachable` would be wrong because no code location must
never execute. This reapplies Part 1's `decoder-totality-over-arbitrary-bytes`
(`part-1-shm-transport/catalog.md:1284-1329`), with the postcondition
strengthened because there is no error variant to fall back on.

**The allocation clause is separated out and downgraded, because it is not
observable from a decoder call.** The original check ended "and allocation is
bounded by a constant multiple of input size", counted alongside the return and
consistency clauses as though one `decode` call could witness it. It cannot: the
functions return `DecodedHarnessMessages` and expose no allocation accounting, so
proving a multiple of input size needs an allocation observer — a
`#[global_allocator]` counting wrapper, a `dhat`-style profiler, or a
`Vec::capacity` sweep over the returned structure — none of which exists in this
tree and any of which is a harness of its own. Its honest status is
**enumeration, not assertion**: the largest allocations in either decoder are
`raw_message.clone()` at `codec/opencode.rs:232` and `raw_entry.clone()` at
`codec/pi.rs:114`, each one input message, and no loop in either file allocates
per iteration without a bound from the input. That is a static reading and it is
already recorded in the `Confidence:` line below. If an allocation observer is ever
built, the clause becomes assertable as stated; until then it is a claim discharged
by reading and it must not be counted as an oracle a decode call satisfies.
Fault/timing angle: none. Both decoders are pure functions over one immutable
slice, exactly as Part 1's three were. The exposure is structural rather than
temporal: totality is achieved by a default ladder (observations 2 and 3), not
by validation, so every malformation is converted into a plausible-looking
decoded message.
Required faults and enabling state: none for the return and consistency clauses.
An arbitrary `Vec<Value>` is the whole enabling state. The interesting members are
a bare string or number as an array element, a `parts` value that is an object
rather than an array, and a part whose `type` is absent. **The allocation clause
additionally needs an allocation-observing harness**, which the tree does not have,
so that clause is the reason this record is `partial` on its own terms.
Confidence: high - [evidence](evidence/codec-b-harness-decoders-accept-every-input-with-no-rejection-channel.md).
Signatures read at `HEAD`: `codec/opencode.rs:23-25`, `:27-32`, `:37-41` and
`codec/pi.rs:19-21`, `:23-26` all return `DecodedHarnessMessages`. Every
fallible extraction in both files was enumerated; all of them are
`Option`-combinator chains terminating in `unwrap_or`, `unwrap_or_default`, or
`unwrap_or_else`. Panic sites in the production halves
(`codec/opencode.rs:1-1321`, `codec/pi.rs:1-1077`): three `debug_assert!`
(`opencode.rs:251`, `:252`, `:466`) and one slice index (`:258`), all covered
by record two; every other index is bounded by the loop that produced it
(`opencode.rs:716-717` by the `while` at `:715`, `:730` by the
`.get(block_index + 1)` test at `:727`, `pi.rs:393` by the `matches!` on
`.first()` at `:389-392`). No decoder allocates unboundedly; the largest
allocations are `raw_message.clone()` at `opencode.rs:232` and
`raw_entry.clone()` at `pi.rs:114`, each one input message.
Existing check: partial and indirect. `codec/mod.rs:78-89` and `:201-212`
assert decode determinism (`decoded == decoded_again`) over the goldens, which
pins purity but not totality. `codec/opencode.rs:1322-2186` (17 tests) and
`codec/pi.rs:1078-1499` (14 tests) all use well-formed fixtures. Status
`unaudited`. Workspace nextest includes daemon library tests
(`.github/workflows/ci.yml:413-417`); the source-catalog absent-CI premise is
not current evidence about these checks.
Impact: the failure mode is not a crash, it is a fabricated message. A harness
that ships a malformed element gets a zero-block `"user"` message that occupies
an ordinal, enters the sidecar, participates in boundary selection, and is
re-encoded from its retained raw. Nothing downstream can tell it apart from an
authentic empty user turn. Part 1's equivalent record could say "the property
holds at HEAD and is under-evidenced rather than violated"; this one cannot,
because the property as stated is violated by design.
Open questions:
- Should a harness codec have a rejection or warning channel at all, or is
  total coercion the deliberate contract on the grounds that the harness is
  trusted? Nothing in either file states a position. (needs human input)

### codec-b-pi-decoder-drops-unrecognised-entry-types-without-a-record

Type: safety
Reachability: test-only
Status: active
Exercised: not yet - no golden case and no unit test supplies an entry whose
`type` is outside
`{message, custom_message, custom, branch_summary, compaction}`. Observation 21
verifies the Pi golden's 11 entries use only three of those.
Guarantee: An input entry the Pi decoder does not recognise is either
represented in the decoded output, retained for replay, or reported; it is not
discarded without trace.
Check: `always` - for every input entry, either a `IngressMessage` exists
whose meta retains the entry's bytes, or the entry's bytes are recoverable from
`sidecar.messages`. `always` because the decode loop visits every entry
unconditionally.
Fault/timing angle: none. The consequence is temporal only in that it
compounds: the dropped entry also shifts every later ordinal, because
`codec/pi.rs:52` derives the ordinal from `decoded.len() + 1` rather than from
the entry index.
Required faults and enabling state: one Pi session entry with an unrecognised
`type` and no `role` key, for example `{"type": "tool_use_v2", "data": {}}`, or
the degenerate `{"type": "message"}` with no `message` key.
Confidence: high - [evidence](evidence/codec-b-pi-decoder-drops-unrecognised-entry-types-without-a-record.md).
`codec/pi.rs:41-50` read at `HEAD`; `pi_message` at `:661-669` returns `None`
unless `type == "message"` (then `raw_entry.get("message")`, itself possibly
`None`) or a `role` key is present; `is_pi_opaque_entry` at `:681-686` admits
exactly `custom_message`, `custom`, `branch_summary`. The `continue` at `:49`
writes nothing. Contrasted against `codec/opencode.rs:194-204`, which routes
every unknown part type to `BlockKind::Opaque` with `raw: part.clone()`, so the
two harness decoders hold opposite policies for the same situation. Also
contrasted against the wire layer's stated contract at `wire.rs:19-21`, which
requires the pass-through path stay `Value`-level "so harmless future wire fields
are not silently dropped".
Existing check: none. `codec/pi.rs:1078-1499` has 14 tests;
`codec/pi.rs:1479-1483` asserts `encode_pi(...).is_empty()` for an
empty-content message, which is the encoder half of a different drop. Status
`unaudited`.
Impact: two consequences, one recoverable and one not. Recoverable: `encode_pi`
cannot reproduce the entry, so a decode-then-encode round trip silently
truncates the session file. Unrecoverable in the same pass: every later entry's
ordinal shifts down by one, so a persisted boundary ordinal or tag keyed to an
ordinal now names a different message. Because Pi has no `absolute_ordinal`
input (record ten), there is no way for the harness to pin the numbering
against this.
Open questions:
- Is the three-type opaque allow-list at `:681-686` a closed set by design, or
  a list that was meant to grow and did not? `codec/opencode.rs:194-204`
  suggests the crate's default answer is "preserve unknown shapes". (needs
  human input)
- Does the TypeScript Pi plugin drop the same entries before the Rust codec
  sees them? `packages/pi-plugin/PARITY.md:107-116` (source-catalog path, not present at HEAD) says Pi "rebuilds
  `AgentMessage[]` from JSONL every pass", which implies a shaping layer
  upstream. Unresolved, needs the TypeScript transcript adapter, which is
  outside 4f scope.

### codec-b-opencode-hides-four-part-types-from-every-transform-decision

Type: safety
Reachability: default-production
Status: active
Exercised: partial - `testdata/codec/opencode-golden.json` includes one `patch`
part (message index 2) and the round trip at `codec/mod.rs:88` therefore does
pin that `patch` survives re-encode. Nothing covers `snapshot`, `agent`, or
`retry`, and nothing asserts the wire-side absence for any of the four.
Guarantee: A part type the OpenCode decoder omits from the wire view is
nonetheless byte-preserved on re-encode, and no downstream decision depends on
seeing it.
Check: `always` - for every accepted OpenCode message, the re-encoded parts
array contains every input part whose type is in
`{snapshot, patch, agent, retry}`, at its original index, byte-identical; and
no `BlockMeta` claims that index. `always` because the decode arm is
unconditional. This is the surviving half of Part 1's
`accepted-decode-consumes-its-declared-width`
(`part-1-shm-transport/catalog.md:1330-1373`): a byte either influences a
decoded field or is retained verbatim, and here it is the second case for four
named types.
Fault/timing angle: none at decode. The interaction to check is with
`remove_unretained_native_parts` (`codec/sidecar.rs:135-145`), which removes a
native index only when it is in `decoded_native_indices` and not in
`retained_native_indices`. The four types never enter `decoded_native_indices`,
so they are structurally immune to deletion compaction. That immunity is
load-bearing and stated nowhere.
Required faults and enabling state: none for the preservation direction; one
OpenCode message carrying any of the four part types suffices. For the
interesting composition, that message must also have a decoded block deleted,
so that `remove_unretained_native_parts` runs with a non-empty removal set.
Confidence: high - [evidence](evidence/codec-b-opencode-hides-four-part-types-from-every-transform-decision.md).
`codec/opencode.rs:193` read at `HEAD`. Traced the preservation path:
`encode_with_meta` starts from `meta.raw`'s parts at `:707-711`, only mutates
matched indices (`:761-779`), pushes unmatched blocks (`:780`), then filters
via `:784`. Confirmed against the golden: the `patch` part at input message
index 2 survives `codec/mod.rs:88`'s
`assert_eq!(encoded, strip_opencode_compaction(case.messages))`, which strips
only `compaction`.
Existing check: partial, and it covers the type by accident rather than by
design. `codec/mod.rs:59-76` lists `patch` as a required coverage class and the
golden supplies one, so the round trip pins it. `codec/mod.rs:216-252`
`codec_conformance_removes_leading_native_blocks_without_reindex_drift`
exercises `remove_unretained_native_parts` but on a message with no immune
parts. Status `unaudited`.
Impact: correct today, and fragile in one specific direction. Because these
four types are invisible to the wire view, the transform's byte accounting, tag
numbering, and boundary selection never see them, while the provider does. If
any of the four ever carries content large enough to matter to the context
budget, the module's measurement of the array is wrong by exactly that amount
and no existing check would notice.
Open questions:
- Are all four types genuinely content-free for provider purposes? `patch` is
  the one that plausibly carries bytes. Unresolved, needs the OpenCode
  part-schema, which is not vendored (observation 20 records that the SDK
  serializer is absent from the test closure).

### codec-b-provenance-recovery-on-decode-is-all-or-nothing-and-opencode-only

Type: safety
Reachability: default-production
Status: active
Exercised: partial - `codec/mod.rs:128-175`
`fresh_boundary_prefix_does_not_borrow_persisted_synthetic_meta` builds a
persisted message whose single part carries `synthetic: true` and asserts at
`:174` that it re-encodes byte-identically, which exercises the all-synthetic
recovery path. Nothing covers a mixed-parts message, and nothing covers Pi.
Guarantee: A codec that reads a message the module previously wrote recovers
the same synthetic-versus-authentic classification the module assigned.
Check: `always` - for every message the module encodes with
`meta.synthetic == true`, decoding the encoded form yields
`meta.synthetic == true`. `always` because the classification is computed for
every decoded message.
Fault/timing angle: the window is a pass boundary. Provenance is lost only when
a module-authored message survives into the next pass's ingress, which is the
normal case for a persisted m0, m1, or injected pair.
Required faults and enabling state: for the mixed-parts hole, one OpenCode
message with one synthetic part and one authored part. For the role hole, a
synthetic assistant or tool message that is not the todo pair. For Pi, any
input at all.
Confidence: high - [evidence](evidence/codec-b-provenance-recovery-on-decode-is-all-or-nothing-and-opencode-only.md).
`codec/opencode.rs:1277-1279` read at `HEAD`:
`!parts.is_empty() && parts.iter().all(is_synthetic_part)`, so an empty-parts
message and a mixed-parts message both classify as authentic.
`is_synthetic_part` at `codec/sidecar.rs:454-462` accepts `synthetic` or
`syntheticTodoMarker`. Encoder side: `codec/opencode.rs:991-995` stamps
`synthetic: true` on every part only when
`msg.meta.synthetic && msg.role == "user"`; `render_synthetic_todo_pair` at
`:941-947` stamps `syntheticTodoMarker: true`. `codec/pi.rs:99` hardcodes
`synthetic: false` with no read of any input field. Part 4e is cited rather
than re-derived for the two halves it owns:
`part-4e-rendering/_lenses/lens-b-nudge-overlay.md:373-378` for the Pi encoder
writing no marker and having no production caller, and `:379-382` for
`HarnessMeta::synthetic` surviving on the wire while never reaching the
model. This record adds only the decode direction and the all-or-nothing
condition, neither of which appears in 4e.
Existing check: `codec/mod.rs:128-175` for the all-synthetic path, and
`codec/mod.rs:290-298` `fixture_builder_drives_synthetic_todo_wire_shape`,
which asserts `message["meta"]["synthetic"] == true` on the native fixtures.
Neither covers a mixed message. Status `unaudited`.
Impact: the module's own writes can come back classified as user-authored.
`meta.synthetic` gates `meta_for_ck`'s positional fallback
(`codec/sidecar.rs:446-450`), so a misclassified module-authored message
becomes eligible to inherit a native envelope by position, which is the failure
`codec/mod.rs:128-175` exists to prevent for the other direction. Pi's
hardcoded `false` means the Pi leg has no provenance at all in either
direction; combined with 4e's finding this leaves synthetic content
indistinguishable from authentic content for that harness at every layer.
Open questions:
- Is all-parts-synthetic the intended rule, or should any synthetic part mark
  the message? The `!parts.is_empty()` guard suggests the author considered
  degenerate cases, which makes the mixed case look unconsidered rather than
  decided. (needs human input)
- Should `codec/pi.rs:99` read a marker at all, given 4e's finding that the Pi
  encoder writes none? The two halves are consistent with each other and
  jointly inconsistent with the OpenCode leg.

## Group E: cross-stage composition and block identity

Three records that are only visible when two stages are read together. The
decoder's output must satisfy the projector's precondition and neither codec checks
that it does, so Part 1's "identity and schema rejection is one contract" becomes a
composition property here. The absolute ordinal every later decision indexes on is
harness-supplied and never validated. And the block-identity stamp is
caller-writable while the fingerprint is computed over the typed projection only,
so it is a change detector rather than an identity. All three live in or depend on
`codec/sidecar.rs`, whose only direct tests cover greedy and linear alignment
(`codec/sidecar.rs:487-557`) and not the stamp or fingerprint contracts.

### codec-b-decoder-output-can-violate-the-projector-precondition

Type: safety
Reachability: default-production
Status: active
Exercised: partial - `wire.rs:1122` and `:1149` assert `UnpairedToolResult`
is produced for two hand-built wire inputs, so the projector's rejection is
pinned. Nothing feeds decoder output to the projector, so the composition is
untested from either end.
Guarantee: Every value a harness decoder returns satisfies the preconditions
`project_messages` enforces, or the decoder rejects or repairs the input that
would violate them.
Check: `always` - for every decoder output,
`project_messages(&decoded.messages).is_ok()`. `always` because the projection
runs on every transform pass. This is Part 1's
`identity-and-schema-rejection-is-one-contract`
(`part-1-shm-transport/catalog.md:1375-1426`) reapplied across a stage boundary
instead of across two sibling readers: there, two decoders had to agree on one
condition set; here, a producer and a consumer must agree, and the producer
enforces nothing.
Fault/timing angle: none temporal. The structural angle is that a violation is
not local: `project_messages_from_state` returns `Err` on the *first* offending
message (`wire.rs:424-426`), which fails the entire projection and therefore
the whole pass, not just the one message.
Required faults and enabling state: two independent shapes, both
harness-controlled. First, one OpenCode message with `info.id` containing `#`,
or one Pi entry with such an `id` or `responseId`; the decoders copy it
verbatim into the mid and the projector rejects it. Second, a Pi `toolResult`
entry whose preceding `toolCall` entry was dropped by record three's mechanism,
which yields a `ToolResult` block with no pending call.
Confidence: high - [evidence](evidence/codec-b-decoder-output-can-violate-the-projector-precondition.md).
`wire.rs:324-337` enumerates the three error variants and all three are
constructed: `MidContainsReservedHash` at `:425`, `UnsupportedBlock` at `:585`,
`UnpairedToolResult` at `:660` and `:667`. Mid provenance traced:
`codec/opencode.rs:61-67` takes `string_field(info, "id")` with no validation,
and `codec/pi.rs:58-62` with `:710-715` takes `id` then `responseId` then a
timestamp then a synthesised fallback, again unvalidated. Only the last two
fallbacks are `#`-free by construction. For the pairing half, confirmed the
OpenCode decoder emits call and result adjacently in one message
(`codec/opencode.rs:496-541`), so it cannot produce an unpaired result from a
single part, while `codec/pi.rs:77-79` with `:86-90` makes each `toolResult`
its own message.
Existing check: partial and one-sided. `wire.rs:1122` and `:1149` cover the
projector's rejection with hand-built inputs. Nothing covers the mid rejection
at all, and no test composes a decoder with the projector. Status `unaudited`.
Impact: a single harness-supplied id containing one `#` character fails every
transform pass for that session until the message leaves the window. The
rejection is correct and fail-closed; the defect is that it is detected two
layers away from the layer that could have normalised it, and the error names a
reserved character the harness never agreed to avoid.
Open questions:
- Should the decoders normalise or reject `#` in a mid, so the failure is
  attributable to one message rather than the whole array? `wire.rs:369-372`
  documents the fallback-to-full-projection policy for out-of-range metadata;
  nothing analogous exists for a malformed mid.
- Is `#` reserved because `block_id` is `format!("{mid}#{index}")`
  (`wire.rs:513-515`)? If so the reservation is stricter than its own parser
  needs: `split_block_id` (`:517-521`) uses `rsplit_once('#')`, which
  round-trips a mid containing `#` correctly. So either the rejection defends a
  consumer other than `split_block_id`, or it is belt-and-braces. Unresolved;
  needs the set of `block_id` consumers, several of which are in 4b and 4c
  scope.

### codec-b-absolute-ordinal-is-harness-supplied-and-never-validated

Type: safety
Reachability: default-production
Status: active
Exercised: partial - `transform.rs:20278` supplies `"absolute_ordinal": 2_414`
and `:27809` and `:27942` supply `1` and `3`, so the explicit path is
exercised. Nothing supplies a duplicate or a zero, which are the producer's two
documented non-dense cases, and nothing asserts the relationship between
`max(ordinal)` and message count.
Guarantee: Every consumer of a decoded ordinal interprets it in the ordinal
space the producer emits, which is session-global, non-dense,
duplicate-permitting, and zero-inclusive.
Check: `always` - for every decoded array, any consumer computing a message
count from ordinals agrees with `decoded.len()`. `always` because the ordinal
is assigned to every decoded message on every pass. Stated over the consumer's
interpretation rather than over the decoder's validation, because the
producer's contract makes the decoder's verbatim pass-through correct.
Fault/timing angle: no temporal window in Rust. Cross-pass ordinal stability is
guaranteed on the producer side by a memo mismatch check
(`packages/plugin/src/hooks/eidnara/module-wire.ts:1041-1048` (source-catalog path, not present at HEAD)), not by
anything in this crate, so a producer change that dropped the memo would
destabilise every ordinal-keyed piece of Rust state with no Rust-side
detection.
Required faults and enabling state: none. A window into the tail of a long
session is the whole enabling state: the producer bases the numbering on a
canonical count (`module-wire.ts:1028-1031`), so a fifteen-message window of a
500-message session carries ordinals around 501-515. `module-wire.test.ts:180`
pins `absolute_ordinal: 501` as a real value.
Confidence: high - [evidence](evidence/codec-b-absolute-ordinal-is-harness-supplied-and-never-validated.md).
`codec/opencode.rs:52-60` read at `HEAD`; the fallback is
`provisional_base.saturating_add(index).saturating_add(1)`, so the fallback is
dense and monotonic and the explicit path is unconstrained. The producer was
then read, which changed the finding: `module-wire.ts:1027-1034` numbers from
`canonicalCount` or an explicit `provisionalBase`, never from an array index;
`:999-1018` states that a synthetic message "borrows the preceding canonical
ordinal instead of consuming a slot", so duplicate ordinals are deliberate;
`:1017` assigns `0` when there is no resolved predecessor.
`boundary.rs:687-691` computes `total_message_count` as `max()` with
`unwrap_or(ordered.len() as u64)` at `:691`, which is direct evidence the
consumer reads `max()` as a count. `codec/pi.rs:52` and `:45` confirmed to use
`decoded.len() + 1`, so Pi is dense-but-unstable where OpenCode is
stable-but-sparse.
Existing check: none for the invariant, in either language.
`codec/opencode.rs:246-281`'s incremental path and `lib.rs:12550-12563`'s
prefix validation both reason about positions, not ordinals. Status
`unaudited`.
Impact: this record answers the open question Lens A left for this lens
(`_lenses/lens-a-decision-units-and-config.md:589-593`), and the answer is that
max-as-count is wrong, not merely fragile: the producer's ordinal space is
session-global by design and permits duplicates by design, so
`boundary.rs:687-691` disagrees with the ingress contract for every windowed
session rather than only for a contrived one. Whether the resulting chunk
estimate is materially wrong is 4a's and 4b's call, since
`ChunkBuilder::finish` is theirs; the decoder's contribution is that it
faithfully passes through a space one consumer was not written for.
Open questions:
- Should `boundary.rs:687-691` take `ordered.len()` instead of `max()`, or does
  it genuinely want the highest ordinal for a different reason? Needs the
  `ChunkBuilder::finish` contract, which is 4a and 4b scope.
- Why does Pi have no `absolute_ordinal` equivalent, given that
  `codec-b-pi-decoder-drops-unrecognised-entry-types-without-a-record` makes
  its positional numbering unstable? Unresolved.
- Can an incremental suffix ever lack explicit ordinals?
  `decode_opencode_sidecar_incremental` passes `replace_from` (an array index)
  as `provisional_base` at `:260`, while the producer's base is a canonical
  count, so the two bases are in different spaces.
  `module-state-sync.test.ts:779` asserts some message has no
  `absolute_ordinal`, so I could not conclude the fallback is unreachable.

### codec-b-block-identity-stamp-is-caller-writable-and-the-fingerprint-is-not-an-identity

Type: safety
Reachability: default-production
Status: active
Exercised: not yet - `codec/sidecar.rs` has three direct `#[test]` functions
(`codec/sidecar.rs:487-557`), all of which exercise `match_block_metas` and
`greedy_block_metas` pairing. None constructs a caller-supplied stamp or two
byte-identical parts, so the stamp and collision halves are still covered only
incidentally through the two harness codecs' goldens.
Guarantee: The block-identity stamp that the encoder trusts to align a mutated
block with its native part is authentic, and the fingerprint stored beside it
distinguishes blocks that differ.
Check: `always` - for every block the encoder aligns via a stamp, that stamp
was written by `stamp_block_identity` during this decode, and no two distinct
native parts in one message share a fingerprint without the stamp separating
them. `always` because the alignment runs for every block of every encoded
message.
Fault/timing angle: none temporal, but the ordering inside `push_block` is
load-bearing and undocumented: `codec/opencode.rs:553-554` and
`codec/pi.rs:303-304` compute the fingerprint *before* stamping, so the
fingerprint is deliberately stamp-independent, which is what makes it stable
across passes and also what makes it collide for identical content.
Required faults and enabling state: for the collision half, one OpenCode
message with two byte-identical parts. For the trust half, a wire ingress message
carrying `provider_extras["_eidnara_codec"]` with plausible `blockIndex`,
`nativeIndex`, and `decodedFingerprint` values; `TransformRequest.messages` is
`Vec<IngressMessage>` (`transform.rs:781`) and `WireBlock`'s `Deserialize`
(`memory-store/src/lib.rs:207-221`) reads `provider_extras` verbatim.
Confidence: high - [evidence](evidence/codec-b-block-identity-stamp-is-caller-writable-and-the-fingerprint-is-not-an-identity.md).
`codec/sidecar.rs:148-151` gives the namespace and three keys as plain string
constants. `stamped_block_identity` at `:196-203` reads them back with no
provenance check; `alignment_candidate` at `:204-211` returns early on a stamp
match, never consulting `kind_matches`, so a forged stamp outranks the kind
check. `decoded_block_fingerprint` at `:151-156` calls
`canonical.mark_modified()` at `:154`, which clears `original`
(`memory-store/src/lib.rs:261-263`), so the hash covers `kind` plus
`provider_extras` only and is blind to the retained pass-through bytes the wire
contract at `memory-store/src/lib.rs:92-95` exists to preserve.
`block_is_unchanged` at `:192-196` is fingerprint-only. The mitigating fact was
checked and holds: the harness decoders never route input into
`_eidnara_codec`, since `block_with_metadata` (`codec/opencode.rs:567-577`)
writes under the `"opencode"` key, so the forged-stamp path is reachable from
wire ingress and not from harness ingress.
Existing check: none in `codec/sidecar.rs`. `codec/opencode.rs:1515-1582` and
`codec/pi.rs:1436-1443` exercise alignment after a block deletion and an encode
replay, which covers the honest path. Status `unaudited`.
Impact: two shapes. The forged stamp lets a wire caller point a block at a native
part it did not come from, and `alignment_candidate`'s early return means the
kind check that would otherwise catch the mismatch is skipped, so the encoder
can write a text block's content into a reasoning part. The fingerprint
collision is contained today because the stamp disambiguates duplicates, which
makes the stamp the sole load-bearing disambiguator for a case the fingerprint
cannot handle: if the stamp were ever dropped from the pass-through path,
duplicate-content blocks would align by the `:225-227` positional fallback
instead, silently.
Open questions:
- Should the stamp carry a per-decode nonce so a stamp from a prior pass or a
  foreign caller is distinguishable? The comment at `:243-247` says the stamps
  "survive reductions, overlays, and deletion compaction", which is the
  property that makes them useful and also the reason they cannot be validated
  by age.
- Which of the three serialization-failure policies is normative? `:155` maps a
  failure to `Value::Null`, `:293` maps it to empty bytes, and
  `wire.rs:585-589` maps it to `WireError::UnsupportedBlock`. The first
  two collapse every failing block onto one hash, which `block_is_unchanged`
  would then read as "unchanged".

## Group F: release behaviour of the codec guards

Two records on the three `debug_assert!` sites, which are the only runtime
assertions in the 4f production halves. The first covers the incremental sidecar
slice, where the adjacent pair of assertions diverge under release: one condition
is re-checked by the language and one is absorbed by a saturating `take`. The
second covers the encode-side uniqueness guard, which has a single arm and
therefore enforces nothing in release while three production call sites depend on
it. Both records must name the profile they hold in, and the shipped profile's
behaviour is the one with no test in either case.

### codec-b-incremental-sidecar-slice-panics-behind-a-debug-assert

Type: safety
Reachability: default-production
Status: active
Exercised: not yet - no test calls `decode_opencode_sidecar_incremental` with
`replace_from > messages.len()`, and no test calls it in a release build.
Guarantee: `decode_opencode_sidecar_incremental` returns a sidecar or a
declared error for every `(messages, prior, replace_from)` triple, including
triples its callers cannot currently produce.
Check: `always` - for arbitrary `replace_from`, the call returns without
panicking. `always` rather than `always-or-unreached` because the function is
called on every native-attachment pass with a cached snapshot and a non-zero
trusted prefix; the *out-of-range* argument is what is currently unreachable,
and that unreachability is a caller property, not a callee property.
Fault/timing angle: none in the callee. The window that matters is a
maintenance window rather than a runtime one: the bound is enforced two frames
up, in a different file, by two separate filters, and neither cites the callee.
Required faults and enabling state: a caller passing
`replace_from > messages.len()`. Reaching it today requires either a new
caller, or `validated_native_prefix`'s `:12561` filter changing, or
`native_sidecar`'s `:12576` condition changing. In a debug build the
`debug_assert!` fires first; in release the slice index panics with "range
start index out of range".
Confidence: high - [evidence](evidence/codec-b-incremental-sidecar-slice-panics-behind-a-debug-assert.md).
`codec/opencode.rs:251-258` read at `HEAD`:
`debug_assert!(replace_from <= messages.len())` then
`&messages[replace_from..]`. Both callers traced and both confirmed to enforce
the bound: `lib.rs:12550-12563` filters `*replace_from <= native_len` at
`:12561`, and `lib.rs:12565-12585` gates the call on
`trusted_prefix > 0 && trusted_prefix <= snapshot.sidecar.order.len()` at
`:12576`. Note the second condition bounds `replace_from` against the *sidecar
order length*, not against `messages.len()`; the `messages.len()` bound arrives
only via `validated_native_prefix`, so the two `debug_assert!`s at `:251` and
`:252` are discharged by two different callers' checks.
Existing check: none for the bound. `lib.rs:12452-12453` and `:12457-12459`
define `CorruptSidecarForTest` and `CorruptFrontierForTest` modes, and
`:12531-12541` deliberately perturbs the projection prefix by `+1` under
`cfg(test)` and then re-clamps with `prefix <= projection.message_count()` at
`:12543`. That machinery proves the authors thought about a corrupted prefix on
the projection path and built a test hook for it; no equivalent hook exists for
the sidecar slice. Status `unaudited`.
Impact: a panic inside the transform on the default production path. This is
the same shape as Part 1's observation that "narrowing `GRANT_BYTES` turns
`ring.rs:430` into an unconditional panic on every call, and no property
currently forbids either" (`part-1-shm-transport/catalog.md:1322-1324`): the
reasoning that keeps the call safe lives only in the callers, and nothing in
the tree records that the callee depends on it.
Open questions:
- Should the function clamp with `messages.len().min(replace_from)` and fall
  back to a full decode, matching the documented policy at `wire.rs:369-372`
  that "malformed or out-of-range local metadata falls back to a full
  projection rather than trusting a partial result"? The projection path
  already does this; the sidecar path does not. (needs human input)

### codec-b-wire-level-tool-use-uniqueness-guard-has-no-release-behaviour

Type: safety
Reachability: default-production
Status: active
Exercised: not yet - `codec/opencode.rs:1486-1513` asserts that two encodes of
the same input produce identical tool parts, which exercises determinism rather
than the duplicate guard. No test constructs a duplicate `callID` in the
encoded array.
Guarantee: The encoded OpenCode array contains no two `tool` parts sharing a
`callID`, or the duplicate is removed before the array is returned.
Check: `always(!duplicate)` - for every returned `Vec<MessageV2Json>`, the
multiset of `callID` values across all `tool` parts has no repeats.
`always(!X)` and not `unreachable`, per METHOD's rule: the forbidden thing is a
*state* of the returned array, and the guard at `codec/opencode.rs:462-470` is
not a code point that must never execute, it is a check that must never find
anything.
Fault/timing angle: none temporal. The ordering that matters is layer ordering:
the wire-level guard runs first on `ServedMessage` (`transform.rs:12147`), the
wire-level guard runs last on the encoded JSON (`codec/opencode.rs:370`). A
duplicate introduced *by encoding* is visible only to the second guard, and the
encoder's own comment at `:750-753` describes exactly that case: "two
independently emitted shells carry the same callID".
Required faults and enabling state: a release build (`debug_assertions` off)
plus an input reaching the
`parts.push(render_tool_pair_as_part(block, result))` arm at `:754` for a call
id that another message already emitted. The comment at `:749-757` says this
arm exists because neither half matched a native index, which is the
fresh-shell case.
Confidence: high - [evidence](evidence/codec-b-wire-level-tool-use-uniqueness-guard-has-no-release-behaviour.md).
`codec/opencode.rs:462-470` read at `HEAD`: the body is
`let duplicates = ...; debug_assert!(duplicates.is_empty(), ...)` and nothing
else, so in release the function computes a `Vec` and discards it. Compared
against `transform.rs:11231-11249`, which `debug_assert!`s at `:11246` and then
has `#[cfg(not(debug_assertions))]` at `:11251` opening a heal branch that
drops the later duplicate and its paired result. So the two same-named guards
diverge in release, and the divergence is in the direction that leaves the wire
unprotected. Two independent `duplicate_tool_use_locations` implementations
exist, `codec/opencode.rs:438-460` over `MessageV2Json` and
`transform.rs:11235` over `ServedMessage`.
Existing check: partial and at the wrong layer. `transform.rs:21509` and
`:21522` exercise `enforce_unique_tool_use_ids` including its heal path.
Nothing exercises `assert_unique_tool_use_ids`. Status `unaudited`.
Impact: a provider request containing two `tool_use` blocks with one id, which
Anthropic-shaped providers reject outright, so the failure mode is a hard
request error rather than a degraded reply. The debug build catches it and the
shipped build does not, which is the inverse of what a wire-level invariant
wants. The guard is also applied inside `encode_opencode_impl` rather than in
the chunk API, so `lib.rs:12949`'s direct call to
`encode_opencode_chunks_with_transition_state` on the incremental native path
has no uniqueness check in any build profile.
Open questions:
- Should the wire-level guard adopt the wire-level heal branch, or should the
  wire-level heal be removed in favour of failing loud in both? The two layers
  currently encode two different answers to the same question. (needs human
  input)
- The scope map (`part-4-module/_lenses/scope-map-and-risk-ranking.md:603`)
  describes `enforce_unique_tool_use_ids` as one of two "fail-loud production
  checks". At `HEAD` it is a `debug_assert!` plus a release heal, so it is
  fail-loud in debug and fail-quiet-and-repair in release. 4e owns that
  function; flagged here as a lead only.

## Group G: round-trip claims and declared coverage gaps

Three records on what the goldens prove. The round-trip claim is made in one
direction on one case per harness, against an oracle derived from the test's own
input, so it cannot detect a decode error the encoder symmetrically reverses. The
coverage manifest is then designed to pass without two block classes it declares
required. And the Pi encoder can return a shorter array than it was given, which is
the failure the round-trip claim would have to be strengthened to catch. Read
together they say the codec suite measures agreement of the code with itself.

### codec-b-round-trip-identity-is-claimed-in-one-direction-on-one-case-per-harness

Type: safety
Reachability: default-production
Status: active
Exercised: partial - one golden case per harness, `codec/mod.rs:78-89` and
`:201-212`, each asserting decode-then-encode against the input array. The
reverse direction is asserted nowhere, and both goldens declare their oracle
incomplete.
Guarantee: The direction each codec actually claims is decode-then-encode byte
identity modulo a declared exception set; encode-then-decode is explicitly not
the identity, and the exception set is complete.
Check: `always` - for every accepted input array,
`encode(decode(input)) == input` after removing exactly the declared exceptions
(`compaction` parts for OpenCode via `codec/mod.rs:273-281`, whole `compaction`
entries for Pi via `:283-288`). Stated as `always` and in one direction only,
because the other direction is provably false: `codec/mod.rs:112-125` pins four
wire messages encoding to three wire messages.
Fault/timing angle: none. The angle that matters is oracle strength, not
timing. Both goldens carry `projection_oracle.status: "todo"` with a reason
stating the harness serializer "is not vendored in the Rust workspace test
closure", so the oracle compares against the retained input array and not
against provider wire bytes.
Required faults and enabling state: none for the claimed direction. To make the
oracle meaningful, an input containing a shape the retained-raw path does not
cover: an unrecognised part or entry type (observations 5, 6, 21), or a mutated
block, since an unmutated block short-circuits at `codec/opencode.rs:763-765`
and `codec/pi.rs:463-465` and is trivially identical.
Confidence: high - [evidence](evidence/codec-b-round-trip-identity-is-claimed-in-one-direction-on-one-case-per-harness.md).
Both goldens parsed at `HEAD`: `cases` has length 1 in each, with 10 OpenCode
messages and 11 Pi entries. `projection_oracle` reasons quoted in observation
20. The asymmetry between the two OpenCode encoders was verified:
`encode_opencode` passes `preserve_compaction: false` (`:289-290`) and
`encode_opencode_with_session` passes `true` (`:305-306`), which is why the
golden's oracle must strip compaction while the native-serving golden at
`codec/mod.rs:125` compares `&encoded[3..]` to the raw messages unstripped. The
four-to-three collapse was traced to `render_synthetic_todo_pair` (`:916-948`)
via `:388-399`.
Existing check: `codec/mod.rs:54-90` and `:177-213`, plus determinism
assertions at `:81`, `:87`, `:204`, `:210`. Genuine oracles, not tautologies:
they compare against an independently captured input array (`generated_from`
names a real `opencode.db` and real Pi JSONL session files), which is
materially stronger than the round-trip assertion Part 1 found at
`harness.rs:112-116` and characterised as "a tautology over accepted inputs"
(`part-1-shm-transport/catalog.md:1360-1361`). The weakness here is breadth and
oracle fidelity, not vacuity. Status `unaudited`.
Impact: one case per harness with a self-declared placeholder oracle is the
entire evidence base for the property the whole encoder design rests on. The
specific gap that matters is that the retained-raw path makes identity nearly
automatic for unmutated input, so the test's pass carries much less information
than its name implies.
Open questions:
- Should the exception set be declared in code rather than reconstructed in the
  test's own helpers (`codec/mod.rs:273-288`)? Today the encoder's compaction
  policy and the test's stripping helper are two independent statements of one
  rule.
- Can the `projection_oracle` TODO be discharged without vendoring the harness
  SDKs? If not, the goldens' status is permanent and should say so.

### codec-b-declared-missing-capture-classes-are-never-decoded

Type: reachability
Reachability: default-production
Status: active
Exercised: not yet - by construction. The classes are recorded as missing
precisely so that no case supplies them.
Guarantee: Every capture class the golden names as required is actually decoded
by at least one case, so the decode arm that handles it is executed.
Check: `reachable` - the decode arms at `codec/opencode.rs:171-181` (`subtask`)
and `codec/pi.rs:199-211` (redacted thinking) are executed at least once per
campaign. `reachable` and not `sometimes`, because the obligation here is
location coverage: the arms exist, are named as required, and are provably
never entered by the suite that claims to cover them.
Fault/timing angle: none.
Required faults and enabling state: one OpenCode message with a `subtask` part;
one Pi assistant entry with a `thinking` part carrying `redacted: true`.
Confidence: high - [evidence](evidence/codec-b-declared-missing-capture-classes-are-never-decoded.md).
`codec/mod.rs:254-271` read at `HEAD`: the filter at `:262-266` retains a
required class only when it is absent from both `coverage` and
`recorded_missing`, so membership in `missing_capture_classes` satisfies the
assertion. Both golden files parsed: `opencode-golden.json` has
`missing_capture_classes: ["subtask"]` against a required list including
`"subtask"` (`codec/mod.rs:72`), and `pi-golden.json` has
`["redacted_thinking"]` against a required list including `"redacted_thinking"`
(`:187`). The two decode arms were read and confirmed to be the only handlers
for those shapes.
Existing check: the mechanism is the check, and it is the thing being reported.
`codec/mod.rs:267-270`'s message, "codec golden neither covers nor records
missing classes", is honest about what it enforces: it is a bookkeeping gate,
not a coverage gate. Status `unaudited`.
Impact: `subtask` decoding is on the default production path and untested; a
`subtask` part currently becomes an opaque block via `:171-181`, and if that
arm were deleted the part would fall to `:194-204` and still become an opaque
block, so the golden would not move. Pi's redacted-thinking arm is the one with
a behavioural difference to lose: `:199-211` produces
`BlockKind::RedactedReasoning` while the non-redacted branch at `:212-217`
produces `BlockKind::Reasoning` with a signature, and the two round-trip through
different encoder arms (`:543-548` versus `:536-542`).
Open questions:
- Is `missing_capture_classes` intended as a temporary ledger with an owner and
  a date, or as a permanent waiver? Nothing in `codec/mod.rs` or either golden
  says. (needs human input)

### codec-b-pi-encoder-can-return-a-shorter-array-than-it-was-given

Type: safety
Reachability: test-only
Status: active
Exercised: partial - `codec/pi.rs:1469-1484`
`deleted_tool_result_does_not_replay_the_retained_raw_entry` clears a
tool-result message's content and asserts `encode_pi(...).is_empty()`, which
pins the `:371` drop for the fully-cleared case. The same drop with content
that survives but holds no `ToolResult` is uncovered, the `:396-397` drop is
uncovered, and no test asserts what a caller should conclude from the shortened
array.
Guarantee: Either `encode_pi` returns one entry per input message, or the
positions it dropped are recoverable by the caller.
Check: `always` - `encode_pi(msgs, sidecar).len() == msgs.len()`, or the return
type carries the dropped indices. `always` because the `filter_map` runs on
every call.
Fault/timing angle: none. The composition risk is index drift: callers that
pair an encoded entry with the wire message at the same index are wrong after the
first drop, and the OpenCode encoder's parallel API returns
`EncodedOpencodeChunk` values carrying explicit `start_index` and `end_index`
(`codec/opencode.rs:343-348`) precisely so that its own collapse is index-safe.
Pi's has no equivalent.
Required faults and enabling state: for the `:371` drop, a message whose meta
role is `toolResult` but whose wire content holds no `ToolResult` block, which
the transform can produce by reducing a decoded tool-result message. For the
`:396-397` drop, a wire message with empty `content` whose matched meta's raw is
not a Pi message; this may be unreachable, since only `decode_opaque_entry`
produces such a raw and those messages carry exactly one opaque block.
Confidence: high - [evidence](evidence/codec-b-pi-encoder-can-return-a-shorter-array-than-it-was-given.md).
`codec/pi.rs:128-137` read at `HEAD`: `filter_map` over `encode_with_meta`
(returns `Option<Value>`) and `encode_new_message` (returns `Value`, wrapped in
`Some` at `:134`). The two `None` returns are `:371`'s `find(...)?` and
`:396-397`'s explicit `return None`. Contrasted against
`codec/opencode.rs:428-433`, which pushes a chunk for every message
unconditionally, and against `EncodedOpencodeChunk` (`:343-348`), whose
`start_index`/`end_index` fields exist so `lib.rs:12949-12961` can splice by
position. Reachability label fixed by `rg` over `crates/` and `packages/`:
`encode_pi` appears only in `codec/pi.rs` and in `codec/mod.rs:208-209` and
`:249`, all inside `#[cfg(test)]`; 4e reached the same conclusion independently
at `part-4e-rendering/_lenses/lens-b-nudge-overlay.md:373-378`.
Existing check: `codec/pi.rs:1469-1484`, which pins the cleared-content drop.
Status `unaudited`.
Impact: today, none, because there is no production caller. The record exists
because the function is a public export (`codec/mod.rs:10`, `lib.rs:12`) whose
contract differs from its OpenCode twin in a way a future caller would not
expect, and because 4e's lens item 18 already notes the Pi encode path is
off-route, which makes this the moment to write the contract down rather than
after it is wired up.
Open questions:
- Should `encode_pi` adopt the `EncodedOpencodeChunk` shape so index mapping is
  explicit? Unresolved, needs a decision about whether the Pi leg is being
  wired up at all.

## Relationship map

Grouped by shared mechanism rather than by the group headings above, because the
mechanism is what decides whether one check can stand in for another. Every
dominance statement is a hypothesis offered to guide ordering, not a verified
claim. Existing checks and their execution are recorded in
[existing-checks.md](existing-checks.md); adequacy remains `unaudited`.

- **One document, one resolver, and every way they disagree.**
  [dec-a-execute-threshold-lower-bound-is-documented-20-and-enforced-1](#dec-a-execute-threshold-lower-bound-is-documented-20-and-enforced-1),
  [dec-a-memory-injection-budget-documented-range-has-no-implementing-code](#dec-a-memory-injection-budget-documented-range-has-no-implementing-code),
  [dec-a-commit-cluster-trigger-config-is-inert-in-this-crate](#dec-a-commit-cluster-trigger-config-is-inert-in-this-crate),
  [dec-a-config-value-clamps-and-zero-rejection-are-invisible-to-the-caller](#dec-a-config-value-clamps-and-zero-rejection-are-invisible-to-the-caller),
  [dec-a-project-tier-can-write-leaves-outside-the-documented-allow-list](#dec-a-project-tier-can-write-leaves-outside-the-documented-allow-list).
  Five instances of one absent artifact: nothing in the tree compares the resolved
  config against `CONFIGURATION.md` (source-catalog path, not present at HEAD). Hypothesis: a single table-driven conformance
  check, one row per leaf carrying documented default, documented bound, expected
  tier policy and expected warning, *dominates all five and is cheaper than any one
  of them*, because each record's oracle is already a row in the table above. What
  it would not cover is the clamp record's second half, that the caller is never
  told, since that is a property of the response rather than of the resolved value.
- **The chain that spends the money.**
  [dec-a-model-key-lookup-walk-has-two-implementations-that-disagree](#dec-a-model-key-lookup-walk-has-two-implementations-that-disagree),
  [dec-a-model-chain-dedup-is-adjacent-only](#dec-a-model-chain-dedup-is-adjacent-only),
  [dec-a-malformed-config-silently-resolves-to-defaults-and-stops-the-historian](#dec-a-malformed-config-silently-resolves-to-defaults-and-stops-the-historian).
  These are distinct concerns, not three open model-chain defects. Full
  deduplication removes repeated models, and unusable files produce path-bearing
  warnings. Those two defect premises are invalidated, while their regression
  contracts remain. The per-model lookup differential has a different input and
  oracle.
- **Guards that hold, recorded so a later change is visible.**
  [dec-a-cache-ttl-parse-is-total-over-arbitrary-strings](#dec-a-cache-ttl-parse-is-total-over-arbitrary-strings),
  [dec-a-boundary-budget-derivation-is-total-over-non-finite-input](#dec-a-boundary-budget-derivation-is-total-over-non-finite-input),
  [dec-a-derive-historian-chunk-tokens-is-total-at-both-integer-extremes](#dec-a-derive-historian-chunk-tokens-is-total-at-both-integer-extremes),
  [dec-a-escalation-bands-stay-ordered-for-every-threshold](#dec-a-escalation-bands-stay-ordered-for-every-threshold),
  [dec-a-selection-decision-order-is-total-under-hashmap-iteration](#dec-a-selection-decision-order-is-total-under-hashmap-iteration).
  The cheapest cluster in the part by a wide margin: five pure-function properties
  over an input domain, no faults, no interleavings, and four of them expressible as
  a property test in a few lines. Hypothesis: none dominates another, and their value
  is not defect-finding but boundary-fixing, since Part 3's analogous unit failed
  exactly here. The one live hazard sits inside the first: a `cache_ttl` of `"0"`
  parses to zero ms and expires when its timestamp preconditions hold, subject to
  later scheduler gates. The totality record and configuration cluster meet on
  one key without implying unconditional execution.
- **The two doors in the guard wall, and they are different shapes.**
  [dec-a-region-hint-clamp-bypassed-by-sentinel-suffix](#dec-a-region-hint-clamp-bypassed-by-sentinel-suffix),
  [dec-a-caller-supplied-trigger-budget-is-the-one-unvalidated-float-and-reaches-a-diagnostic](#dec-a-caller-supplied-trigger-budget-is-the-one-unvalidated-float-and-reaches-a-diagnostic).
  This cluster was one record and is now two, and pairing them is what makes the
  pattern legible. The region-hint clamp is a guard that *is* a door: the same
  idempotence property that makes it safe to re-apply is what lets a value carrying
  the sentinel through unclamped, so the defect is inside a correct-looking
  mechanism. The `trigger_budget` passthrough is the opposite shape: it is a door
  where the wall simply stops. Three neighbouring fields on the same struct are
  `is_finite`-gated and this one is not, so there is no clever mechanism to
  misunderstand, just a missing gate at `boundary.rs:377-379` and again at
  `:756-761`. Hypothesis: no dominance between them, and neither belongs in the
  guards cluster above, because a property test over the guarded domain passes
  while both defects stand. The trigger-budget one is additionally the cheapest
  falsifying oracle in the whole part — one struct literal and one call — and it
  fails today, which the guards cluster's oracles by definition do not.
- **What the decoder accepts, and who never learns.**
  [codec-b-harness-decoders-accept-every-input-with-no-rejection-channel](#codec-b-harness-decoders-accept-every-input-with-no-rejection-channel),
  [codec-b-pi-decoder-drops-unrecognised-entry-types-without-a-record](#codec-b-pi-decoder-drops-unrecognised-entry-types-without-a-record),
  [codec-b-opencode-hides-four-part-types-from-every-transform-decision](#codec-b-opencode-hides-four-part-types-from-every-transform-decision),
  [codec-b-provenance-recovery-on-decode-is-all-or-nothing-and-opencode-only](#codec-b-provenance-recovery-on-decode-is-all-or-nothing-and-opencode-only).
  Hypothesis: the no-rejection-channel record *dominates the other three as a
  framing* and none of them as a check, because each names a different disposal.
  Pi discards bytes, OpenCode retains bytes and hides the decision, and provenance
  recovery either works wholly or not at all on one harness. The pairing that matters
  for a reviewer is Pi against OpenCode on the same input class: two codecs in one
  crate hold opposite policies for the same situation, and the wire layer's stated
  contract endorses only one of them.
- **Two stages, one unchecked precondition.**
  [codec-b-decoder-output-can-violate-the-projector-precondition](#codec-b-decoder-output-can-violate-the-projector-precondition),
  [codec-b-absolute-ordinal-is-harness-supplied-and-never-validated](#codec-b-absolute-ordinal-is-harness-supplied-and-never-validated),
  [codec-b-block-identity-stamp-is-caller-writable-and-the-fingerprint-is-not-an-identity](#codec-b-block-identity-stamp-is-caller-writable-and-the-fingerprint-is-not-an-identity).
  All three are properties of the seam rather than of either side, and all three
  route through `codec/sidecar.rs`, whose three direct tests cover alignment
  pairing only (`codec/sidecar.rs:487-557`). Hypothesis: the projector
  precondition record *hypothetically dominates* the ordinal record, because a
  composition check that decodes then projects and asserts the projector accepted
  would also catch an ordinal the projector rejects; it does not dominate the
  identity record, because a caller-written stamp with a fingerprint that ignores
  retained bytes is accepted by the projector by construction.
- **The profile decides whether the guard exists.**
  [codec-b-incremental-sidecar-slice-panics-behind-a-debug-assert](#codec-b-incremental-sidecar-slice-panics-behind-a-debug-assert),
  [codec-b-wire-level-tool-use-uniqueness-guard-has-no-release-behaviour](#codec-b-wire-level-tool-use-uniqueness-guard-has-no-release-behaviour).
  The cross-part tie is the point of this cluster: 4e's
  `enforce_unique_tool_use_ids` has two arms and two tests, and this part's
  encode-side counterpart has one arm and one debug-gated test, so the same
  invariant is defended on the render side and abandoned on the encode side.
  Hypothesis: neither dominates the other, and both are blocked on the same
  unresolved question, which build profile the distributed artifact uses. Until that
  is answered every record in this cluster has two readings.
- **Agreement of the code with itself.**
  [codec-b-round-trip-identity-is-claimed-in-one-direction-on-one-case-per-harness](#codec-b-round-trip-identity-is-claimed-in-one-direction-on-one-case-per-harness),
  [codec-b-declared-missing-capture-classes-are-never-decoded](#codec-b-declared-missing-capture-classes-are-never-decoded),
  [codec-b-pi-encoder-can-return-a-shorter-array-than-it-was-given](#codec-b-pi-encoder-can-return-a-shorter-array-than-it-was-given).
  Hypothesis: the round-trip record *hypothetically dominates* the shorter-array
  record, because a strengthened round trip over generated inputs would catch a
  `filter_map` that drops a message, and it dominates neither of the others: the
  declared-missing record is about a gate that is designed to pass, which no
  strengthening of the existing oracle reaches. The cluster's shared cause is one
  design choice, that the expected value in both goldens is a transformation of the
  test's own input, so the suite cannot distinguish a decode error the encoder
  symmetrically reverses.

### Cross-part relationships

Three ties are strong enough to state, and one of them answers a sibling's open
question.

**4e asked whether one wire message can carry a full-drop tool block followed by two
or more taggable blocks, and left it to 4f.** This catalog does not resolve it. The
OpenCode decoder collapses a wire tool call plus its result into one native part and
hides four part types from `content`, so the shape of a decoded message is decided
by `codec/opencode.rs:193` and `:194-204` rather than by anything 4e can see. What
is established here is narrower and still useful to 4e: because the four hidden
types never enter `content`, they cannot be the taggable blocks 4e's index-shift
record needs, so the question reduces to whether two *retained* taggable blocks can
follow a full-drop tool block in one message. Recorded as unresolved on both sides
rather than answered.

**Build-profile and parity questions need their own evidence.** The CI payload
smoke builds `eidnara-host` without `--release`
(`.github/workflows/ci.yml:625-629`); that does not identify every distributed
artifact's profile. Likewise, workspace execution of frozen-vector checks does
not establish a live cross-language comparison. The original cross-part
investigations remain historical leads, not blockers on config-test execution.
