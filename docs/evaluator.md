# Evaluator core

`crates/eval-core` holds the value-level contracts of the long-horizon
evaluator: the run manifest, run identity, residue rules, the surface census
pins, the generated world model (keyed draws, the choice tape, the event log,
and the step drive), the eligibility spec, the bitemporal reducer, the
occurrence identity rule, the fixture renderer, and the coverage-marker
registry. It is sans-I/O. Every function takes values and returns values;
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

## Manifest `eval-manifest/v3`

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

The 26 required fields, sorted:

| Field | Content |
| --- | --- |
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
| `ingestion` | `adapter-ingested, production caller: none` or `direct-database, non-aged`; the latter with a `replay` construction is refused (`DirectDatabaseAged`). |
| `reachability` | `default-production`, `explicit-config-only`, or `test-only`. |
| `residue` | Every non-`Keep` field with its rule, including the manifest's own. |
| `result_digest`, `witness_digest` | Lowercase hex SHA-256. |
| `retry_lineage` | Prior `eval_run_id` values of retried attempts; each is lowercase hex SHA-256. |
| `run_identity` | The nine-component identity tuple, including the build sub-record, the eligibility-spec digest, and the linearization rule version. |
| `sample_epoch`, `sample_ids`, `sample_order` | Stable sample identity and execution order; `sample_order` must be a permutation of `sample_ids`. |
| `schema` | `eval-manifest/v3`. |
| `status` | `completed`, `incomplete`, `refused`, or `blocked`. |
| `tokenizer_profile` | Name, revision, digest. |

`Manifest::digest` re-parses the manifest, applies the manifest's own residue
rules (`start_ms`, `end_ms`, and `envelope_peaks` are `Drop`; everything else
is `Keep`), and hashes with protocol `eval-manifest-digest/v3`. Version 2
added `execution_mode` (the reducer differential runs under `enumerate`);
version 3 added `ingestion`, because no ingestion entry point has a production
caller and every manifest must say so. The digest is a function of every kept
field, not of the run identity alone: two processes that record the same
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

## Coverage markers

`MARKERS` is the evaluator-owned registry: constant, globally unique names,
each with the test that records it. A test records a marker through
`Coverage::record` only after asserting the case's preconditions, never the
invariant; `record` refuses an unregistered name, and `Coverage::complete` is
`Incomplete { missing }` unless every registered marker fired. The first entry
is the four-seam lifecycle witness. The daemon suite `eval_ingestion.rs` owns
every current marker, checks uniqueness and that each named test exists, and
runs the completeness proof on every pass: all scenarios once, then
`Coverage::complete`.

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
