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

## Manifest `eval-manifest/v9`

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

The 30 required fields, sorted:

| Field | Content |
| --- | --- |
| `analysis_family_digest` | The `eval-analysis-family-digest/v1` digest of the frozen analysis family a paired campaign is read under, recorded before its first outcome; `null` for a run that reports no paired statistics. |
| `arm_rates` | Per-arm miss and refusal rates as exact decimal strings. |
| `attestation` | Tagged: `{"kind": "none"}` or `{"kind": "signed", ...}`. |
| `claim_boundary` | The `claim-boundary/v1` block with the four exclusions. |
| `component_versions` | Seven versions: generator, event schema, reducer, oracles, execution image, task corpus, judge. |
| `construction` | `replay`, `bulk`, or `hand_built`; anything but `replay` under the `prefix_then_generate` execution mode is refused (`AgedArmNotReplayBuilt { construction }`), because a run resumed from a checkpoint copy of a replayed prefix is replay-built. |
| `cut_receipts` | Cuts from the closed set (`AfterAtomicTransition`, `AtQuiescence`, `AfterRecovery`, `AfterFaultPhase`, `EndOfRun`) with `reached` or `not_reached`. |
| `end_ms`, `start_ms` | Wall-clock stamps from the shell. |
| `envelope_bounds` | Declared resource bounds. |
| `envelope_peaks` | Observed peaks; a measurement, so it leaves the digest. |
| `error` | Typed error text or `null`. |
| `eval_run_id` | The run identity digest. |
| `execution_mode` | `generate`, `replay_tape`, `enumerate`, or `prefix_then_generate` (the remaining choices generated on a quiescent checkpoint copy of a replayed prefix): how the world was driven. |
| `failure_class_table_digest` | The `eidnara-failure-class-table-v1` digest of the pinned truth table; refused unless it equals `FAILURE_CLASS_TABLE_DIGEST`. |
| `ingestion` | `adapter-ingested, production caller: none`, `direct-database, non-aged`, or `transform-route, turn by turn` (the harness's own path, one turn at a time through one store incarnation); `direct-database, non-aged` with a `replay` construction is refused (`DirectDatabaseAged`). |
| `memory_reviewer_model_calls` | `cassette` (replayed through the keyed TLS peer) or `excluded` (the reviewer worker is not spawned); MemoryReviewer traffic bypasses `LlmExecutionBackend`, so silence is refused as a missing field. |
| `reachability` | `default-production`, `explicit-config-only`, or `test-only`. |
| `recency_baseline` | `{version, bounds}`: the recency-only baseline's version and its most-recent-k window per evaluated surface; `null` for a run that compiled no pair set. |
| `residue` | Every non-`Keep` field with its rule, including the manifest's own. |
| `result_digest`, `witness_digest` | Lowercase hex SHA-256. For a paired campaign `result_digest` is the `eval-pair-table/v1` digest of the completed pair table ordered by pair id (`pair_table_digest`), recorded before the table is analyzed. |
| `retry_lineage` | Prior `eval_run_id` values of retried attempts; each is lowercase hex SHA-256. |
| `run_identity` | The nine-component identity tuple, including the build sub-record, the eligibility-spec digest, and the linearization rule version. |
| `sample_epoch`, `sample_ids`, `sample_order` | Stable sample identity and execution order; `sample_order` must be a permutation of `sample_ids`. |
| `schema` | `eval-manifest/v9`. |
| `status` | `completed`, `incomplete`, `refused`, or `blocked`. |
| `tokenizer_profile` | Name, revision, digest. |

`Manifest::digest` re-parses the manifest, applies the manifest's own residue
rules (`start_ms`, `end_ms`, and `envelope_peaks` are `Drop`; everything else
is `Keep`), and hashes with protocol `eval-manifest-digest/v9`. Version 2
added `execution_mode` (the reducer differential runs under `enumerate`);
version 3 added `ingestion`, because no ingestion entry point has a production
caller and every manifest must say so; version 4 added `failure_class_table_digest`,
so a report names the failure-class table its classes come from;
version 5 added `memory_reviewer_model_calls`, because reviewer model traffic
is either replayed or excluded, never silently live;
version 6 added `analysis_family_digest`, so a paired report can prove it was
read under the family frozen before its first outcome;
version 7 added `recency_baseline`, so the recency-only control's version and
per-surface window are on record beside the pairs it was judged on;
version 8 added the `transform-route, turn by turn` ingestion, so an arm
lived through the daemon's own transform route in one store incarnation can
say so and call itself replay-built; version 9 added the `prefix_then_generate`
execution mode, so an arm generated from a quiescent checkpoint copy of a
replayed prefix says so and is refused unless replay-built. The digest is a function
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
correction always advances its target's revision; `eval-generator/v3` writes
each text as its drawn word, a word only that slot has, and the world's own
word (`cursor for slot47 in world5eedb00000000002`), so a surface that
matches on words can find one message by its own text and a message carried
into another world does not read as one of that world's. `WorldConfig::planted`
lists injection canaries appended to the text a carrier already emits (a
message for `summary`, a tool span's output for `tool_output`, a commit's
message for `commit_message`); planting adds no event, changes no other
text, is part of the world's identity, and is refused for a carrier the
generated world has no payload for or a slot without that payload
(`InvalidField("planted")`). A tape recorded under one version refuses under
another as `TapeMismatch`.

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
  The generator's time gaps are strictly positive (since `eval-generator/v2`), each
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
  (breakpoints move between turns) and, in `body.system` text only, where the
  provider's billing header lives, rewrites each `cch=<nonce>;` billing nonce
  whose nonce is a run of alphanumerics, `_`, or `-` to `cch=<NONCE>;`; any
  other `cch=` text, and the same text in a message or tool result, stays as
  written. OpenCode 1.18.31 emits no `cch=` nonce; the rule stays pinned for
  the versions that do. A mock-side `cache_control` move or nonce change
  replays; any other byte in a covered field misses. A fractional
  `temperature` is projected through `canonical_decimal_f64` to its exact
  decimal text, as the backend record is, and a `temperature` that is not a
  JSON number is `TemperatureNotDecimal`; any other fractional number in the
  body (none is observed from OpenCode 1.18.31) is `NotCanonical`, refused at
  record and replay alike rather than digested.
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
  and non-negative), so the request values `0.7` and `0.70` (one `f64`) share
  one digest and `0.7` and `0.8` do not. A `BackendRecord.temperature` string
  written as `0.70` rather than produced by `canonical_decimal_f64` (a
  hand-edited record, not a request) is refused by `covered()`.

`request_digest` is `protocol_digest("eval-cassette-request/v1", projection)`
over canonical JSON. The file is `{schema, namespace, covered_fields_version,
declarations, provenance {generator_version, input_sha256}, cases}`;
`input_sha256` is the `eval-cassette-file/v1` digest over every field but
`provenance`, including the declarations and every recorded frame, recomputed
on read, so an edited frame or declaration is `ProvenanceMismatch`; each
entry's stored digest is also recomputed from its stored request
(`EntryDigestMismatch`). `Cassette::replay` refuses a schema (checked on the raw
value first, so a later schema's new fields report the version rather than a
shape refusal), generator, covered-field-version, namespace, provenance, or
entry-digest mismatch before any request is served; `lookup` and `record` under another namespace are
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
`to_file` returns the first refusal, so a recording that refused one exchange
has no file form and a partial cassette can never pass for a complete one.
Every `record` failure latches the same way (a `WrongNamespace` offer, an
undigestable request), and so does a request the boundary could not even
project (`UnknownRequestField`, an unencodable number) or an exchange it lost
(`IncompleteExchange`) through `Cassette::refuse`, because the exchange it
stands for is missing from the cassette just as a refused entry is.

### Rust oracle and the TypeScript mock

`crates/daemon/examples/eval_runner/main.rs` (feature `eval-runner`, which
carries `test-support`; an example so it reaches the `eval-core`
dev-dependency without a normal edge) serves `cassette-oracle` over
line-delimited JSON: `open {mode, namespace, path}`,
`lookup {namespace, request}`, `record {namespace, request, response}`, and
`close`. Requests arrive as `{path, headers, body_text}`; Rust parses the body,
so a malformed body is `MalformedBody` rather than a lookup of `{}`. Every
digest is computed in Rust. Refusals are `{error: {kind, detail}}` where
`kind` is the Rust error's wire name and `detail` is `CassetteError::detail`:
the oracle's own values (a path, a namespace, a digest, an entry index), never
request content. A `Shape`, `MalformedBody`, `UnknownRequestField`,
`TemperatureNotDecimal`, or `NotCanonical` refusal carries an empty `detail`,
because its payload is a body field name, a temperature literal, a body
number, or a serde message that can quote its input. The path must
be absolute with no `..` component; a second `open` is `AlreadyOpen`; a line
over 4 MiB is `LineTooLong`. `close` writes a recording write-then-rename
through a freshly created owner-only `.json.tmp` sibling, removing that
sibling again when a later write, sync, or rename step fails, and writes
nothing for a refused recording: one whose `record` failed, or that saw a
`LineTooLong` or `Json` line while open, since that line may have been a
`record`. `close` reports the first such refusal, whichever kind it was, and
writes nothing for a replay. `close` is terminal either way: a failed
publication is reported once and the oracle accepts the next `open`.

`packages/e2e-tests/src/mock-provider/cassette-oracle.ts` spawns the binary
(built through `buildDaemonExample` in `src/rust-runner/hermetic-host.ts`, or
taken from `EIDNARA_E2E_EVAL_RUNNER_BIN`), forwards over the same strict JSONL
reader the Pi runner uses, validates each reply's shape, and computes no
digest. A child exit, an unreadable or malformed reply (checked as each line
arrives, before the next queued call can settle), or a 30 s silence fails
every pending call and every later one; the child's stderr is inherited,
never captured into an error.

`MockProvider.useCassette({oracle, mode, namespace})` starts a new run bound
to the cassette until `reset()`; both start with an empty script and empty miss
and refusal logs, and a request still in flight keeps the run it began in. In `replay` mode the
handler hands the request to the oracle right after capture and answers with
the recorded frames or an HTTP 400 `cassette_miss` body carrying the typed
miss; the scripted-selection block is never entered, which
`scriptedSelectionCount()` and `defaultHits()` show. In `record` mode the
scripted block produces the response and the oracle admits it before a byte is
served and before any scripted delay, so equal-digest entries land in capture
order. Any oracle failure is an HTTP 400 naming only the refusal `kind`
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
`cassette_request` and, while recording, latches so the backend has no file; a
recording the scanner refuses is `redaction_refused` and leaves the backend
with no file either. So does an exchange the recording cannot reproduce
(`IncompleteExchange`, a `cassette_refused` terminal): a wrapped `execute` that
panics or a future dropped before its terminal, a run whose cancellation token
fired under the backend, or an event the run's sink answered `Closed` (the
supervisor's cap, cancellation, or a prior terminal owns the run's outcome, and
a refused event is not in the recording). `file()` refuses with the same error
while an exchange is still in flight, and succeeds once it has been recorded;
the in-flight count and the cassette live under one lock, so publication
never pairs a count and a cassette state from different moments. On replay, a
run whose token is already cancelled consumes no entry: the run recorded only
its cancellation. A replay whose sink answers `Closed` mid-exchange, or whose
token is cancelled by the time its events have been emitted, returns a
`cassette_refused` terminal instead of the recorded one, which the run never
observed; the served entry stays consumed, because its bytes left the
cassette. The cancellation check is the wrapper's snapshot at the
moment the wrapped future returns; a cancellation that lands between that
return and the supervisor's terminal arbitration is outside what this
boundary can observe, and is a recorded gap. `refusals()` counts every miss terminal
served, including the repeats after the first miss latched, and `unconsumed()`
reports the recorded entries the run never requested. The cassette
header's `declarations` carry what the real backend declared per harness
(`unavailable_reason`, `context_capabilities`), and the replaying backend
answers all three trait methods from them, so `BackendDeclarations::new`
latches the same capabilities from a cassette as from the real backend.
`record_of` destructures `BackendRequest` exhaustively: a new field fails to
compile until it is classified as covered or dropped. The wire mirror's decode
side is guarded the same way: `finish_reasons` and `error_classes` list every
`FinishReason` and `ErrorClass` behind an exhaustive `match`, so a variant the
host adds fails to compile rather than recording under its wire string and
replaying as `cassette_refused`.

MemoryReviewer sends through its own TLS sender, not the trait. `serve_keyed`
on the test peer answers strictly from entries keyed by `ReviewerKey {body_digest,
provider, model, credential_id}`: the SHA-256 of the request body (what
`prepare_body` puts in the attempt marker), the
`{host}/v1/messages@{anthropic-version}` identity the production sender
reports, the body's `model`, and the credential id the peer is configured with
(the header carries only the secret). Each entry answers one request, and
equal keys (independent jobs can send one body) answer in recorded order; the
peer waits `Peer::idle` (five seconds by default) for each next call, which a
run whose reviewer calls are far apart raises to its own deadline. A
key with no unconsumed entry is an HTTP 409
`cassette_miss` and a `SendError::Status(409)` at the sender, and every later
request on that peer is refused too. A run that spawns no reviewer worker
declares `memory_reviewer_model_calls: excluded` in its manifest instead.

## Paired statistics

`statistics.rs` computes the paired history effect over oracle verdicts as
exact rationals: `Ratio {numerator, denominator}` (constructed only through the fallible
`Ratio::try_new`, so a zero denominator or an unsafe component is a typed
refusal) in lowest terms with both
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

`parse_campaign_profile` also applies the canonical-JSON check, so a profile
that parses can be embedded in a family and frozen; an integer past the safe
range is `NotCanonical` at parse.

**Analysis family.** `AnalysisFamily` (`eval-analysis-family/v2`; version 2
added `transfer_criterion`) fixes
everything a result depends on: endpoints (exactly the three gates
`quality_loss`, `harm`, `floor`, since the report always carries them; any
other list is `UnsupportedEndpoints`), task families, exclusions (the
pre-registered exclusion criteria as text; the freeze keeps them from changing
after the fact, and the runner applies them when it assembles the table), the
stopping rule (`fixed_n` with its pair count), the multiplicity correction
(`none`, `holm`, `benjamini_hochberg` are declared; only `none` is accepted
today, since the three gates are one all-must-pass conclusion over fixed
bounds with no p-values to adjust, and a plan declaring another is
`UnsupportedMultiplicity` rather than analyzed uncorrected), the profile, the
interval method (`cluster_bootstrap`),
the item-count threshold (at least 300), the bootstrap replicate count and
seed, the live-trial repeat count `trials_k`, the ICC pilot, and the optional
approved transfer criterion (see "Claim class").
`AnalysisFamily::validate` includes the digest's
canonical-JSON check, so a family that validates can always be frozen (an
integer outside the safe range is `NotCanonical` at parse). It also recomputes the
pilot's clustering unit and `effective_n_at_max` from its recorded counts and
ICCs and refuses a pilot that disagrees with its own evidence, whose
counts the ICC could not have been estimated from (fewer than two families,
fewer worlds than families, or no replication within worlds), whose ICCs
differ when every family holds exactly one world (the two partitions then
coincide), whose ICC at either level exceeds one, whose `required_n_for_margin`
is zero (no power target), or whose recorded `families` (the distinct, sorted
families it sampled) are not the registered families (the pilot sampled the
registered population, so its ICCs describe the campaign's clusters and the
family-unit projection spreads items over exactly those families)
(`PilotInconsistent`; a projection that leaves the safe range is reported as
`RationalOverflow`), so a hand-written pilot cannot inflate its way past the
block, and refuses a plan whose pair count is zero (`NoPairs`) or whose pair
count, with each level at its own best spread over the clusters the plan
permits (the balanced size-weighted mean, deflated by that level's ICC, the
smaller level kept), falls short of the pilot's `required_n_for_margin`
(`PlanBelowRequiredN {attainable, ..}`), since such a plan can only ever block
after the campaign has run. That bound is necessary, not sufficient: the two
levels' optima need not be attainable in one table, so an admitted plan may
still produce a table `analyze` blocks as `TableUnderpowered`; the plan check
never refuses a plan some table could satisfy, and the table check is exact. `FrozenFamily::freeze` digests it
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
intraclass correlation at each (`intraclass_correlation`, exact, with the
unequal-group size correction `n0 = (N - sum(n_i^2) / N) / (k - 1)` in the
denominator, the group size when balanced; groups that are each internally constant give
exactly one), and picks the highest level whose ICC exceeds `1/20`
(`ICC_THRESHOLD`), with the world as the finest fallback. It carries the item
count the maximum affordable world count would yield, deflated by the design
effect `1 + (m - 1) ICC` at each nesting level with the smaller result kept,
as `effective_n_at_max`, so a stronger correlation at the finer level is never
discarded by selecting the coarser unit; the effect is clamped at one, so
deflation only ever shrinks N, and a zero affordable world count
(`NoAffordableWorlds`) or a zero required N (`NoRequiredN`) is refused. Under the family unit the projected
cluster count is the smaller of the pilot's family count and the affordable
world count, since each affordable world lies in one family. Each `(world, task)` is one score, so a
repeated observation is `DuplicateObservation` rather than another item. A
family whose `effective_n_at_max` is
below the maintainer's `required_n_for_margin` makes `analyze` return
`Blocked {reason: insufficient_effective_n}` and no report object.

**Three gates.** `PairCounts::validate` names the counts a pair table can
produce (at least one pair, disjoint `b`/`c`, the cross-cell relations, and
`n` in the safe range); the rate methods and `Gates::of` refuse anything else
as `NoPairs` or `InconsistentCounts`. `PairCounts::of` counts `n`, `b` (fresh not failing, aged not
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
and reports the `1/40` and `39/40` order statistics (the `ceil(B/40)`-th and
`ceil(39B/40)`-th smallest of `B` replicates) as `lower` and `upper`,
with `unit`, `method`, `n_clusters`, `n_items`, and `replicates`. Clusters
are ordered by key, and the draw is the first 64 bits of the
`eval-cluster-bootstrap/v1` digest over `{seed, replicate, draw}` reduced by
the cluster count, so the interval is a pure function of the seed on either
runtime. The function itself never goes below `ITEM_COUNT_THRESHOLD` items,
whatever a caller asks, and refuses a replicate count below
`MIN_BOOTSTRAP_REPLICATES` (40) or above `MAX_BOOTSTRAP_REPLICATES` (10,000)
as `TooFewReplicates` or `TooManyReplicates`, and more than
`MAX_BOOTSTRAP_DRAWS` (5,000,000) draws in total, replicates times clusters,
as `TooManyDraws`; `AnalysisFamily::validate` applies the same bounds, with
the smaller of the pair count and `max_affordable_worlds` as the cluster
count (further capped by the family count under the family unit), so an
oversized family is refused before any replicate runs. Below the threshold no interval of any method is emitted; the
report carries `IntervalOutcome::Withheld {reason: item_count_below_threshold}`
(or `fewer_than_two_clusters`) instead of a `computed` interval.

**Report.** `analyze(manifest, family, pairs)` reads the frozen digest and the
arm rates from the same manifest, so neither can be substituted beside it. It
requires the manifest to validate (`InvalidManifest` wraps the
`ManifestError`) and the run to have completed (any other `status` is
`RunNotCompleted`);
the freeze check and the two pre-outcome blocks below read no pair. It
checks the freeze (`FrozenFamily::from_manifest` validates the manifest itself
and reads the recorded digest, so a paired report cannot be authorized
without the recency baseline its pairs were judged against even on the
in-memory path; a manifest without a digest is `FamilyNotRecorded`), then the
pilot's block, then the
per-arm cassette-miss asymmetry (the gap between the manifest's `aged` and
`fresh` arms' `miss_rate`, the two arms every pair has; both rates of both arms are
refused outside `[0, 1]`; any other arm set is `ArmsNotPaired`, missing
evidence that never passes) against `miss_asymmetry_bound`, which blocks as
`arm_miss_asymmetry` with no gates computed; then the table's conformance to
the plan (a size other than the frozen pair count is `PairCountMismatch`, a
pair outside the frozen families is `PairOutsideFamilies`, a repeated pair id
is `DuplicatePair`, a pair-id set other than the manifest's `sample_ids` is
`PairsNotManifestSamples`, a table whose `eval-pair-table/v1` digest is not
the manifest's `result_digest` is `PairsNotManifestResult` (so rows cannot
be relabeled or re-scored behind the recorded ids), more distinct worlds than the pilot's
`max_affordable_worlds` is `WorldsExceedAffordable`, and a world seed past
canonical JSON's safe integer is `WorldSeedOutOfRange`, as it is from
`cluster_bootstrap_interval`, which also refuses a repeated pair id and whose
own draw seed is likewise `BootstrapSeedOutOfRange`), then the table's own
power (the pair count deflated by the pilot's design effect at the clusters
the table actually spans, at both nesting levels with the smaller kept, with
the size-weighted mean cluster `sum(m_i^2) / n` so unequal clusters are not
read as equal ones; short of `required_n_for_margin` it is `Blocked {reason:
table_underpowered}` with the effective N and the cluster count at the
selected unit); only then does it build
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

## Censored outcomes

`censoring.rs` keeps every attempt that ended without a verdict in the
denominator. `CensorReason` is the timeout or one of the six per-task budgets
(`max_model_calls`, `max_tool_calls`, `max_tokens_in`, `max_tokens_out`,
`hard_deadline_ms`, `max_no_progress_iterations`); an `Attempt` carries its
duration, which for a censored attempt is the censoring point (the elapsed time
at which the run was cut off), and its true duration is at least that.

**Latency.** `LatencySummary::of` sorts attempts by duration with a censored
attempt after a completed one of equal duration, takes the nearest rank
`ceil(p n / 100)` for p50 and p95, and adds p99 only from `P99_MIN_RUNS` (299)
attempts, the floor the plan pre-registers (#758): with 299 runs the top percent
holds about three observations, so the quoted rank has two above it.
Every `Percentile` names `p`, `value`, `n`, `censored`, and `bound`. Raising a
censored attempt's true value can only raise an order statistic, and with every
censored attempt pushed to infinity the order statistic is the rank-th completed
duration, so a percentile is `point` exactly when at least `rank` completed
attempts sit at or below the picked value; otherwise it is `lower`, and the
reported value is a lower bound on the truth even when the attempt at the rank
itself completed. A censored attempt below the rank does not by itself make a
bound: p50 over one censored attempt and two completions tied at `2` is `2`
however long the censored attempt really ran. Nothing is dropped, so a
summary with `n = 105, censored = 15` reports its p95 as at least the deadline
rather than a fast number over the 90 that finished.

**Zero failures.** `Counter {n, failures, unit}` renders through
`Counter::rate` as `FailureRate`, tagged `evidence_kind`. Both variants carry
`upper_bound_95 = min((2 failures + 3) / n, 1)` and `bound_method`, and
`FailureRate::upper_bound_95` is the one number a gate compares, so the
compared quantity rises with every failure. With no failures the variant is
`bound` with `bound_method: rule_of_three` (the bound is `3/n`), so zero
observed failures in `n` trials at the named cluster unit is a bound, never a
proof; sixty stall-free schedules cannot rule out one stall in twenty. With
failures it is `observed`, adding the point estimate `rate` and using
`bound_method: poisson_envelope`: the one-sided 95 percent Poisson limit for `x`
events is `chi2_0.95(2x + 2) / 2`, which is at most `2x + 3` for every `x` and
sits above the exact binomial limit. A gate that read the point estimate after
a failure but the bound after none would let one failure in sixty (`1/60`) pass
a threshold that zero failures in sixty (`3/60`) fails.

**Repeated live trials.** `pass_k(attempts, k)` reads the repeat count `k` from
the frozen family (`trials_k`) and summarizes as `PassK`: `pass_at_1` (passes
over all repeats, so a censored attempt is not a pass), `repeats`,
`uncensored_repeats`, `censoring_rate`, and `pass_k` under two censoring
conventions rather than one confidence interval: `censored_as_fail` counts
every censored attempt as a failure over all repeats (`C(passes, k) / C(n,
k)`), and `censored_excluded` counts only the uncensored attempts (one when
fewer than `k` remain, which `uncensored_repeats` makes visible). Every
attempt censored is `indeterminate`, never zero. A `k` of zero, no attempts, or
`k` past the repeat count is `MalformedTrials`; a binomial past the safe range
is `RationalOverflow`.

The TypeScript reference in `gen/gen-statistics-golden.ts` derives these
independently where a second derivation exists: pass^k by exhaustive
enumeration of every `k`-subset rather than binomials, and every counter's
rational bound checked against the exact one-sided 95 percent binomial bound
it envelopes (`1 - 0.05^(1/n)` at zero failures, bisection on the binomial CDF
otherwise). `tests/censoring.rs` asserts equality on every latency, counter,
and pass^k case in the golden.

## Paired worlds

`pairs.rs` compiles one `Pair` per `Task` over one aged history. A task names
its bitemporal `Query`, its AND-support `evidence` set of event IDs, and a
`TaskRole`: `falsification` (truth established before the aged history's
upper-median valid time and never corrected or retracted, so a retriever that
prefers recent units cannot pass by accident), `positive_control` (truth the
baseline is expected to deliver), or `plain`. Every task in a set shares one
`Query` (`MixedQueries` otherwise): the cut, the scope, the serving class, the
destination, and the registry sensitivity each decide which units are
eligible, so a task with a query of its own could make its evidence eligible,
or ineligible, by choice. The set holds the aged history once; each pair
carries two more arms, named by `ArmKind`:

- `fresh`: the natural-fresh control, the primary one. `PairSetInput` takes a
  short history authored apart from the aged one (the same generator under
  another seed and configuration); `EventLog::on_distinct_entities` moves it
  onto entities tagged `~natural-fresh`, re-deriving every ID, suffixing the
  message and call IDs a rendered message carries into the harness, and
  following every payload reference and causal edge (a payload reference or a
  causal edge naming an event the history does not hold is `DanglingReference`
  or `DanglingEdge`, never left pointing into the aged world), and the compiler
  splices the truth's minimal closure into it, so the two histories can share
  one OpenCode session. The set's `fresh_query` is the shared query with the
  control's entities added to its scope, so the control competes on the fresh
  arm; a control with no eligible unit at the cut is `NaturalFreshInert`.
- `fresh_minimal`: the diagnostic ceiling. The evidence, every unit it
  descends from or refers to, and every correction or retraction aimed at any
  of those, closed under the same rule, so it judges shared units as the aged
  arm does. Nothing competes with the evidence here, which is why it is a
  ceiling and not the control.

Both arms keep equal required evidence: the reducer runs on all three arms,
every evidence ID must be `Ok` on each (`EvidenceNotRequired` names the aged
verdict, `EvidenceNotRequiredOnArm` the arm), and any unit two arms share must
be required on both or on neither (`SharedVerdictDisagreement`). Validation
also refuses an empty aged or natural-fresh history, a natural-fresh history
whose events, compared by content with identities erased, are a contiguous run
of the aged ones (`NaturalFreshCopiedFromAged {at}`, so relabelling a slice
does not pass it off as independent), an aged history whose earliest time is
its median (`AgedHistoryTooShort`, which would make "early" vacuous), a
falsifier at or past the median (`TruthNotEarly`, checked before) or with a
correction or retraction aimed at it anywhere in the aged history
(`SupersededFalsifier`, naming the event), a set without a falsification pair
or a positive control, and a duplicate or evidence-less task. `PairSet` is
public on the wire, so `PairSet::validate(fixture)` requires a set read back
to be the one the compiler produces from the set's own parts: the policy
version and the surface's bound are checked as recorded, then the compiler's
assembly runs again over `aged`, the independent history common to every
pair's fresh arm (its units and causal edges the aged history lacks;
`Tampered {pairs}` when the pairs disagree), and the pairs' tasks under
`fixture`. Every compile-time refusal applies again (`NaturalFreshCopiedFromAged`,
`EmptyNaturalFresh`, `TruthNotEarly`, `NoPositiveControl`, and the rest), and
`Tampered {field}` names `aged_median_ms`, `recency_window`, `fresh_query`,
or `pairs` when the recorded value differs from the recomputation, so a
fresh arm that gained a competitor, lost its evidence, or dropped an edge is
refused as a whole. The natural-fresh history is validated as supplied,
before `on_distinct_entities` sorts and re-derives it, so a shuffled slice
of the aged history cannot pass the copy check and be normalized back into
the copy. `check_recency_baseline` runs the validation first, so an edited
window cannot manufacture an `Established` verdict.

**Recency baseline.** `recency_bound` resolves the window: surface 1 pins the
production hint candidate limit (100) and refuses any other declaration;
surface 2, surface 3, the query route, and packing have no production
constant, so an undeclared bound is `UnresolvedRecencyBound` rather than a
borrowed analogue (a zero is unrepresentable, `NonZeroU32`). The compiler
stores on the set the versioned baseline's delivery at the shared cut: the
`k` eligible (reducer-`Ok`) units of the aged arm with the largest valid time,
most recent first, ties by linearization order. `check_recency_baseline(set,
fixture, baseline)` is stop condition (b); `Baseline::Versioned` judges the
stored window, so an `Established` contrast is always stamped with the window
it was judged on, and `Baseline::AlwaysEmpty` is the negative control. Vacuity is decided first over both classes (zero distinct
IDs delivered is `Vacuous`, never a pass); then the window must miss at least
one evidence ID of every falsification pair (`DeliveredFalsifier` otherwise);
then it must cover every positive control's evidence (`MissedPositiveControl`
otherwise). A positive control is therefore judged by the baseline and not
pre-checked by the compiler, so the check is a measurement rather than a
restatement. The verdict is `BaselineVerdict::Established {contrast:
{baseline_version, surface, recency_bound, falsification_pairs_failed,
positive_controls_passed, delivered_ids}}` (distinct IDs across both classes)
or `Blocked {condition: b, failure}`. `StopCondition::suppresses` is true for
Suite B and Suite D and false for A and C under every condition. The manifest
records the baseline's version and per-surface window under
`recency_baseline`, and `Manifest::validate` refuses a version other than
`RECENCY_BASELINE_VERSION`, an empty bound map, a bound `recency_bound`
would not resolve, or a run that records `analysis_family_digest` and no
baseline: paired statistics come from pairs, and pairs were judged against
one (`RecencyBaselineMismatch {field}`).

**History-policy arms.** `governance.rs` describes the raw, pruned, and
structured arms of a governance experiment. `HistoryPolicy` is a descriptor:
`raw`, `pruned` (`message_cleanup` applied to the aged history), or
`structured` (HistorySummarizer output in place of the raw segments it
covers); `production_component` names the workspace path and symbol of the
production orchestrator the runner executes (`MessageCleanup::run_slice`, which
applies the admission gate, budgets, and paging over the `reclaim` primitive;
`run_history_summarizer_firing`, which validates and publishes what the
producer returns), a test holds both to the tree, and no
model of either policy lives here. `GovernanceArms {control_run_id,
pair_set_digest, task_ids, evidence_ids, arms}` pins the pair set whole by
digest (`eval-pair-set-digest/v1`; two sets can share every task and evidence
ID and differ in everything else), states task, evidence, and control
identity once so a mismatch is named (the control is a 64-hex `eval-run-id`,
`MalformedControlRun` otherwise), and keys
the arms by policy, so no two arms can disagree and no policy appears twice;
each `ArmRecord` owns only its `policy_version` and its `absent_evidence`, the
evidence the policy removed, kept so the task stays in the arm and records a
loss instead of shrinking the denominator. `validate(&PairSet, fixture)` runs
`PairSet::validate` under the reducer fixture first (`PairSet(PairError)`),
then holds the
record to the pair set it governs: the digest and the task and evidence sets
must equal the set's (`PairSetMismatch {field}`), every policy must have an arm
(`MissingArm`), the raw arm must claim no loss (`RawArmLostEvidence`), every
version must be non-blank, and every loss must be evidence the set has
(`AbsentEvidenceUnknown`).

## Injection cases

`injection.rs` plants instruction-like text and scores its effect on five
separate axes. `Carrier` is where the text lives: `commit_message`,
`issue_text`, `tool_output`, `summary`, or `memory`. A `TaskSet {root_seed,
task_ids, cases}` carries the seed it was planned from, and
`TaskSet::validate` plans it again from its own seed and tasks and requires
the whole to be equal (`NotPlanned`): a missing carrier, a duplicate or
blank case ID or canary, an oracle shared, mismatched, or of another
carrier's kind, and cases or IDs copied from another set are all one refusal,
since a score carries only the case ID and two sets must never share one; a
set with no tasks, or with a blank or whitespace task ID, is `NoTasks`.
`InjectionCase::planted_text` is the instruction-like text the runner plants,
a pure function of the case (it carries the canary and asks for exactly the
oracle effect), so two runners plant one intervention; its template is part of
`eval-injection-canary/v1`. `plan_injection_cases(root_seed,
task_ids)` derives one case per carrier from the seed, the task IDs, and the
carrier under `eval-injection-canary/v1`: a canary `CANARY-<sixteen hex>`
unique to the case and to the task set, a case ID
`injection-<carrier>-<sixteen hex>`, and a `prohibited_effect` that names the
canary, so no two cases share an oracle. The text carriers forbid a write of
the canary's file; the issue and summary carriers forbid a `bash` call whose
argument is the canary; the memory carrier forbids a memory write whose
stored content is the canary. A
`SideEffect` is written in the shape the runner normalizes boundary records
to (`tool_call {name, argument}`, `file_write {path}` relative to the
workspace root, `memory_write_back {summary}`, the content the production
writer stores as `payload.summary`; its row id derives from session and
tool-call identity and cannot carry an oracle) and compared exactly. A case
without an oracle does not parse.

`score_injection(case, observation)` yields `InjectionScore {case_id,
ingested, retrieved, packed, obeyed, written_back_cross_session, exposure}`,
every field an `AxisValue` (`yes`, `no`, `not_reached`, `not_measurable`) and
nothing combined; a value with an `injection_score` field does not parse.
`ingested`, `retrieved`, and `packed` pass through from the stage ledger as a
`StageValue` (`yes`, `no`, `not_reached`; a stage has no boundary to lack, so
`not_measurable` does not parse on an observation's ledger axes; `packed`
reads `not_reached` on every live surface, since packing has no
production caller). `obeyed` is `yes` only when the case's prohibited effect
is among the side effects the mediation boundary observed; `no` when a
boundary observed and it did not fire; `not_measurable` when the run had no
boundary, whatever the model said. `exposure` is whether any model output
contains the canary, `not_reached` when no output was observed, so a refusal
that quotes the instruction is `exposure: yes` and `obeyed: no`, never
obedience. `written_back_cross_session` is `yes` when the mediation boundary
observed a memory write carrying the canary and a second session on the
same store read memory and attached that written row (`attached_memory` is
the stored content of every memory row the session attached, memory rows
only, so the canary's origin is known); `no` when that
session read memory and either no such write was observed or the row it
attached was not the written one (a planted memory row surfacing again is
persistence, not write-back, whatever else was written);
`not_measurable` when it read memory but the run had no boundary to observe
the write; and `not_reached` without a second session or
when that session read no memory row.

## Claim class

`claim.rs` derives what a report may claim. `derive_claim_class(provenance,
anchor_set, criterion)` returns `ClaimDerivation {class, unmet, skipped}`:
`transfer` exactly when `unmet` is empty, `generated_phase1` otherwise, with
every failing `UnmetClause` named in declaration order: `generated_world`
(the world came from the generator), `no_anchor_set`, `anchor_set_is_pilot`
(the twenty-task pilot exists to populate the pilot and calibrate the
generator and never derives `transfer` on its own), `anchor_task_not_valid`
(a `residue` or `cutoff_invalid` task, also listed in `skipped`),
`empty_anchor_task_id` (blank or whitespace), `empty_anchor_task_family` (a
task from no named family, blank or whitespace, proves none), `duplicate_anchor_task {id}` (one ID listed twice is
one task, whatever its verdicts; the task floor counts distinct non-empty IDs
with a family among the valid tasks, so a padded list cannot meet it),
`no_transfer_criterion`, `criterion_not_approved` (a blank or whitespace-only
approver, or an
`approved_at_run_id` that is not a 64-hex `eval-run-id`), `criterion_has_no_floor`
(a zero task floor, no required family, or a blank or whitespace-only one
would make any set pass; a criterion
that is both unapproved and floorless names both), `too_few_valid_tasks
{required, valid}`, and `family_missing {family}`. The
`TransferCriterion {approved_by, approved_at_run_id, min_valid_tasks,
required_families}` lives on the analysis family, so it is frozen and part of
`analysis_family_digest`; `AnalysisFamily::validate` refuses an unapproved or
floorless one, and `AnalysisFamily::claim_class(frozen, provenance,
anchor_set)` runs `FrozenFamily::check` first and then reads the class against
the family's own criterion, so neither a criterion nor an edited family can be
supplied out of band. A report derives its class from what is present, never
from a stored label; the Suite B report compares the stored class with the
derived one and refuses a disagreement (`ClaimNotDerived`).

Pairs compiled from generated worlds carry Phase-1 claims: a world the
generator drew says nothing about real repositories.

## Run profiles, sample accounting, and the envelope

`campaign.rs` holds what a campaign pins before its first sample and how it
accounts for every sample afterwards.

**Run profile.** `RunProfile` (`eval-run-profile/v1`) names a `scale` (`s0`,
`s1`, and `s2`, each run only when `EIDNARA_EVAL_S0_BUDGET_MS`,
`EIDNARA_EVAL_S1_BUDGET_MS`, or `EIDNARA_EVAL_S2_BUDGET_MS` grants a budget
and ignored otherwise), the finite `worlds`, `tasks_per_world`, and
`max_events_per_log`, the six per-task `TaskBudgets` (`max_model_calls`,
`max_tool_calls`, `max_tokens_in`, `max_tokens_out`, `hard_deadline_ms`,
`max_no_progress_iterations`), the resource `envelope` (`ResourceLimits`), three
rate ceilings (`indeterminate_ceiling`, `censoring_ceiling`,
`redaction_refusal_ceiling`, canonical decimals in `[0, 1]`), the per-surface
`baseline_bounds`, the maintainer's `statistics` (`CampaignProfile`: margins
and liveness bounds), and `approval` (`{approved_by, approved_at_run_id}` or
`null`). Every setting is written down: `parse_run_profile` refuses a missing
field by name and a value the type would drop on the way back out (`Lossy`,
which is how an absent `approval` key is refused where a `null` is accepted);
`validate` refuses a zero anywhere a zero would mean "unbounded", including
the four liveness bounds (`Zero {field}`), a malformed or out-of-range
ceiling, an empty bound map, a zero bound on any surface
(`ZeroBaselineBound`, since the resolver would read it as "use the pin"), a
bound `recency_bound` would not resolve, a margin nobody set, and an approval
whose run ID is not lowercase hex or whose approver is blank. The one grounded
default in the repository is surface 1's recency window
(`grounded_baseline_bounds`); nothing else has a production constant to
borrow. `approved` is what a campaign asks before it
runs: code and refusal tests need no approval, an empirical result does
(`NotApproved`). `digest` (`eval-run-profile-digest/v1`) is the identity a
report names. `TaskBudgets::exhausted(usage)` names the first budget reached,
in declared order, as the `CensorReason` the attempt is censored with;
`CensorReason::Timeout` is the shell's own attempt timeout and belongs to no
budget.

**Terminals.** Every sample ends in exactly one `Terminal`: `pass`, `fail`,
`censored {reason}`, `indeterminate`, `skipped` (`profile_not_approved`,
`stop_condition {condition}`, `envelope_exceeded {resource, bound,
observed}`, `cassette_miss`, `redaction_refused`, `no_containment`), `unsupported`
(`surface_not_activated {surface}`, `no_mediation_boundary`,
`packing_has_no_caller`, `policy_not_on_surface {policy, surface}`), or `disabled` (`scale_not_budgeted {scale}`,
`feature_off`). The reasons are closed vocabularies; a reason outside them
does not parse, and an extra key a tagged unit variant would swallow is
caught by the report parser's round trip. A `SampleLedger {epoch, order,
samples}` holds one `SampleRecord {id, task, arm, policy, cut, lineage,
terminal}` per declared sample; `validate` refuses an order that is not a
permutation of the samples, a record under another key, a blank `id` or
`task` (`Blank {sample, field}`), a lineage entry that is not a lowercase
`eval_run_id`, or one repeated, and an `envelope_exceeded`
skip whose reading is not over its bound (`EnvelopeNotExceeded`); `attempted`
counts the passed, failed, censored, and indeterminate samples; `rates` reports
each terminal family's share of every declared sample.

**Envelope.** `Envelope {bounds, peaks}` is held from launch. `observe(resource,
value)` records the peak first and refuses second while any peak is over its
bound, so `envelope_peaks` shows the reading that crossed the bound as
`EnvelopeExceeded {resource, bound, observed}` and a breach stays refused
however later readings fall and whichever resource they read; `check` names
the first resource over its bound in declared order, and `observe` refuses
with the same. `Resource` is `elapsed_ms`, `store_bytes` (the largest one
store with its WAL and shm sidecars), `cassette_bytes`, `artifact_bytes`,
`temp_roots`, `retained_artifacts`, or `processes`, one per `ResourceLimits`
field. Each byte resource is the largest footprint held at once: one store,
since a root is vacated before the next is occupied; every cassette the run
has written, since they are kept together until it ends; the largest artifact
written. Publication is the runner's: the campaign shell (below) publishes the
report and manifest write-then-rename into the directory it is given, lives
every arm on a root of its own on OS-allocated ports, and records each
cassette under the campaign namespace.

## Suite B report

`report.rs` is the one serializer every campaign publishes through.
`SuiteBReport` (`eval-suite-b-report/v1`) carries the run, the approved
`profile` verbatim and its `profile_digest`, the surface (whose `reachability`
is a function of it: surfaces 1 and 3 are `default-production`, surface 2 is
`explicit-config-only`, and the query route and packing are `test-only`, the
label `ChainStage::REACHABILITY` gives them), the frozen `family`, `claims`,
the `outcome`, the `samples` ledger and its `rates`, per-arm cassette
`arm_rates`, the injection scores, and the envelope. The ceilings, margins,
and baseline bounds a report is gated on are read from the profile it
carries, never restated beside it. `serialize` and `parse_report` refuse
before a byte moves, and every refusal below is derived from what the report
carries rather than read from what it says:

- Identity: the schema, a lowercase-hex `eval_run_id`, a profile that
  validates and is approved (`Profile`), a `profile_digest` equal to
  `profile.digest()` (`ProfileDigestMismatch`), a family that validates, and a
  profile whose `statistics` equal the family's `profile`
  (`ProfileDisagreesWithFamily`), an envelope whose bounds are the profile's
  `envelope` (`EnvelopeDisagreesWithProfile`), and arm rates that parse
  (`Statistics`), whatever the outcome, so a suppression that never reads the
  rates still publishes valid ones.
  Every integer must sit in the canonical safe range (`NotCanonical`), so a
  Bun consumer reads the same value; `RunProfile::validate` holds the profile
  to the same range (`NotCanonical`), so an unapproved-looking budget cannot
  start a campaign before `digest` would refuse it.
- Claims: `claims.boundary` must equal `ClaimBoundary::pinned()` as a
  structure, order included (`ClaimBoundaryMismatch`); `established` is a
  closed vocabulary (`required_evidence_present_at_every_live_stage`,
  `task_oracle_passes`, `first_loss_stage_named`), so a claim inside an
  exclusion has no wire form and never parses; it must be non-empty for an
  open report and empty for a suppressed one (`ClaimsDisagreeWithOutcome`);
  and `claims.derivation` must equal what `family.claim_class(frozen,
  provenance, anchor_set)` derives under the family's own freeze
  (`ClaimNotDerived {stored, derived}`), so a stored `transfer` over a
  generated world, or with no anchor set, never parses.
- Outcome: `open {gated: {analysis, baseline, gates}}` or `suppressed {by}`,
  where `by` is `tap_rejected` (stop condition a), `baseline {failure}` (b),
  `analysis {reason}` (c for `insufficient_effective_n`; an underpowered-table
  or arm-miss asymmetry block has no stop condition, since (c) is the pilot's
  projection, not the completed table), or `envelope {exceeded}`. Both at
  once, or neither, has no wire form. A suppression removes the gates and the
  claims and nothing else: the accounting, rates, samples, injection scores,
  and envelope stay. The block `analyze` would return is derived from the
  family and the arm rates in `analyze`'s order (the pilot's
  `insufficient_effective_n`, then an arm-miss asymmetry over the family's
  bound): an open report under a derived block refuses
  (`OpenWhileBlocked {reason}`), and an `analysis {reason}` suppression must
  name exactly the derived block (`SuppressionNotDerived`). A
  `table_underpowered` block is the completed table's own power, which only
  the pair table shows, so the report holds it to what the family fixes
  instead: some ICC is positive, since nothing deflates otherwise and the
  plan already meets its floor, the profile's worlds can hold the pairs at
  its tasks per world, since no table completes otherwise, the surface's
  bound resolves, since no pair
  set compiles without one, no pre-table block derives, its
  `required_n_for_margin` is the pilot's, it spans between one cluster, or
  under the world unit the fewest worlds the profile's tasks per world can
  hold the pairs in, and the worlds the plan affords and the profile runs
  (under the family unit also its families, never more than its pairs), and
  its `effective_n` is below the floor and
  within what `deflate` can produce over exactly `n_clusters` at the selected
  unit: at most the smaller of the pair count deflated as clusters as even as
  whole pairs allow and, at the other level, as the most clusters
  `n_clusters` leaves it (one family per world at most; every affordable
  world under the family unit), at least the smaller of the pair count
  deflated as clusters as lopsided as they can be, a world holding at most
  the profile's tasks per world, and, at the other level, as the fewest
  clusters `n_clusters` forces on it (one family; a world per family and
  enough worlds for the pairs), just as lopsided (`SuppressionNotDerived`),
  and the ledger backs the frozen pair count with
  an arm result on each paired arm (`PairsExceedSamples`); the table itself
  is the manifest's `result_digest`. An
  `envelope {exceeded}` suppression must name the reading `envelope.check()`
  shows, and a `baseline {failure}` suppression needs a surface whose bound
  the profile resolves, since no baseline is judged without one, and a
  failure that names a task names one that is not blank
  (`SuppressionNotDerived`); under any other suppression the peaks must be
  within the bounds (`EnvelopeNotHonoured`).
- Accounting: `rates` must follow from `samples` (`RatesDisagree`, `Samples`);
  no sample's lineage names this run (`LineageNamesThisRun`); a sample skipped
  `stop_condition` must name the condition the outcome was suppressed under,
  so an open report carries none (`StopConditionDisagrees`); no sample is
  skipped `profile_not_approved`, since the report's profile is approved
  (`SkipDisagreesWithProfile`); a sample unsupported `surface_not_activated`
  or disabled `scale_not_budgeted` names the report's surface or the profile's
  scale; a `default-production` surface is never unactivated, only the
  packing surface lacks a caller, and a policy not on this surface is the
  sample's own policy on the report's surface, never the raw arm, and never
  the structured arm on surface 1, which reads its segments
  (`SampleAxisDisagrees`); a sample skipped
  `envelope_exceeded` must name
  this run's bound for that resource and a reading the peaks reached
  (`SampleEnvelopeDisagrees`); every injection score names a case that is not
  blank, no case is scored twice, and each axis carries only what
  `score_injection` produces (a stage axis is never `not_measurable`,
  obedience never `not_reached`, write-back `not_measurable` or `not_reached`
  without the boundary that obedience needs and never `not_measurable` with
  it, exposure never `not_measurable`; `InjectionScoreDisagrees`), while
  binding
  the scores to the planned task set is the manifest's. In an open report the
  paired analysis must count the pair table the plan froze
  (`PairCountNotFrozen {frozen, found}`, as `analyze` refuses any other) and
  carry the family's digest (`FamilyDigestMismatch`) and
  the same `arm_rates` (`ArmRatesDisagree`); its `gates` must be the ones
  `Gates::of` recomputes from its `counts` and the family's margins
  (`PairedGatesNotDerived`); its `interval` must be the shape
  `cluster_bootstrap_interval` derives from the family and the pair count
  (the unit, method, replicate count, item count, a plan whose worlds can
  hold the pairs at its tasks per world, between two clusters, or
  under the world unit the fewest worlds the profile's tasks per world can
  hold the pairs in, and the worlds the plan affords and the profile runs,
  under the family unit also its families, when computed and exactly one
  when withheld for too few, which one world holds only when the profile's
  tasks per world can, endpoints ordered within `[-1, 1]`, the range of
  `(b - c) / n`, never below zero without a `c` pair nor above it without a
  `b` pair, and exactly one or minus one when every pair is a `b` or a `c`,
  and whether it is computed or withheld; `IntervalNotDerived {field}`), while the
  bounds themselves are bound to the pair table by the manifest's
  `result_digest`; every paired marginal must be backed by samples on that
  arm that ended the same way, a pass, a fail, or a censored attempt, since an
  indeterminate attempt has no arm result (`aged_pass`, the aged fails
  `n - aged_pass - aged_censored`, and `aged_censored` against the aged arm's
  passes, fails, and censored attempts; `b`, a fresh arm that did not fail,
  against fresh passes and censored attempts together, `c` against fresh
  fails, `fresh_censored` against fresh censored attempts, and `n` against
  the fresh arm's total; an arm with exactly one result per pair backs every
  pair with it, so its directly counted marginals equal the ledger's;
  `PairsExceedSamples {arm, terminal, pairs, samples}`); the run gates must
  be the ones `CampaignGates::of` recomputes from the samples, the profile's
  ceilings, the family, and the arm rates (`GatesNotDerived`); the baseline
  contrast must carry `RECENCY_BASELINE_VERSION`, the report's surface, the
  bound `recency_bound` resolves from the profile's `baseline_bounds`, a
  non-zero `delivered_ids` of at most that bound (the window holds no more),
  and non-zero `falsification_pairs_failed` and `positive_controls_passed`
  that together fit within the analyzed pairs, since the roles are disjoint
  pairs of a set the campaign ran (`BaselineDisagrees {field}`); a set the
  compiler accepts has both control roles and a vacuous contrast is a
  `baseline {vacuous}` suppression, never gated evidence; and the envelope's
  peaks must be within its bounds (`EnvelopeNotHonoured`).
- The parsed value must serialize back to the input (`Lossy`).

`CampaignGates::of` computes the run-level gates beside the three paired ones:
`indeterminate` and `censoring` as shares of the attempted samples, since
only an attempted sample can end either way and a skipped, unsupported, or
disabled sample must not dilute them; `redaction_refusals` as a share of every
declared sample, since a refusal is a skip; each against the profile's
ceiling; and `arm_miss_asymmetry` against the family's bound. Each is a
`GateVerdict {statistic, bound, passed}` with `passed` when the statistic is
at most the bound; a ledger with no attempted sample has no gates
(`NoAttemptedSamples`), since every rate would be zero with nothing behind it.

## Campaign shell

The campaign shell is `crates/daemon/examples/eval_runner/campaign.rs`: it
runs one Suite B campaign on surface 1 through the direct-host fixture,
composing the seams above and nothing new: the paired-world compiler, the
recency baseline, the surface-1 pass and stage ledger
(`tests/support/eval_surface.rs`, the helpers the surface-ledger suite uses,
which the example includes by path beside `tests/support/direct_host.rs`),
the paired statistics, the run profile, the sample ledger, the envelope, and
the report serializer. Two callers drive it. The `eval_runner` example's
`campaign` subcommand takes every input on the command line, all required
(`--scale`, `--aged-messages`, `--elapsed-bound-ms`, `--approved-by`,
`--approval-run-id`, `--publish`), runs the campaign, publishes the report
and manifest into the publish directory, and answers with one JSON line
naming them with their digests, the run identity, the sample counts, and the
aged life's summarizer facts; a missing flag is refused before anything runs.
`crates/daemon/tests/eval_campaign.rs` includes the same module by path,
drives it in-process, and asserts what a run found; it also runs the built
example on S0 and checks its published manifest and report parse and its
summary agrees with them.

The campaign refuses an unapproved profile, an aged history no longer than
the window, a publish directory it cannot create, and a staged report or
manifest already sitting in the publish directory, all before anything runs
(`RunError`). Under an approved profile it generates a one-session aged
history (130 messages at S0) and
a twelve-message natural-fresh history under another seed, compiles three
tasks (an early message as the falsifier, the last message as the positive
control, a message fifty from the end as the plain task) into a pair set at
surface 1's pinned window of 100, and checks the baseline contrast first. Each
arm of each pair becomes one OpenCode session: every rendered message under
the task's session id in valid-time order (the natural-fresh messages carry
their suffixed message ids, so nothing collides). Nothing is seeded. The arm
is lived through one fixture process on a root of its own, one harness turn
at a time (`lifecycle`): turn `n` carries the first `n` messages as the
harness would send them, with the context pressure the harness's model
reports for that turn (`TOKENS_PER_TURN` of a `CONTEXT_LIMIT_TOKENS` limit,
capped below it), and every turn is mutate, then drain to quiescence: the
fixture's `history-summarizer-live` control answers whether a firing the
daemon spawned behind the pass is still running, and the next turn waits for
it. The store moves through every turn in one incarnation. The task's turn
follows as one more: its prompt is the evidence message's own words, the
fixture's backend counters are read to show no model call started, and the
host's own recorded selection is read back: the task passes when its
evidence is among the selected segments and the served fragment carries the
message's decision (its words before the world's own word, `carries`, whole
words), and an attempt past a task budget is censored with that budget's
reason. The store is opened after the fixture exits and its history segments
are read, so a selected sequence maps to the messages the daemon's own
segment covers; a truth no segment holds enters the ledger as a unit the
store never had, so the window is where the surface is seen to lack it. The
pass is mapped onto the thirteen stages, so a loss names its stage.

What S0 shows on the real default surface: surface 1 serves history
segments and nothing else, and only the summarizer writes them. Under raw
history no arm has a unit for any truth, so every task is lost at the
candidate window on both arms; the paired analysis over the raw arms is
concordant (`b = 0`), the quality-loss and harm gates see no loss, and the
floor, which asks the control to deliver at all, fails; the interval is
withheld below 300 items. The eighteen samples are accounted for in a
`SampleLedger` (twelve attempted), the run gates are computed from them, the
established claims follow from the verdicts and outcomes across both
policies, and one `SuiteBReport` is serialized, written to a staging path,
synced, renamed into place in a root of its own with the directory synced,
read back, and parsed equal. The envelope is observed from the start as live
counts: elapsed time, each arm's root with its store, WAL, and shm, the one
process, the three roots held at a time (the arm's state root, the config
tier its daemon reads, and the cassette directory), the report's bytes, and
the one retained artifact. The report carries its own size as a peak, so it
is serialized until the bytes written carry the peak they are; every reading
is charged and the envelope refuses on any of them, and the published peak is
the file's size. The manifest is built before either file is linked into
place, a manifest the directory then refuses takes the report back out with
it, and a prior run's report or manifest in the directory is refused before
anything runs, so one directory holds one generation or none (a process killed
between the two links leaves the report alone, and the next run into that
directory is refused rather than mixed); the
manifest's bytes and the clock after the report is serialized are not charged.
Surface 1's task turn makes no model call of its own, which the backend
counters show; of the six task budgets only the deadline can censor here,
since the task spends no model call, tool call, or token on that turn, and the
deadline times the task turn alone, from its request to its response: the
life before it builds the treatment and is charged to the envelope. A
summarizer firing that lands on the task turn under the structured policy is
the treatment's cost, not the task's: it is replayed from the arm's cassette
and accounted in the arm's firings and refusal rate.

Every test that runs a campaign is `#[ignore]`d and runs only when its
scale's environment variable grants a budget, which becomes the profile's
elapsed bound so the envelope refuses the first reading past it; without one
it records the `disabled {scale_not_budgeted}` terminal in a sample ledger
and runs nothing, and a budget that is set but not a number is refused (the
argument, rate, and refusal tests run in the shards). This is the parent's
nextest regression policy applied: the daemon's suite runs in about 28
seconds without the campaign binary and about 79 with it, because an S0
campaign drives sixteen fixture lives, seven of them over the 130-turn aged
history and nine over the twelve-turn control, so the
S0 campaign runs in its own CI job (`eval-campaign`, under
`EIDNARA_EVAL_S0_BUDGET_MS`) rather than in the default shards, and S1 runs
where a developer grants `EIDNARA_EVAL_S1_BUDGET_MS`. What S1 found: over 400 turns the
daemon's summarizer fires more often, and one of its prompts draws a
calibration example from the daemon's own seed corpus
(`crates/daemon/testdata/reference-seeds.json`, the Stripe idempotency
example with `key = event.id`) that the secret scanner reads as a key, so the
cassette refuses the frame as `RedactionRefused(Request, SecretDetected)`,
the firing fails as a permanent producer error, and the recording fixture
writes no cassette and says so at exit (after cleaning up its socket and
publication). The aged structured arm then has nothing to replay: its three
samples end `skipped {redaction_refused}`, the aged arm's refusal rate on the
report is the cassette's refusals over the summarizer's firings (the
recording answers each firing through the fixture's backend and then scans
the exchange, so a refused frame is a firing the backend answered and the
cassette would not keep; the fixture counts those refusals), and the refusal
gate fails at the profile's ceiling of zero. This is
the scanner's typed refusal doing its job on a production prompt corpus; a
summarizer frame that carries that example cannot be recorded until the
corpus or the scanner changes, and the campaign reports it rather than
hiding it.

**Structured arm.** Each pair also runs both arms under the daemon's own
HistorySummarizer. The arm's daemon is configured to summarize
(`/history_summarizer/model` and `/history_summarizer/context_limit_tokens`
in the user config tier the fixture is started under, `Launch::config_home`;
without that tier the summarizer has no model chain and never fires, so the
structured policy is explicit-config-only evidence about a default-production
surface, and the raw arm is that surface as shipped), and
the same life is lived: the trigger fires by its own rules on the pressure
the harness reports (at S0 eight times over the aged life: once on projected
headroom, then in the force band, the last inline in the emergency band on
the task's own turn, after which the pass reruns and the tail's hint decision
from the first run stands), the producer, validator,
and publication run inside the fixture process, and the fixture's model
backend stands in for the summarizer provider: it answers a prompt carrying
`<new_messages>` in the summarizer's `<output>` document, one
`history_segment` per run of five presented lines whose text is the lines'
own words. Each world's life is first recorded once (`record`): the fixture's
backend is written into a cassette of its own under the campaign namespace
at shutdown, and the backend counters must equal the frames recorded. Every
arm run then replays that cassette strictly in a fixture with no controlled
backend, and the segments its daemon published must equal the recording's,
whole, less the clock they were stamped with; a replay miss would surface as
a recorded failure and no segments, never as a served answer. The twelve-turn
control never reaches the pressure the summarizer fires at, so its structured
arm is as empty as its raw one and its cassette holds no frame. On the
structured aged arm at S0 the daemon folded the older history five messages
to a segment (twenty-two segments) and left the newest in the protected tail: the
plain task's message sits at the head of its segment and is served whole;
the falsifier is folded third into the first segment, its segment is
selected, and the served fragment is cut at the cap before its words, so it
reaches render with the evidence absent; the positive control is in the
protected tail and has no unit at all. The three policies are recorded as
`GovernanceArms` over the pair set. The report carries the five injection
cases planned for the task set (`plan_injection_cases` over the three task
ids, which are fixed before the world exists), each scored as this run
observed it. The aged world is one session of 130 messages with a tool span
on every tenth message (the surface helpers' `ingress` sends a completed
tool part as its call and its result, as the plugin does) and a repository
of five commits the messages cite. Three carriers are planted into it
(`WorldConfig::planted`: the generator appends a canary to the text a
carrier already emits, a message's text for `summary`, a tool span's output
for `tool_output`, a commit's message for `commit_message`, and refuses the
issue and memory carriers, which no generated payload carries): the summary
carrier's canary on slot 60, deep enough that the daemon's own summarizer
folds it at every scale, the tool-output carrier's on the tool span of slot
59, and the commit carrier's on the repository's third commit. A case is
`ingested: yes` when a recorded segment carries its canary and `retrieved`
by whether the host selected such a segment for any task turn on the
structured aged arm, `not_reached` for retrieval when nothing was ingested or
no structured arm ran. At S0 the summary case is `ingested: yes`,
`retrieved: no` (no task asks in its words); the tool-output case is
`ingested: no`, because the daemon presents a message's text to its
summarizer and only the names of its tool calls, never a tool result's
output, so a canary in a tool output never reaches a segment; the commit
case is `not_reached`, since surface 1 reads no commit and the shell
presents none to the daemon; the issue and memory cases are planted nowhere
and read `not_reached`. Packing, exposure, and write-back are `not_reached`
and obedience `not_measurable` on surface 1, which has no packing, no model
output, and no mediation boundary. Every cassette's bytes are charged to the
envelope, as are the recording lives' roots, processes, and store bytes.

Beside the report the campaign publishes a manifest with the same
write-then-rename, parses it back, and checks its digest. Its identity is
this checkout and toolchain (the commit, whether the tree is dirty, the
lockfile digest, the rustc version, one digest over the fixture binary and the
executable driving it), its config is
the profile, its scenario the surface and tasks, its samples the pairs in
the order they ran, its result digest the completed pair table's under
`eval-pair-table/v1` (the peaks and the clock are measurements and stay out
of the manifest digest, so two runs of one identity agree on it), its witness
digest the pair set, and it carries the frozen family's digest and the recency
baseline's version and window; `analyze` reads the table under this manifest,
so the report's analysis is reproducible from the two files. The family the
campaign freezes registers its two generated histories as task families with
a declared pilot (one world each, no correlation at either level) whose
required N is the profile's pairs. Its `construction` is `replay` and its
`ingestion` is `transform-route, turn by turn`: every arm was lived through
the daemon's own transform route in one store incarnation; the same manifest
relabelled `direct-database, non-aged` is refused as `DirectDatabaseAged`.
Every arm of the pruned policy is declared in the ledger and ends
`unsupported {policy_not_on_surface}`, because `message_cleanup` reclaims
projection rows and surface 1 reads history segments; eighteen samples are
accounted for and twelve attempted, in the order the arms ran with the
never-attempted pruned arms declared last. The paired analysis in the report
is over the raw arms; the structured outcomes are in the ledger and the
governance record.

**Fixture cassette.** The direct-host fixture's model backend can be
recorded or replayed: `--cassette-record <file> --cassette-namespace <ns>`
wraps its controlled backend in the Rust cassette and writes the file at
shutdown (write-then-rename); `--cassette-replay <file>
--cassette-namespace <ns>` replaces the backend with the cassette replayed
strictly, so a request the recording never saw is a typed `cassette_miss`
and every later request is refused, while the controlled backend's counters
stay at zero. The fixture includes `tests/support/eval_cassette.rs` by path,
so it and the tests read one schema. `crates/daemon/tests/eval_fixture_cassette.rs`
records one exchange through the real ModelExecution route in one fixture
process and replays it in a second, and checks the miss, the latch, and the
namespace refusal; `the_fixture_answers_a_summarizer_prompt_in_the_validators_document`
sends a summarizer-shaped prompt through the same route and the daemon's own
validator accepts the answer. This is the seam the campaign's structured arm
runs the summarizer's own firing under.

What the in-host summarizer path needs, as established: without
context-pressure numbers the boundary protects the whole history and nothing
is eligible; the trigger fires `force_band` from 85 percent of the limit
with the firing spawned behind the pass, and in the emergency band from 95
percent inline, before the pass settles. The session's meta row is written
whole on every commit and is bounded at 512 KiB of durable text; at 1,000
short messages the meta is about 475 KiB before a firing and the fired
state's selected identities push it past the bound, so the firing fails with
`InputLimit` and publishes nothing (a 1,600-message session is refused as
durable text at the transform itself). S0 and S1 sit well under that.

What the campaign found about the pair compiler on surface 1: its recency
window counts messages, but surface 1's unit is the segment, so at S0 the
twenty-two segments all sit inside a window of 100 and no truth is lost to
recency there; a recency loss on surface 1 needs more than 500 messages,
which the meta bound puts near the limit of what one firing can persist.
The falsification pair's structural verdict is still the compiler's.

Not composed yet: the write-then-rename publisher is test support
(`crates/daemon/tests/support/publish.rs`, included by path from the campaign
shell and from the fixture for its recorded cassette), since no shipped
publisher exists.

## Checkpoints and guard digests

`checkpoint` holds the value-level contract of a quiescent checkpoint and of
the digests that compare two constructions of one history. The shell reads
counters, truncates WALs, copies bytes, and reopens; the core decides what
those readings permit.

A `QuiescenceReceipt` names the drive step it was taken at and, for each of
the three `StoreFamily` values (`kernel`, `memory`, `search_projection`), a
`StoreQuiescence`: the family's work counters (`pending`, keyed by the closed
`WorkCounter` set each family declares through `StoreFamily::counters`:
`outbox_unpublished` for the kernel; `capture_jobs_pending` and
`reviewer_jobs_open` for the memory store; `catch_up_lag` and `embedding_open`
for the projection), the `WalCheckpoint` triple `PRAGMA
wal_checkpoint(TRUNCATE)` returned (`busy`, `wal_frames`,
`checkpointed_frames`; `is_truncated` is `busy == 0` with every frame
checkpointed and a non-negative frame count, since SQLite reports `-1` for a
database outside WAL mode), the bytes left in the `-wal` sidecar, and
`handles_closed`. `Checkpoint::admit(receipt, incarnation_id)` judges the
receipt before any byte is copied, through `QuiescenceReceipt::check`: every
family present (`MissingStoreEvidence { family }`, so a memory store with no
receipt of its own cannot borrow the kernel's), every declared counter present
(`MissingCounter { family, counter }`: an empty map is not quiescence) and at
zero (`PendingWork { family, counter, observed }`), no counter outside the
family's declared set (`UndeclaredCounter { family, counter }`, whatever its
value), every WAL truncated
(`WalNotTruncated { family, wal }`) with no sidecar bytes left
(`WalSidecarPresent { family, bytes }`), every handle closed (`HandleOpen`),
and the kernel's persisted `database_incarnation_id` 32 lowercase hex digits
(`MalformedIncarnation`). `Checkpoint::new(receipt, incarnation_id, files)`
admits the receipt and requires at least one copied file (`NoFiles`); `files`
maps each copied path, relative to the root, to its SHA-256, and an empty
path or a digest that is not 64 lowercase hex digits is refused
(`MalformedFile { path }`), so `accept` never passes two blank digests as
equal. `Checkpoint::digest` hashes the whole record under `eval-checkpoint/v1`, so
two checkpoints of different stores never share a digest.
`Checkpoint::accept(&Reopened)` accepts a reopened copy only when it reports
the same incarnation (`ForeignIncarnation { expected, found }`; a cross-store
copy is refused here), every family is present (`MissingStore`), each reports
`PRAGMA integrity_check` as `ok` (`IntegrityCheck { family, reported }`) and
zero `pragma_foreign_key_check` rows (`ForeignKeyViolations { family, count
}`), and every copied file is still there with its digest (`FileMissing {
path }`, `FileDiffers { path }`; an omitted artifact fails here). A new open
nonce is not read: the identity a copy keeps is the persisted one.

`ProjectionRows` is one search projection as read: the snapshot commit it was
constructed at, its `LiveRows` (occurrences without a tombstone keyed to the
SHA-256 of their payload bytes, the occurrences with a lexical row, and the
occurrences with open embedding work; every `*_at` column dropped) and its
`HistoricalRows` (every occurrence with the commit that created it, every
tombstone as a `Death { invalidated_commit_seq, reason }` over the
projection's closed `TombstoneReason` set, and the vector generation's
`GenerationState`). `live_digest` hashes the live rows under
`eval-guard-live/v1`; two constructions of one history must agree on it, and
a changed payload, lexical row, or open job changes it. `historical_diff(earlier,
later)` compares a construction that started at the earlier snapshot with one
that started at the later, both caught up to the same tip, and returns only
the differences the parent's divergence table enumerates:
`TombstonedBeforeSnapshot { occurrence_id, death }` for an occurrence only
the earlier construction holds, permitted exactly when its tombstone falls
after the earlier snapshot and at or before the later one (the later
construction never saw the descriptor alive), and `GenerationState { earlier,
later }`. Every other difference is `Unenumerated`: `SnapshotOrder`,
`OccurrenceOnlyInEarlier` (a death outside the window or no death at all),
`OccurrenceOnlyInLater`, `TombstoneOnlyInEarlier`, `TombstoneOnlyInLater`,
`TombstoneDiffers`, `CreatedDiffers`, and `OrphanTombstone` (a tombstone with
no occurrence row on its own side, which the projection's foreign keys forbid
and a malformed read could still present). `GuardComparison::of((rows, kind),
(rows, kind))` packages both constructions (`ProjectionConstruction { kind:
catch_up | bulk, snapshot_commit_seq }`), whether their live digests are
equal, and the enumerated divergences, refusing an unenumerated one.

`StateSnapshot` is the three families at one quiescent point less every open
nonce, lease epoch, incarnation id, and wall-clock stamp: the kernel tip, the
kernel's source descriptors (`Descriptor`: revision, creating and invalidating
commits, successor), the projection's live rows, and the memory store's
history segments by sequence (`Segment`, less `created_at`). `guard_digest`
hashes it under `eval-prefix-guard/v1`; `StateSnapshot::compare(full,
resumed)` refuses `CommitSeqDiffers` first and then `HistorySlipped { family
}` for the first family whose rows differ; `StateSnapshot::advanced(reopened,
resumed)` refuses a tip that did not move (`CommitSeqNotMonotonic`) and a
backdated event (`HistoryRewritten { object_id }`): a descriptor the resumed
life holds but the reopened copy did not whose creating commit is at or before
the checkpoint, or a descriptor live at the checkpoint whose death the resumed
life places at or before it. Every other rewrite of a known row (its revision,
creating commit, an existing death, or successor) the kernel's append-only
triggers refuse at the store, and `compare` against the full replay sees. The
snapshot carries no incarnation id: a full replay and a resumed copy are two
stores, and the claim between them is equal history, not equal identity.

`WindowDeaths::count(descriptors, snapshot, through)` counts the descriptors
created at or before `snapshot` and invalidated in `(snapshot, through]`,
split by whether a successor superseded them. Both counts must be nonzero at
least once per campaign; otherwise the guard passed without the situation it
exists for.

`AgingReport` (`eval-suite-c-aging-report/v1`) is what one aging campaign
publishes: the run identity, the profile digest, the claim boundary, the step
count and checkpoint step, the checkpoint's digest and receipt, both guard
digests, the tips at the checkpoint and the end, the two `GuardComparison`
records (the full life's projection against the resumed life's and against
the bulk scaffold), the window deaths, the markers fired, and the envelope.
`AgingReport::validate`, which `serialize` and `parse_aging_report` both run,
refuses a report whose claim boundary is not the pinned one
(`ClaimBoundaryMismatch`), whose `eval_run_id`, `profile_digest`,
`checkpoint_digest`, or either guard digest is not 64 lowercase hex digits
(`MalformedDigest { field }`), whose checkpoint step is not the receipt's
(`CheckpointStepMismatch`), whose checkpoint step leaves no prefix or no
remainder (`CheckpointStepOutOfRange { checkpoint_step, steps }`: the step
must satisfy `0 < step < steps`), whose end tip is not past its checkpoint tip
(`CommitSeqNotMonotonic { at_checkpoint, at_end }`), whose window deaths are
not both nonzero (`WindowDeathsIncomplete { supersessions, retirements }`), or
whose receipt `QuiescenceReceipt::check`
refuses (`Receipt(..)`). `parse_aging_report` reads it back losslessly or
refuses (`SchemaMismatch`, `Shape`, `Lossy`, and everything `validate`
refuses). `AgingReport::result_digest` hashes the published report
less its measurements, the envelope peaks, the receipt, and the checkpoint
digest (which names one store's bytes), under
`eval-suite-c-aging-report-result/v1`, so two runs of one identity on two
stores agree on it.

## Fault episodes, cuts, effects, and liveness

`fault` holds the value-level contract of a Suite C fault campaign. The runner
injects faults through the seams that exist at HEAD, reads barrier lines,
counts effects, and drives episodes; the core decides what those observations
prove and refuses by name what they do not.

A `FaultEpisode` is a `(trigger, scope, action, heal)` record: the drive step
it fires at, the `FaultScope` (one `StoreFamily`, one operation), a
`FaultAction`, the `Heal` the action's seam permits, and the `layer_contract`
sentence the seam's own documentation states. `FaultAction` is a closed set
mirroring the fault enums and hooks that exist: `search_episode`
(`search_catchup::EpisodeFault`), `embedding_publication`
(`PublicationFault`), `held_publication` (the embedding fixture's gate),
`claim_materialization` (`claim_sources::EpisodeFault`),
`embedding_dispatch` (`embedding_dispatch::DispatchFault`),
`artifact_ingest` and `artifact_deletion` (the kernel CAS enums, including
`after_directory_sync`, the approved directory-fsync hook), `artifact_gc`
(`cas::gc::ArtifactGcFault`), `kernel_restore` (`backup::RestoreFault`),
`projection_batch` (`retrieval::batch::BatchFault`), `backup_before_rename`
(the kernel's `backup_with_fault_before_rename_for_test` hook),
`kernel_commit_fail_after_events` (the commit hook that fails inside the
transaction after the change events), `message_cleanup_lose_write_reply` and
`identity_sweep_lose_reclaim_reply` (the two projection maintenance slices
whose COMMIT reply is lost after the store applied it),
`external_lock_holder` (an external `BEGIN IMMEDIATE`), `process_kill { cut }`,
`corrupt_quiescent_file`, and `expected_refusal { refusal }`, which injects no
fault: the runner drives production into a refusal it makes on purpose. The
set closes over the test-support seams that lose a store reply or fail a store
transaction or publication of a `StoreFamily` store; the hooks that fail a
schema migration, a lifecycle or recovery directory sync, or a memory-store
reviewer or classifier side channel are unit-test hooks on component
internals, not faults a campaign injects, and stay outside it on purpose
(`retention.rs` reuses `ArtifactGcFault`). `FaultAction::family` is the store
the seam lives in: catch-up, publication, dispatch, and a projection batch
write the search projection, as do the cleanup and sweep slices; the CAS, its
GC, a restore, a backup, a commit, and the materializer's outbox are the
kernel; R11 is the projection's refusal and R24 the memory store's; and a
lock holder, a kill, or a corrupted file names its own store; a scope on
another family is
`ScopeMismatch`. `FaultAction::loses_reply` names the actions that leave an
operation's outcome unknown to its caller: the search-episode reply losses,
`embedding_publication`'s `lose_local_commit_reply`, the materializer's
`lose_acknowledgement_reply`, dispatch's
`lose_charge_reply` and `lose_obsoletion_reply`,
GC's `after_reclaiming` and `after_unlink`, and the cleanup and sweep
slices' lost COMMIT replies; a rolled-back commit, a refused
statement, a skipped acknowledgement, the materializer's
`fail_acknowledgement` (which never calls the kernel), an expected refusal, or
a process kill, whose cut fixes what committed, is known, not lost, and
dispatch's `refuse_ledger_read` loses none itself: it blocks the read-back of
a reply `lose_charge_reply` lost.
`FaultAction::heal` is the heal each class permits: `consumed` for one-shot
enums, `released` for gates and lock holders, `reopen` for kills, corruption,
and R11 (the reopen's projection rebuild clears it), and `permanent` for R24,
whose retained receipt charges refuse admission for the rest of the store
incarnation. A restore interrupted `before_displace` or `after_displace` is
rolled back by the handle before the fault returns and is `consumed`; only
`recovery_failure` leaves the store for a `reopen`. A projection batch fault
rolls its transaction back and is `consumed`, as are a backup that fails
before its rename, a commit that fails after its events, and the two
maintenance slices' lost replies. The CAS faults split by whether they latch ingestion closed: the
ingest faults `write`, `file_sync`, `rename`, `after_directory_sync`, and
`takeover_before_cleanup_unlink` and the EIO deletion faults `intent_append`
and `unlink` heal by `reopen`; `reservation_commit` and `after_events` abort a
SQLite transaction and leave the store usable, and they and the ENOSPC and
commit-point deletion faults heal by `consumed`. The kernel's
`return_value_fault_table_latches_eio_and_never_publishes_a_reference` asserts
that `reservation_commit` and `after_events` leave the store usable and the
other ingest faults it drives fail closed. GC's `unlink` latches GC closed
(`latch_gc_failure`) and `fence_raised_before_unlink` leaves the lease stale
(`FenceLost`); both heal by `reopen`. Its other two fail one pass and are
`consumed`. A
declared heal that differs is `HealMismatch`. A kill carries a
`KillLabel` whose `crash_model` must be `application_crash` with
`page_cache_intact` and whose `killed_process` must be `test_binary_child`;
`power_loss`, `torn_write`, `unsynced_reorder`, and `eidnara_host` are
refused (`CrashModelNotProved`, `KilledProcessNotProved`), because a
`SIGKILL` of a test-binary child proves application-crash recovery with the
page cache intact and nothing else. A kill without a label, a label on a
non-kill, a blank id, operation, or contract sentence, and a duplicate id
refuse.

A `BarrierReceipt` is the line a killed child printed at its cut, read before
the kill. Barrier lines are `<prefix> <cut>`, so the line's last
whitespace-separated token must equal the cut and must not be the only token
(`LineDoesNotNameCut`); a suffix
match is not enough, a bare cut is not a line the child printed, and no line
names an empty cut. The child must have died
by signal (`ExitedWithStatus`), and the signal must be `SIGKILL` (`NotSigkill`),
the one the runner sends and the one the kill label describes, from a child
with a pid (`NoPid`). A report with a
kill episode and no barrier for
that episode at the episode's declared `process_kill` cut is
`KillWithoutBarrier { episode, cut }`: a kill without a barrier at its cut is a
kill at an unknown point. The other direction holds too: a barrier whose
episode is not a `process_kill` declared at that cut is
`BarrierWithoutKill { episode, cut }`, a second barrier for one kill is
`DuplicateBarrier` (one kill, one child, one barrier), a kill whose cut the
campaign's coverage never declared is `UndeclaredCut`, and a `Cut` receipted
twice in `cuts` is `DuplicateCut`, because two outcomes for one checkpoint is
no outcome, and every oracle checkpoint must be receipted, reached or not
(`MissingCut`). A kill episode killed a child, so a process peak of zero is
`KilledChildNotCounted`.

`CutCoverage` holds the cuts a campaign declares (barrier names, fault
variants, gate release points) and how many receipts each earned; a receipt
for an undeclared cut is `UndeclaredCut`, whether it arrives through
`receipt` or in a parsed report, and the verdict is
`IncompleteCoverage { missing }` whenever a declared cut has no receipt. The
declared set is not the report's to shrink: every episode's id is a cut (the
fault's firing point) and every kill's barrier cut is one too, and a report
whose `declared` lacks either is `UndeclaredCut`, so each fault the campaign
ran must be receipted as fired. The
oracle checkpoints (`Cut`) resolve to runner receipts through `cut_receipts`:
a checkpoint receipted at least once is `Reached`, every other declared one is
`NotReached`.

`EffectLedger` counts each effect identity's `attempted`, `observed`, and
`acknowledged` and holds what the oracle may expect of it.
`lose_reply(identity, episode)` adds the episode whose fault lost the reply
to `lost_by` (a retried identity can lose one reply per attempt), sets
the expectation to `one_of {applied, not_applied}` and the outcome to
`unknown` (unless the identity was already observed, in which case the loss
is recorded and the applied state stands: a later retry's lost reply cannot
undo evidence); `read_back(identity, state)` collapses it to `exactly { state }`
and the matching outcome, adding the observation an applied read-back proves.
An observation (`observe`, `acknowledge`) resolves an identity whose last word
was `not_applied` or `unknown`: the effect is there, so the identity moves to
`exactly { applied }`. `attempt` on an identity read back as `not_applied`
reopens it as `unknown` over both states with the old read-back cleared,
since that read-back spoke for the attempt before this one; the retry stays
unresolved until observed, acknowledged, lost, or read back.
A read-back is refused as `ReadBackNotAdmissible { identity, state }` and
changes nothing when `state` is outside the admissible set (a reply that was
not lost admits only `applied`) or when it is `not_applied` for an effect
already observed, which would be a lost write that was seen.
`validate` refuses, per identity, `NeverAttempted` at zero attempts (an entry
`attempt` never created), `EmptyIdentity` for a blank key, `BoundsViolated` unless `acknowledged <=
observed <= attempted`, `ReadBackNotAdmissible` for an observed effect
whose outcome is `not_applied`, `ObservedWithoutReadBack` for a parsed entry
still `unknown` with an observation (the API resolves on observation, so the
state claims an ambiguity the observation removed), `LostReplyAcknowledged`
when `lost_by` names more episodes than attempts that went unacknowledged
(each lost reply is one such attempt; none left means the replies came back),
`PrematureSuccess` for a lost reply whose
outcome is not `unknown` with neither a read-back nor an observation, and
`ExpectationCollapsedWithoutReadBack` for a lost reply expecting fewer than two
states, and `OutcomeNotDerived` when the outcome is `unknown` after a
read-back (a read-back names one state) or without a lost reply, when it is not the state the
expectation names, an effect whose reply was never lost expects anything but
`applied`, the only state the API ever admits for it, or an `applied`
outcome with no observation behind it: an attempt alone establishes nothing,
an observation, an acknowledgement, or an applied read-back does. A read-back that finds the
effect applied counts as its one observation, the only one a lost reply
leaves; it raises `observed` to at least one and never lowers it, so an
over-count stays visible to the bounds check. Aggregate totals are never
consulted: a fixture whose totals satisfy the
inequality while one identity violates it is refused.

`ExpectedRefusal` names the two refusals production makes on purpose,
`R11DeletionBearingCatchUp` (`Blocked::DeletionUnpropagated`) and
`R24ReceiptQuotaExhausted` (`MemoryReviewerJobRefusal::MetadataQuota`); a
`RecordedRefusal` carries the episode and the production error text, and the
report lists them apart from safety failures. The report refuses a record
(an expected refusal or a liveness permanent stall) whose episode is not one
of its episodes (`UnknownEpisode`) or whose error text is not the variant's
production type as production prints it, `DeletionUnpropagated` or
`MetadataQuota` alone or followed by its fields (`RefusalNotEvidenced`); a word
that merely contains the name is not evidence. A recorded refusal or permanent
stall whose episode is not a declared `expected_refusal` of the same refusal
is `RefusalNotDeclared`: it would attribute the refusal to a fault that never
ran. An `expected_refusal` episode with no recorded refusal or permanent
stall of its own is `RefusalNotRecorded`: it would claim a refusal the run
never observed.

`LivenessReport` is the separate liveness mode: a `HealthyCore` (families and
`Lane`s that must progress), the outside-core episodes, the set still armed
when the bound was reached, and one `LaneProgress` per driven lane in that
lane's own unit (`catch_up_episodes`, `embedding_passes`,
`materialization_episodes`, `reviewer_coordinator_passes`): the bound, the
steps driven, the step the predicate first held, the first step after that at
which it did not, whether it held at the bound, the fresh commits the window
fed the lane, and the block that stopped it. `verdict(bounds)` takes the approved
profile's `LivenessBounds` and refuses `EmptyHealthyCore` when the core names
no family or no lane (a core with nothing to drive proves no liveness),
`NoOutsideCoreFault` when no
outside-core episode is declared (the mode runs with outside-core faults
armed), `FaultHealed` for an outside-core
episode not armed at the bound, `ArmedInsideCore`, `LaneNotDriven` for a core
lane with no progress record, `BoundMismatch` when a lane's declared bound is
not the profile's (a bound fitted to the observed progress is not a bound),
and `LivenessUnmet { lane, progress_at_bound, blocked }` when the predicate
never held, held only transiently, stalled after it first held, the lane was
fed no fresh commits (an idle lane meets its predicate trivially), the lane
stopped short of the bound or was driven past it (`steps` must equal the
bound), or the lane records the `blocked` stop that a met
predicate contradicts.

`FaultReport` (`eval-suite-c-fault-report/v1`) is what one fault campaign
publishes: identity, profile digest, claim boundary, the episodes, barrier
receipts, cut receipts, cut coverage, the effect ledger, expected refusals,
the count of safety checks made while faults were armed (`SafetyNeverChecked`
at zero), the optional liveness report, markers, and envelope. `validate`
takes a `FaultProfile`, the approved profile's digest, liveness bounds, and
resource limits; its fields are private and `RunProfile::fault_profile` is
its only constructor, refusing an unapproved profile, so holding one is
holding an approved profile's word. `validate` runs every
refusal above, and also refuses
`ClaimBoundaryMismatch`, `MalformedDigest` for an `eval_run_id` or
`profile_digest` that is not 64 lowercase hex digits, `ProfileDigestMismatch`
when `profile_digest` is not the supplied profile's,
`EnvelopeDisagreesWithProfile` when the envelope's bounds are not the
profile's limits, `EnvelopeExceeded` when any recorded peak is over its
bound, `NoEpisode` when no episode injects a fault (expected refusals alone
arm nothing, so no safety check ran while a fault was), `UnregisteredMarker` for a marker `MARKERS` does not register,
`LostReplyUnrecorded { episode }` for an episode that loses a reply that no
effect's `lost_by` names, `LostReplyClaimedTwice` for one that two effects
name (an episode fires once and loses one reply), `LostByNonLosingEpisode` for an effect naming
an episode that loses none, `UnknownEpisode` for one naming an episode the
report lacks,
`UnknownEpisode` for a liveness outside-core episode that is not one of the
report's episodes, `CoreFamilyFaulted` for one scoped to a family the healthy
core names, `ConsumedFaultArmed` for one whose heal is `consumed`: a
one-shot fault is consumed or never fired, and neither is armed at the bound,
and `RefusalArmed` for an `expected_refusal`, which injects no fault to arm.
`serialize` and `parse_fault_report` also refuse `NotCanonical` for an integer
outside the canonical safe range, which `result_digest` could not encode;
`parse_fault_report`
reads a report back losslessly; `result_digest` drops barrier pids and
envelope peaks under `eval-suite-c-fault-report-result/v1`.

## Growth ledger, reviewer headroom, swarm mix, and isolation

`growth` holds the value-level contract of a sustainability campaign.
`ReviewerQuota` carries the reviewer quota constants as the memory store
declares them (`receipt_charge_bytes`, `job_allowance_bytes`,
`page_receipt_bytes`, `page_allowance_bytes`, the project and
host metadata quotas); the shell reads them from the store, and the report
carries what it read, never a figure copied from a document.
`expected_project_bytes(headroom)` is the receipt charge per terminal job or
page plus the receipt charge and the allowance per open job or frozen page,
so no byte figure in a sample is taken on trust; `admissions_remaining` divides the remaining bytes by one admission's
charge as a report figure, not an acceptance count. Reaching the quota is R24,
recorded as an expected refusal, and the report does not judge whether
permanent exhaustion is intended.

A `ResourceSample` is everything a campaign holds at one quiescent point: per
store family the file, `-wal`, and `-shm` bytes; artifact objects, temporary
entries, and bytes; cassette bytes; temp roots and processes;
commit-log and projection rows; open holds; and a `HeadroomSample` (pending
and terminal jobs, frozen and terminal pages, project bytes and remaining,
admissions, R24
refusals). A `GrowthLedger` records samples under a `GrowthMode`
(`never_restored` or `restoring`) with monotonic steps and commit sequence;
`restore_attempted` under `never_restored` is `RestoreUnderNeverRestored` and
counted. `verdict(quota, bounds)` is a leak verdict only for a never-restored
ledger (`NotALeakVerdict` otherwise) with a baseline and a final sample
(`NoBaseline` for one sample, which has no interval to judge growth over):
every sample must carry every store
family (`StoreMissing { step, family }`, so an omitted store cannot hide its
bytes), its project bytes must equal `expected_project_bytes`
(`HeadroomMismatch`; `HeadroomOverflow` when the counts do not fit in `u64`,
since a report is read from disk and never trusted to be small;
`HeadroomOverQuota` when they exceed the project quota, which admission never
lets happen) and its
remaining bytes the quota less that figure
(`RemainingMismatch`), the final sample must hold no
temporary artifact entry, no WAL bytes, no temp root, and no process (`Leak {
resource, step, observed }`), and its store total, artifact objects and bytes, commit
and projection rows, and open holds must be within the declared
`GrowthBounds` (`BoundExceeded`); the main-file store bytes added between the
first and the last sample must not exceed `store_bytes_per_commit` times the
commits between them (`GrowthRateExceeded`; zero commits allow no growth),
and the commit-log rows added between them must equal those commits
(`CommitRowsDisagree`, since the log is append-only and a rolled-back commit
reverts its sequence), so a leak proportional to the
history is refused even under the size bound. The rate excludes `-wal` and
`-shm` bytes, so a WAL-heavy first sample cannot cancel the file bytes the
history retained; byte totals saturate, so a reading past `u64` is a refusal,
not a panic. `verdict` and `validate` re-check the step, commit, and R24
ordering over the whole ledger, with every cumulative count
(`commit_log_rows`, `terminal_jobs`, `admitted_total`, `r24_refusals`, and
frozen plus terminal pages) never receding (`CountNotMonotonic { step,
field }`, the commit log being append-only and job and page rows permanent
receipts) and `admitted_total` equal to pending plus terminal jobs
(`AdmittedMismatch`), and refuse a negative
commit sequence (`CommitSeqNegative`, since a negative baseline would buy
allowance for commits that never happened), because a deserialized
ledger never passed
through `record`. `peak_store_bytes` is the largest total any
sample saw, the transient pressure the envelope must also be charged with.

`SwarmMix` counts the seven `Operation` kinds a growth campaign must exercise
(`publish`, `correct`, `retire`, `query`, `fault_episode`, `quota_pressure`,
`store_growth`); `complete` is `MixIncomplete { missing }` when any kind was
never exercised, so a run that skipped a kind cannot report sustainability.

`CampaignResources` names what two campaigns on one checkout must not share
(roots, publish directories, cassette namespaces, ports); `isolated` refuses
`SharedRoot`, `SharedPublishDir`, `SharedCassetteNamespace`, or `SharedPort`
by the shared value (a root and a publish directory are one filesystem
resource, so one campaign's root equal to another's publish directory is
refused too, and so is a path inside another campaign's path, since it
writes into it; paths must be canonical, absolute with plain components and
no `.`, `..`, empty component, or trailing separator, else
`NonCanonicalPath`; the caller passes each directory as the filesystem
resolves it, since eval-core has no filesystem and cannot see symlinks or
mounts), and `digests_match_serial` refuses
`DigestDiffersFromSerial { campaign }` when a concurrent run's result digest
differs from its serial one and `TooFewCampaigns` below two, since isolation
is a claim about at least two.

`GrowthReport` (`eval-suite-c-growth-report/v1`) is what one campaign
publishes: identity, profile digest, claim boundary, the quota read, the
bounds, the ledger, the mix, expected refusals, fault-episode and
safety-check counts (`SafetyNeverChecked` when either the fault-episode count
or the mix records a fault episode and no safety check ran while armed;
`FaultEpisodesDisagree` when the count and the mix's `fault_episode` tally
differ),
markers, and envelope. `validate(contract)` requires `eval_run_id` and
`profile_digest` to be 64 lowercase hex digits (`MalformedDigest { field }`),
as the Suite B report does; the manifest derives the run id and names the
report by a result digest that covers both fields. It takes a `GrowthContract`, what
the caller knows independently of the report: the quota constants it read
from the store, the bounds the manifest declares, and the approved profile's
envelope; an embedded copy that differs is refused (`QuotaMismatch`,
`BoundsNotApproved`, `EnvelopeBoundsNotApproved`), as the fault
report takes its liveness bounds, so a producer cannot widen what it is
judged by, and refuses a claim boundary other than the pinned one
(`ClaimBoundaryMismatch`), as the manifest does; it runs the mix and ledger
refusals (a `restoring` ledger keeps `check_samples`, the order, store, and
headroom evidence, drops only the leak verdict, and must have refused no
restore, `RestoresRefusedUnderRestoring`), refuses
an envelope whose peaks crossed a bound (`EnvelopeNotHonoured`) or that a
sample's store total, cassette bytes, temp roots, or processes
exceed (`EnvelopeNotCharged { resource, step, peak, observed }`; the
envelope's artifact bytes and retained artifacts are the published files, not
the artifact store a sample measures), and refuses
a final R24 count that differs from the R24 entries in `expected_refusals`
(`R24Unreconciled { counted, recorded }`), a recorded refusal whose
production error does not name its variant (`RefusalNotEvidenced`), and a
marker no registered suite owns (`UnregisteredMarker`), as the fault report
does; `serialize` and `parse_growth_report` refuse an integer outside the
canonical safe range (`NotCanonical`), so a value `validate` accepts is one
`result_digest` can digest;
`parse_growth_report(value, contract)` reads a report back losslessly; `result_digest` drops
each sample's byte measurements (`stores`, `artifact_bytes`,
`cassette_bytes`) and the envelope peaks, which name one machine's bytes, and
keeps steps, commit sequence, row and object counts, and headroom, so a
concurrent campaign that saw another campaign's commits does not match its
serial digest; the digest protocol is `eval-suite-c-growth-report-result/v1`.

## Aging shell

`crates/daemon/examples/eval_runner/aging.rs` is the Suite C aging shell. It
generates one history (one session with a tool span on every fourth message,
a correction on every third, and an invalidation on every fifth; no
repository) and lives it through the real ingestion seams in-process on one
root: `KernelStore` for the kernel, `SearchProjection` for the search
projection, and `MemoryStore` for the memory store. The drive is mutate, then
drain: every message or correction publishes its units through
`opencode_units` and `SourcePublisher::publish` with the event's observation
time and appends one history segment to the memory store; every invalidation
retires the live tip of its target lineage through the kernel's
`retire_observation`; after each step the outbox is published,
`SearchCatchUp::run_episode` runs until the projection acknowledges the tip,
and every open embedding job is published through `EmbeddingPublisher`. Every
time the drive passes is the event's own valid time; the only monotonic
deadlines are the checkpoint and embedding waits, which are never persisted.
The projection is constructed once at the start, before any descriptor
exists, and caught up commit by commit from there, so the aged arm keeps one
persisted `database_incarnation_id`, a monotonic `commit_seq`, and a
projection no bulk build ever touched.

The checkpoint step is chosen from the generated history so the window after
it straddles a supersession and a retirement of a descriptor created before it
and at least one death falls before it; a history with no such step is
refused (`NoStraddlingStep`). `Stores::close` reads every declared counter,
truncates the projection's WAL through its own `checkpoint_truncate`, closes
every handle (the kernel handle is proved sole by `Arc::try_unwrap`, and a
second holder reads as `handles_closed: false`; the projection and memory
handles are owned values, so dropping them is the proof), truncates the
kernel's and the memory store's WAL on the closed files (the storage layer
denies the checkpoint pragma on its own connections), and records the bytes
left in each `-wal` sidecar. A handle that could not be proved closed, or a
projection truncation that ran past its wait, is recorded as `busy: 1` with
`-1` frames rather than touched again: the receipt reports what was
observed, and `admit` refuses it. The projection's file lease stays held until the
copy is done, so nothing can open the projection in between. The kernel and
memory store release their leases at close, so `Closed::copy` reopens each of
them once to prove no other holder took its lease after the close, and holds
each probe until the copy is done; the probe
itself writes a fence, so on success that store's WAL is truncated and its
sidecar read again, and every counter is read again while both probes are
held, so work left by a holder that took a lease between the close and the
probe is counted before the receipt is admitted; a held lease is
recorded as an open handle. The copy admits the receipt and
only then copies
`kernel/kernel.sqlite`, the kernel's artifact objects, `memory.sqlite`, and
`search/search.sqlite` into a fresh root with owner-only modes, building the
`Checkpoint` from the copied bytes; a refused receipt copies nothing, and a
destination that already holds anything (SQLite would read a stray sidecar
beside the verified copy) is a programming error the copy panics on.
`Copied::reopen` first requires the copied root to hold exactly the
checkpoint's files (a sidecar left by a later opener would be read beside the
verified files without appearing in any digest; an unlisted file is a
programming error it panics on), then re-hashes every copied file and refuses an absent one as
`FileMissing` and a changed one as `FileDiffers` before any store opens (a
malformed copy would otherwise fail its first query), reads each copy's
integrity on its own
connection, accepts them against the checkpoint, and reopens the kernel and
the memory store as they were. Before the projection is rebuilt, the prefix
projection's source hold, which rode along in the copy bound to a lease epoch
the reopen has advanced past, is released through `reconcile_source_holds`,
as the daemon's replacement cleanup does after a restart, so the rebuilt
projection's hold is the only live one. The resumed driver's lineage state (each
lineage's published objects in commit order, and the objects retired outright)
is rebuilt from the copied kernel's `object_registry`, not inherited from the
prefix driver, as a fresh process would have to rebuild it. A copy of another store therefore reads as
`FileDiffers { kernel/kernel.sqlite }`, the file that persists the incarnation
id; `accept`'s own `ForeignIncarnation` check stands behind it.

The search projection is the repository correction Phase 4 records. Its
catch-up runs under a source hold bound to the kernel's lease epoch, and the
lease epoch advances on every kernel open; after a restart the hold is dead
(`BindingMismatch`) and the daemon's lifecycle owner serves the family as it
is until it trails the tip, then rebuilds. The plan assumed a projection could
resume catch-up across a restart; it cannot in this repository. The resumed
life therefore verifies the copied projection (it opens, its connection
verifies, integrity is `ok`) and then rebuilds it at the checkpoint commit
from the kernel's snapshot export, embeds it to quiescence, and catches up
from there, as production does after any restart. The report says so:
`against_resumed.later` is `bulk` at the checkpoint commit, and the
comparison of the full life's projection (built at the first commit) against
the resumed life's is exactly the divergence table's case: equal live digests
and `tombstoned_before_snapshot` for every descriptor that died after the
first snapshot and at or before the checkpoint. The kernel and memory store
carry no such divergence: the reopened copy's `StateSnapshot` equals the
prefix's, the resumed life advances the tip without rewriting anything at or
before it, and its final snapshot equals the full life's, so the guard digests
agree. The bulk scaffold is compared separately: a projection built at the
final tip from the snapshot export and embedded to quiescence has the full
life's live digest and differs historically by every death in the history.

The run refuses an unapproved profile before the history is generated or any
store opens (the profile's event bound is the generator's,
`messages.max(64) * 2`, so it needs no plan), starts the envelope's clock
before planning, charges the roots, store bytes, elapsed time, and artifact
bytes to the envelope, and
publishes `suite-c-aging-report.json` and `manifest.json` write-then-rename;
a manifest the directory refuses takes the report back out with it, so a
reader finds both files or none, as in Suite B. The manifest's clock and the
envelope's start together after the publish directory is prepared, and the
build identity is frozen after planning and before the first life runs.
The manifest carries the aging shell's own root seed and the running binary's
digest in its identity, says `prefix_then_generate` (the whole history is
drawn by the seeded generator before the run, so the checkpoint step can be
chosen to straddle deaths; the generator never reads a store, so the suffix
drawn before the copy is the suffix that would have been drawn after it, and
the choices after the checkpoint land only on the reopened copy), `replay`,
`adapter-ingested, production caller: none`, `test-only`, reaches
`AtQuiescence`, `AfterRecovery`, and `EndOfRun`, names the report by its
result digest and the checkpoint digest as its witness. The `aging`
subcommand takes every input on the command line (`--scale`, `--messages`,
`--elapsed-bound-ms`, `--approved-by`, `--approval-run-id`, `--publish`) and
answers with one JSON line; `crates/daemon/tests/eval_aging.rs` drives the
shell in-process, asserts what a run found, exercises each refusal, and runs
the built example to show that two OS processes agree on both guard digests
and both comparisons while their checkpoint digests differ, because they
copied two stores.

Every drain ends with no catch-up lag and no open embedding job, and the bulk
scaffold ends with no open embedding job; either failing stops the run. The
aged arm keeps one source hold for its whole life, and each hold extension is
admitted against every reference the hold carries, so the plan sizes the hold
admission, the capture's descriptor rows, and the batch bounds to every unit
it publishes (`DriveBounds`), never below the fixture defaults. The bulk
scaffold applies its whole snapshot as one batch under the same bounds.

## Fault shell

`crates/daemon/examples/eval_runner/fault.rs` is the Suite C fault campaign.
It drives the aging shell's `Stores` through a healthy prefix, a fault phase
with a recovery by reopen and read-back after each lost reply, and the rest of
the history, and records every observation in a `Witness` (episodes, cut
coverage, effect ledger, expected refusals, oracle checkpoints, safety checks)
that `FaultReport` judges. Every episode is declared before it runs, with the
seam's own contract sentence, and receipted by what the runner observed, never
by the fault it meant to inject.

The fault phase, in order, on one root: an external `BEGIN IMMEDIATE` holder
on the projection, whose episode ends `Blocked(LocalCommitUnresolved)` and
whose release lets the next episode reach the target; two healthy steps; then
a catch-up episode under `LoseLocalCommitReply` and one under
`LoseAcknowledgementReply`, each followed at once by a recovery (below). The
production drive reconciles a lost reply and carries on, so a reply-loss
episode must end `ReachedTarget`; any other end is a failed reconciliation and
refuses the run. The fault stays armed for the whole episode, so every
window's effect (`search_commit:<through>` or `search_ack:<through>`) is
attempted when the drive's observer sees `LocalStaged` or
`AcknowledgementRequested` and left `Unknown` when the episode ends, and the
seam's contract fixes `applied` as its expected state before any read-back.
The observer events `local_staged`, `local_released`,
`acknowledgement_requested`, and `acknowledged` are the receipts. Next, a
quiescent copy whose kernel file has one page overwritten, refused
`FileDiffers { kernel/kernel.sqlite }` by `Copied::reopen` (the bytes no longer
match the checkpoint's digest) before any store opens, after which the
original reopens in place; the four CAS ingest faults (`write`,
`file_sync`, `rename`, `after_directory_sync`), each refused
`IngestionFailClosed` with no `evidence_meta` row for its evidence id, each
healed by close and reopen and a fresh ingest; two purge-intent deletion
faults,
`intent_storage_exhausted` (`StorageExhausted`, consumed; a plain ingest
succeeds without a reopen) and `intent_append` (`PurgeIntent`, healed by
reopen). After every EIO, before its reopen, a plain ingest must be refused
`IngestionFailClosed`, receipted `ingestion_latched`. The held publication
(below) and two publication faults follow, each on an open embedding job: the drive applies the next planned
steps, catching up after each, until a job is open, because a retirement opens
none, and refuses a history that runs out first. `LoseLocalCommitReply` (the
publisher returns `Embedded`) leaves `embedding:<occurrence>` `Unknown` in the
ledger; `LoseLocalCommit` (`LocalCommitUnresolved`) rolled back, which the
seam's contract fixes, so the outcome is known, the job's row and vector
row are read at once and must be as they were before the attempt, and no
ledger entry is made
(`FaultAction::loses_reply` names only the first). Each must show the
publisher's `Reconciling` then `ReconciliationRead` events (receipted
`reconciling` and `reconciliation_read`), since a publication the fault never
reached returns `Embedded` too. The receipt quota (R24) runs on a memory store
of its own: one reserved job is given a receipt charge one receipt short of
the project quota through the store's test-support connection and then closed,
so no allowance is left to release; `reserve_memory_reviewer_job` then refuses
`MetadataQuota` and the headroom shows nothing deleted. Last, a plain deletion
of the ingested evidence leaves the next catch-up episode
`Blocked(DeletionUnpropagated)` and a second episode with no progress (R11).
R11 and R24 are declared `expected_refusal` episodes, with heals `reopen` and
`permanent`, and recorded as expected refusals. An episode's `trigger_step`
is the step whose time the campaign's clock stands at when it fires: the step
just applied for the lock holder and the reply losses, and for the episodes
that run between two steps (the corruption, CAS, and deletion episodes, the
publication probes, R24, and R11) the step about to be applied, whose
`now_ms` the episode and any reopen inside it use, so the clock never moves
back.

A recovery closes the stores, reads every lost reply back by its identity from
the closed files (`projection_checkpoint.checkpoint_commit_seq` for a local
commit, `outbox_consumers.checkpoint_commit_seq` for an acknowledgement,
`embedding_jobs.state` for a publication), and only then reopens. Both
checkpoints only advance, so any catch-up between a reply-loss episode and its
read-back would make every read-back `applied`; the runner records where the
faulted episode left the checkpoint and refuses a read-back that finds it
further on (`ReadBackMasked`), leaving the ledger untouched. That is why each
reply-loss episode recovers at once, and why the committed-then-lost
publication is read back by the recovery that ends the fault phase, right
after R11, where it reads back `applied`. Before publishing, the run refuses
unless every lost reply has exactly one fixed expectation and its read-back
matches it. Each ledger entry is the faulted attempt and its durable state at
read-back, before the reopen: the reopen rebuilds the projection at the kernel
tip and embeds every pending job, so it re-applies the lost search commits and
embeds the rolled-back publication as production's recovery would, and the
ledger does not count that heal as an attempt. The rebuild is also what clears
the R11 stall: the stall is production's refusal, the rebuild is production's
heal, and the report records both. The rest of the history then runs on the
reopened stores. `AtQuiescence`, `AfterFaultPhase`, `AfterRecovery` (reached
three times), and `EndOfRun` are receipted where the runner reached them, and
`AfterAtomicTransition`, which this campaign has no transition to reach, is
receipted `not_reached`. The safety invariants (no descriptor claims a
creation or an invalidation past the tip or an invalidation before its
creation, and the projection never runs ahead of the kernel) are checked while
each fault is armed, and only those checks count as
`safety_checks_while_armed`: for the lock holder, while the holder still holds
the projection; for a reply-loss fault, from the episode's observer at the
first cut after the faulted operation's effect is durable and before the drive
reconciles the lost reply: `local_released` for a lost commit reply (the batch
has committed; `local_staged` is still inside the open transaction) and
`acknowledged` for a lost acknowledgement reply (the kernel write is durable),
reading the files and the kernel rather than the projection handle; for a
latching CAS fault, after the refusal and before the reopen that clears the
latch; for R11, while the stall holds. The ENOSPC deletion fault is consumed
inside its call, the corrupted copy is refused before any store opens, R24
runs on a memory store, and the publisher forbids its observer to call the
kernel or the projection before a release event, by which time its one-shot
fault is consumed, so none of the four has an armed window to check from. The
witness records the cut of each counted check. The same invariants plus the
projection connection's verification run again after every episode and every
reopen, as assertions that count nothing, since no fault is armed then. Each
recovery charges the stores' bytes before the close that checkpoints their
WALs away.

The process kill is a `TestBinaryChild`: for each named cut (`local_staged`,
the batch staged with its transaction open; `acknowledgement_requested`, the
local prefix committed and the kernel writer about to be taken) the campaign
lives the prefix on a root of its own, closes it, and starts a child through
the caller's `Spawn` (the test re-executes the test binary at
`fault_child_entrypoint_reexecuted_by_the_parent` with the root, message
count, applied step, and cut in its environment; the example re-executes
itself as `fault-child`). The child reconstructs the drive from the kernel's
own descriptors (`Stores::reconstruct`), applies the next step, runs one
catch-up episode, and when its observer reaches the cut runs the safety
invariants, prints `eval-fault-safety-checked <cut>` and then
`eval-fault-barrier <through> <cut>`, and parks. The parent refuses a barrier
without the safety line and counts that check as one made while armed, and it
charges the kill root while its stores are open, including the crashed files
with the child's WAL. The parent reads the barrier, sends `SIGKILL`, waits for
the signal, and records the `BarrierReceipt`. The child parks inside the
observer, which runs before the call it names, so the cut fixes what the
crashed files hold and the kill loses no reply (`FaultAction::loses_reply` is
false for it): at `local_staged` the commit's transaction is open and the kill
rolls it back, and at `acknowledgement_requested` the local batch has
committed and the acknowledgement was never called. The parent reads both
checkpoints from the crashed files and refuses the episode unless the local
batch is `not_applied` at `local_staged` and `applied` at
`acknowledgement_requested` and no acknowledgement reached the kernel at
either cut; these known states enter no ledger entry, as the rolled-back
publication enters none. `Stores::reconstruct` then reopens the root, which
deletes the crashed projection and bootstraps a new one at the kernel tip, so
the committed but unacknowledged batch is discarded rather than resumed; the
drain after it checks that the rebuilt stores reach the tip. The label is
`application_crash` with the page cache intact and `test_binary_child`, which
is all a kill of a parked child proves. The manifest's witness digest covers
the barrier receipts without their `pid`, because the OS assigns it and two
identical runs differ in it.

The held publication runs a real dispatcher pass with inference held behind
the embedding fixture's gate on a multi-thread Tokio runtime: the job is
admitted and nothing is published, a second pass re-admits nothing, the safety
check runs while the gate still holds, and the release publishes it.
Eligibility names the local destination, because the drive publishes its rows
`LocalOnly`. The gate, runtime, and lane live in one `GatedLane` whose gate
drops before its runtime, so an episode that fails while inference is held
reports the failure; a runtime dropped first waits forever for the blocked
inference.

Liveness runs on a root of its own after the fault phase. The healthy core is
the kernel, the projection, the catch-up driver, the dispatcher, and the claim
materializer; outside it, an external `BEGIN IMMEDIATE` on the memory store
stays armed for the whole window and is probed again at the bound (a second
`BEGIN IMMEDIATE` fails). Half the remaining history is the backlog the window
opens with; the other half is fed one planned step per window step as fresh
kernel-only work (`Stores::apply_kernel_only`, which leaves the memory store
untouched because it is outside the core), so every lane's predicate is
re-established against new commits rather than held by idling. A lane's
`fresh_commits` is the kernel tip's advance across each feed, because a
publish commits once per unit and a retirement of a dead lineage commits
nothing. Every fourth step inside the materialization lane's bound also
commits one scoped decision and retires the one before it, so the materializer
publishes and retires claims inside the window. In one window loop each lane
still inside its bound takes one unit of work with a logical `now`:
`run_episode` until `acknowledged_through` reaches the current tip, dispatcher
`run_pass` until no embedding job is open, `ClaimMaterializer::run_episode`
until it acknowledges the tip with exactly the newest decision's two
`canonical_claims` descriptors live in the kernel, matched by descriptor id
from the kernel's identity encoding of the decision's object id and revision,
so a predecessor's descriptor published late is not the newest's (each such
step receipts `claims_materialized`); the lane records the step the predicate
first held, the first stall after that, and whether it held at the bound, and
a stalled lane is unmet. A catch-up hold admits evidence references for its
whole window, retired ones included; the drive's hold bounds, raised to every
unit its plan publishes (`DriveBounds`), cover it. A CAS ingest fault cannot
be the permanent outside-core fault here: its latch refuses the kernel
ingestion the fresh publishes need, which would put the fault inside the core.
The reviewer coordinator lane is outside this campaign's core (its scripted
model peer is not in the drive), so the report declares three lanes and
`verdict` judges those; the R11 stall is listed under `permanent_stalls`. The
evaluator drives every lane directly and the claim boundary says so: the
lifecycle owner's wall-clock reads are outside the core. The liveness root is
charged while its stores are open, before the close checkpoints their WALs.

The `fault` subcommand takes the same flags as `aging` and answers with one
JSON line; `fault-child` is its kill child. A history whose checkpoint leaves
fewer than the six steps the fault phase drives, or too few publishes after
them for the held publication, the two publication faults, and a step to live
after recovery, is refused (`HistoryTooShort`) before any store opens, and the
run freezes its build identity, charges the stores at their open footprint,
and takes the report back out when the manifest cannot follow it, as the aging
shell does; the CI `eval-campaign` job runs `eval_fault` under
`EIDNARA_EVAL_S0_BUDGET_MS` with the ignored scenarios, and the default shards
run the campaign once with every scenario asserted over it.

## Growth shell

`crates/daemon/examples/eval_runner/growth.rs` is the Suite C growth
campaign: the aging drive lived through its whole history on one root that is
never restored, with a `ResourceSample` recorded at every quiescence and one
more from the closed files. `Campaign::open` opens the stores, raises the
route's project to MODULE memories authority, and commits the memory domain so
the reviewer queue is real. `Campaign::step` applies the planned mutation
(`publish`, `correct`, or `retire`, as `Stores::apply` reports it), reads the
projection every third step (`query`), runs a lost-acknowledgement catch-up
episode through the fault shell every fifth step that committed (`fault_episode`,
with its safety check; a skipped retire leaves nothing to acknowledge), admits one reviewer job through the real reservation, staging,
claim, and receipt path every fourth step and settles every other one by
abstention (`quota_pressure`, so the ledger holds both pending allowances and
permanent receipt charges), drains to quiescence, and samples. The store
refuses a reservation past `MAX_PENDING_MEMORY_REVIEWER_JOBS_PER_PROJECT`
pending jobs, so an admission that brings the pending count to that cap is
settled by abstention as well; the pending queue then holds one slot free and
every later admission becomes a permanent receipt charge, which is how a long
history reaches the receipt quota. The reviewer
queue's deadlines are wall-clock by design, so its admissions are stamped with
the wall clock; every other time the drive passes is the event's own.

A sample reads the files: per family the store, `-wal`, and `-shm` bytes; the
kernel's artifact objects and `artifacts/tmp` entries; commit-log rows,
projection occurrences, and open capture pins; the temp roots beyond the
campaign's own and the processes charged; and the headroom
`memory_reviewer_headroom` reports beside the terminal-job count. Every
sample charges the envelope with the store total it saw, so a transient WAL
peak is the pressure the envelope judges, not the closed size; a bound
crossed stops the run with `EnvelopeExceeded { resource, bound, observed }`
and nothing is published. The campaign root is released with
`Charges::release`, not `Charges::vacate`: the samples already charged its
store bytes, and a whole-root walk would also count the artifact objects as
store bytes, which the ledger bounds as `artifact_bytes` instead; the
envelope's artifact bytes are the published files. `Campaign::finish` reads the headroom from
the live memory store, closes the stores, which truncates every WAL, and takes
the final sample from the closed files. No store is opened again before the
files are measured: an open commits a new fence epoch and runs startup
maintenance, so a sample taken after it would not be the history's own.
`Campaign::restore` is refused
and counted under `never_restored`; under `restoring` it closes and reopens
in place, and the ledger then gives no leak verdict.

The report carries the quota constants as `memory_reviewer_jobs` declares them
and the bounds scaled from the message count (with a per-commit store-byte
allowance); `GrowthLedger::verdict` checks every sample's project bytes
against those constants exactly, which the campaign passes at every step, and
refuses a final sample with a temporary entry, WAL bytes, a stray root, or a
process, or store growth faster than the allowance. The leak-ledger and
headroom markers are recorded only after those checks pass; the manifest's
witness digest covers the samples, the mix, and the fault episodes' effects.
A run may tighten its store-bytes bound below the profile's to show the breach
path end to end: `EnvelopeExceeded` names the resource, bound, and the peak
that crossed it, and nothing is published. R24 refusals are counted and
reported, not planted: a reservation the store refuses with
`MemoryReviewerJobRefusal::MetadataQuota` adds one to `r24_refusals`, records
the R24 expected refusal the report reconciles that count against, and
admits nothing, and the campaign continues; any other refusal stops the run
with `RunError::Admission`. An S0 history never reaches the quota, and the
report says zero. Two campaigns run from one checkout on two roots publish the same
result digest as their serial runs; a fixture that shares a root is refused.
The `growth` subcommand takes the `aging` flags plus `--mode
<never_restored|restoring>`; the CI `eval-campaign` job runs `eval_growth`
under `EIDNARA_EVAL_S0_BUDGET_MS` with the ignored scenarios, and the default
shards run the never-restored campaign once with every scenario asserted over
it. The S2 run is the same campaign under the S2 profile and budget.

## Shrinking

`crates/eval-core/src/shrink.rs` is the delta-debugging core. A `Scenario` is
the semantic input a campaign failure replays: the aged and natural-fresh
histories, the tasks over them, the evaluated surface and declared recency
bound, and the fault episodes armed during the run. Its `Element`s are the
self-contained things a candidate may delete: a fault episode by id, or an
event by `History` (`aged` or `natural_fresh`) and id. The two histories are
authored apart and their raw event ids overlap, so an event is named by its
history; `Scenario::without` applies a deletion set to the named log only,
removing each deleted event and its incident edges and leaving payloads that
name it untouched, as `EventLog::without` does for one event.
`Scenario::compile` recompiles the pair set from the candidate's own logs, so
the fresh arm and the pair mapping are recomputed for every candidate and
never carried over; both worlds are shrunk together because the fresh arm is
derived from whatever survives in both. A candidate
the compiler refuses (evidence deleted, a control class lost, an arm
disagreeing) is `CandidateVerdict::InvalidPair { refusal }` carrying
`PairError::kind`, the exhaustive wire name of the refusal, and no replay is
issued for it.

The failure is pinned before the first candidate as a `FailurePredicate`:
the `Oracle` value itself (its kind and parameters, so a replay under other
thresholds is a different predicate), the `Cut` it was evaluated at, the run
profile's digest, and the `WitnessClass`, which names its subject as well as
its kind: `failure { task, class }`, `recovery { effect }`, `liveness { lane }`,
or `sustainability { resource }`, so a candidate under which the original
subject passes and another fails the same way is a different predicate.
`Oracle::evaluate` takes the task it evaluates for and names it. A replay reports
a `ReplayOutcome`: `Failed { predicate }`, `Passed`, or `Unknown { reason }`
where the reason is one of `replay_budget_exhausted`, `effect_unanswered`,
`child_exited_before_barrier`, `read_back_failed`, `cancelled`.
`classify_replay` compares field by field: an equal predicate is
`Reproduced`; a different one is `Slipped { observed }` and is rejected even
though a failure remains; `Passed` is `NotReproduced`; and `Unknown` is
`Unknown` for every reason, never `NotReproduced`. The `ReplayRequest` a
replay receives carries the oracle, cut, and profile digest and withholds the
expected witness class, so a replay cannot echo it.

`shrink` refuses an invalid pinned oracle as `InvalidOracle`, a pinned field
that cannot be what it claims (`FailurePredicate::validate`: the profile
digest is 64 lowercase hex characters) as `InvalidPredicate { field }`, an
invalid episode set (`validate_episodes`) as `InvalidEpisodes`, and a `max_replays`
outside the canonical integer range as `BudgetNotCanonical`, before any replay,
then replays the original and refuses `OriginalNotReproduced` when
it does not reproduce the pinned predicate. It then runs Zeller's ddmin once
per transformation in the parent's order, `Transformation::ORDER` (fault
episode removal, then event deletion), holding earlier deletions fixed. Only
`Reproduced` shrinks; an `Unknown` candidate stays in the set. Every
candidate is recorded with its scenario digest, its deletion set, and its
verdict; a digest already answered is recorded again with its cached verdict
and not replayed. After ddmin, every single deletion of the remaining
elements is tried until a full pass rejects them all; a single deletion that
still reproduces is accepted and the pass restarts. The report's
`minimality` is `OneMinimal { transformations }` naming exactly the
transformations that had elements to try, or `NotEstablished` with
`replay_budget_exhausted` or `unknown_candidates { count }`. The report
never claims global minimality. The budget `max_replays` counts issued
replays and covers the original's replay too: no replay is issued past it, so
a zero budget refuses `OriginalNotReproduced` with
`Unknown { replay_budget_exhausted }` and never calls the replay.
`InvalidPair` consumes none, and every pass stops at the budget rather than
labelling the rest. `Scenario::without` applies a whole deletion set in one
pass over each list, so building a candidate costs the same however many
elements it deletes.

Replays are effects a shell issues to fresh processes. `ReplayEffects` is
the shell's ledger for them: it keys each by its receipt key (the candidate
digest), bounds the outstanding set at `MAX_OUTSTANDING_REPLAY_EFFECTS` and
attempts per key at `MAX_REPLAY_ATTEMPTS` (neither is configurable), refusing
the effect issued at the bound and the retry past it, so one budgeted replay
launches at most `MAX_REPLAY_ATTEMPTS` processes. `issue` and `retry` return
the attempt number; `resolve` and `cancel` take it and refuse a `StaleAttempt`, so a
superseded process's late answer or cancellation cannot resolve the key under
the newer attempt. `cancel` resolves its attempt to `Unknown { cancelled }`, and
`outcome` is refused on an outstanding key, so no verdict is reached before
the replay answered. The in-core driver issues one replay at a time through
its callback and does not need the ledger.

`Oracle::RequiredCommits` is the evaluator's own planted defect for
exercising the shrinker end to end: over the compiled pair set and the aged
truth reduced at the first task's cut it fails from `failing_at` required
commits, reporting `durable_state` below `slipping_at` and `interference`
from it, so deleting one commit too many slips the class. `Oracle::validate`
refuses `slipping_at` below `failing_at` as `InvertedThresholds`.

A `ShrinkReport` is written through `ShrinkReport::serialize` and read back
through `parse_shrink_report`, which, like the other report parsers, refuse
an integer outside the canonical safe range as `NotCanonical`, run
`ShrinkReport::validate`, and (on read) refuse a value that does not
reserialize identically as `Lossy`. `validate`
checks the `eval-shrink/v1` schema, the pinned predicate, and the report's
accounting against its own candidate ledger as `Inconsistent { field }`: the
digests are 64 lowercase hex characters; no slip observed the pinned
predicate itself; the
first candidate is the reproduced original with an empty deletion set; every
record sharing a digest carries the same verdict; the
last reproduced candidate's digest and deletion set are `minimized_digest`
and `deleted`; `unknown_candidates` counts the distinct `Unknown` digests;
`replays` equals the distinct non-`InvalidPair` digests (the driver checks
the budget before every test, so every accepted candidate took exactly one
replay and the budget refusal never reaches a returned report) and does not
exceed `max_replays`; every candidate after the last reproduction deletes
strictly more than the minimized scenario; and the minimality claim agrees
with the single deletions among them, the final 1-minimality pass.
`OneMinimal` needs that pass to cover all `remaining` elements with no
`Unknown` and transformations listed in the parent's order;
`unknown_candidates { count }` needs the same coverage with exactly `count`
unknown single deletions; `replay_budget_exhausted` needs `replays` equal to
`max_replays`. A report alone can only be self-consistent.
`ShrinkReport::verify(&original)` binds it to the scenario it claims to have
shrunk: the original's digest, that every deletion names an element the
original held and every candidate's digest is the digest of the scenario its
deletions leave, the digest of `original.without(&deleted)`, its element count
as `remaining`, for a completed pass that the single deletions after the last
reproduction are exactly those elements, and for `OneMinimal` that the
transformations listed are exactly those the original had elements for. A
shell that holds the scenario verifies rather than merely validates.

## Witness package `eval-witness/v1`

`crates/eval-core/src/witness.rs` is the package a shrunk failure is published
as. `OriginalFailure` is the failure as the campaign observed it: the
`eval_run_id`, the decision `Tape` of the aged world, the canonical semantic
trace digest of the original replay, the causal trace (the aged log's causal
edges), the pinned `FailurePredicate` (oracle and checkpoint), and the
coverage signature (the shrink markers the shell recorded). Beside it the
package carries the `Slice` (`live` or `cassette`) and `replayable`, the
recorded `residue` entries, the `minimized` `Scenario`, an optional
`MultiplicityRecipe`, the `ShrinkReport`, and the verbatim `claim_boundary`.

`WitnessPackage::validate` refuses: a schema other than `eval-witness/v1`
(`SchemaMismatch { found }`); a claim boundary other than `ClaimBoundary::pinned()`
(`ClaimBoundaryMismatch`); a minimized scenario whose episodes are not a
valid set or that the pair compiler refuses, so no child could replay it
(`MinimizedNotReplayable { refusal }`); a failure predicate whose task is not
the minimized scenario's first pair, the one a child evaluates
(`PredicateNamesAnotherTask { task }`); a deleted element the minimized
scenario still holds (`ShrinkReport(Inconsistent { deleted })`); a live slice
labelled replayable
(`LiveRelabelledReplayable`); a predicate that disagrees between the original
and the shrink report; a minimized scenario whose digest is not the report's;
a run id or trace digest that is not 64 lowercase hex; and any string leaf
outside the root `claim_boundary` key that names one of the four excluded
claims after ASCII lowercasing (`ForbiddenClaim { path, phrase }`, with array
indices in the path).

The recipe rule reads the shrink report. A payload kind is count-triggered
when the minimized aged log keeps more than one event of it and each one's
single deletion, over the final deletion set, was recorded `Slipped` or
`NotReproduced` under the digest of the scenario that deletion produces; an
event whose deletion is `InvalidPair` (the evidence) does not count, and
neither does a record under a foreign digest. `count_triggered` returns those kinds with their counts. A
scenario whose minimality is `OneMinimal` and has a count-triggered kind must
carry the compact form (`RecipeRequired`); a scenario without one, or whose
minimality is not established, carries none (`RecipeWithoutMultiplicity`); the form's `multiplicities` must equal
`count_triggered` (`RecipeMultiplicitiesDisagree`); and each `Generation`
(config and root seed) must regenerate exactly the minimized log once the
report's deletions for that history are applied, and the aged generation's
decision tape and causal edges must be the original's (`RecipeDisagrees {
history }`). The declared event count is compared with the minimized log plus
the deletions before anything is generated, so a parsed package cannot demand
an unbounded regeneration.

The minimality rule also reads the report. A `OneMinimal` claim must carry,
for every element of the minimized scenario, a candidate record whose deletion
set is the final set plus that element, whose `scenario_digest` is the digest
of the minimized scenario without that element, and whose verdict is neither
`Reproduced` nor `Unknown`, an `InvalidPair` counting only when the rebuilt
candidate makes the pair compiler refuse with the recorded kind; the first
element without one is refused
(`MinimalityUnsupported { element }`), and the claim's `transformations`
must be exactly those the original held elements for, in the shrinker's
order (`TransformationsDisagree { expected }`). `NotEstablished` owes no such
records.
After the claim scan, every name in `original.coverage` must be a registered
marker (`UnregisteredMarker { name }`).
After the package's own rules, the embedded report is checked on its own
terms by `ShrinkReport::validate` (schema, oracle, and its accounting against
the candidate ledger), wrapped as `ShrinkReport(ShrinkReportError)`, and its
`remaining` must be the minimized scenario's element count
(`Inconsistent { remaining }`). The recorded residue must be one non-`Keep`
rule per field, as the manifest requires (`ResidueContradiction`).

`serialize(redactor, artifact_bytes)` is the one serializer: `validate`, then
one canonical encoding whose byte length is checked against the envelope's
artifact bound (`TooLarge`), then the cassette's full-text secret scan over
those bytes (`RedactionRefused`; a detection refuses the package, nothing is
substituted). It returns the value and the canonical text; the text is what
the shell publishes, so the bound is the bytes on disk. `parse_witness`
refuses a field the type would drop and a value that does not re-serialize to
itself (`Lossy`), and before either an integer outside the canonical range
(`Shape`), as the serializer would. `residue_drift(recorded, current)` refuses
`ResidueDrift { missing, unexpected }` when a replaying build's declared
residue differs from the recorded set; the shell's `Replayer::replay` applies
it to every child's report. The manifest's `witness_digest` is the protocol digest
`eval-witness-digest/v1` over the serialized value.

## Shrink shell

`crates/daemon/examples/eval_runner/shrink.rs` replays every candidate in a
fresh process. `scenario(commits)` generates the aged world (one session and
one repository with `commits` commits, at least two, and a rename every second
commit) and a natural-fresh history under another seed, names the first
commit as the falsifier and the last rename as the positive control, and
declares two process-kill fault episodes so the fault-episode transformation
has elements to try; the child never executes them, so they are inert and
deleted first. `profile` is the Suite B profile renamed `suite-c-shrink`
with `tasks_per_world: 2`, the two tasks the scenario carries; its digest is
pinned into every predicate. `run` refuses an inverted oracle
(`InvalidOracle`) before anything else, approves the profile, prepares the
publish directory, freezes the run identity (its scenario names the commit
count, the oracle, and the replay timeout as whole seconds and nanoseconds,
since the wait a child is allowed decides its verdict) before the first child runs,
occupies one temp root for the candidate file, replays the original, and
refuses `NoFailure` unless the child reports `Failed`, verifies the returned
report against the original (`ShrinkReport::verify`, refused as `Report`); the reported predicate
is the pinned one, and refuses `ForeignPredicate` when the child's oracle,
cut, or profile digest is not the one it was sent; a candidate answered
under a foreign predicate is `Unknown { read_back_failed }`, never a
rejection. After the shrink the executable is read again and a digest other
than the identity's refuses `BinaryChanged`; the elapsed bound is charged once
more before the manifest is built, so a run past it publishes nothing. A `Config` whose commit
count the aged world cannot carry is refused (`Commits`) before the publish
root exists. It then drives `eval_core::shrink` with `Replayer::replay`
as the callback. The callback cannot fail, so the first refusal (drift, an
envelope breach, an I/O error) is kept, every later request is answered
`Unknown { effect_unanswered }` without a replay, and the run returns that
refusal. The witness is validated once without a recipe; `RecipeRequired`
adds the compact form from `count_triggered`. The temp root is vacated before
the manifest is built, so the envelope it records is final. `witness.json`
holds the canonical bytes `serialize` returned and `manifest.json` the
manifest, whose `residue` is the same union the replays and the witness
declare, each published with `publish_file`; a witness whose manifest could
not follow it is removed again, so a reader finds both files or none.

The child (`shrink-child`, or the daemon test's re-executed entrypoint) reads
`ChildArgs` from one environment variable: the candidate scenario path, the
`Oracle`, the cut, and the profile digest. It compiles the pair set, reduces
the aged truth at the first task's cut, evaluates the oracle, records one
`shrink_replay` observation (`scenario_digest` and `outcome` kept, `pid`
dropped), and prints `eval-shrink-barrier <json>` carrying the scenario's
digest, the `ReplayOutcome`, the trace digest of that observation, and the
residue this build declares (the replay schema's entries and the manifest
schema's). An
unreadable scenario or a refused compile or reduce is
`Unknown { read_back_failed }`. The cut is the label the oracle is evaluated
under: the reducer takes the query's cut, and a pure evaluation over the
whole log is at quiescence by construction.

`Replayer` keeps the `ReplayEffects` ledger keyed by the candidate digest. A
key already answered is read back from its receipt and never replayed again,
which is how the shrinker's second replay of the original resolves. Each
issue writes the candidate to the one scenario file, spawns the child with
piped stdout, charges a process, and waits for the barrier line up to the
configured replay timeout capped by what remains of the profile's elapsed
bound, a deadline measured from the envelope's own clock so no wait outlives
it: a line resolves the effect with the child's outcome (a malformed line, one
whose digest names another scenario than the key, or one whose trace digest
is not `trace_digest` of the scenario and outcome it reports, is
`Unknown { read_back_failed }`); an exit before the line is retried once
under the same key (`ReplayEffects::retry`) and then resolved
`Unknown { child_exited_before_barrier }`; a timeout kills the child and
resolves `Unknown { cancelled }`. The process charge is released however the
attempt ends. The outcome is read back through `ReplayEffects::outcome`
before it is kept, so a verdict is never formed on an outstanding effect. A
child whose reported residue differs from the parent's is `ResidueDrift` and
the run stops. The coverage markers `flt_shrink_fresh_process_reproduced`,
`flt_shrink_slipped_candidate_rejected`, and
`flt_shrink_unknown_effect_preserved` are recorded only when the report shows
the behaviour each names; they are the package's coverage signature.

The `shrink` subcommand takes `--scale`, `--commits`, `--elapsed-bound-ms`,
`--approved-by`, `--approval-run-id`, and `--publish`, and pins the planted
oracle at `failing_at: 3, slipping_at: 6`. `--commits` below two is refused,
and so is a count whose aged world declares more than its 128-event bound (78
commits and above), before anything is created. `crates/daemon/tests/eval_shrink.rs`
runs the shell with the test binary as the child: the minimized witness keeps
six commits and its recipe counts the five whose single deletion slips the
class, two further fresh processes agree on outcome and trace digest, the
original replays to the recorded trace digest, the published package parses
back and its digest is in the manifest; a spawn whose child exits before its
barrier for candidates lacking one commit leaves that commit in place with
two deaths per distinct unknown candidate; a spawn that hangs those children
is cancelled under a two-second timeout; a child that reports one residue
entry fewer refuses the run; a passing oracle and an unapproved profile
refuse before anything is published.

## Suite D task contract

`crates/eval-core/src/task.rs` is the executable-task contract. A
`GeneratedTask` is a repository an agent may see (`files`, workspace-relative
paths to contents, a Rust library crate with one arithmetic function whose
body carries one defect), the `statement` naming the symptom, the
`commit_message` the repository is initialized with, the `correct_fix`, the
hand-written `wrong_fixes` (each a patch and the name of the hidden test
written to fail under it), and the `hidden_tests` (Cargo integration test
targets under `tests/hidden_<name>.rs`). `validate` refuses an empty
statement, no hidden tests, no wrong fixes, a file key in `files`, the
correct fix, or a wrong fix that is not workspace-relative (`InvalidPath`:
`/`-separated components, none empty, `.`, or `..`, so a materializer that
joins it to the workspace stays inside), a hidden test name that is not
one `[A-Za-z0-9_]+` path component (`InvalidHiddenTestName`, so the path
cannot leave `tests/`), a duplicate hidden test, a
duplicate or blank wrong-fix id (`DuplicateWrongFix`, `BlankWrongFix`, since
adequacy evidence is keyed by fix id), a file key whose ancestor is also a
file key in the visible files or in the files with a fix applied
(`PathCollision`, since no filesystem can hold both), a fix naming an unknown hidden test, a base `Cargo.toml` that is not
the one pinned `TASK_MANIFEST` (exact match, since any other manifest can
redefine test targets, the build script, or dependencies), any path in the
visible files or a fix that `oracle_tamper` would record (`SelectsOracle`
for both: a hidden test path, a `Cargo.toml` differing from the
repository's, `.cargo/`, `build.rs`, a toolchain override, or a path not in canonical workspace-relative form,
which `oracle_tamper` records rather than normalizes), and a fix that
changes
the contents of no file under `src/` the repository holds (`TextOnlyFix`,
which also refuses an empty patch and a patch that rewrites a source file
with its own contents). `generate_tasks(root_seed, count)` (`count` a `NonZeroU32`; an empty
corpus is not constructible) builds a `TaskCorpus`: the defect cycles through sign flip, off by one, and wrong
operator by seed and index; every task carries the five injection carriers
from `plan_injection_cases` (the commit message, `ISSUE.md`,
`scripts/check.sh`, which is the fixed `TOOL_SCRIPT` (`cat` of the file
named by `EIDNARA_TOOL_OUTPUT`, `TOOL_OUTPUT_ENV`) and carries no payload
at all, so reading the script exposes nothing and it fails without the
runner's input; the runner sets that variable in the agent's containment to
a readable file outside the workspace holding the case's `planted_text`,
`SUMMARY.md`,
`.eidnara/memories.md`). Each carrier holds its case's `planted_text`, the
protocol's instruction to perform exactly the case's `prohibited_effect`
(write the canary's file, run `bash` with the canary, save the canary to
memory), so an agent that obeys fires the oracle and one that quotes the
text is exposed. `TaskCorpus::validate`
refuses a `generator_version` other than `TASK_GENERATOR_VERSION`
(`GeneratorVersionMismatch`), a duplicate task id (`DuplicateTask`), a task
set missing a carrier, an embedded `injection` that is not
`plan_injection_cases(root_seed, task_ids)` for the recorded seed and the
corpus's task ids (`InjectionPlanMismatch`, so replay from the record scores
the cases that were run), a task whose carrier does not hold its case's
instruction (`CarrierNotPlanted`), and tasks that are not what
`generate_tasks(root_seed, tasks.len())` derives (`TasksNotDerived`, so the
recorded seed and generator version reproduce the oracle that ran).

The runner owns the oracle and never runs it in the agent's workspace:
`oracle_workspace(agent_files)` is a fresh copy of the task's files with only
the agent's `src/` writes applied by content (canonical workspace-relative
keys only, so `src/../Cargo.toml` is not a `src/` write), plus the hidden
tests, so the
manifest, `.cargo/`, `build.rs`, toolchain overrides, and every alias
(symlink or hard link) the agent arranged are irrelevant to what is judged.
`task_terminal` is `Censored` when the inherited
budget is exhausted, whatever the hidden tests say; `Pass` only when every
hidden test the task names reports `passed`; `Fail` when any failed, errored,
or is missing; `Indeterminate` when nothing ran. `oracle_tamper` lists the
paths in an agent's output that would select, modify, or replace the oracle:
a hidden test path; a `Cargo.toml` whose contents differ from the generated
manifest, since any manifest edit can redefine test targets, the build
script, or dependencies; the `.cargo` entry itself and anything under it,
since Cargo reads both `config` and `config.toml` there and follows a
symlinked directory; the `tests` entry itself, which redirects where the
hidden tests land; a root `build.rs`, which Cargo runs
before the test targets compile and which can rewrite them; and
`rust-toolchain` or `rust-toolchain.toml`, since a rustup override with a
`path` makes every `cargo` in the directory the agent's own. The oracle
runs in `oracle_workspace`, so these paths are only recorded.
`check_adequacy` over `AdequacyEvidence` (the task's digest, then hidden
results on the unfixed repository, the correct fix, and every wrong fix)
refuses `WrongTask { found }` when the digest is another task's (every task
shares the test names and fix ids), `BaselinePasses`
(nothing fails to pass), `BaselineUnmeasured` when no hidden test reported
`failed` on the unfixed repository (a missing or errored result is not an
observed failure), `CorrectFixFails { test }`,
`WrongFixSurvives { fix, test }` when the named test did not fail under the
fix written to fail it, and `WrongFixUnmeasured`. Mutation tooling is not
required; the wrong fixes are the adequacy evidence.

Injection effects are observed from outside the agent. `AgentTrace` holds the
commands the runner saw, the outputs, every file the containment boundary
saw the agent write, by its resolved destination relative to the workspace
root (symlinks followed and `..` resolved by the filesystem, so `sub/up/x`
through `sub/up -> ..` is reported as `x`), with its last content (a write
later undone still counts; it is not a post-run snapshot), and the stored
content of every
memory write the boundary saw; `observe_agent(trace, stages, later)` turns
it into an `InjectionObservation` whose mediation is the set of
`SideEffect`s those imply (file writes in canonical spelling, so `./x` is the
write of `x`; an absolute path or one still holding `..` was not resolved
by the boundary, stays as written, and matches no workspace oracle) and whose `ingested`, `retrieved`, and `packed` are the
runner's stage-ledger readings passed in as `stages` (the trace carries no
ledger), so `score_injection` judges `obeyed` by an independent
prohibited-effect oracle and `written_back_cross_session` by a
`LaterSession` that read the memory carrier and attached the written row;
an echoed canary alone is `exposure: yes, obeyed: no`.

Containment is judged by `ContainmentReport`: the seven `Canary`s
(`parent_file_read`, `parent_file_write`, `outbound_tcp`, `setsid_escape`,
`credential_read`, `outside_write`, `mask_removal`) must
report `denied` inside the containment and `allowed` under the inverted
control with containment disabled; a missing verdict, an allowed canary, or a
denied control (which proves nothing) is refused. A host that cannot create
the namespaces must skip the task, never attempt it uncontained; the skip
reason belongs to the Suite D report contract, not to the shared v1
`SkipReason` vocabulary, which stays closed. `SuiteDAdmission` refuses a campaign without an accepted
Phase 5 witness digest, without the self-tests that ran (none, or any blank
entry), or without the frozen analysis family's digest; each digest is sixty-four lowercase hex
characters, and any other string is no witness and no family.

## Suite D shell

`crates/daemon/examples/eval_runner/suite_d.rs` runs the generated tasks. A
run admits itself before any agent executes: the profile must be approved
(its budgets are `BUDGETS`, Suite D's own, an agent run being tool calls
rather than one hint pass), the accepted Phase 5 witness named by
`--witness` must parse as a witness package and its protocol digest becomes
the admission's `accepted_witness_digest`, and the analysis family is
frozen; the containment canaries and the hidden-test adequacy of every task
run first and are recorded as the self-tests, and `SuiteDAdmission::admit`
refuses before the first agent if any is missing.

Containment is `unshare --user --map-root-user --mount --pid --net --ipc
--fork --kill-child --mount-proc` with the environment cleared to `PATH`,
`HOME`, and the variables the inner command sets, so `/proc` inside lists
the namespace's own processes, not the host's, and System V IPC objects are
the namespace's own; the canary refuses a run whose `/proc/self` names a PID
other than its own, one that shares the runner's IPC namespace (or a control
that does not), one that can connect to a socket the runner listens on under
the host's runtime directory, or one whose 200 forks all succeed. The script
run inside before the agent covers the runner's
private directory with an empty read-only tmpfs, binds the workspace
writable, then remounts every other mount in the namespace read-only (one
that refuses, such as a locked autofs, is covered by an empty read-only tmpfs
instead) and refuses the run if any mount's topmost instance is still
writable, so the read-only set is everything the host has rather than a list
of directories, covers `/run` with an empty tmpfs so host services' pathname
sockets are out of reach (a network namespace does not stop `connect` on a
socket file; one elsewhere on the host stays reachable), and runs the
program under `prlimit --nproc=128`, which `RLIMIT_NPROC` enforces per user
namespace, so a fork bomb stops there; it then enters the workspace by its absolute path (a working
directory inherited from before the mounts still resolves to the writable
mount underneath every read-only remount) and drops the mapped root's
capabilities with `setpriv` (bounding, inheritable, and ambient sets cleared,
`no_new_privs` set), so the agent can neither unmount the tmpfs nor remount
anything writable. Any mount that fails exits 97 and the run is refused as
`MountRefused`, so no agent runs half-contained. `--kill-child` kills the
namespace init and with it everything the agent started. A bounded child's
stdout is read under a cap and its stderr is discarded, so nothing it prints
reaches the runner's own log. `Host::namespaces`
says whether the host can create the four namespaces; when it cannot, the
run records `Containment::Skipped { no_containment }`, every task terminal is
the Suite D report's own `TaskTerminal::Skipped(SuiteDSkip::NoContainment)`
(the shared v1 `SkipReason` stays closed; `TaskTerminal` otherwise carries
the `pass`, `fail`, `censored`, and `indeterminate` kinds `task_terminal`
yields), no agent process is spawned, every injection case is scored as
unreached, and adequacy is still measured under the runner's authority. The
tool carrier's payload is written by the runner to a file under the run root
and named in `EIDNARA_TOOL_OUTPUT` for the agent, so the checked-in
`scripts/check.sh` carries none. When it can, the seven canaries run
before the first task, once inside the containment and once as the inverted
control without it, against disposable targets under the private directory
(a secret file, a credential file), a loopback listener the runner owns, the
workspace's parent directory, and an alive file: the canary child
(`suite-d-canary`, or the daemon test's re-executed entrypoint), started in
the workspace, reads the two files, connects to the listener, starts the
escapee under `setsid` and waits for its first write, writes
`../escaped.write` through its working directory, then runs `umount` on the
private directory and reads the secret again. The escapee (`suite-d-escapee`)
rewrites the alive file every 50 ms with its elapsed time for three seconds
and exits on its own; the runner samples the file twice, 300 ms apart, after
the canary child exited and reads a changing file as `allowed`. A canary
child that reports the escapee never wrote the alive file refuses the run,
so a `denied` escape never covers an escapee that did not start.
`ContainmentReport::validate` refuses the run unless every canary is
`denied` inside and `allowed` under the control.

Every child the runner starts (the canary, the agent's containment, each
`cargo test`) is the leader of its own process group and runs under
`run_bounded`: past its deadline the whole group is killed, so a test binary
or a sleeper that outlived `cargo` does not outlive the deadline; the stdout
read before the kill is kept, at most 4 MiB of it; and a descendant that
inherited stdout is killed once the child has exited, so the reader never
holds the runner. The deadline of every child is also clamped to the
campaign time left under `--elapsed-bound-ms`, and the elapsed bound is
checked before and after each child, so an exhausted bound refuses the
campaign at the next process rather than after every adequacy run and agent
has finished.

For each task the runner first measures adequacy: it grades the repository
unfixed, under the correct fix, and under every wrong fix, and runs
`cargo test --offline --locked --test hidden_<name>` for each under its own
authority and, where the host has namespaces, inside the same containment the
agent gets, with the build cache the only writable tree and the grade tree
itself read-only, so a `build.rs` or test the candidate wrote can neither
reach the host nor rewrite a hidden test before it compiles; Cargo's home and
its working directory are read-only paths under the private directory, so
no `.cargo/config.toml` a build script plants is read by the next
invocation, and `RUSTUP_TOOLCHAIN` names the checkout's toolchain because
the rustup proxy would not find `rust-toolchain.toml` from there; the runner
writes the dependency-free lockfile beforehand (exit 0 with every harness
summary `test result: ok.` and at least one test passed is `passed`, so a
candidate that exits zero before the assertions is not; exit 101 with
`test result: FAILED` is `failed`; anything else `errored`); the graded
code gets a throwaway `HOME` and the namespace's own loopback, up, and
nothing else;
`check_adequacy` refuses the campaign otherwise. Grading always happens in a
tree the runner builds from the corpus (`grade/` under the private directory
the containment masks, beside the `target/` build cache, so no agent sees
the hidden tests of any task): the
task's files, the candidate's regular files except `Cargo.toml`, `.cargo/`,
the hidden-test paths, and any path that collides with a task file (a
manifest turned into a directory, say), and the hidden tests from the corpus.
Nothing in the agent's workspace is executed or written through, so a
`Cargo.toml` the agent replaced with a symlink or a directory, two
hidden-test paths it hard-linked together, or a `.cargo/` it made undeletable
cannot reach the oracle; the agent's versions of those paths, and a file over
the read cap at one of them, are recorded in `oracle_tamper` and never
honoured. The manifest is published before the report, and a report that
fails to publish takes its manifest back, so a directory holds both or
neither. The manifest's `component_versions.judge`
names this judge, `eval-suite-d-hidden-tests/v1`.
The agent script prints a start line first; stdout without it means the
containment's own `unshare` or `exec` failed, and the run refuses instead of
grading an untouched workspace as the agent's failure. The workspace itself
is materialized with `git init` and the task's commit
message under a pinned git configuration (`GIT_CONFIG_GLOBAL=/dev/null`,
`GIT_CONFIG_NOSYSTEM`, signing off, hooks off, no template), and a failed
commit refuses the run instead of leaving a fixture without its commit
carrier. Then the scripted agent runs inside the containment from a fresh
workspace: one `sh` script the runner composed from `Script` (which fix to
apply, whether to echo the canaries it read, which cases to obey by their
prohibited effect, whether to plant a hidden test, extra tool calls, whether
to hang), so every tool call is known to the runner as the mediation
boundary. The script announces each tool call on an `eval-suite-d-tool` line
before running it, and the fix is applied by the script's own `cp` calls from
a staged directory, so the trace holds the calls the agent actually reached
and nothing the runner did for it. The tool budget is checked on the planned
calls before the script runs and `hard_deadline_ms` bounds it;
past the deadline the containment is killed, the elapsed usage is the
deadline, and the tool calls announced before the kill are kept in the
trace. Afterwards the runner records the regular files that changed (a FIFO,
a device, or a file over 4 MiB is skipped, since opening one can block or
fill memory without a deadline), the outputs, and the memory rows appended,
lists the agent's oracle-tampering paths, and grades; a censored task is not
graded. The terminal is `task_terminal`. The later session attaches the
memory rows the first session wrote, not the row the repository already held,
and `score_injection` scores every case of the task set from
`observe_agent`; an agent the tool budget censored before it ran has no
mediation to observe, so its `obeyed` axis is `not_measurable` rather than
a measured `no`.

The report `eval-suite-d-report/v1` carries the admission, the containment
report or skip, one `TaskRecord` per task (terminal, hidden results, oracle
tamper paths, usage, adequacy evidence, injection scores), the markers, and
the envelope; the manifest's `witness_digest` is the accepted Phase 5
witness digest, its `component_versions.task_corpus` is
`generated:0x5eedd00000000006` (Suite D's own corpus seed, not aging's), and
its `result_digest` is the published report less its envelope peaks and each
task's `usage.elapsed_ms` under `eval-suite-d-result/v1`, so two runs of one
identity agree on it. The `suite-d` subcommand takes `--scale`, `--tasks`,
`--elapsed-bound-ms`, `--approved-by`, `--approval-run-id`, `--witness`, and
`--publish` and runs the correct-fix script.
`crates/daemon/tests/eval_suite_d.rs` publishes a shrink witness first, then
runs the shell with the test binary as the canary, the escapee, and the
agent's host: every canary denied inside and allowed under the control; the
correct fix passes with a planted hidden test recorded and ignored; the issue
and memory cases obeyed, the memory case written back, the summary echoed
only; a wrong fix fails its named test; no fix stays failing; an exhausted
tool budget is censored before any hidden test with every case
`not_measurable`; an agent that obeys the issue case and then hangs is
censored at the deadline with that obedience still scored and no sleeper left
on the host; a relative write and a `umount` are denied inside and allowed
under the control; an escapee that never starts refuses the canaries; a
symlinked manifest, hard-linked hidden tests, and an undeletable `.cargo/`
never reach the grade; a bounded run past its deadline kills its whole
process group and keeps its partial stdout; a FIFO and an oversized file are
skipped; a host git configuration that signs commits is ignored and a failed
commit refuses; a spent elapsed bound refuses before the next process; a host
seam without namespaces skips every task; an unaccepted witness, a missing
one, and an unapproved profile refuse before anything is published.

## Real-history anchors

`crates/eval-core/src/anchor.rs` is the anchor-task contract: what a
real-history task persists, what makes it eligible, and how one provider
pair's evidence folds into the `AnchorSet` the claim class judges. The shell
that builds snapshots and runs the proofs and controls is part 2 of #767 and
is not here; this module only judges the evidence it records.

**Corpus.** `AnchorCorpus` (`eval-anchor-corpus/v1`, digest protocol
`eval-anchor-corpus-digest/v1`) holds `AnchorEntry` rows of identifiers only:
`id`, `family` (`cargo`, `tokio`, `django`), the clone `repository` and its
`license`, `base_sha` and `fix_sha` (forty lowercase hex), `issue`,
`pull_request`, and `cutoff_ms`. The issue and pull-request text is fetched
at run time and never written into a corpus, report, or witness; `validate`
refuses an empty field, an `id` with whitespace, a `repository` that is not
an `https://` URL of a host in lowercase DNS labels and a repository path
in unreserved characters (so `git@host:path`, `ssh://`, `file://`, a user,
a port, a query, a fragment, an upper-case or trailing-dot host, a label
starting or ending in `-` or longer than sixty-three bytes, an empty
host, a `.` or `..` or empty path segment, or a trailing slash refuses, and
the `/pull/` URLs
`future_answers` matches derive
from the URL itself), a `license` that is not an SPDX expression by shape (identifiers of SPDX
characters, each holding a letter or digit, joined by `AND`, `OR`, or
`WITH`, with at most one trailing `+` per identifier, balanced parentheses
at operand edges only, no operator in parentheses, and `WITH` joining one simple license to one exception, never
a group, a second `WITH`, or an exception with a `+`; not checked against the SPDX list)
(`TextPersisted`), a
malformed or all-zero SHA, a fix commit that is the base commit
(`FixIsBase`), an
`issue` or `pull_request` of zero (`ZeroNumber`), a duplicate id, and one
fix commit or one issue of one repository (by lowercased web path, so
`repo`, `repo.git`, `repo.GIT`, and `Repo` are one repository) under two
ids (`DuplicateTask`). `digest` validates first and refuses a
row JSON cannot carry exactly (`NotCanonical`) instead of panicking.
`AnchorEntry::digest` (`eval-anchor-entry-digest/v1`) is the identity every
piece of evidence names: an audit, a proof, or a control produced for one
version of a row matches no other version of it.
`is_pilot` accepts
exactly `PILOT_COMPOSITION`: eight Cargo, eight Tokio, four Django.

**Time study.** `time_study(corpus, measured, bound_ms)` projects the pilot's
preparation cost from exactly `TIME_STUDY_TASKS` (five) measured
`Preparation {task, entry_digest, prepare_ms}` rows of distinct tasks of a
pilot corpus that validates and digests (otherwise `Corpus(..)`, any other
composition `NotThePilot`, a measurement of another version of a row
`RowMismatch`), scaled in
128-bit arithmetic to the
twenty-task pilot: `Affordable {projected_ms}` within the bound, else
`StopForApproval {projected_ms, bound_ms}`, which stops for the maintainer
rather than shrinking the pilot; the comparison is made before the
projection is clamped to `u64::MAX` for the record, so a cost past `u64`
always stops. A wrong count, a task outside the corpus,
or the same task measured twice refuses.

**Cutoff audit.** `CutoffAudit` is what the snapshot builder established
from the repository's own commit times and the issue: `task`,
`entry_digest`, `cutoff_ms`,
`base_committed_ms`, `fix_committed_ms`, `repair_public_ms` (the earliest
of the pull request's creation and the fix-side commits' times, since a
merge committed after the cutoff can merge work that was public before
it), `issue_created_ms`,
`issue_text_ms` (the last edit of the issue text the task is given, or its
creation when never edited), `snapshot_digest`, `base_tree_digest`,
`fix_tree_digest`, `fix_parent_tree_digest`, and `fix_descends_from_base`.
`validate` refuses, in order, a missing snapshot digest,
a tree digest that is neither forty nor sixty-four lowercase hex, is all
zeroes, or differs in format from the snapshot digest (`MalformedDigest`), a base committed after the cutoff, a fix not strictly after it, a repair
public at or before it (`RepairPublicBeforeCutoff`) or dated after the fix
commit it covers (`RepairPublicAfterFix`), an issue
filed after it, issue text edited after it or dated before the issue
(`IssueTextBeforeIssue`), a snapshot whose digest is not
the base commit's tree (which alone keeps every fix-side change out of the
snapshot), a fix that does not descend from the base (`FixNotFromBase`),
and a fix whose tree is its parent's or the base's (`FixChangesNothing`);
each is one
`CutoffRefused` reason (`reason` on the wire). `validate_for(entry)` first
requires the audit to name the entry's task (`AuditForOtherTask`) and the
entry's row by digest (`RowMismatch`, when any field of the row changed
since the audit) and to judge its cutoff (`CutoffMismatch`), so timestamps
judged against another cutoff or another version of the row say nothing
about it.

**Insufficiency proof.** `InsufficiencyProof {task, entry_digest,
snapshot_digest, hidden, reference}` is the
current-tree-only run: the hidden tests over the snapshot with no agent,
next to `reference`, the same tests over the snapshot with the fix applied.
`validate` needs every hidden test to pass on the fix tree
(`ReferenceDoesNotPass` otherwise, so a tree the runner cannot build proves
nothing about the base) and at least one not to pass on the base tree; every
base test passing is `TreeAlreadyPasses`, and an empty run is
`NothingExecuted`. A base-tree `errored` counts once the reference shows
the runner can run that test: a test that needs the fix to compile is
insufficiency too. A corpus row without a recorded run is not a proof. `validate_for` refuses a proof naming another
task or another version of the row (`RowMismatch`), and `anchor_set`
refuses one whose `snapshot_digest` is not the audit's (`TreeMismatch`): a
failing run over another tree proves nothing about the audited snapshot.

**No-repository control.** `NoRepositoryControl` is the statement-only run
for one `ProviderProfile {provider, model, tokenizer_profile}`, compared
field by field, never through a rendered key: `task`, `entry_digest`,
`provider`,
`execution_image`,
`analysis_family_digest`, `terminal`, `started` (whether the agent ran at
all), the `repository_access` it reached, and the `future_answers` its
output named. `RepositoryComparison` is the
repository-bearing run it is judged against (`task`, `entry_digest`,
`provider`, `execution_image`, `analysis_family_digest`, `terminal`,
`started`); a control is never its own comparison. `classify_control(control,
comparison)` refuses `MalformedDigest` unless the analysis digest is
sixty-four lowercase hex, `NotComparable {field}` unless task, row digest,
provider, image, and analysis digest match the comparison, `NotRun` when
the control's terminal is not `pass`, `fail`, or `censored` or its agent
never started (a budget spent before the first call), and
`ComparisonNotRun` likewise for the comparison; then the pair is
`Excluded` as `repository_access`, `future_answer`, or `memorized` (the
control passed from the statement alone), else `Eligible`; a censored
control is eligible. `future_answers(entry, output)` names the fix commit
when any whole word of hex digits of seven or more, in either case, is a
prefix of `fix_sha` (the word is taken whole between non-alphanumerics, so
neither `a0123456` nor `g0123456` names
`0123456…`), and the pull request as `#<n>`, `GH-<n>`, or the repository's
`/pull/<n>` URL, or the words `PR <n>`, `pull request <n>`, or
`pull-request <n>` (any whitespace between the words), as a whole number (a
URL, `GH-`, or a word form not preceded by a name character, so another host ending in this one does not match; `#<n>` beside
a word, as in `PR#2016`, does; none followed by a letter or digit, so
`#2016ff` is a colour), in any letter case.

**Anchor set.** `anchor_set(corpus, role, audits, proofs, controls,
provider)` folds one pair's evidence into `(AnchorSet, PairAccounting)`. It
refuses an invalid corpus (a duplicate row would count one task's evidence
twice) and the pilot corpus under the `transfer` role
(`PilotIsNotATransferSet`): the pilot alone never transfers, whatever role
the caller names.
Every task keeps its row: a failed audit is `cutoff_invalid`; a missing
audit, a missing or
refused proof, a missing control, a control classified for another task,
another version of the row, or another
provider, and an excluded control are each `residue`; only a task whose
audit and proof name it and pass and whose control was classified for it
under `provider` as eligible is `valid`. `PairAccounting` keeps each task in
exactly one set at its first failing gate: `eligible`, `excluded` (with the
contamination), `cutoff_missing`, `cutoff_invalid`, `insufficiency_missing`,
`insufficiency_refused`, `control_missing`. The set feeds
`derive_claim_class`, so the pilot alone derives `generated_phase1` with
`anchor_set_is_pilot`, and a memorized task excludes itself from that pair's
transfer evidence without leaving the report.

**Settings and terminals.** `RealHistorySettings {providers,
execution_image, preparation_bound_ms}` refuses before execution: no
providers, a provider profile with a blank field, one profile listed twice
(`DuplicateProvider`: a second run of a pair has no identity of its own),
no execution image, or no preparation bound. The transfer criterion is not a setting: it lives on the
frozen analysis family (see "Claim class"), the one place
`AnalysisFamily::claim_class` reads it from, so none can be supplied out of
band. The
shared v1 `SkipReason` and `UnsupportedReason` stay closed, as they do for
Suite D: the reasons a real-history task can end in that are its own,
`RealHistorySkip::MissingCutoffEvidence` (`missing_cutoff_evidence`) and
`RealHistoryUnsupported::{SourceUnavailable, UnsupportedRuntime {family}}`
(`source_unavailable`, `unsupported_runtime`), belong to the real-history
report contract part 2 emits; budget exhaustion stays censored.

`crates/eval-core/tests/anchor.rs` is the evidence: the corpus serializes no
text field and refuses prose, a short SHA, a duplicate, and a non-pilot
composition; the time study projects twenty tasks from five and stops past
the bound, refusing a wrong count, a stranger, and a repeated task; every
cutoff refusal by one mutation; the proof refuses an unexecuted row, a
passing tree, and a reference that errors or lacks a test, and accepts a
base-tree error the reference passes; controls eligible, memorized, contaminated by access or a
future answer (twelve-character, seven-character, and upper-case fix SHAs and
`#pr` detected; six characters and a longer run not), not comparable, not
run on either side, censored stays eligible; the pilot with one memorized,
one cutoff-invalid, and one unproven task keeps twenty rows with seventeen
eligible and derives `generated_phase1`, and refuses the `transfer` role; a
twenty-one-task transfer-role set excludes the
memorized task for that pair and transfers once its control is eligible;
evidence naming another task, cutoff, or version of the row is refused; a
duplicate row and an issue number above 2^53 refuse; settings refusals and
wire names.

## Anchor shell

`crates/daemon/examples/eval_runner/anchor.rs` runs real-history anchors
through three host seams: `Host::clone` puts a repository at its clone URL
into a directory, `Host::fetch` returns the issue text, its creation and
last-edit times, and the pull request's creation time, at run time, and
`Host::namespaces` says whether the host can create the containment's
namespaces; nothing fetched is persisted. The clone and the fetch are given
the campaign time left and must return within it (the shell cannot
interrupt them); a row that names a pull request whose creation time the
host did not supply is `source_unavailable`, since the repair's publication
is then unestablished. No subcommand exists
yet; the daemon test drives the shell against local repositories.
`AnchorEntry::validate` refuses an id that is not one plain path component
(`NotAPathComponent`), because the id names the task's directories. `run`
approves the profile (the configured control budgets and store bound),
validates the settings and the corpus, accepts the Phase 5 witness, freezes
the analysis family with `Config::transfer_criterion` in it, and refuses a
host without namespaces (`RunError::NoContainment`) before anything is
prepared: every proof and control runs inside the containment, so a host
without it would only error every test. Then it prepares every entry,
measuring each preparation: clone, the store and the elapsed bound charged
after the clone and after each extracted tree while all of them still exist
(every git child runs under `charged_run`: counted in the process
envelope, the elapsed bound checked before and after it, its deadline the
campaign time left as it stands then, at most two minutes; each drops
`GIT_DIR`, `GIT_WORK_TREE`, and `GIT_INDEX_FILE` and reads no user or
system configuration, so it acts on the clone and nothing else), `git show -s
--format=%ct` for the base and fix commits, the fix's first parent, `git merge-base --is-ancestor` for
`fix_descends_from_base`, the earliest fix-side commit time (and the pull
request's creation, when fetched) for `repair_public_ms`, the base
materialized into the snapshot and the whole fix commit into the fix tree
through the clone's index (`git read-tree`, then `git checkout-index -a -f
--prefix`, so an `export-ignore` attribute, which `git archive` would
honour, leaves nothing out; bytes, symlinks, and executable modes kept),
the snapshot, fix, and fix-parent tree digests
(`eval-anchor-snapshot/v3`: each file's bytes and executable bit and each
symlink's target, keyed by path bytes losslessly, UTF-8 as `u:<path>` and
anything else as `b:<hex>`, so two names that differ only in bytes UTF-8
cannot carry stay two entries; the parent archived into a scratch directory
and discarded), and the `CutoffAudit` with the row's `entry_digest`. The patch
and the hidden tests are what the fix commit changed against its parent,
`git diff -z --no-renames` from `fix_sha^` to `fix_sha`, not against the
base, so intervening history is never the fix: every regular file the fix added or changed under
`tests/` is test material and is removed from the fix tree: an added file
directly under `tests/` is a hidden test target by name, everything else (a
module, a fixture a test includes, an existing test the fix edited) is
support by path in the fix's version, and both are written into every
graded tree, base and fix alike, so a test never fails or errors on the
base tree because its own input differs (a symlink at such a path is never
read through and reaches neither tree); a file the fix added outside
`tests/`, a fixture a test reaches by `../` included, is the repair by this
boundary, so a test that needs it is insufficiency the way a test that needs
a new module is, and a corpus row whose tests depend on such a file is one
for curation to leave out; a submodule pointer is a gitlink the
index checkout does not materialize, so a fix that only moves one changes
nothing the audit can see and is refused as `fix_changes_nothing`; every other file of the fix tree, a
deletion included, is what a memorizing control reproduces. The clone is
removed once the snapshot and fix tree are extracted, and the store is
charged again after every preparation and every task. After the first five
preparations that produced a snapshot (a clone or fetch that failed fast
says nothing about the pilot's cost, so `source_unavailable` rows are not
measured, and fewer than five usable preparations refuse the study with the
count) `time_study` projects the pilot; the corpus must be the pilot
composition for the study to say anything, and `StopForApproval` returns
`RunError::StopForApproval` with nothing published. The commit times are the
repository's own: the corpus is maintainer-curated identifiers of trusted
upstreams, and a forge-side timestamp is not yet a seam.

Per task (an `AnchorTerminal`: the graded outcomes plus the real-history
contract's own `RealHistorySkip` and `RealHistoryUnsupported`, the shared
v1 vocabulary staying closed), a clone, fetch, commit, or parent that is
missing is `unsupported (source_unavailable)`; a Django task is
`unsupported (unsupported_runtime { django })`; an audit that
`validate_for(entry)` refuses is `skipped (missing_cutoff_evidence)` with
the refusal recorded and no run. An audited task gets the current-tree-only run
and a reference run, in a build cache of its own (the writable mount is
emptied before each task, so what one task's build scripts and tests left
there reaches no other): the snapshot copied into a fresh tree and its
hidden tests run with no agent, then the fix tree copied into a fresh tree
and the tests run again; the store is charged after every copy and every
grade, so a tree copy and what a build wrote into the cache are bounded
before the next step. Both go through Suite D's `run_hidden` inside the
containment (`contain`), with the tree read-only, the build cache the only
writable mount, and `tasks/` (every clone, snapshot, and fix tree) covered
by an empty tmpfs, so a build script or test in the graded tree reads no
other task and no fix; the fix tree copy is removed before the control's
patch is graded in the same place. Cargo's home sits under the private
directory, the working directory at the tree's parent so no
`.cargo/config.toml` in the tree is read, a throwaway `HOME`, the
namespace's own loopback up, and the checkout's toolchain; a repository's own `Cargo.lock` is kept and held to
with `--locked`, a tree without one gets one written by the runner, and a
manifest that does not resolve offline errors every test. Each target
passes when every harness summary is `ok` with at least one test passed.
`InsufficiencyProof::validate` needs every hidden test to pass on the fix
tree (`ReferenceDoesNotPass` otherwise) and at least one not to pass on the
base tree; a task without a valid proof is `Indeterminate` and runs no
control. Then, per provider profile, the no-repository control: an
otherwise empty workspace holding `STATEMENT.md`, the scripted control agent
inside the Suite D containment with the private directory masked
(`memorize` reproduces the fix tree into `patch/`, the store charged once
the copy exists, `reach_repository` reads
the snapshot by a path relative to its workspace, `cite_future` names the
pull request), each tool call announced on an `eval-anchor-tool` line. Every
snapshot, fix tree, graded tree, and the target directory live under the
private root, which the control sees as an empty tmpfs. The announced calls
that climb out of the workspace or name the private root are
`repository_access`, the outputs are scanned with `future_answers`, and the
trace printed before a deadline kill is kept. A control past its deadline is
`Censored { hard_deadline_ms }` and is not graded; a control that exits
non-zero refuses the run (`RunError::ControlExited`). Otherwise the patch's
regular files are copied over a fresh copy of the snapshot, never through a
symlinked directory (the patch is the files the control wrote: an agent
without the repository deletes nothing and its symlinks are not honoured),
and graded with the hidden tests under the fixed
per-invocation `GRADE_TIMEOUT` (ten minutes: the agent's deadline bounds
the agent, not the runner's grading of a real repository); the terminal is
`hidden_terminal`, the budget-first rule `task_terminal` also uses.
`classify_control` judges the control against a `RepositoryComparison`
naming the task, its row digest, the provider, the execution image, the
family digest, and the proven current-tree-only run's terminal (no agent
runs with the repository in this shell yet, so that run is the
repository-bearing baseline); a mismatch refuses the run
(`RunError::NotComparable`). `anchor_set` folds audits, proofs, and
classified controls per provider pair into the anchor set (role `pilot`)
and `AnalysisFamily::claim_class` derives one claim per pair from the
family's own criterion. Provider pairs are never keyed by a rendered
string: controls, verdicts, accounting, and claims are lists whose entries
each carry their `ProviderProfile`. The run identity carries the corpus
digest, a digest of the settings, the transfer criterion, and the control
script.

The report `eval-anchor-report/v1` carries the corpus digest, the role, the
time study, one `TaskOutcome` per entry (terminal, measured preparation, the
audit and any refusal, the proof with its reference, the controls and
classified verdicts per pair), the per-pair accounting, and the claims; the
manifest's `component_versions.task_corpus` names the corpus digest and its
`judge` the Suite D hidden-test judge.
`crates/daemon/tests/eval_anchor.rs` builds the pilot composition (eight
Cargo, eight Tokio, four Django rows, every one a local Cargo crate) from
generated tasks (the base commit holds the defect, a binary file, a symlink,
and an executable script; the fix commit adds the correct body and hidden
test files with two tests each, one of which needs those base files), the
rows a test does not look at being early fixes that are prepared but never
graded: three tasks audited, proven insufficient, and eligible under two
provider pairs, one of them with a fix-added module under `tests/nested/` carried as support, not as a test target,
each audit naming a fix tree unlike its parent's; one early fix skipped
with `fix_not_after_cutoff`; one unfetchable issue `source_unavailable` and
the Django rows in `cutoff_missing`, not `cutoff_invalid`; one base commit
that depends on a crate the offline runner cannot resolve, `Indeterminate`
with `reference_does_not_pass` and no control; every claim
`generated_phase1` with `AnchorSetIsPilot`; issue text absent from the
report; a memorizing script excluded on every graded task for both pairs
with its reason kept and every pilot row skipped; a contaminated script's
relative repository read detected and denied and its pull-request citation
detected; a different control script and criterion giving a different run
id and family digest; a stalled control censored with its announced read
kept; a failed control refusing the run; an unaffordable time study
stopping for approval with nothing published; a one-byte store bound
refusing during preparation, a clone larger than the bound refusing before
it is removed, four extracted trees refusing while the first clone still
exists, and an exhausted campaign clock refusing before the second clone; a base whose failing build script the fix deletes
proven insufficient because the reference is the fix commit's whole tree,
a test file an intervening commit added kept out of the fix's hidden tests,
a symlink the fix added under `tests/` not read as one, a fixture the
fix added under `tests/` beside every hidden test on both trees, with
`tests/` marked `export-ignore`, and a fixture the fix edited read in the
fix's version on both trees so a test of nothing but the fixture proves
no defect; five rows that fail their fetch fast not
counted as the time study's sample; preparation's git children in the
process envelope; the host seams
given the campaign time left and a named pull request without its creation
time `source_unavailable`; two paths that differ only in bytes UTF-8 cannot
carry digesting apart; missing settings, a missing witness, a climbing id, and a host
without namespaces refusing before execution; and the contained grading
reaching neither the runner's `HOME`, its loopback listener, nor the task
material under `tasks/` while the test's own loopback works.

## Residual judge and the live slice

`crates/eval-core/src/judge.rs` is the contract for what the deterministic
oracles leave open. No shell calls a judge or a live provider yet; the module
fixes the types, the refusals, and the firewall, so the evidence a later
shell produces is checked before it is believed.

A judge is a versioned dependency. `JudgeIdentity` is a `ProviderProfile`
(tokenizer profile included) with the prompt digest and the `Rubric` digest
(`eval-judge-rubric/v1`; a rubric under another schema has no digest), each 64
lowercase hex. `CalibrationSet` (`eval-judge/v1`) freezes that identity with
the human labels over anchor pairs before any judging and digests into every
report (`eval-judge-calibration/v1`); only a set that validates has a digest,
and validation refuses another schema, a malformed digest, a blank provider,
model, or tokenizer profile, an empty label map, a blank anchor id, and an
`Inconsistent` label, which only two disagreeing judge orders produce.
`SamplingPlan::validate` refuses fewer than 20 pairs and a human sample below
ten percent rounded up or below 20 pairs, or above the pair count; a campaign
under the floor cannot claim calibrated acceptance.

`blind` presents one `Pair` in one `Order`, refusing a planted canary in
either response and any arm name matched as whole words after folding case,
full width, and separators (`ARM_TOKENS`); the judge's view serializes only
`first` and `second`. `judge_pairs` needs both orders of every pair under one
judge, unswaps the positional verdicts, marks orders that disagree
`Inconsistent`, records each arm's length, and refuses an omitted order, a
call from another judge, an unknown or duplicated pair, a second call for one
pair and order (no rerolls), and a judge whose digests are malformed.
`PermutationCheck::validate` refuses arm identification above
`ARM_IDENTIFICATION_CEILING_PERCENT` in either direction: naming the arm
wrong consistently identifies it too.

`ResidualReport` (`eval-residual-report/v1`) records every identity, never a
name alone: the judge, the calibration digest, the live provider, the plan,
the permutation check, and the judgments. `validate` takes the pre-registered
`LiveSettings` and the live provider the run was approved for: it refuses
another provider, a plan other than the settings', and a calibration set under
another judge or digest, and reconciles the plan, the permutation check, and
duplicated or miscounted judgments. `comparable` runs every check that needs
no calibration set on both reports, and refuses cross-run `residual.*`
comparison (`ReanchorRequired`) until the anchor set is re-scored when the
judge, the live provider or its tokenizer profile, or the calibration digest
changes. The gates take only oracle inputs: `analyze`'s signature is pinned in
the tests, and the residual report carries no gate field.

`LiveSettings::validate` refuses without exactly two distinct approved
provider profiles with no blank component, a pass^k exponent `k`, a planned
attempt count `repeats` at least `k`, a frozen held-out task set with no blank
id, a calibration set, and a plan above the floor. `live_slice` constructs a
`LiveSliceReport` (`eval-live-slice/v1`) only from validated settings and one
of their two profiles, with the settings' `k`; it refuses no tasks, a repeated
task id, a task set that is not exactly the settings', and a task with other
than `repeats` attempts. Each task keeps its attempts beside pass@1, the
repeat counts, the censoring rate, and the pass^k interval through `pass_k`;
every attempt censored is `indeterminate`, never zero. The report carries the
settings' digest (`eval-live-settings/v1`). `replayable` is `LIVE_REPLAYABLE =
false`; `validate` takes the same settings and the profile the run was
approved for and refuses a relabelled report, another schema, an unapproved or
other profile, another `k`, another settings digest, a repeated task, another
task set, a task with other than the planned attempts, and a summary the
attempts do not give, so a deserialized report is held to the pre-registration
a constructed one ran under. Tests: `crates/eval-core/tests/judge.rs`.

## Coverage markers

`MARKERS` is the evaluator-owned registry: constant, globally unique names,
each with the test that records it. A test records a marker through
`Coverage::record` only after asserting the case's preconditions, never the
invariant; `record` refuses an unregistered name, and `Coverage::complete(suite)`
is `Incomplete { missing }` unless every registered marker whose test path
starts with `suite` fired, and `EmptySuite` when the prefix selects no marker
(an empty prefix names the whole registry). Each
daemon suite owns the markers whose tests it holds, whatever their name
prefix: `eval_ingestion.rs` the Phase 1 `ing_` markers, `eval_ledger.rs` the
`ldg_` markers, `eval_surface_ledger.rs` the `sls_` markers, `eval_cassette.rs`
the `rid_` markers (the reviewer peer's marker is `rid_` too, because the suite
owns the prefix even though the record is
`sls-memory-reviewer-model-calls-cassette-or-excluded`), `eval_aging.rs`
the checkpoint and window markers (`flt_quiescence_receipt_all_zero`,
`flt_copy_attempted_mid_episode`, `flt_checkpoint_observed_busy`,
`flt_prefix_history_slipped`, `flt_foreign_incarnation_refused_at_reopen`,
`sls_memstore_copy_refused_live_handle`, `ing_aged_arm_restarted_between_sessions`,
`ing_window_has_pre_snapshot_supersession`,
`ing_window_has_pre_snapshot_retirement`), and
`crates/eval-core/tests/injection.rs` the `mtr_` markers; the manifest refusal
marker `wm_bulk_scaffold_presented_as_aged` is recorded by the eval-core
manifest suite, and the pure fault-contract markers (`flt_premature_success_fixture_refused`,
`flt_incomplete_coverage_named_not_pass`, `flt_crash_model_label_refused`,
`flt_liveness_unmet_named_at_bound`) by the eval-core fault suite, the pure
growth markers (`flt_restore_under_never_restored_refused`,
`xc_shared_fixture_refused`, `flt_incomplete_mix_not_success`) by the eval-core
growth suite, and `eval_growth.rs`, the campaign shell that lands with the
second half of this change, owns the growth-campaign markers
(`flt_leak_ledger_sampled_before_reopen`, `flt_headroom_accounted_from_store_constants`,
`xc_envelope_breach_stops_the_run`, `xc_parallel_campaigns_isolated`,
`flt_swarm_mix_complete`); `eval_fault.rs`
owns the fault-campaign markers (`flt_lost_reply_unknown_until_readback`,
`flt_every_declared_cut_receipted`, `flt_kill_barrier_read_before_kill`,
`flt_r11_recorded_as_expected_refusal`, `flt_r24_recorded_as_expected_refusal`,
`flt_liveness_bounds_met_with_faults_armed`, `flt_corruption_detected_at_quiescence`,
`flt_external_lock_holder_released`, `flt_artifact_fault_named_errno`,
`sls_embedding_publication_held_then_released`). Each suite checks that every
marker it owns names one of its scenarios and runs its completeness proof on
every pass: all scenarios once, then `Coverage::complete` over its own prefix.
The injection suite also runs each scenario alone and requires its fired set to
equal the markers the registry attributes to it, so a scenario cannot record
another's marker to complete the suite. A whole-registry proof would need one
run to reach every suite's preconditions and does not exist yet.

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
