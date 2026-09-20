# Evaluator core

`crates/eval-core` holds the value-level contracts of the long-horizon
evaluator: the run manifest, run identity, residue rules, the surface census
pins, and the generated world model (keyed draws, the choice tape, the event
log, and the step drive). It is sans-I/O. Every function takes values and returns values;
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

## Manifest `eval-manifest/v1`

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

The 24 required fields, sorted:

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
| `reachability` | `default-production`, `explicit-config-only`, or `test-only`. |
| `residue` | Every non-`Keep` field with its rule, including the manifest's own. |
| `result_digest`, `witness_digest` | Lowercase hex SHA-256. |
| `retry_lineage` | Prior `eval_run_id` values of retried attempts; each is lowercase hex SHA-256. |
| `run_identity` | The nine-component identity tuple, including the build sub-record, the eligibility-spec digest, and the linearization rule version. |
| `sample_epoch`, `sample_ids`, `sample_order` | Stable sample identity and execution order; `sample_order` must be a permutation of `sample_ids`. |
| `schema` | `eval-manifest/v1`. |
| `status` | `completed`, `incomplete`, `refused`, or `blocked`. |
| `tokenizer_profile` | Name, revision, digest. |

`Manifest::digest` re-parses the manifest, applies the manifest's own residue
rules (`start_ms`, `end_ms`, and `envelope_peaks` are `Drop`; everything else
is `Keep`), and hashes with protocol `eval-manifest-digest/v1`. The digest is
a function of every kept field, not of the run identity alone: two processes
that record the same identity and the same kept contents produce the same
digest (`two_process_same_identity_yields_equal_manifest_and_trace_digests`),
and two runs that share an identity but differ in `status`, `sample_order`,
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
is refused; it names no build, and an absent digest needs a non-empty reason.
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
snake_case is refused (`FieldNotSnakeCase`) because the gates below match that
spelling, as is a `Keep` or
`Relative` value canonical JSON cannot encode (`NotCanonical`). A refused
observation leaves the trace and its `Relative` numbering unchanged, so a
recorded trace always digests.
`CLOCK_FIELD_KEEP_ALLOWLIST` (`now_ms`, `observed_at_ms`, `valid_time_ms`)
names the only clock-named fields a schema may keep; any other clock-named
field under `Keep` is refused at schema construction, as is any field named
for a hostname (`hostname` or a `host` token), cwd, pid, or incarnation
(`HostFieldKept`). The trace digest
uses protocol `eval-trace/v1`. Rules apply to the top-level fields of an
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
event exists (`WorldError::EventBound { events, max }`). The check gates on the
slot count first, so a config declaring billions of messages is refused in
time proportional to the entity count. There is no default for the bound: a
missing field fails to parse, and a zero bound, a non-positive tick, an epoch
outside the valid-time domain, an empty entity set, or an entity with no slots
is `WorldError::InvalidField(name)`. The bound is a config value rather than a
manifest field; the manifest carries it inside `run_identity.config`.
`GENERATOR_VERSION`, `RANDOM_SCHEMA_VERSION`, and `LINEARIZATION_RULE_VERSION`
are constants the shell copies into the run identity's `generator_version`,
`random_schema_version`, and `linearization_rule_version`.

### Keyed draws

Every random decision is `keyed_draw(root_seed, &site)`: the first 64 bits of
the protocol digest (`eval-random/v1`) over `(root_seed, axis, kind, actor,
site, occurrence)`, reduced by `% candidates` (the small modulo bias is part
of the random schema). `actor` is the entity (`session-0`, `repository-1`),
`site` is the mutation slot (`slot:3`), and `occurrence` counts earlier choices
of the same kind at that slot. A draw depends only on its key, so shortening
one entity's history leaves every other entity's text, renames, revision
targets, and times unchanged. The axis label (`text` for words, `topology` for
rename targets, `evolution` for time gaps, observation lags, citations, and
correction or invalidation targets) is fixed per `ChoiceKind` through
`ChoiceKind::axis`.

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
names present events (`DanglingEdge`), and that each edge runs from an earlier
position (`EdgeAgainstOrder`) and a smaller depth (`EdgeAgainstDepth`) to a
later one. It does not recompute depths, so a log with a deleted event keeps
the depths it was generated with.

`EventLog::without(id)` removes one event and its incident edges and touches
no other payload: a correction whose target was removed keeps naming it, and
the log still validates. This is the deletion rule the ticket asks for ("no
repair of surviving semantic payloads"); the reducer treats a target that
names an absent event as a distinct case rather than promoting the correction
to an original. Equality and `EventLog::digest` (`eval-event-log/v1`) cover
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
