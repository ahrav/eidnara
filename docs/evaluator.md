# Evaluator core

`crates/eval-core` holds the value-level contracts of the long-horizon
evaluator: the run manifest, run identity, residue rules, the surface census
pins, the generated world model (keyed draws, the choice tape, the event log,
and the step drive), the eligibility spec, the bitemporal reducer, the
occurrence identity rule, the fixture renderer, the stage ledger, the
coverage-marker registry, and the model-I/O cassette. It is sans-I/O. Every function takes values and returns values;
the runner shell owns processes, stores, clocks, temp roots, and the build
sub-record.

## Placement fences

- `[dependencies]` is exactly `context-core`, `serde`, `serde_json`, `sha2`.
  No kernel, daemon, retrieval, storage, memory-store, host-runtime, rusqlite,
  or tokio edge.
- `eval-core` appears in the workspace only under `[dev-dependencies]`
  (`crates/daemon`). `scripts/forbid-test-support-dependencies.ts` rejects a
  normal, build, or target-specific edge to it, and rejects any such edge whose
  requested features turn on test-support in the target package, either by
  naming a `*test-support` feature or by reaching one through the target's
  feature table (a forwarding alias such as `bench = ["kernel/test-support"]`
  counts). It also rejects a local package whose `default` feature enables its
  own `test-support` or reaches a `*/test-support` entry. The `gates` CI job
  runs the scan.
- Digests come from `context_core::canonical_json::protocol_digest`, which is
  public so callers name a protocol string instead of restating the
  `<protocol>\n<canonical JSON>` framing.

## Manifest `eval-manifest/v6`

`parse_manifest` reads a JSON object, compares its key set against
`REQUIRED_FIELDS`, checks the `schema` literal, and only then deserializes and
validates. Refusals are typed: `MissingField(name)`, `UnknownField(name)`,
`SchemaMismatch`, `RunIdMismatch`, `GeneratorVersionMismatch`,
`ClaimBoundaryMismatch`, `ResidueIncomplete`, `ResidueContradiction` (a `Keep`
entry or two rules for one field), `SampleOrderNotAPermutation`,
`MalformedDigest`, `MalformedDecimal`, `RateOutOfRange` (an arm rate outside
`[0, 1]`), `EmptyComponent` (an empty component version, tokenizer name or
revision, or attestation signer), `NotCanonical` (an integer outside the
canonical safe range). `Manifest::validate` is public so a manifest built in
code can be checked before it is written; it applies every check above except
the key-set closure, so a manifest it accepts also parses and digests. Adding a
field to `Manifest` without bumping the schema fails the closure test, and the
fixture digests in `tests/manifest.rs` are frozen so an encoding change is
reviewed.

The 29 required fields, sorted:

| Field | Content |
| --- | --- |
| `analysis_family_digest` | The `eval-analysis-family-digest/v1` digest of the frozen analysis family a paired campaign is read under, recorded before its first outcome; `null` for a run that reports no paired statistics. |
| `arm_rates` | Per-arm miss and refusal rates as exact decimal strings. |
| `attestation` | Tagged: `{"kind": "none"}` or `{"kind": "signed", ...}`. |
| `claim_boundary` | The `claim-boundary/v1` block with the four exclusions. |
| `component_versions` | Seven versions: generator, event schema, reducer, oracles, execution image, task corpus, judge. |
| `construction` | `replay`, `bulk`, or `hand_built`. |
| `cut_receipts` | Cuts from the closed set (`AfterAtomicTransition`, `AtQuiescence`, `AfterRecovery`, `AfterFaultPhase`, `EndOfRun`) with `reached` or `not_reached`. |
| `end_ms`, `start_ms` | Wall-clock stamps from the shell. |
| `envelope_bounds` | Declared resource bounds. |
| `envelope_peaks` | Observed peaks; a measurement, so it leaves the digest. |
| `error` | Typed error text or `null`. |
| `eval_run_id` | The run identity digest. |
| `execution_mode` | `generate`, `replay_tape`, or `enumerate`: how the world was driven. |
| `failure_class_table_digest` | The `eidnara-failure-class-table-v1` digest of the pinned truth table; refused unless it equals `FAILURE_CLASS_TABLE_DIGEST`. |
| `ingestion` | `adapter-ingested, production caller: none` or `direct-database, non-aged`; the latter with a `replay` construction is refused (`DirectDatabaseAged`). |
| `memory_reviewer_model_calls` | `cassette` (replayed through the keyed TLS peer) or `excluded` (the reviewer worker is not spawned); MemoryReviewer traffic bypasses `LlmExecutionBackend`, so silence is refused as a missing field. |
| `reachability` | `default-production`, `explicit-config-only`, or `test-only`. |
| `residue` | Every non-`Keep` field with its rule, including the manifest's own. |
| `result_digest`, `witness_digest` | Lowercase hex SHA-256. |
| `retry_lineage` | Prior `eval_run_id` values of retried attempts; each is lowercase hex SHA-256. |
| `run_identity` | The nine-component identity tuple, including the build sub-record, the eligibility-spec digest, and the linearization rule version. |
| `sample_epoch`, `sample_ids`, `sample_order` | Stable sample identity and execution order; `sample_order` must be a permutation of `sample_ids`. |
| `schema` | `eval-manifest/v6`. |
| `status` | `completed`, `incomplete`, `refused`, or `blocked`. |
| `tokenizer_profile` | Name, revision, digest. |

`Manifest::digest` re-parses the manifest, applies the manifest's own residue
rules (`start_ms`, `end_ms`, and `envelope_peaks` are `Drop`; everything else
is `Keep`), and hashes with protocol `eval-manifest-digest/v6`. Version 2
added `execution_mode` (the reducer differential runs under `enumerate`);
version 3 added `ingestion`, because no ingestion entry point has a production
caller and every manifest must say so; version 4 added `failure_class_table_digest`,
so a report names the failure-class table its classes come from;
version 5 added `memory_reviewer_model_calls`, because reviewer model traffic
is either replayed or excluded, never silently live;
version 6 added `analysis_family_digest`, so a paired report can prove it was
read under the family frozen before its first outcome. The digest is a function
of every kept field, not of the run identity alone: two processes that record the same
identity and the same kept contents produce the same digest
(`two_process_same_identity_yields_equal_manifest_and_trace_digests`), and two
runs that share an identity but differ in `status`, `sample_order`,
`result_digest`, or any other kept field do not.

Canonical JSON rejects fractional numbers, so every fraction is a canonical
decimal string: `is_canonical_decimal` accepts `0`, `12`, `0.25` and rejects
`.5`, `5.`, `007`, `0.250`, `1e3`, and signs, so one value has one encoding.
`root_seed` serializes as a decimal string because canonical JSON caps integers
at 2^53 - 1, and only the form `u64::to_string` produces is accepted back.

## Run identity

`eval_run_id` hashes `(build, simulator_version, config, scenario, root_seed,
random_schema_version, generator_version, eligibility_spec_digest,
linearization_rule_version)` with protocol `eval-run-id/v1`. `build` enters as
the digest (`eval-build/v1`) of the shell-supplied `BuildRecord`: code SHA,
dirty flag, lockfile digest, rustc version, feature set, target triple, and a
binary digest that is either `{"kind": "present", "sha256"}` or
`{"kind": "absent", "reason"}`. The feature set is a `BTreeSet`, so its order
cannot change the identity. A present digest equal to the SHA-256 of zero bytes
is refused; it names no build, an absent digest needs a non-empty reason, and
a dirty tree needs a present binary digest, since two dirty trees on one
commit are told apart only by the binary.
Empty version strings, a malformed
`eligibility_spec_digest`, and a `config` or `scenario` value canonical JSON
cannot encode are refused by `RunIdentity::validate`, so an identity it accepts
also produces its run ID. There is no seed-only constructor.

## Residue rules

`ObservationSchema` maps every field of one observation type to a `Rule`:

- `Keep`: hashed as observed.
- `Drop`: removed.
- `Presence`: reduced to whether the value is null.
- `Relative`: renumbered by first appearance within one trace, so two runs
  that minted different identifiers for the same objects compare equal while
  merged or swapped identifiers still differ.

`SemanticTrace::record` refuses an observation whose field set differs from
its schema (`UnclassifiedField`, `MissingField`), so classification is total;
registering one type twice is refused (`DuplicateType`), declaring one field
twice in a schema is refused (`DuplicateField`), a field not spelled in
snake_case (lowercase words and digits joined by single underscores) is refused
(`FieldNotSnakeCase`) because the gates below match those tokens, as is a
`Keep` or
`Relative` value canonical JSON cannot encode (`NotCanonical`). A refused
observation leaves the trace and its `Relative` numbering unchanged, so a
recorded trace always digests.
`CLOCK_FIELD_KEEP_ALLOWLIST` (`now_ms`, `observed_at_ms`, `valid_time_ms`)
names the only clock-named fields a schema may keep; any other clock-named
field under `Keep` is refused at schema construction, as is any field named
for a hostname (`hostname` or a `host` token), cwd, a filesystem location (a
`root` or `path` token), a boot (`boot` token), a Unix identity (`uid`, `euid`,
`gid`, or `egid` token), pid (a `pid` or `ppid` token or `process_id`
anywhere), or incarnation (`HostFieldKept`). These name gates are
a heuristic that refuses obvious mistakes at schema construction; the
guarantee that host values stay out of a digest is the two-process equality
test in `tests/two_process.rs`, and a new host-specific spelling is added to
the gate when it is found. The trace
digest uses protocol `eval-trace/v1`. Rules apply to the top-level fields of an
observation; nested values under `Keep` enter the digest whole.

## Surface census pins

`Surface1Stage` names the thirteen surface-1 stages and `SURFACE1_STAGES`
lists them in production order, so the array index is the ledger ordinal: tail
eligibility, suppression, length gate, token gate, candidate window, match
filter, threshold, cap, render, decision freeze, deferral, overlay apply,
attachment. `SURFACE1_HINT_BOUNDS` pins 100 candidates, 24 query tokens, 3
results, 2 matched tokens, 80-unit fragments, and an 800-unit total. The daemon
census tests compare production behavior against these pins; the pins alone
prove nothing.

## Claim boundary

Every manifest carries `claim-boundary/v1` with the four exclusions:
scheduler-order independence, power-loss durability, wall-clock retention
behavior under manipulated age, and live-model quality. A manifest with a
different block is refused.

## Generated worlds

`generate_all(root_seed, &config, mode)` builds a `World { log, tape }` from a
`WorldConfig` and nothing else. The config lists each session (message count
and how often a tool span, correction, or invalidation fires) and each
repository (commit count and how often a rename fires), the valid-time epoch
and tick, and `max_events_per_log`. Counts are exact, so
`WorldConfig::declared_events` is the number of events generation emits, and
`validate` refuses a config whose declared count exceeds the bound before any
event exists (`WorldError::EventBound { events, max }`). The count is
arithmetic, so a config declaring billions of messages is refused in time
proportional to the entity count. Corrections and invalidations skip slot `0`,
which has no earlier message to target, so `*_every = 1` fires on every slot
for tool spans and renames but on every slot after the first for revisions.
There is no default for the bound: a missing field fails to parse, and a zero
bound, a non-positive tick, an epoch outside the valid-time domain, an empty
entity set, or an entity with no slots is `WorldError::InvalidField(name)`. The
bound is a config value rather than a
manifest field; the manifest carries it inside `run_identity.config`.
`GENERATOR_VERSION`, `RANDOM_SCHEMA_VERSION`, and `LINEARIZATION_RULE_VERSION`
are constants the shell copies into the run identity's `generator_version`,
`random_schema_version`, and `linearization_rule_version`. `GENERATOR_VERSION`
changes whenever a draw domain or the schedule changes, because the same seed
and config then produce a different world: `eval-generator/v1` drew time gaps
from `{0, 1, 2, 5}` ticks; `eval-generator/v2` draws from `{1, 2, 5}`, so a
correction always advances its target's revision. A tape recorded under v1
refuses under v2 as `TapeMismatch`.

### Keyed draws

Every random decision is `keyed_draw(root_seed, &site)`: the first 64 bits of
the protocol digest (`eval-random/v1`) over `(root_seed, axis, kind, actor,
site, occurrence)`, which `Chooser::choose` reduces by `% candidates` (the
small modulo bias is part of the random schema). `actor` is the entity
(`session-0`, `repository-1`), `site` is the mutation slot (`slot:3`), and
`occurrence` counts earlier choices of the same kind at that slot. A draw
depends only on its key, so shortening one entity's history leaves every other
entity's text, renames, revision targets, and times unchanged. The axis label
(`text` for words, `topology` for rename targets, `evolution` for time gaps,
observation lags, citations, and correction or invalidation targets) is fixed
per `ChoiceKind` through `ChoiceKind::axis`.

One choice is deliberately cross-entity: a message's `Cites` candidates are
the commits of every repository emitted before it, so adding or removing a
repository commit may re-point later citations (and with them `causal_depth`
and the log order) while leaving every text and rename digest alone. The
independence test asserts exactly this split.

### The choice tape

Each draw is a typed `Choice`: kind, actor, site, occurrence, a digest of the
candidate set (`eval-candidates/v1` over the candidates as serialized), and
the selected index. The tape's `identity` is `tape_identity(root_seed,
&config)`, the `eval-tape/v1` digest over the seed, the config, and the two
generator constants, so a tape replays only under the identity it was
recorded under. `Mode::Generate` records the tape; `Mode::ReplayTape` reads
it and refuses to deviate:

| Refusal | Cause |
| --- | --- |
| `TapeMismatch { expected, found }` | The tape was recorded under another seed, config, or generator. |
| `MissingChoice { entry }` | The tape ended before the generator's next choice. |
| `SiteMismatch { entry, expected, found }` | The next entry belongs to a different kind, actor, site, or occurrence. |
| `ChangedCandidates { entry, expected, found }` | The candidate set at this point differs from the recorded one. |
| `IndexOutOfRange { entry, selected_index, candidates }` | The recorded index does not name a candidate. |

A refusal poisons the `Generator`: every later `step`, `log`, and `finish`
returns the same error and no `World` is produced. Replay never falls through
to fresh generation; extending a replayed prefix is a separate mode that does
not exist in Phase 1. Under replay no draw is computed, so a replay pass
proves the tape reproduces the world, not that the draw function is
unchanged; the two-process generation test covers the latter. Entries past
the last choice the generator makes are ignored.

### Events and the log

An `Event` carries `id`, `stream` (`repository` or `session`), `entity_id`,
`local_seq`, `valid_time_ms`, `observation_time_ms`, `causal_depth`, and a
`Payload`: `message` (with an optional `cites` link to a commit), `tool_span`,
`commit`, `rename` (with an optional `previous` rename on the same path),
`correction { target }`, or `invalidation { target }`. Links name other events
by `EventId`, never by position; `EventId::derive(stream, entity_id,
local_seq)` forms the id and the validator refuses any other. Times are
canonical decimal strings on the wire, and only `i64::to_string` forms are
read back. Commits chain to their parent, renames chain to the previous
rename of the same path, tool spans follow their message, and corrections and
invalidations follow their target, so every stream has within-stream causal
edges and citations give cross-stream ones.

`EventLog` holds `events` in linearization order and `causal_edges` sorted.
The linearization key is `(valid_time_ms, causal_depth, stream label,
entity_id, local_seq)`, comparing the stream by its wire name;
`causal_depth` is one more than the deepest causal predecessor over the whole
log, so a cause always sorts before its effect even when both share a
millisecond. `EventLog::validate(max_events)` checks the schema and
rule-version literals (`SchemaMismatch`), the event bound (`EventBound`),
derived ids (`IdNotDerived`), one event per id (`DuplicateId`), strict key
order (`NotLinearized`), the valid-time domain `0..=MAX_VALID_TIME_MS` and a
non-negative observation time (`TimeOutOfDomain`), `valid_time_ms <=
observation_time_ms + MAX_REVISION_LEAD_MS` (`RevisionAhead`), that every edge
names present events (`DanglingEdge`), that each edge runs from an earlier
position (`EdgeAgainstOrder`) and a smaller depth (`EdgeAgainstDepth`) to a
later one, and that the edges are strictly sorted (`EdgesNotSorted`), so equal
causal graphs have equal digests. It does not recompute depths, so a log with a
deleted event keeps the depths it was generated with.

`EventLog::without(id)` removes one event and its incident edges and touches
no other payload: a correction whose target was removed keeps naming it, and
the log still validates. This is the deletion rule the ticket asks for ("no
repair of surviving semantic payloads"); the reducer does not promote such a
correction to an original or repair it in any way. It stays a `correction`
unit judged on its own facts, and the absent target contributes no state, so
every other unit's truth is unchanged (see "Bitemporal reducer"). Equality and
`EventLog::digest` (`eval-event-log/v1`) cover
events and edges, so two logs with the same events and different edges differ.

### Step drive

`Generator::new` validates the config, binds the tape, and draws every slot
time up front, so its cost is proportional to the declared slot count.
`Generator::step` executes one mutation slot (a message with its optional tool
span, correction, and invalidation; or a commit with its optional rename) and
returns `Step::Emitted(events)` only at quiescence, or `Step::Done`. Batches
are non-empty, arrive in emission order, and are provisional until `finish`
succeeds: a shell must treat one drive as one transaction. `Generator::log`
returns the events emitted so far, re-linearized; because the key sorts on
depth and stream, an earlier snapshot is not a prefix of a later one, though
every snapshot validates. `Generator::finish` drives any remaining slots,
closes the tape, validates the log against `max_events_per_log`, and returns
the `World`; `generate_all` is `new` followed by `finish`, so both produce
equal worlds and the emission-ordered batches sorted by the key are the log.
The bound is re-checked before each mutation's events are reserved; that check
can only fire if `declared_events` drifts from the emitters, and a sweep over
spec shapes keeps the two in agreement.

The crate's `clippy.toml` disallows `HashMap` and `HashSet`, so no
process-seeded iteration order can reach an event, a digest, or the tape.

## Eligibility spec

The kernel's `judge` decides eligibility by testing predicates in a fixed
order and returning the first verdict that holds. `eval-core` carries that
partition as a value, `EligibilitySpec`, built from the in-code table
`PREDICATES`, an array of `Predicate { name: PredicateKind, verdict }` in the
kernel's short-circuit order (state absent, superseded, invalidated, revision
differs, out of scope, sensitivity denies destination, unserved or hidden,
artifact denied). None holding is `ok`. The order is the kernel's, not the
`EligibilityVerdict` declaration order (the kernel tests provider sensitivity
before hidden). `PredicateKind` is a closed enum, so a predicate the table
does not know is a parse error rather than a runtime surprise.

Three derivations are named in the spec: the sensitivity judged is the served
class when a read serves the object and the registry class otherwise, with
secret denying every destination and sensitive denying remote; the
per-surface visibility is `hidden` when unserved and otherwise the served
class's `auto_inject`, `auto_search`, or `visibility` by surface, and a
surface permits a candidate only when the verdict is `ok` and that visibility
is not `hidden`; and a supersession invalidates its predecessor in the same
envelope, so superseded implies invalidated.

`FactTuple` is what the judge reads, as values: `state` (present or not; when
present, supersession, invalidation, revision equality, scope, and registry
sensitivity), the served class (sensitivity plus three visibilities), artifact
eligibility, and destination. An object the registry has never seen has no
state cells to fabricate. `judge(&facts)` evaluates `PREDICATES`;
`judge_with(&predicates, &facts)` evaluates any predicate list, which is how a
test shows a wrong order disagrees with a fact table; `judge_surface(&facts,
surface)` adds the fold.

The spec also carries a vector set, one `FactTuple` and verdict per row,
mirroring the kernel's independently authored eligibility table including its
precedence pairs. `serialize_spec()` is the canonical value;
`ELIGIBILITY_SPEC_DIGEST` pins its `eidnara-eligibility-spec-v1` protocol
digest. The kernel owns the same value as a file,
`crates/kernel/testdata/eligibility-spec-v1.json`, and its test
`eligibility_spec.rs` asserts the file digests to the constant, equals
`serialize_spec()`, and lists the predicates in the order copied from `judge`
by hand. Because the digest is over the parsed value, whitespace is never
drift; a reordered predicate, an added verdict, or a changed fact cell is.
`check_spec(&fixture)` returns the parsed `EligibilitySpec` only when the
digest matches, and otherwise `SpecError::SpecDrift { expected, found }` (or
`NotCanonical` for a value canonical JSON cannot encode). Those are the only
two refusals: the spec carries no numbers, the one JSON type canonical
encoding can merge, so a matching digest is value equality with
`serialize_spec()` and the parse cannot fail. The reducer refuses
before producing any truth and judges with the predicates it parsed.

The kernel differential
(`the_reducer_agrees_with_the_kernel_on_the_hand_authored_fact_tuple_table`)
is the independent check: a hand-authored table of store objects with their
fact tuples, expected verdicts per destination, and expected visibilities per
surface, run in `enumerate` mode over both destinations and all three
surfaces against `KernelStore::judge_surface_eligibility` and against
`judge_surface`, with the hand-authored facts also compared to the tuple
projected from the store's `egress_candidates`. A second test shows every
adjacent transposition of the predicate order disagrees with that table
except the first pair, which no store object can separate. The differential
iterates the kernel's own `ArtifactDestination::ALL` and `Surface::ALL` and
maps every kernel enum onto its `eval-core` mirror with an exhaustive match,
so a variant added on the kernel side fails to compile there. The kernel test
carries `eval-core` as a dev dependency only; `eval-core` keeps its four
dependencies and never names a kernel type, and
`scripts/forbid-test-support-dependencies.ts` now asserts both facts (the
closed dependency set with no build script and the library root under `src/`,
and no path into another workspace
crate, any eval-core dependency outside the closed set (dev-dependencies
included), `rusqlite`, `tokio`, or a `std` effect module (`fs`, `path`, `process`,
`time`, `net`, `env`, `io`, `os`, `thread`) in the core's source; the crate list comes
from `cargo metadata` under the names Rust code uses (Cargo renames
included), so a new workspace crate or dev-dependency is fenced without editing
the script. Paths are caught whether written as a full path or inside a brace-grouped
`use std::{...}`; renaming a crate root or globbing `std` is refused so no
alias or bare name can hide an effect path, `extern crate` of a fenced crate
is refused, `#[path]` and `include*!` are refused so no source enters from
outside the scanned tree, and the stdio and `env!` macros are refused as
effects). The source scan runs from the `cargo metadata` workspace
root and fails when it matches no files, so it cannot pass vacuously.

## Bitemporal reducer

`reduce(&log, &fixture, &query)` turns an `EventLog` into `Truth`. It refuses
an invalid query (`ReduceError::InvalidQuery`), a drifted fixture
(`ReduceError::Spec`), and an invalid log (`ReduceError::Log`, including the
event bound the query carries) before doing anything else. The `Query` is a
cut plus the facts the world does not carry: `valid_time_ms` (what is true),
`observation_time_ms` (what is known), the entity ids the project scope
names, the destination, the served class ingestion gives every unit (`None`
is unadmitted), the registry sensitivity, and `max_events_per_log`. The
served class and registry sensitivity apply to every unit alike, so `hidden`
and `provider_sensitive` are world-wide switches in this phase rather than
per-unit facts.

An event is in the cut when both its valid time and its observation time are
at or before the query's. Every event except an invalidation is a unit. A
unit in the cut has state; it is superseded when an in-cut correction targets
it, and invalidated when an in-cut correction or invalidation does. A unit
outside the cut has no state, which the rules judge `retracted`, the same
verdict the kernel gives an object it has never seen. Each unit is judged at
its own revision, so `stale` is out of reach here; the shell produces it by
asking about an older revision. A correction whose target the log does not
contain changes nothing: it is still a unit judged on its own facts, no other
unit gains or loses state, and nothing is repaired, because deletion leaves
such targets behind by design.

`Truth::required` is the set of units judged `ok`, which is exactly when a
historical question about the unit must stay answerable. `Truth` also carries
`reducer_version`, the constant `REDUCER_VERSION` (`eval-reducer/v1`), which
the shell copies into the manifest's `component_versions.reducer` the same way
the generator constants reach the run identity. The reducer never
reads a kernel result and never adjusts truth toward one: a typed kernel
refusal at run time is recorded as a refusal by the shell, not repaired into
an expectation here.

## Occurrence identity

`encode(&Occurrence { class, identity, revision, representation, span })` is
the evaluator's versioned copy of the kernel's identity rule: SHA-256 over the
class code, the identity fields in class order, the canonical decimal
revision, the representation, and the span, under `OCCURRENCE_ENCODING_VERSION`
2 and `IDENTITY_CONTRACT_VERSION` `search-projection-identity-v3`. Payload
bytes never enter. The refusal order (`EncodingRefusal`) is the kernel's. The
kernel test `eval_identity.rs` reproduces all 23 identity goldens with both
encoders, pins the two version constants and the shared limits
(`MAX_IDENTITY_VALUE_BYTES`, `HARNESSES`, `OBJECT_FORMATS`) equal, and shows
the twin rule that
makes valid time identity-bearing: a message with a later completion time is a
new occurrence of the same lineage, while a commit's time is not an input at
all.

## Renderer

`render(&log, &RenderConfig { project_id, repository_id, object_format })`
turns an event log into the fixtures the real adapters read, with explicit
times:

- Every message becomes an OpenCode message JSON (`info.id`, `sessionID`,
  `role`, and `time`: a user turn's `created` is the event's valid time; an
  assistant turn's `completed` is the valid time and its `created` sits one
  millisecond earlier, so the adapter's completed-over-created precedence is
  exercised rather than assumed) with its text part first and one completed
  tool part per tool span. The expected units are the text unit (class `messages`, revision
  the valid time) and one `raw_tool_spans` unit per tool part (revision and
  `result_revision` the span's valid time), each with the identity the encoder
  assigns. A message and its tool parts are one fixture observed once, so a
  span whose `observation_time_ms` differs from its parent's refuses
  (`ToolSpanObservationDiffers`), and a span whose session has no message with
  its `message_id` refuses (`ToolSpanParentMissing`) rather than vanish from
  accounting with neither a unit nor an exclusion rule.
- Every correction becomes a new message JSON for the same `message_id` at
  the correction's valid time with the corrected text: the same lineage, a
  later revision, so `publish` reports `replaced_object_id`. A rendered unit's
  valid time is immutable; corrections are new events. The renderer refuses a
  correction that would break that promise: a target in another session
  (`CorrectionTargetInOtherSession`; the session is an identity field, so the
  result would be a fresh lineage) or a valid time at or before the target's
  (`CorrectionDoesNotAdvance`; the result would reuse or precede the target's
  occurrence), a second base message with the same session and `message_id`
  (`MessageIdReused`; only a correction may reuse a lineage, and it says so),
  or a second rendered message with the same session, `message_id`, and valid
  time, or a second tool span with one `call_id` at one valid time
  (`OccurrenceReused`; two events would share one occurrence).
  Two corrections of one target at different valid times both render: the
  store then replaces the earlier correction with the later one inside the
  lineage, while `eval-reducer/v1` supersedes only each correction's explicit
  target and leaves both corrections required. No Phase 1 scenario compares
  `Truth` with store verdicts, so nothing observes that difference yet;
  closing it is a reducer version change, not a rendering rule.
  The generator's time gaps are strictly positive (`eval-generator/v2`), each
  slot emits at most one correction and one tool span, and its correction
  targets stay in the correcting entity, so generated worlds never meet these
  refusals.
- Every commit becomes a `RenderedCommit { message, valid_time_ms,
  observation_time_ms }`. The oid exists only once the shell writes the commit
  into a real repository, so the shell keeps the evaluator-owned
  `oid -> valid_time_ms` projection and calls `git_identity(&config, oid)` for
  the expected identity. Git units keep revision `"1"`. `RenderConfig` binds
  one `repository_id`, so a log whose commits span two repository entities
  refuses (`SecondRepository`) rather than render the second under the first's
  identity.
- Renames and invalidations have no adapter; `excluded_by_rule` counts them by
  rule so accounting never mistakes them for loss.

`check_accounting(&expected, &published, &refused)` is the load-bearing
equation for a tolerant reader: every expected identity is exactly one of
published or refused (`Missing`, `Unexpected`, `PublishedAndRefused` name the
failure). `observation_time_ms` is always at or after the valid time, so
generated fixtures never trip the one-hour `MAX_REVISION_LEAD_MS` refusal;
the boundary is proved with a hand-built unit.

## Stage ledger

`Ledger<S>` joins value-level observations of one request's stages into a
`StageVerdict`. A `Stage` type lists its stages in production order (`ALL`;
a stage's position in it is the only ordinal the ledger uses) and gives each a
`StageKind`: a `Source` produces candidates in parallel with the other sources,
a `Filter` receives what the stages before it kept and may drop some.
`ChainStage` is the activated query route and packer: `Exact`, `Lexical`,
`Dense` (sources), then `Eligibility`, `Fusion`, `Selection` (revalidation and
the response cap), and `Packing` (filters), labelled `test-only` through
`Stage::REACHABILITY`. `Surface1Stage` is the default surface, labelled
`default-production`: all thirteen stages are filters over the session's
segments and a required occurrence enters at the tail.

An `Observation` is one production return as values: the stage, a sequence
number (the highest sequence is the stage's output), the shell's token for the
`CommitReadIncarnation` the return was judged under (`None` when the return
carries none), and `Evidence`: the `Candidates` the stage kept, bounded by
`MAX_CANDIDATES_PER_STAGE_OBSERVATION` (1024, the kernel batch, pinned equal
to `kernel::MAX_ELIGIBILITY_CANDIDATES` by the seam probe) and refused above
it, or `Unjoinable` when the return describes no reusable state. The shell records an over-bound stage return as `Unjoinable` instead of
aborting the evaluator; its verdict is therefore `Indeterminate` unless later
filter evidence proves the occurrence passed it. The fields are private, so
the bound holds for every observation a ledger sees;
`Observation::at` relabels one for the self-test's misattribution control. The
ledger keys observations by (ordinal, sequence), so the fold is the same in
every arrival order; an identical repeat is a no-op and a different observation
at the same key is a contradiction, after which that key reads as unjoinable in
either order. Presence is three-valued: `NotReached` (no observation),
`ReachedEvidenceAbsent`, `Reached`; `presence` returns `None` for an
unjoinable stage, never absence.

The shell owns the required list and the terminal stage: each `Required`
names an occurrence and the source stage it must enter through, and
`verdict(required, stale, through)` names the stage the run was meant to
reach. A required occurrence's path is its entry, then every later filter up
to `through`; other sources are not its path. The verdict is the earliest
event the observations support: `FirstLoss(stage)` at the first stage on the
path whose output lacks the occurrence; `StaleIngress(stage)` when a stale
occurrence is present at `through`, naming the stage where the delivered copy
entered: its earliest sighting after the last filter whose output lacked it
(stale evidence the chain removed before `through` is `Clean`); at equal
ordinals the loss is named; `Clean` only when every required occurrence is
present at `through` and `through` itself reported candidates, so an empty
required list certifies nothing about a run that never reached it. A filter
passes only what it received, so presence at a later filter proves presence at
an unjoinable one before it, and a filter's output without a stale occurrence
proves the delivered copy did not enter earlier; an absence right behind an
unjoinable stage, a stale sighting right behind one, an unjoinable stage with
no later sighting, an entry or terminal that was never reached, a
contradiction, and two incarnation tokens in one fold are all `Indeterminate`.
A stage that ends the request reports an empty output and is a loss.

`Completed` pairs a verdict with the store's persisted
`database_incarnation_id`; `Completed::agrees_with` compares two folds'
verdicts and refuses `CrossStore` when the ids differ, so a restart that keeps
the database incarnation keeps comparability and a restore does not.

### Production taps

The route returns richer values and nothing else changes: `execute` is
`admit_lanes` then `select`, and the handler and the evaluator share that one
path. `Admitted` carries the lane statuses, the `DeclaredLanes` rankings, an
`ExactReport`, and the request context the lanes were judged under, so
`select` cannot be handed another; the rankings are read through
`Admitted::lanes`, never replaced. `ExactReport` is the exact lane's live rows
before admission plus an `ExactAdmission`: `NotJudged` (the lane ended before
admission or read nothing), `Judged` (every row under one reusable snapshot),
`Moved` (a batch's state differed from the first batch's or described no
reusable window; the report covers that batch alone), or `KernelError`.
`QueryOutcome` carries the same `ExactReport` and the revalidation
`EligibilityReport` over every fused entry. `PackingTrace::read_occurrences`
names the occurrences either packer phase read a row for; an optional request
whose row is missing is excluded, not read.
`KernelStore::hold_classification_change_for_test` (feature `test-support`)
holds the classification window open so every eligibility snapshot taken
meanwhile has no reusable generation. It holds the writer lock like every
production opener, so a classification change waits behind the window instead
of closing it early, and it refuses to open a second window from the thread
that already holds one. The `before_phase` closure is unchanged and is not a
ledger channel.

### Ledger shell

`crates/daemon/tests/eval_ledger.rs` maps the returns onto the chain:
`Exact` is the exact report's rows, or `Unjoinable` when the lane ended before
reading; `Lexical` and `Dense` are their lane rankings, or `Unjoinable` when
the lane ended unavailable; `Eligibility` is the exact report's eligible
verdicts plus the lexical and dense rankings (those lanes judge inside
retrieval), or `Unjoinable` when the exact admission is `Moved` or
`KernelError` or a lexical or dense lane ended unavailable; `Fusion` is every
entry revalidation judged; `Selection` is the entries the response carries.
A refusal after admission is an empty output at fusion when the union bound
(`fused_union`) ends the request before revalidation; every other refusal,
including the response bounds (`response_bytes`, `response_measure`),
discards the revalidation report and its incarnation, so fusion is
`Unjoinable`. `Packing` is the
required items plus every member of every `Charged::Range` in the closed
ledger, joined through the admitted group the range labels; a packer refusal
at a bound (`Required`, `OptionalBound`, `Accounting`, `CloseOverBudget`) is
an empty output, and a deadline, a projection, storage, or kernel fault, or a
caller mismatch leaves no closed render and is `Unjoinable`. The shell numbers
each distinct `CommitReadIncarnation` in first-seen order as the observation
token, and the terminal stage is `Selection` for a query-only run and
`Packing` once the packer ran.

Seven injections, one per stage, each classify to their stage and record a
`ldg_` marker only after asserting their preconditions: the exact page bound,
the lexical accepted bound, the dense `k` bound, a retired object, the fused
union bound, the result-rows cap, and the optional packing budget. The suite
also shows that a classification window held from admission or from fusion
folds as unjoinable (the verdict is `Indeterminate`, never a loss at
eligibility or selection), that an exact lane which ends after reading leaves
eligibility unjoinable, that reports from two kernel stores carry two
incarnation tokens and fold `Indeterminate`, that the observing shell and a
direct `execute` produce byte-equal bodies and equal packing admissions, that a
shell which swaps two stage labels names the wrong stage, and that missing
coverage is `Incomplete`. The query route and the packer are activated
components with no production caller; these results are labelled
`test-only` and describe no shipped behavior.

### Default-surface taps and shell

The transform returns what auto-search did: `TransformResponse.user_hint`
(daemon-internal, never on the wire) is a `UserHintPass`, either `Decided`
with a `UserHintOutcome` or `Skipped` with a `UserHintSkip` reason
(`no_eligible_tail`, `exempt_tail`, `no_text_block`, `already_decided`,
`behind_frontier`), or `None` when auto-search did not run. The outcome holds
the frozen `block_id` and `hint_text`, a `UserHintTrace`, and three fates. The
trace records each stage as the stage computed it: the suppression, length,
and token gates as passed or not (a gate field is meaningful only when every
earlier gate passed); the candidate window as the segment sequences loaded,
newest first; the match filter as the sequences with enough matched tokens,
best first; the threshold as met or not; and the capped selection the hint
renders. `deferred` is the pass holding the hint back for an already served
block, `applied` is the served block ending with the hint (the overlay's own
idempotence test), and `attached` is `attach_native_messages_incremental`'s
check that the native output carries the hint on the message the block names.
`Handler::core_for_test` and `HandlerCore::user_hint_outcome_for_test`
(features `test-support` or `direct-host-fixture`) let the direct-host
fixture's control socket return the newest pass through its
`user-hint-outcome` command, so the survivor proof is the host process's own.
The fixture build stays the production configuration: the feature compiles
only the recorder and its readers.

`crates/daemon/tests/eval_surface_ledger.rs` renders a one-session world, seeds
the fixture's store with one history segment per message whose
`end_message_id` is the message's native identity, drives a native-serving
transform through the fixture, and maps the pass onto the thirteen stages in
the evaluator's occurrence-id space. A decided pass: a gate that passed kept
every segment and a refusing gate kept none, with the search stages unreached;
the window, match filter, threshold, and cap keep the traced sequences; render,
decision freeze, deferral, overlay apply, and attachment keep the selection
when the hint is non-empty, not deferred, applied, and attached respectively.
A pass skipped as `already_decided` or `behind_frontier` is unjoinable at the
tail, because an earlier decision may still be served; any other skip is an
empty tail. Segment sequences reach occurrence ids through the segment's
stored native identity, and the identity proof runs the production adapter
over the request's own native message and the evaluator encoder over its
units to reproduce the renderer's expected id. Five injections classify to
their stage and record `sls_` markers: a segment older than the 100-segment
window (`CandidateWindow`), a prompt under the minimum length (`LengthGate`),
a raised score threshold (`Threshold`), a fourth matching segment (`Cap`), and
a native array without the tail message (`Attachment`, rendered but
undelivered). A forged adapter-side survivor and a mis-mapped identity both
fail the self-test, and a repeated pass over a frozen decision folds
`Indeterminate`. Every verdict is also placed in the failure-class table: the
lost ones classify as `interference`, the clean run as `indeterminate` under a
cassette. Surface 1 is default production, so these results are labelled
`default-production`, distinct from the activated chain's `test-only` ones.

## Failure classes

`classify(Cell)` is the pinned 48-cell truth table: `Delivery` (the ledger
verdict without its stage: clean, first loss, stale ingress, indeterminate) by
`DurableState` (held, refused, unknown) by `Slice` (live, cassette) by
`Outcome` (pass, fail). A pass has no failure to classify. A failing task is
`Interference` when the store held the knowledge and the chain lost it or
served stale evidence, `Reasoning` when the knowledge was held and delivered
and the model was live, `DurableState` when the store refused it and delivery
was anything but clean (lost, stale, or indeterminate: a refused store cannot
have been delivered, so the store is the failure), and `Indeterminate`
otherwise: an unknown durable state, a cassette slice with clean delivery
(reasoning is claimable only live), a held store with an indeterminate
delivery, or a refused store paired with a clean delivery, which contradicts.
The failing rows, with the class for each slice
(`crates/eval-core/tests/failure_class.rs` reads this table and checks every
row against `classify`):

| Durable state | Delivery | Live | Cassette |
| --- | --- | --- | --- |
| `held` | `clean` | `reasoning` | `indeterminate` |
| `held` | `first_loss` | `interference` | `interference` |
| `held` | `stale_ingress` | `interference` | `interference` |
| `held` | `indeterminate` | `indeterminate` | `indeterminate` |
| `refused` | `clean` | `indeterminate` | `indeterminate` |
| `refused` | `first_loss` | `durable_state` | `durable_state` |
| `refused` | `stale_ingress` | `durable_state` | `durable_state` |
| `refused` | `indeterminate` | `durable_state` | `durable_state` |
| `unknown` | `clean` | `indeterminate` | `indeterminate` |
| `unknown` | `first_loss` | `indeterminate` | `indeterminate` |
| `unknown` | `stale_ingress` | `indeterminate` | `indeterminate` |
| `unknown` | `indeterminate` | `indeterminate` | `indeterminate` |

`serialize_table` is the whole table in `cells()` order and
`FAILURE_CLASS_TABLE_DIGEST` pins its `eidnara-failure-class-table-v1` digest;
the manifest carries and checks it.

## Cassette `eval-cassette/v1`

Model I/O is strict-miss replay keyed by a canonical request digest over a
closed, pinned field allowlist. One schema version carries two covered-field
lists, both pinned in `eval-core` and named together by
`COVERED_FIELDS_VERSION` (`eval-cassette-covered/v1`):

- `OPENCODE_COVERED_FIELDS` is authoritative for agent-loop replay: `path`,
  `body.model`, `body.messages`, `body.system`, `body.tools`,
  `body.tool_choice`, `body.max_tokens`, `body.temperature`, `body.stream`,
  `headers.anthropic-version`, `headers.anthropic-beta`. The list was fixed by
  recording OpenCode 1.18.31 against the mock through `@ai-sdk/anthropic`: the
  body carries exactly `model`, `max_tokens`, `messages`, `system` (an array of
  text blocks), `tools` (`name`, `description`, `input_schema`), `tool_choice`
  (`{"type": "auto"}`), and `stream: true`; `temperature` is absent unless
  configured; tool results travel as `tool_result` blocks inside user messages,
  so the tool-result stream and the plugin's `eidnara_search` notes are request
  content and inside the digest. The headers are `anthropic-version:
  2023-06-01`, `x-api-key`, `user-agent` (versioned), `x-session-id`,
  `x-session-affinity`, `content-length`, `host`, and connection headers; no
  `anthropic-beta` header is sent by default. Every observed body field is
  covered, so a body field outside the list is `UnknownRequestField` rather than
  silently ignored: a provider upgrade that adds a field fails loudly instead of
  replaying the wrong answer. The system text embeds the working directory,
  today's date, and the user's instruction files, so a cassette is bound to the
  environment and day that recorded it.
- The volatile rule removes, before digesting, every `cache_control` marker
  (breakpoints move between turns) and rewrites each `cch=<nonce>;` billing
  nonce whose nonce is a run of alphanumerics, `_`, or `-` to `cch=<NONCE>;`;
  any other `cch=` text stays as written. OpenCode 1.18.31 emits no `cch=`
  nonce; the rule stays pinned for the versions that do. A mock-side
  `cache_control` move or nonce change replays; any other byte in a covered
  field misses. A fractional `temperature` is projected through
  `canonical_decimal_f64` to its exact decimal text, as the backend record is.
- `OPENCODE_HEADER_ALLOWLIST` (`anthropic-beta`, `anthropic-version`) is the
  only header set a cassette retains; `x-api-key`, `authorization`,
  `user-agent`, `x-session-id`, `host`, and `content-length` never reach the
  projection, the digest, or the file.
- `BACKEND_COVERED_FIELDS` covers the daemon's `BackendRequest`: `prompt`,
  `system`, `provider`, `model`, `max_output_tokens`, `temperature`, `harness`.
  `run_id` (a per-incarnation counter) and `session` (a per-run identity that
  would miss on every replay) are dropped; a turn replayed under another
  session therefore hits, which the daemon suite pins. `temperature` travels as
  the exact decimal `f64::to_string` produces (`canonical_decimal_f64`, finite
  and non-negative), so `0.7` and `0.70` share one digest and `0.7` and `0.8`
  do not; a hand-written `0.70` is refused.

`request_digest` is `protocol_digest("eval-cassette-request/v1", projection)`
over canonical JSON. The file is `{schema, namespace, covered_fields_version,
declarations, provenance {generator_version, input_sha256}, cases}`;
`input_sha256` is the `eval-cassette-file/v1` digest over every field but
`provenance`, including the declarations and every recorded frame, recomputed
on read, so an edited frame or declaration is `ProvenanceMismatch`; each
entry's stored digest is also recomputed from its stored request
(`EntryDigestMismatch`). `Cassette::replay` refuses a schema, generator,
covered-field-version, namespace, provenance, or entry-digest mismatch before
any request is served; `lookup` and `record` under another namespace are
`WrongNamespace`, so equal digests in another world variant never answer.
Every `CassetteError` names its wire `kind()`.

Each entry carries its `Boundary` (`opencode` or `backend`), and a lookup
answers only from entries of its own boundary. A lookup consumes the first
unconsumed entry whose digest equals the request's; equal digests replay in
recorded order, so concurrent agent-loop requests (OpenCode's title request
racing the main turn) replay in whatever order they arrive. A request with no
matching unconsumed entry is `Lookup::Miss(CassetteMiss {turn, class,
request_digest, nearest_recorded})`: `turn` counts lookups so far, `class` is
`ToolResultDrift` when only `tool_result` block contents differ from the
nearest unconsumed entry and `ModelRequestChanged` otherwise, and
`nearest_recorded` is that entry's digest (the last entry of the boundary once
every entry is consumed). The miss is the cassette's terminal: every later
lookup returns the same miss and `misses()` is one. `unconsumed()` after a run
reports entries the run never requested.

Responses are opaque to the core: the OpenCode boundary records `{status,
content_type, frames, aborted}` with the exact SSE frames the mock served
(message ids, usage, stop reasons, and provider error bodies included; an
aborted recording ends before `message_stop`), and the backend boundary records
`{events, terminal}`. Replay serves the recording byte for byte and never
regenerates a frame.

`Cassette::record` scans the covered request projection and the response
through `context_core::redaction::Redactor` in one full-text pass, so no match
can straddle a window edge, before the entry exists anywhere. A finding is
`RedactionRefused(location, SecretDetected)`; text past the scanner's 512 KiB
input cap is `RedactionRefused(location, InputLimit)`. A refused entry is never
substituted with a placeholder and never persisted, and the refusal latches:
`to_file` returns the refusal, so a recording that refused one exchange has no
file form and a partial cassette can never pass for a complete one.

### Rust oracle and the TypeScript mock

`crates/daemon/examples/eval_runner.rs` (feature `eval-runner`, an example so
it reaches the `eval-core` dev-dependency without a normal edge) serves
`cassette-oracle` over line-delimited JSON: `open {mode, namespace, path}`,
`lookup {namespace, request}`, `record {namespace, request, response}`, and
`close`. Requests arrive as `{path, headers, body_text}`; Rust parses the body,
so a malformed body is `MalformedBody` rather than a lookup of `{}`. Every
digest is computed in Rust. Refusals are `{error: {kind, detail}}` where
`kind` is the Rust error's wire name and `detail` carries only the oracle's own
values (a path, a namespace, a digest), never request content. The path must
be absolute with no `..` component; a second `open` is `AlreadyOpen`; a line
over 4 MiB is `LineTooLong`. `close` writes a recording write-then-rename
through a freshly created owner-only `.json.tmp` sibling and writes nothing for
a replay or a refused recording.

`packages/e2e-tests/src/mock-provider/cassette-oracle.ts` spawns the binary
(built through `buildDaemonExample` in `src/rust-runner/hermetic-host.ts`, or
taken from `EIDNARA_E2E_EVAL_RUNNER_BIN`), forwards over the same strict JSONL
reader the Pi runner uses, validates each reply's shape, and computes no
digest. A child exit, an unreadable reply, or a 30 s silence fails every
pending call; the child's stderr is inherited, never captured into an error.

`MockProvider.useCassette({oracle, mode, namespace})` binds the mock until
`reset()`, which also clears the miss and refusal logs. In `replay` mode the
handler hands the request to the oracle right after capture and answers with
the recorded frames or an HTTP 400 `cassette_miss` body carrying the typed
miss; the scripted-selection block is never entered, which
`scriptedSelectionCount()` and `defaultHits()` show. In `record` mode the
scripted block produces the response and the oracle admits it before a byte is
served. Any oracle failure is an HTTP 400 naming only the refusal `kind`
(`redaction_refused` or `cassette_refused`; a dead or unreadable oracle is
`OracleUnavailable`), logged in `cassetteRefusalLog()`; no message text is
served, and the server's error handler returns a fixed body instead of Bun's
stack page. Misses and refusals are 400 because the AI SDK retries 408, 409,
429, and 5xx. `MockResponse.abortAfterFrames` records a provider disconnect.

### `LlmExecutionBackend` and MemoryReviewer

`crates/daemon/tests/support/eval_cassette.rs` holds `CassetteBackend`, the
evaluator-owned `LlmExecutionBackend` impl. Recording wraps the real backend,
tees the events its sink accepted, and records `{events, terminal}` under the
`BackendRecord` projection through a serde mirror of the host types (which
carry no serde because their `Debug` redacts); replay emits the recorded events
until the sink closes and returns the recorded terminal, and a miss is
`BackendTerminal::Failed` with `provider_code: "cassette_miss"` and a message
naming the turn, class, and nearest digest. An unencodable request is
`cassette_request`; a recording the scanner refuses is `redaction_refused` and
leaves the backend with no file. `refusals()` counts every miss terminal
served, including the repeats after the first miss latched. The cassette
header's `declarations` carry what the real backend declared per harness
(`unavailable_reason`, `context_capabilities`), and the replaying backend
answers all three trait methods from them, so `BackendDeclarations::new`
latches the same capabilities from a cassette as from the real backend.
`record_of` destructures `BackendRequest` exhaustively: a new field fails to
compile until it is classified as covered or dropped.

MemoryReviewer sends through its own TLS sender, not the trait. `serve_keyed`
on the test peer answers strictly from entries keyed by `ReviewerKey {body_digest,
provider, model, credential_id}`: the SHA-256 of the request body (what
`prepare_body` puts in the attempt marker), the
`{host}/v1/messages@{anthropic-version}` identity the production sender
reports, the body's `model`, and the credential id the peer is configured with
(the header carries only the secret). A key with no entry is an HTTP 409
`cassette_miss` and a `SendError::Status(409)` at the sender, and every later
request on that peer is refused too. A run that spawns no reviewer worker
declares `memory_reviewer_model_calls: excluded` in its manifest instead.

## Paired statistics

`statistics.rs` computes the paired history effect over oracle verdicts as
exact rationals: `Ratio {numerator, denominator}` in lowest terms with both
components inside canonical JSON's safe integer range, so the two runtimes
that implement it serialize the same bytes and no fraction is ever a float.
Construction and deserialization normalize (a zero denominator or an
unreduced wire form never reaches a comparison), and every operation is
checked in 128-bit arithmetic: a statistic that would leave the safe range is
`RationalOverflow`, never a wrapped value. Nothing in the module names a
judge: `ArmResult` is `pass`, `fail`, or `censored(reason)` from the task
oracle, and the gates take nothing else.

**Campaign profile.** `CampaignProfile` is the maintainer's pre-registered
input: `noninferiority_margin`, `harm_bound`, `floor_threshold`,
`miss_asymmetry_bound` (canonical decimals in `[0, 1]`) and the four
`liveness_bounds` (`catch_up_episodes`, `embedding_passes`,
`materialization_episodes`, `reviewer_coordinator_passes`). Every field is
required; `parse_campaign_profile` refuses a missing one, and nothing derives a
value from the outcomes it gates. The margins are experimental values, not
product targets. The code and its refusal tests land without values; an
empirical acceptance needs an approved profile.

**Analysis family.** `AnalysisFamily` (`eval-analysis-family/v1`) fixes
everything a result depends on: endpoints, task families, exclusions, the
stopping rule (`fixed_n`), the multiplicity correction (`none`, `holm`,
`benjamini_hochberg`), the profile, the interval method (`cluster_bootstrap`),
the item-count threshold (at least 300), the bootstrap replicate count and
seed, and the ICC pilot. `FrozenFamily::freeze` digests it
(`eval-analysis-family-digest/v1`); the manifest records that digest as
`analysis_family_digest` before the first outcome, and `FrozenFamily::check`
refuses a family whose digest differs as
`FamilyChangedAfterResults {recorded, found}`, so a post-hoc edit to any
component is a typed refusal rather than a quiet re-analysis.

**ICC pilot.** A `ClusterKey` is a task family and a world seed; the
`family` clustering unit groups by family and the `world_seed` unit by the
world itself (family and seed together), and the pilot and the bootstrap use
the same partition. `run_icc_pilot` groups pilot observations (one paired
score per task per world) at both levels, computes the one-way ANOVA
intraclass correlation at each (`intraclass_correlation`, exact, with `m0` the
arithmetic mean group size; groups that are each internally constant give
exactly one), and picks the highest level whose ICC exceeds `1/20`
(`ICC_THRESHOLD`), with the world as the finest fallback. It carries the item
count the maximum affordable world count would yield, deflated by the design
effect `1 + (m - 1) ICC` of the selected unit, as `effective_n_at_max`; the
effect is clamped at one, so deflation only ever shrinks N, and a zero
affordable world count is refused. A family whose `effective_n_at_max` is
below the maintainer's `required_n_for_margin` makes `analyze` return
`Blocked {reason: insufficient_effective_n}` and no report object.

**Three gates.** `PairCounts::of` counts `n`, `b` (fresh not failing, aged not
passing), `c` (fresh fail, aged pass), `aged_pass`, and the censored arms. Each
censored arm resolves to the verdict least favorable to the aged arm: a
censored aged arm is not a pass (so it lands in `b` beside a fresh pass and
never in `aged_pass`), and a censored fresh arm is a pass (so it lands in `b`
beside an aged non-pass, and beside an aged pass it is a concordant pair, never
`c`). For every cell with a censored arm, `quality_loss` and `harm` are no
smaller and the aged pass rate no larger than under any definite resolution of
that arm; `censoring_never_makes_a_gate_easier_than_any_definite_resolution`
enumerates the five cells. `Gates::of` takes the profile's parsed
`ProfileRates` and evaluates `quality_loss = (b - c) / n` against the
noninferiority margin (signed: a negative value means the aged arm did better
and passes), `harm = b / n` against the harm bound, and the aged pass rate
against the floor, as three independent verdicts whose bounds come from the
profile and never from the counts; the report has no collapsed effect field.

**World-clustered interval.** `cluster_bootstrap_interval` folds each cluster
(the pilot's unit) into one `PairCounts`, resamples clusters with replacement
`replicates` times, folds each resample into one `PairCounts` whose
`quality_loss` is the replicate statistic (the same definition the gate uses),
and reports the `1/40` and `39/40` order statistics as `lower` and `upper`,
with `unit`, `method`, `n_clusters`, `n_items`, and `replicates`. Clusters
are ordered by key, and the draw is the first 64 bits of the
`eval-cluster-bootstrap/v1` digest over `{seed, replicate, draw}` reduced by
the cluster count, so the interval is a pure function of the seed on either
runtime. The function itself never goes below `ITEM_COUNT_THRESHOLD` items,
whatever a caller asks, and refuses a replicate count below
`MIN_BOOTSTRAP_REPLICATES` (40) or above `MAX_BOOTSTRAP_REPLICATES` (10,000)
as `TooFewReplicates` or `TooManyReplicates`; `AnalysisFamily::validate`
applies the same bounds, so an oversized family is refused before any
replicate runs. Below the threshold no interval of any method is emitted; the
report carries `IntervalOutcome::Withheld {reason: item_count_below_threshold}`
(or `fewer_than_two_clusters`) instead of a `computed` interval.

**Report.** `analyze(frozen, family, pairs, arm_rates)` checks the freeze
(`FrozenFamily::from_manifest` reads the manifest's recorded digest; a
manifest without one is `FamilyNotRecorded`), then the pilot's block, then the
per-arm cassette-miss asymmetry (the gap between the highest and lowest
`arm_rates.*.miss_rate`; fewer than two arms is `TooFewArms`, missing evidence
that never passes) against `miss_asymmetry_bound`, which blocks as
`arm_miss_asymmetry` with no gates computed; only then does it build
`PairedReport {analysis_family_digest, counts, gates, interval, arm_rates}`.
Per-arm miss and refusal rates travel with the report, so unsupported evidence
is visible beside every gate.

**Frozen reference.** `crates/eval-core/gen/gen-statistics-golden.ts` is an
independently written TypeScript implementation of the same quantities in
BigInt rationals, including the three gate verdicts against a fixture profile;
`bun crates/eval-core/gen/gen-statistics-golden.ts` writes
`crates/eval-core/testdata/statistics-golden.json` with
`provenance {generator_version, input_sha256}`, where `input_sha256` is the
SHA-256 of the pretty-printed, key-sorted case array, expectations included.
`tests/statistics.rs` recomputes that digest, so a hand-edited expectation is
caught, and asserts the Rust counts, rates, gate verdicts, ICC, clustering
unit, effective N, and bootstrap bounds equal the reference exactly on every
case (balanced and unbalanced pilots, constant worlds, fewer affordable worlds
than the pilot had, and both bootstrap units), so a flipped sign or a drifted
estimator on either side fails the differential. The reference reproduces the
model, not the refusals: degenerate inputs the Rust side refuses are not
fixtures.

## Coverage markers

`MARKERS` is the evaluator-owned registry: constant, globally unique names,
each with the test that records it. A test records a marker through
`Coverage::record` only after asserting the case's preconditions, never the
invariant; `record` refuses an unregistered name, and `Coverage::complete(suite)`
is `Incomplete { missing }` unless every registered marker whose test path
starts with `suite` fired, and `EmptySuite` when the prefix selects no marker
(an empty prefix names the whole registry). Each
daemon suite owns the markers whose tests it holds: `eval_ingestion.rs` the
`ing_` markers, `eval_ledger.rs` the `ldg_` markers, and
`eval_surface_ledger.rs` the `sls_` markers, and `eval_cassette.rs` the `rid_`
markers (the reviewer peer's marker is `rid_` too, because the suite owns the
prefix even though the record is `sls-memory-reviewer-model-calls-cassette-or-excluded`). Each suite checks that
every marker it owns names one of its scenarios and runs its completeness
proof on every pass: all scenarios once, then `Coverage::complete` over its
own prefix. A whole-registry proof would need one run to reach both suites'
preconditions and does not exist yet.

## Ingestion shell

`crates/daemon/tests/eval_ingestion.rs` is the Phase 1 shell over the real
adapters. It generates a world, renders it, and publishes every unit through
`opencode_units` and `SourcePublisher::publish` with the event's observation
time, then checks: published identities equal the renderer's expected
identities with `expected == published + refused` and one typed outcome per
unit (the adapter's dropped parts and a message it refuses are both accounted
for); corrections replace their predecessor and republication replays every
receipt; generated observations never lead and the lead boundary refuses;
git units read by `read_selection` from a real repository keep revision `"1"`
and their identity field set, and take valid time only from the projection,
which agrees with the committer time git recorded for each oid;
and observation time is inert for identity and eligibility (two stores ten
years apart agree on every occurrence id and verdict, and a republish at a
later observation time replays without changing the stored time). Because it
is inert, `observed_at_ms` stays on the `Keep` allowlist as an
evaluator-controlled time parameter rather than moving to `Drop`.

The composed lifecycle test bootstraps a search projection whose embedding
job for the predecessor is held pending, commits a correction through the
adapter (after showing that a same-time and a backdated correction are refused
as broken fixtures), catches the projection up, releases the held embedding
into `Publication::Obsolete(Canonical(Superseded))` while the successor's
embeds, and runs `query_route::execute` to find the successor and not the
predecessor. `eval_seam_probe.rs` names every seam the shell uses so the
`--all-features` daemon build fails when one moves; retrieval's `test-support`
seams are reachable there only because the daemon's `[dev-dependencies]`
enable that feature.

Every ingestion entry point still has no production caller; manifests carry
`adapter-ingested, production caller: none`, and no adapter is made
production-live here.
