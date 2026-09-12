# Wildcard surface

The five sibling lenses cover the request ingress decode and admission path,
the deep copies inside the transform pass, the `cache_state` load and commit
traffic with its pass trace and side channel, the TypeScript plugin's pre-send
work, and the ring arena, direct serialize, and CAS byte accounting. The
parent catalog covers canonical-memory pushdown (K1-K4), execution placement
(E1-E3), callback batching (S1-S2), history-budget render (H1-H4), and
prepared-field ownership (R1-R3). The portfolio evaluation has already queued
the SOFT pressure-refold predicate (finding 10) and commit-versus-bookkeeping
consistency under a blocking worker (finding 9).

This pass hunted outside those frames: the measurement contract that would
make "faster" a checkable claim, the process-global token-count cache and the
estimator interface, the secret scanner's whole-input regex pass, the
historian firing inputs, the cron stepper on the scheduler task, the per-pass
config merge, and the repository-level constraints that bind any optimization
regardless of stage. Anchors are checked in `/local/home/ahrav/scratch/eidnara`
at `913234433ae36a80a6e22c6aac14c7f9aab74386` on 2026-09-10. This is analysis
only; no test ran and nothing outside this file changed.

## Candidate properties

### optimized-stage-is-measured-at-production-shape

- Type: reachability
- Check: `sometimes` - For every stage the specification authorizes to change,
  a recorded measurement run reaches that stage with an input in the
  production size class, before and after the change, under one build, host,
  and workload identity, and the record names the stage and the input. This
  is situation coverage: a stage's lines execute under a 69-byte body or a
  100-message request without the production state ever occurring, so
  `reachable` is the wrong semantics.
- Guarantee: "the optimization is faster" is a claim with an artifact behind
  it rather than an assertion.
- Rationale: No CI performance gate exists. [`ci.yml`][ci-bench] runs every
  bench target once in test mode as a smoke check and compares nothing;
  [`.config/nextest.toml`][nextest] excludes bench targets from nextest. The
  daemon bench's own header says [one process's numbers are not a
  baseline][hp-header]. Its end-to-end arms call
  [`transform_cached`][hp-e2e], which is
  [`transform_with_projection_cached(store, req, ctx, cache, None)`][bi-tc]
  on an already-typed request built by [`serde_json::from_value`][hp-req]
  against a [fresh tempfile store][hp-store]. The production handler wraps
  that call with work the bench never sees: the projection-cache lookup,
  side-channel drain, and `trace_pass_received` at [8120-8131][h-pre], the
  `project_memory` read, `historian_active`, and guidance-date lookups inside
  [`run_transform`][h-run], a `Some` projection-cache input at
  [8186-8192][h-call], `prepare_historian_fire`, and response encoding in
  [`respond_transform`][respond]. The bench tops out at 1_000 messages because
  the store's 512 KiB durable-text bound rejects a 1_400-message first HARD
  pass ([hot_path.rs:30-35][hp-counts], [267-269][hp-cliff]); that cliff is
  pinned by [`transform_meta_bound.rs`][meta-bound]. The two
  production-sized fixtures that do exist are `#[ignore]` and print to stderr:
  [`apply_once_stage_timings_large_fixture`][fx-1400] at 1_400 messages and
  [`full_module_pass_timing_fixture`][fx-2500] at 2_500 messages and 47_075
  frozen units. The transport bench measures a fixed [256-byte smoke or
  4096-byte campaign payload][he-payload] and [rejects
  `--designated-host`][he-designated], while its
  [manifest][he-manifest] declares 24 byte-size probes and three workload
  classes; the bench's own record says
  [`designated_host_verdict: BLOCKED`][he-blocked]. The host-runtime harness
  uses a [69-byte fixture body][pm-body] through
  [`FIXTURE_BODY.to_vec()`][ring-body]. Production does emit per-stage
  numbers: [`TransformTimings`][tt] rides in every transform response,
  [`emit_pass_timing`][emit] prints an `eidnara-pass-timing` line, and the
  plugin logs [`rust module stages:`][ts-stages]; none of that is recorded
  as a comparable artifact. The host-runtime bench's
  [`evidence.rs`][evidence] already implements the manifest discipline
  (schema, workload, build, host, and arm identifiers, checksummed sidecars)
  that a daemon record would need.
- Fault/timing angle: none; a coverage record.
- Required faults and enabling state: A session at the production size class
  (the bench header's 1_400-message, 2 KiB mixed point, or the fixture's
  2_500), reached by incremental growth because a first pass cannot commit
  it; an ingress body through `Handler::handle`, not a typed request; a warm
  store with an existing row for steady passes and a cold store for the first
  pass; a 64 MiB direction arena for the ring probes.
- Reachability: test-only - benches need `--features bench-internals`
  ([Cargo.toml:62-75][cargo-bench]) or `harness = false`, run in CI only in
  test mode, and the production-sized fixtures need manual `--ignored` runs.
- Existing check: [`ci.yml:514-518`][ci-bench] (smoke only);
  [`evidence.rs`][evidence] manifests for host-runtime arms;
  [`pass_timing_line_is_parseable_for_an_empty_session`][t-line];
  [`timings_are_present_and_old_responses_deserialize_without_them`][t-timings];
  the plugin test [`emits discriminating pass and stage logs`][ts-test]. None
  records a daemon measurement or compares two builds.
- Open questions:
  - Which size class is "production-shaped": the bench header's 1_400 or
    the fixture's 2_500 messages? (needs human input)
  - Does the specification adopt an `evidence.rs`-style manifest for the
    daemon, or accept the stderr timing line as the record? (needs human
    input)
  - The bench comment attributes the 512 KiB cliff to `meta` growing about
    460 bytes per message; the cliff is pinned at 1_000 ok and 1_400
    refused, the per-message figure is not verified here.

### stage-timing-fields-keep-their-boundaries

- Type: safety
- Check: `always` - Every field named by [`format_pass_timing_line`][fmt] and
  by the plugin's [`rust module stages:` line][ts-stages] exists in
  [`TransformTimings`][tt]; after a relocation each field's start and stop
  instants still bracket the same work, and a field for a removed or merged
  stage is removed from the struct and both consumers rather than left to
  report `0.0` through `#[serde(default)]`; the two `local_stats` reads that
  form the token-cache delta run on one thread.
- Guarantee: A stage that reports a smaller number after the change got
  faster rather than moving out from under its timer.
- Rationale: Every `TransformTimings` field carries `#[serde(default)]`
  ([1018-1197][tt]), so a dropped field deserializes as zero and the plugin
  prints `n/a` only when the key is absent from the JSON object
  ([1019-1024][ts-stage-fn]). The handler assigns its stage fields from
  `Instant` pairs taken on the handler task at [8463-8488][h-timings]; a
  `spawn_blocking` relocation separates those pairs from the worker's.
  [`record_token_cache_delta`][rtcd] subtracts two reads of the
  [thread-local counters][tc-local]; both reads sit inside the synchronous
  [`apply_additive_only`][snap-add] and [`apply_once`][snap-once] bodies
  today, so a relocation that keeps `apply_once` whole preserves them, and
  one that splits it across an `.await` does not. The plugin reads
  `handler_total` and `total` at [999-1012][ts-read] and uses them only for
  logging. The wire contract has no `timings` statement; the response body
  field is a daemon-to-plugin convention.
- Fault/timing angle: The transform moves to a blocking thread while the
  handler-level `Instant` pairs stay on the handler task; a stage split
  across an `.await` splits its thread-local counter delta.
- Required faults and enabling state: none beyond a code change and a pass
  that populates `timings`.
- Reachability: default-production - the handler populates `timings` on the
  ordinary path ([8463][h-timings]) and [`respond_transform`][respond] emits
  the line for every response ([14463-14481][emit-call]).
- Existing check: [`pass_timing_line_is_parseable_for_an_empty_session`][t-line]
  pins the line's key set; [`timings_are_present_and_old_responses_deserialize_without_them`][t-timings]
  pins the default; the plugin test at [test.ts:244][ts-test] asserts the
  `rust module stages:` line appears. None ties the TypeScript key list to
  the Rust struct; none asserts what a field brackets.
- Open questions:
  - Are the stage fields a contract with the plugin, or free to change with
    the plugin's log line? (needs human input)

### token-count-cache-replacement-is-a-pure-memo-with-declared-retention

- Type: safety
- Check: `always` - For every input the replacement returns
  `tokenizer::estimate_tokens(input)`; the tail-hygiene key domain
  `kind ‖ NUL ‖ content` and the raw `NUL ‖ content` domain never alias;
  `calls == hits + misses + bypassed` on every thread; a count above
  `u32::MAX` is returned uncached; and the constant summed into
  [`DECLARED_RETAINED_RESIDENT_BYTES`][declared] is recomputed from the new
  shard count, generation count, and cap.
- Guarantee: Sharding or resizing the cache changes latency only. No
  rendered byte, budget decision, or resident-memory declaration changes.
- Rationale: The [module doc][tc-doc] states the memo contract. The cache is
  one global [`Mutex`][tc-static] over two generations of
  [`GENERATION_CAP = 65_536`][tc-cap]; the [`ponytail:` note][tc-shard]
  proposes sharding "if concurrent sessions ever contend here", conditionally
  and without evidence. [`RETAINED_BYTES_BOUND`][tc-bound] is derived from
  exactly two generations of that cap and is one term of
  [`DECLARED_RETAINED_RESIDENT_BYTES`][declared], whose doc says the runtime
  bound holds only when the declaration is truthful and lists each retention
  class so a change cannot omit one ([2236-2241][declared-doc]); no test
  checks the sum. [`count_with_digest`][tc-cwd] returns counts above `u32`
  uncached ([135-137][tc-u32]) and tokenizes outside the lock, so concurrent
  misses may tokenize twice ([107-109][tc-concurrent]).
  [`cached_estimate_tokens`][tc-cet] prefixes a NUL so raw content cannot
  alias a tail-hygiene key; [`tail_hygiene.rs:264`][th-cwd] and
  [`m0_compose` via `bench_internals`][bi-trim] are the other callers.
- Fault/timing angle: Two sessions miss on the same digest at once; a
  generation rotation at the cap while a promote-on-hit insert runs.
- Required faults and enabling state: Two concurrent transform passes for
  contention; 65_536 distinct digests for a rotation.
- Reachability: default-production -
  [`transform_with_projection_cached`][tc-inject] passes
  `cached_estimate_tokens` on every pass.
- Existing check: [`cached_counts_match_the_tokenizer`][t-tc-match] (four
  inputs), [`digest_keyed_hits_skip_retokenization`][t-tc-hits],
  [`insert_current_rotates_at_capacity`][t-tc-rotate],
  [`stats_partition_calls_into_hits_misses_and_bypassed`][t-tc-stats],
  [`kind_prefixed_and_raw_content_keys_do_not_alias`][t-tc-alias]. None for
  the declared-bytes sum; none measures contention.
- Open questions:
  - Is there any measured lock contention at HEAD? The comment is
    conditional and the audit should measure before sharding.
  - Is `MIN_CACHED_LEN` (64) in scope for change? It shifts work between
    `bypassed` and `misses` without changing any count.

### pass-token-estimates-route-through-one-estimator-interface

- Type: safety
- Check: `always` - Inside a pass, every token estimate that feeds a budget,
  a threshold, or a persisted `token_count` is obtained through the injected
  `estimate_tokens` parameter of [`apply_once`][ao-sig] or through
  `cached_estimate_tokens`, so it is counted in `tokenize_calls`; the HEAD
  exceptions are enumerated and the specification routes or keeps each one
  explicitly.
- Guarantee: Generalizes
  [`protected_floor_has_no_global_estimator_bypass`][t-bypass] from one
  helper to the pass: an optimization cannot make a decision-bearing count
  invisible to the pass's own accounting, and a test can substitute the
  estimator to reach a threshold.
- Rationale: `apply_once` takes `estimate_tokens: impl Fn(&str) -> usize +
  Copy` ([2836-2840][ao-sig]). The existing test is a source-text scan
  limited to the span from [`protected_tail_floor_ordinal`][floor] to
  `post_end_revision_inputs_moved`. Direct `tokenizer::estimate_tokens`
  calls in production transform code at HEAD: the SOFT pressure predicate's
  `m0_tokens` and `m1_tokens` at [4298-4309][soft-direct] (finding 10), the
  tag-mint `token_count` persisted into tag rows at [7160][mint-direct], and
  `ActiveTagForNudge.token_count` at [8558][nudge-direct]. The tokenizer
  crate exposes no call counter ([`estimate_tokens`][tok-fn]), so the only
  runtime oracle is the injected estimator plus the thread-local stats.
- Fault/timing angle: none.
- Required faults and enabling state: A pass whose SOFT predicate crosses a
  threshold; a pass minting new tags on the tail.
- Reachability: default-production - tag minting runs whenever new taggable
  tail blocks arrive; the SOFT arm needs the SOFT action.
- Existing check: [`protected_floor_has_no_global_estimator_bypass`][t-bypass]
  (one helper); [`cached_counts_match_the_tokenizer`][t-tc-match].
- Open questions:
  - Should 7160 and 8558 stay direct because their inputs are new tail
    blocks that miss the cache anyway, or route through the interface for
    accounting? (needs human input)
  - Extend the source-scan test to the whole `apply_once` body, or replace
    it with an injected counting estimator plus a `tokenize_calls` oracle?

### bounded-secret-scan-finds-every-whole-input-finding

- Type: safety
- Check: `always` - For every rule set, profile, limits, and input, a scan
  that restricts regex evaluation to regions around anchor hits returns the
  same `findings` (same spans, same order), the same `limits_hit`, and the
  same `semantic_digest` as the whole-input scan; if any input can differ,
  [`REVISION.semantic_digest_version`][revision] is bumped.
- Guarantee: A bounded scan never drops a detection the current scan
  reports, and the audit trail identifies which evaluator produced each row.
- Rationale: [`evaluate`][eval] runs [`preselect`][preselect] over the whole
  input with an [ASCII-case-insensitive Aho-Corasick][anchor-ci] automaton,
  then for each selected rule runs [`captures_iter(bytes)` over the whole
  input][captures]; `radius` bounds only the context window around the full
  match at [266-297][radius-window] for `must_contain` and `keywords_any`.
  `MAX_MATCH_BYTES` is 32 KiB ([api.rs:16][max-match]) and
  `MAX_RULE_RADIUS` 16 KiB ([20][max-radius]). Rule validation checks radius
  bounds only ([598-602][radius-valid]); nothing requires an anchor to occur
  in every match. By inspection of the 223 default and 17 overlay rules, 127
  declared anchors are not literal substrings of their regex source; for
  those rules the binding between anchor and match comes from
  `keywords_any` or `must_contain` needles that equal the anchors, as in
  [`airtable-personnal-access-token`][airtable] (anchor `airtable`, regex
  `\bpat...`, `keywords_any: [airtable]`, radius 256), so a finding lies
  within `radius` of an anchor hit only by that gate. Artificial slice edges
  change `\b`, `^`, and `$`; [`redaction.rs:380-385`][edge-margin] states
  this and defers edge findings, and [`WINDOW_OVERLAP_BYTES`][overlap] is
  sized from the same constants. Candidate counting order decides which
  findings a `Candidates` or `Work` limit drops ([112-128][captures],
  [`ScanLimits::DEFAULT`][limits]); findings are sorted at
  [130-147][sort]. The digest binds rules, profile, and limits, and the
  [comment at rules.rs:382][digest-doc] says evaluator semantics are bound
  by `semantic_digest_version`, pinned by
  [`evaluator_constants_are_pinned`][t-pinned]. The memory store persists
  the digest per scan batch ([2357-2392][ms-digest]), so an evaluator
  change without a bump makes old and new audit rows indistinguishable.
- Fault/timing angle: none; a data-shape difference.
- Required faults and enabling state: An input where a rule's match and its
  nearest anchor hit are separated by more than the region bound; a match
  ending within `EDGE_MARGIN_BYTES` of a region edge; an input reaching
  `max_candidates` (262_144) so evaluation order matters; an input over
  `MAX_REDACTABLE_BYTES` so redaction windows and region bounds compose.
- Reachability: default-production - every committing pass prepares `meta`
  and `core_state` through the durable redaction path
  ([`content`][ms-content]; state lens
  `meta-json-preparation-scans-every-persisted-byte`).
- Existing check:
  [`preselection_never_drops_a_rule_whose_pattern_matches`][t-preselect]
  (16 inputs, preselection soundness only);
  [`provider_canaries_return_stable_rule_ids_and_value_spans`][t-canaries];
  the qualification fixture holds one case with
  `authority_qualified: false` ([qualification.rs:49-63][t-qual]);
  [`evaluator_constants_are_pinned`][t-pinned]; redaction window tests at
  [827-856][t-windows]. None compares a bounded scan to the whole-input
  scan.
- Open questions:
  - Is bounding wanted at all, given the redaction windows already cap one
    scan at `MAX_REDACTABLE_BYTES`? (needs human input)
  - A per-rule proof that every match contains an anchor or is
    keyword-bound within `radius` does not exist; it would be a new rule-set
    test, not a property of the evaluator.

### historian-firing-input-is-preserved-by-cheaper-construction

- Type: safety
- Check: `always` - [`truncate_historian_input_if_needed`][trunc] returns
  bytes identical to the frozen reference in
  [`historian_truncate_differential.rs`][diff-ref] (same cut point, same
  marker), and a snapshot item carrying only `bytes.len()` yields the same
  [`compute_chunk_fingerprint`][fp] string as one carrying the bytes.
- Guarantee: The historian receives the same prompt bytes, and the durable
  chunk fingerprint still matches across restart.
- Rationale: The chunk snapshot is built at [417-429][snap-build] with
  `bytes: block.bytes.to_string()`; its only reader is
  [`as_item`][as-item] feeding [`compute_chunk_fingerprint`][fp], which
  reads `item.bytes.len()` into the literal `id:kind:len|...`. That string is
  stored in [`HistorianDurableState.chunk_fingerprint`][fp-field] and
  compared by [`verify_chunk_fingerprint`][fp-verify] and the publish
  predicate at [407-417][fp-predicate], so the format is durable state.
  Truncation at [742-777][trunc] binary-searches UTF-16 unit positions with
  an uncached `estimate_tokens` per probe and is called at [692][trunc-call].
  The differential test's header says the probe sequence must be preserved
  because token counts are not monotonic in prefix length
  ([1-11][diff-header]); it pins byte equality against the frozen reference
  at production windows ([100-113][diff-prod], 24 cases, budget 1..32_001),
  small windows ([131-140][diff-small]), and the exact-budget identity
  ([115-129][diff-exact]). The golden
  [`forced_overflow_preserves_existing_truncation_output`][t-golden] pins
  one case from `testdata/historian-chunk-golden.json`.
- Fault/timing angle: A fingerprint format change lands while a firing is
  in flight across a restart, so the stored string no longer equals the
  recomputed one and publication fails with `FingerprintMismatch`.
- Required faults and enabling state: A restart with an in-flight historian
  firing; a chunk whose text exceeds `token_budget`.
- Reachability: default-production for the fingerprint (every firing);
  truncation runs only when a chunk exceeds the budget, which needs a large
  session.
- Existing check: [`chunk_fingerprint_uses_id_kind_and_byte_length`][t-fp]
  pins the literal; the three differential proptests and the golden above;
  [`truncation_uses_marker_and_keeps_multibyte_boundaries`][t-marker].
- Open questions:
  - May the specification relax truncation to "any prefix within budget on
    a scalar boundary plus the marker"? The tests pin byte identity and the
    historian prompt bytes would change. (needs human input)

### cron-next-occurrence-matches-the-minute-stepper

- Type: safety
- Check: `always` - For every expression [`parse_cron`][parse] accepts,
  every `after_ms`, every `max_search_ms`, and both `Utc` and `Local` across
  DST transitions, a replacement returns the same `Option<i64>` as
  [`next_occurrence`][stepper] at HEAD, including `None` for
  unsatisfiable-but-valid expressions within the cap and `None` at
  unrepresentable instants; [`next_cron_occurrence`][occurrence] keeps the
  `ms != 0` filter.
- Guarantee: A field-jumping search never fires a schedule at a different
  instant, never fires one the stepper would never fire, and never runs past
  the cap.
- Rationale: [`parse_cron`][parse] accepts day-of-month 1..31 independent of
  month, so `0 0 30 2 *` is valid and unsatisfiable; [`matches_day`][vixie]
  implements Vixie OR semantics; the stepper evaluates local civil fields
  for each epoch minute ([166-192][stepper]), so a spring-forward gap skips
  that day and a fall-back overlap matches the earlier instant. The cap is
  [`MAX_SEARCH_MS`][cap], 4 x 366 days, 2_108_160 iterations per call for an
  unsatisfiable expression through [`next_cron_occurrence`][occurrence];
  smart notes cap at `SMART_NOTE_CHECK_CEILING_MS` instead
  ([236-239][note-cap]). The dreamer scheduler calls it as
  [`next_due`][sched-due] on its async task; the schedule defaults to `None`
  ([config.rs:127][sched-default]) and is accepted by
  [`is_valid_smart_note_cron`][valid] at [config.rs:887][sched-accept].
- Fault/timing angle: The scheduler's tick runs the stepper synchronously;
  an unsatisfiable schedule pays the full cap on that task.
- Required faults and enabling state: A `Local` zone with DST; expressions
  such as `30 2 * * *` on spring-forward, `0 0 30 2 *`, `0 0 31 4,6,9,11 *`,
  and `0 0 29 2 *` (satisfiable once within the cap); `after_ms` at the
  `i64` extremes.
- Reachability: explicit-config-only - the dreamer schedule
  (`/dreamer/tasks/review-user-memories/schedule`) defaults to `None`;
  smart notes need a cron on the note.
- Existing check:
  [`star_prefixed_day_fields_are_unrestricted_like_vixie_cron`][t-vixie],
  [`next_occurrence_survives_extreme_instants`][t-extreme], the golden
  [`smart_note_evaluation_golden_matches_production_behaviour`][t-golden-cron]
  with a fixture timezone. None for DST or for an unsatisfiable expression.
- Open questions:
  - Should validation reject calendar-impossible dates instead? That is a
    config-acceptance change outside a latency change. (needs human input)

### effective-config-reads-observe-a-tier-change-by-the-next-pass

- Type: safety
- Check: `always` - After the user tier, the project tier, or the configured
  guidance override file changes on disk (mtime advance for the tiers; any
  content change for the override, which HEAD re-reads on every call), the
  next `effective_config` call for that `project_root` returns the merge of
  the new contents; a cache is keyed by `(user_path, project_root)` and
  mtimes; [`merge_tiers_with_warnings`][merge] including
  [`ProjectRaiseOnly`][raise-only] is applied to the fresh contents
  unchanged; bind-frozen `SessionBinding.config` stays frozen.
- Guarantee: A config edit takes effect within one pass, as today, and a
  project tier can never gain privilege through a stale or cross-project
  cache entry.
- Rationale: [`effective_config`][eff-cfg] locks `self.config` and calls
  [`effective_for_project`][eff-proj], which re-reads `XDG_CONFIG_HOME` and
  `HOME` per call, then [`effective_with_warnings`][eff-warn]: two
  [`read_tier_cached`][tier-cached] calls (one `fs::metadata` each; a
  same-path, same-mtime hit clones the cached `Value`), the merge,
  [`resolve_user_guidance_override`][guidance] (a `metadata` plus a full
  read of the override file on every call, with no mtime gate), and a deep
  clone of the merged config at [288][eff-clone]. Per-pass callers are
  [`maybe_spawn_reattach`][call-reattach],
  [`prepare_historian_fire`][call-fire], and the wrapup path
  [5368][call-wrapup]; [`bind`][call-bind] freezes a copy into
  `SessionBinding`, whose doc says config can change while the route stays
  open ([lib.rs:216-217][binding-doc]). One `ConfigCache` per handler holds
  one project tier, so two bound project roots alternating re-read the file
  every call ([368-372][tier-cached]). The staleness contract at HEAD
  already ignores a same-mtime edit ([test 2182-2188][t-mtime]).
- Fault/timing angle: A tier edit between two passes; two routes on
  different project roots alternating; the override edited without touching
  a tier.
- Required faults and enabling state: A user or project tier rewritten with
  a later mtime; `prompt_surface.guidance_override_path` configured and its
  file edited; two project roots bound at once.
- Reachability: default-production for the tier reads
  (`prepare_historian_fire` runs each pass); explicit-config-only for the
  override.
- Existing check:
  [`mtime_cache_reuses_unchanged_reads_and_invalidates_on_mtime_change`][t-mtime];
  [`project_threshold_may_only_raise`][t-raise],
  [`project_tier_cannot_raise_the_user_memory_gate`][t-gate],
  [`hostile_project_tier_cannot_change_privileged_values_and_warns_per_key`][t-hostile].
  None for override staleness; none for two project roots.
- Open questions:
  - The doc at [config.rs:266-267][eff-warn-doc] says tier read failures are
    reported on every load so a long-running daemon keeps surfacing them; a
    merged cache silences the repeat. Keep that behavior? (needs human
    input)
  - Should the historian read the bind-frozen config, removing the per-pass
    merge entirely? (needs human input)

## Repository constraints that bind the optimization (authority document cited)

- No schema change to a durable store. [`crates/storage/AGENTS.md`][a-storage]
  forbids migrations and version ledgers; [`open_sqlite`][open-sqlite]
  refuses any file whose schema objects differ from the baseline
  ([`classify`][classify]), so a new table, column, or index for a durable
  usage counter or a pass-trace fold makes every existing store unopenable.
  [`ci.yml` "Schema-migration identifiers"][ci-migration] mechanically
  rejects migration vocabulary outside tests and prose.
- Unsafe changes in `shm-transport` need the Miri and Valgrind runs.
  [`crates/shm-transport/AGENTS.md`][a-shm]; the jobs are
  [`miri`][ci-miri] and [`valgrind`][ci-valgrind].
- Unsafe is forbidden where the transform runs. `#![forbid(unsafe_code)]`
  in [daemon][forbid-daemon], [memory-store][forbid-ms],
  [context-core][forbid-cc], [kernel][forbid-kernel],
  [secret-scanner][forbid-ss], and [tokenizer][forbid-tok];
  [host-runtime][deny-hr] uses `deny` with one scoped allow. A zero-fill-free
  buffer or any pointer trick belongs only in `shm-transport`, `lease`, or
  `storage`.
- Wire names and literals are frozen. [Root `AGENTS.md`][a-root] makes
  [`docs/host-wire-protocol.md`][wire] normative and requires a versioned
  protocol change for any wire name or literal; the doc itself lists the
  version-2 literals that must not be renamed ([line 14][wire-literals]).
- Every process-global cache is declared.
  [`DECLARED_RETAINED_RESIDENT_BYTES`][declared]
  lists each retention class so a budget change cannot omit one
  ([2236-2241][declared-doc]); a merged-config cache, a sharded token cache,
  a cron cache, or a scanner anchor index adds its bound there. No test
  checks the sum.
- Scanner semantics are versioned. An evaluator change that can alter any
  finding bumps [`REVISION.semantic_digest_version`][revision]
  ([rules.rs:382][digest-doc]); the memory store persists the digest per
  scan batch ([2357-2392][ms-digest]).
- Bench targets stay out of nextest and run in CI only as smoke.
  [`.config/nextest.toml`][nextest]; [`ci.yml:514-518`][ci-bench]. A new
  bench must also compile and pass in test mode.
- `--locked` on every Cargo command and `ci.yml` as the source of truth for
  required checks. [Root `AGENTS.md`][a-root].
- Property catalogs follow [`METHOD.md`][method] per
  [`docs/properties/AGENTS.md`][a-props].

## Existing checks

| Check | Source condition | Status |
| --- | --- | --- |
| [`ci.yml` Benches in test mode][ci-bench] | every bench target runs once; no comparison, no threshold | unaudited |
| [`evidence.rs`][evidence] manifest rules | host-runtime arm records carry schema, workload, build, host, arm ids | unaudited |
| [`pass_timing_line_is_parseable_for_an_empty_session`][t-line] | timing line key set and `key=value` shape | unaudited |
| [`timings_are_present_and_old_responses_deserialize_without_them`][t-timings] | `timings` absent deserializes to default | unaudited |
| [`emits discriminating pass and stage logs from ordinary Rust transforms`][ts-test] | plugin logs `rust module stages:` from response timings | unaudited |
| [`first_hard_pass_meta_respects_the_store_durable_text_bound`][meta-bound] | 1_000 messages commit; 1_400 fail with `InputLimit` | unaudited |
| [`cached_counts_match_the_tokenizer`][t-tc-match] | cached count equals tokenizer count, hit and miss | unaudited |
| [`digest_keyed_hits_skip_retokenization`][t-tc-hits] | second lookup is a hit, not a miss | unaudited |
| [`insert_current_rotates_at_capacity`][t-tc-rotate] | `current` never exceeds `GENERATION_CAP` | unaudited |
| [`stats_partition_calls_into_hits_misses_and_bypassed`][t-tc-stats] | `calls == hits + misses + bypassed` | unaudited |
| [`kind_prefixed_and_raw_content_keys_do_not_alias`][t-tc-alias] | tail-hygiene and raw key domains disjoint | unaudited |
| [`protected_floor_has_no_global_estimator_bypass`][t-bypass] | no direct tokenizer call in one helper's source | unaudited |
| [`preselection_never_drops_a_rule_whose_pattern_matches`][t-preselect] | a matching rule is always preselected (16 inputs) | unaudited |
| [`provider_canaries_return_stable_rule_ids_and_value_spans`][t-canaries] | canary inputs yield stable rule ids and spans | unaudited |
| [`minimal_fixture_is_truthful_and_executable`][t-qual] | one-case qualification fixture, `authority_qualified: false` | unaudited |
| [`evaluator_constants_are_pinned`][t-pinned] | evaluator tables and constants digest pinned | unaudited |
| [`windows_start_on_line_boundaries_and_overlap_when_lines_are_short`][t-windows] | redaction window placement and overlap | unaudited |
| [`scanner_is_the_only_redaction_path`][t-only-path] | no redaction path bypasses the scanner | unaudited |
| [`chunk_fingerprint_uses_id_kind_and_byte_length`][t-fp] | fingerprint literal `id:kind:len` | unaudited |
| [`optimized_matches_frozen_reference_at_production_windows`][diff-prod] | truncation bytes equal the frozen reference | unaudited |
| [`exact_token_budget_returns_original_input`][diff-exact] | input at budget returns unchanged | unaudited |
| [`optimized_matches_frozen_reference`][diff-small] | small-window byte equality | unaudited |
| [`forced_overflow_preserves_existing_truncation_output`][t-golden] | golden truncation case | unaudited |
| [`truncation_uses_marker_and_keeps_multibyte_boundaries`][t-marker] | marker suffix, budget, scalar-boundary prefix | unaudited |
| [`star_prefixed_day_fields_are_unrestricted_like_vixie_cron`][t-vixie] | Vixie `*`-prefix semantics | unaudited |
| [`next_occurrence_survives_extreme_instants`][t-extreme] | `None` at `i64` extremes | unaudited |
| [`smart_note_evaluation_golden_matches_production_behaviour`][t-golden-cron] | reduction and due-time golden, fixture timezone | unaudited |
| [`mtime_cache_reuses_unchanged_reads_and_invalidates_on_mtime_change`][t-mtime] | same mtime hides an edit; new mtime reloads | unaudited |
| [`project_threshold_may_only_raise`][t-raise] | `ProjectRaiseOnly` threshold | unaudited |
| [`project_tier_cannot_raise_the_user_memory_gate`][t-gate] | `ProjectRaiseOnly` gate | unaudited |
| [`hostile_project_tier_cannot_change_privileged_values_and_warns_per_key`][t-hostile] | privileged keys ignored with per-key warning | unaudited |

None found:

- A daemon benchmark that records build, host, and workload identity.
- Any bench reaching the handler path (`Handler::handle`) or a
  production-sized steady session.
- A check that `DECLARED_RETAINED_RESIDENT_BYTES` includes every cache.
- Token-cache lock contention evidence.
- A bounded-scan versus whole-input differential for the secret scanner.
- A DST or unsatisfiable-expression case for the cron stepper.
- Guidance override staleness, or two project roots sharing one
  `ConfigCache`.

Suspiciously quiet: the shm bench manifest declares probes the binary cannot
run, and the transport bench's record labels itself `BLOCKED`, yet nothing
fails; the qualification fixture for the scanner has one case and declares
itself tooling only.

## Contract-versus-code disagreements

- [`default_rules.yaml:12`][rules-radius-doc] describes `radius` as the
  "Byte radius around an anchor match fed to the regex". The evaluator feeds
  the whole input to the regex ([112][captures]) and uses `radius` only for
  the context window around the full match ([266-297][radius-window]). The
  doc describes a design the code does not implement; a bounded scan would
  move the code toward the doc and must be judged against the property above,
  not against the doc.
- [`benches/manifests/v1.json`][he-manifest] declares 24 byte-size probes,
  three workload classes, and designated-host fields; the bench runs a fixed
  [256- or 4096-byte payload][he-payload], [rejects
  `--designated-host`][he-designated],
  and labels its own record [`BLOCKED`][he-blocked]. Manifest versus
  binary, not a code bug.
- [`docs/runbooks/autoresearch-tokenizer-perf.md`][runbook] names
  `crates/tokenizer/benches/report.sh`, `guard.sh`, and
  `benches/BASELINE.md`; `crates/tokenizer/benches/` does not exist at HEAD
  and the only tokenizer bench is [`bench_tokenizer`][hp-tok] in the daemon
  crate. The runbook is a plan to build them (its Phase 0), not a description
  of committed artifacts. No baseline artifact of any kind is tracked.
- [`config.rs:266-267`][eff-warn-doc] states tier read failures are reported
  on every load. The code matches. A merged-config cache contradicts the
  statement and must rewrite it or preserve the behavior.
- [`hot_path.rs:30-33`][hp-counts] attributes the 512 KiB cliff to `meta`
  growing about 460 bytes per message. The cliff is pinned by
  [`transform_meta_bound.rs`][meta-bound] at 1_000 ok and 1_400 refused; the
  per-message figure is unverified here.

## Anchors

Corrections to the supplied anchors: the token cache's generations and
static live at `token_cache.rs:34-40` and `count_with_digest` at 110-142; the
sharding note is at 112-113, not 110. The `radius` description in
`default_rules.yaml` is line 12, not 13. `evaluate` spans
`evaluator.rs:35-157`; the whole-input `captures_iter` is at 112 and the
radius window at 266-297. `effective_with_warnings` spans `config.rs:268-288`
with its doc at 266-267; `read_tier_cached` spans 368-398.
`truncate_historian_input_if_needed` spans `historian_chunk.rs:743-777` with
its doc at 742 and the marker at 739-740. `next_occurrence` spans
`smart_note_evaluation.rs:166-192`; `next_cron_occurrence` is 216-218 as
supplied; `is_valid_smart_note_cron` is 208-210 and `config.rs:887` is its
call site.

[ci-bench]: ../../../../../.github/workflows/ci.yml#L514-L518
[ci-migration]: ../../../../../.github/workflows/ci.yml#L280-L358
[ci-miri]: ../../../../../.github/workflows/ci.yml#L597-L635
[ci-valgrind]: ../../../../../.github/workflows/ci.yml#L637-L658
[nextest]: ../../../../../.config/nextest.toml#L4-L7
[a-root]: ../../../../../AGENTS.md#L5-L6
[a-storage]: ../../../../../crates/storage/AGENTS.md#L3
[a-shm]: ../../../../../crates/shm-transport/AGENTS.md#L3
[a-props]: ../../../AGENTS.md
[method]: ../../../METHOD.md
[wire]: ../../../../host-wire-protocol.md
[wire-literals]: ../../../../host-wire-protocol.md#L14
[runbook]: ../../../../runbooks/autoresearch-tokenizer-perf.md#L10-L14

[hp-header]: ../../../../../crates/daemon/benches/hot_path.rs#L1-L10
[hp-counts]: ../../../../../crates/daemon/benches/hot_path.rs#L29-L35
[hp-tok]: ../../../../../crates/daemon/benches/hot_path.rs#L50-L66
[hp-req]: ../../../../../crates/daemon/benches/hot_path.rs#L172-L185
[hp-store]: ../../../../../crates/daemon/benches/hot_path.rs#L214-L219
[hp-e2e]: ../../../../../crates/daemon/benches/hot_path.rs#L221-L254
[hp-cliff]: ../../../../../crates/daemon/benches/hot_path.rs#L291-L293
[cargo-bench]: ../../../../../crates/daemon/Cargo.toml#L62-L75
[meta-bound]: ../../../../../crates/daemon/tests/transform_meta_bound.rs#L1-L22
[bi-tc]: ../../../../../crates/daemon/src/lib.rs#L192-L200
[bi-trim]: ../../../../../crates/daemon/src/lib.rs#L173-L181
[he-payload]: ../../../../../crates/shm-transport/benches/hardware_envelope.rs#L220-L223
[he-designated]: ../../../../../crates/shm-transport/benches/hardware_envelope.rs#L211-L214
[he-blocked]: ../../../../../crates/shm-transport/benches/hardware_envelope.rs#L283-L286
[he-manifest]: ../../../../../crates/shm-transport/benches/manifests/v1.json
[pm-body]: ../../../../../crates/host-runtime/tests/support/perf_measurement.rs#L16-L19
[ring-body]: ../../../../../crates/host-runtime/benches/support/ring.rs#L77
[evidence]: ../../../../../crates/host-runtime/benches/support/evidence.rs#L1-L8
[fx-1400]: ../../../../../crates/daemon/src/transform.rs#L12389-L12394
[fx-2500]: ../../../../../crates/daemon/src/transform.rs#L27680-L27685

[h-pre]: ../../../../../crates/daemon/src/lib.rs#L8122-L8139
[h-run]: ../../../../../crates/daemon/src/lib.rs#L8145-L8200
[h-call]: ../../../../../crates/daemon/src/lib.rs#L8193-L8199
[h-timings]: ../../../../../crates/daemon/src/lib.rs#L8470-L8495
[respond]: ../../../../../crates/daemon/src/lib.rs#L14411
[emit-call]: ../../../../../crates/daemon/src/lib.rs#L14470-L14488
[emit]: ../../../../../crates/daemon/src/lib.rs#L14492-L14514
[tt]: ../../../../../crates/daemon/src/transform.rs#L1026-L1205
[rtcd]: ../../../../../crates/daemon/src/transform.rs#L1207-L1218
[fmt]: ../../../../../crates/daemon/src/transform.rs#L1224-L1357
[snap-add]: ../../../../../crates/daemon/src/transform.rs#L2381
[snap-once]: ../../../../../crates/daemon/src/transform.rs#L2860
[t-timings]: ../../../../../crates/daemon/src/transform.rs#L12239
[t-line]: ../../../../../crates/daemon/src/transform.rs#L12289
[ts-stage-fn]: ../../../../../packages/opencode-plugin/src/hooks/context/rust-mode-transform.ts#L1019-L1024
[ts-read]: ../../../../../packages/opencode-plugin/src/hooks/context/rust-mode-transform.ts#L999-L1012
[ts-stages]: ../../../../../packages/opencode-plugin/src/hooks/context/rust-mode-transform.ts#L1013-L1042
[ts-test]: ../../../../../packages/opencode-plugin/src/hooks/context/rust-mode-transform.test.ts#L244

[tc-doc]: ../../../../../crates/daemon/src/token_cache.rs#L1-L7
[tc-cap]: ../../../../../crates/daemon/src/token_cache.rs#L16
[tc-bound]: ../../../../../crates/daemon/src/token_cache.rs#L24-L28
[tc-static]: ../../../../../crates/daemon/src/token_cache.rs#L34-L40
[tc-local]: ../../../../../crates/daemon/src/token_cache.rs#L57-L76
[tc-concurrent]: ../../../../../crates/daemon/src/token_cache.rs#L107-L109
[tc-cwd]: ../../../../../crates/daemon/src/token_cache.rs#L110-L142
[tc-shard]: ../../../../../crates/daemon/src/token_cache.rs#L112-L113
[tc-u32]: ../../../../../crates/daemon/src/token_cache.rs#L135-L137
[tc-cet]: ../../../../../crates/daemon/src/token_cache.rs#L165-L181
[t-tc-match]: ../../../../../crates/daemon/src/token_cache.rs#L188
[t-tc-hits]: ../../../../../crates/daemon/src/token_cache.rs#L209
[t-tc-rotate]: ../../../../../crates/daemon/src/token_cache.rs#L233
[t-tc-stats]: ../../../../../crates/daemon/src/token_cache.rs#L249
[t-tc-alias]: ../../../../../crates/daemon/src/token_cache.rs#L266
[th-cwd]: ../../../../../crates/daemon/src/tail_hygiene.rs#L614
[tc-inject]: ../../../../../crates/daemon/src/transform.rs#L1807-L1823
[declared-doc]: ../../../../../crates/daemon/src/lib.rs#L2243-L2248
[declared]: ../../../../../crates/daemon/src/lib.rs#L2250-L2264
[ao-sig]: ../../../../../crates/daemon/src/transform.rs#L2844-L2848
[floor]: ../../../../../crates/daemon/src/transform.rs#L5846
[soft-direct]: ../../../../../crates/daemon/src/transform.rs#L4306-L4317
[mint-direct]: ../../../../../crates/daemon/src/transform.rs#L7160
[nudge-direct]: ../../../../../crates/daemon/src/transform.rs#L8569
[t-bypass]: ../../../../../crates/daemon/src/transform.rs#L24455-L24466
[tok-fn]: ../../../../../crates/tokenizer/src/lib.rs#L148

[eval]: ../../../../../crates/secret-scanner/src/evaluator.rs#L35-L157
[captures]: ../../../../../crates/secret-scanner/src/evaluator.rs#L112-L128
[sort]: ../../../../../crates/secret-scanner/src/evaluator.rs#L130-L147
[radius-window]: ../../../../../crates/secret-scanner/src/evaluator.rs#L266-L297
[t-pinned]: ../../../../../crates/secret-scanner/src/evaluator.rs#L1660-L1683
[preselect]: ../../../../../crates/secret-scanner/src/rules.rs#L353-L370
[digest-doc]: ../../../../../crates/secret-scanner/src/rules.rs#L382
[anchor-ci]: ../../../../../crates/secret-scanner/src/rules.rs#L449-L473
[radius-valid]: ../../../../../crates/secret-scanner/src/rules.rs#L598-L602
[t-preselect]: ../../../../../crates/secret-scanner/src/rules.rs#L685-L722
[max-match]: ../../../../../crates/secret-scanner/src/api.rs#L16
[max-radius]: ../../../../../crates/secret-scanner/src/api.rs#L20
[limits]: ../../../../../crates/secret-scanner/src/api.rs#L219-L226
[revision]: ../../../../../crates/secret-scanner/src/api.rs#L276-L280
[rules-radius-doc]: ../../../../../crates/secret-scanner/default_rules.yaml#L12
[airtable]: ../../../../../crates/secret-scanner/default_rules.yaml#L297-L309
[t-canaries]: ../../../../../crates/secret-scanner/tests/rule_canaries.rs#L4
[t-qual]: ../../../../../crates/secret-scanner/tests/qualification.rs#L49-L63
[overlap]: ../../../../../crates/context-core/src/redaction.rs#L365-L377
[edge-margin]: ../../../../../crates/context-core/src/redaction.rs#L380-L385
[t-windows]: ../../../../../crates/context-core/src/redaction.rs#L827-L856
[t-only-path]: ../../../../../crates/context-core/src/redaction.rs#L857
[ms-content]: ../../../../../crates/memory-store/src/lib.rs#L2070-L2078
[ms-digest]: ../../../../../crates/memory-store/src/lib.rs#L2357-L2392

[snap-build]: ../../../../../crates/daemon/src/historian_chunk.rs#L417-L429
[as-item]: ../../../../../crates/daemon/src/historian_chunk.rs#L37-L46
[trunc-call]: ../../../../../crates/daemon/src/historian_chunk.rs#L692
[trunc]: ../../../../../crates/daemon/src/historian_chunk.rs#L742-L777
[t-golden]: ../../../../../crates/daemon/src/historian_chunk.rs#L1749-L1760
[t-marker]: ../../../../../crates/daemon/src/historian_chunk.rs#L1762-L1763
[fp]: ../../../../../crates/daemon/src/historian.rs#L140-L158
[fp-field]: ../../../../../crates/memory-store/src/lib.rs#L588
[fp-verify]: ../../../../../crates/daemon/src/historian.rs#L326-L334
[fp-predicate]: ../../../../../crates/daemon/src/historian.rs#L407-L417
[t-fp]: ../../../../../crates/daemon/src/historian.rs#L3925
[diff-header]: ../../../../../crates/daemon/tests/historian_truncate_differential.rs#L1-L11
[diff-ref]: ../../../../../crates/daemon/tests/historian_truncate_differential.rs#L13-L58
[diff-prod]: ../../../../../crates/daemon/tests/historian_truncate_differential.rs#L100-L113
[diff-exact]: ../../../../../crates/daemon/tests/historian_truncate_differential.rs#L115-L129
[diff-small]: ../../../../../crates/daemon/tests/historian_truncate_differential.rs#L131-L140

[cap]: ../../../../../crates/daemon/src/smart_note_evaluation.rs#L31-L34
[parse]: ../../../../../crates/daemon/src/smart_note_evaluation.rs#L125-L145
[vixie]: ../../../../../crates/daemon/src/smart_note_evaluation.rs#L150-L160
[stepper]: ../../../../../crates/daemon/src/smart_note_evaluation.rs#L163-L192
[valid]: ../../../../../crates/daemon/src/smart_note_evaluation.rs#L205-L210
[occurrence]: ../../../../../crates/daemon/src/smart_note_evaluation.rs#L212-L218
[note-cap]: ../../../../../crates/daemon/src/smart_note_evaluation.rs#L236-L239
[t-golden-cron]: ../../../../../crates/daemon/src/smart_note_evaluation.rs#L1126
[t-extreme]: ../../../../../crates/daemon/src/smart_note_evaluation.rs#L1580-L1591
[t-vixie]: ../../../../../crates/daemon/src/smart_note_evaluation.rs#L1594
[sched-due]: ../../../../../crates/daemon/src/dreamer_scheduler.rs#L412-L416
[sched-default]: ../../../../../crates/daemon/src/config.rs#L127
[sched-accept]: ../../../../../crates/daemon/src/config.rs#L881-L895

[eff-cfg]: ../../../../../crates/daemon/src/lib.rs#L4563-L4572
[binding-doc]: ../../../../../crates/daemon/src/lib.rs#L223-L224
[call-reattach]: ../../../../../crates/daemon/src/lib.rs#L4797
[call-fire]: ../../../../../crates/daemon/src/lib.rs#L5058
[call-wrapup]: ../../../../../crates/daemon/src/lib.rs#L5375
[call-bind]: ../../../../../crates/daemon/src/lib.rs#L11795
[eff-proj]: ../../../../../crates/daemon/src/config.rs#L242-L245
[eff-warn-doc]: ../../../../../crates/daemon/src/config.rs#L266-L267
[eff-warn]: ../../../../../crates/daemon/src/config.rs#L268-L288
[eff-clone]: ../../../../../crates/daemon/src/config.rs#L288
[tier-cached]: ../../../../../crates/daemon/src/config.rs#L368-L398
[guidance]: ../../../../../crates/daemon/src/config.rs#L414-L496
[merge]: ../../../../../crates/daemon/src/config.rs#L716
[raise-only]: ../../../../../crates/daemon/src/config.rs#L740
[t-raise]: ../../../../../crates/daemon/src/config.rs#L1314
[t-gate]: ../../../../../crates/daemon/src/config.rs#L1654
[t-hostile]: ../../../../../crates/daemon/src/config.rs#L1722
[t-mtime]: ../../../../../crates/daemon/src/config.rs#L2165-L2202

[open-sqlite]: ../../../../../crates/storage/src/lib.rs#L1117-L1125
[classify]: ../../../../../crates/storage/src/lib.rs#L1644-L1674
[forbid-daemon]: ../../../../../crates/daemon/src/lib.rs
[forbid-ms]: ../../../../../crates/memory-store/src/lib.rs
[forbid-cc]: ../../../../../crates/context-core/src/lib.rs
[forbid-kernel]: ../../../../../crates/kernel/src/lib.rs
[forbid-ss]: ../../../../../crates/secret-scanner/src/lib.rs
[forbid-tok]: ../../../../../crates/tokenizer/src/lib.rs
[deny-hr]: ../../../../../crates/host-runtime/src/lib.rs#L3-L5
