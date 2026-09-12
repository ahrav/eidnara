# synthetic-normalization-is-scoped-to-the-pass

Baseline: `913234433ae36a80a6e22c6aac14c7f9aab74386`, 2026-09-10.
The [scope and provenance](../catalog.md#scope-and-provenance) apply here.
The discovery and investigation sections describe that baseline. Their source
links are pinned to it. The implementation evidence below describes the live
pass-local view.

## Discovery trigger

The audit proposes replacing the whole-request clone in
`normalize_synthetic_todo_ingress` with a shared view or an in-place flag. At
HEAD the flag is set on a clone that only `apply_once` reads; the handler keeps
passing the un-normalized `parsed` to the historian, the projection-cache
charge, and the native attach. A design that marks the shared request changes
which messages those observers treat as synthetic, and a design that rebuilds
the message drops `original`, so the flag reaches the wire for the first time.
The transform catalog's [synthetic-strip record][tc-synthetic] states the
inside-the-pass invariant; nothing states the outside one.

## Evidence trail

- [`normalize_synthetic_todo_ingress`][normalize] returns `None` unless some
  non-synthetic message carries a [`synthetic_todo_`][todo-prefix] call or
  result id; then it clones the request once and sets
  `messages[index].ck.meta.synthetic = true` on the clone.
- Inside [`apply_once`][apply-head] the clone shadows the input:
  [`:2855-2856`][shadow-first] binds `ingress_req` to the normalized clone or
  the original, and [`:2951`][shadow] rebinds `req` to the rebased or the
  `ingress_req` view, so every later read in the pass sees the flags.
- The projection is built from `ingress_req.messages`, so
  [`flatten_block`][flatten] copies the normalized flag into
  `FlatBlock.synthetic` and the projection's `message_meta` holds the
  normalized `HarnessMeta` ([`:514`][meta-clone]).
- The handler binds `parsed` before the transform ([`:8115`][arc-parsed]) and
  hands that un-normalized value to [`prepare_historian_fire`][historian-fire]
  at [`:8245-8247`][prepare-call], which calls
  [`boundary_messages`][boundary-call] and
  [`assemble_historian_firing`][assemble] with `parsed`. In
  [`cached_boundary_messages`][cached-boundary] the message filter is
  `!message.ck.meta.synthetic` ([`:16586-16588`][msg-filter]) and the block
  filter is `!block.synthetic` ([`:16597`][boundary-filter]).
- [`store_projection_cache`][store-pc] takes `request: &TransformRequest`
  from the handler; [`attach_native_messages_incremental`][native-attach]
  selects the newest assistant by `!message.ck.meta.synthetic` over
  `request.messages` ([`:13138-13143`][newest-assistant]).
- On a delta turn [`reattach_messages_prefix`][reattach] rebuilds prefix
  shells with `message.meta.clone()` from the projection's `message_meta`
  ([`:181`][reattach-meta]), so a previously normalized message arrives in
  `parsed` with `synthetic: true` already set.
- [`Serialize for WireMessage`][ser-msg] replays `original` when present; a
  `meta` edit does not clear it, only [`mark_modified`][meta-doc] does. The
  normalized clone therefore serves the same bytes as the un-normalized input.
- [`pending_passthrough_messages`][pending-pass] serves `req.messages` of the
  normalized view through `from_message_reusing`, so its fingerprints use the
  typed flag while its bytes carry none.
- [`tail_reclaim`][tail-reclaim] is `true` for every shipping profile, so a
  bust pass can freeze a todo pair; the [replayed-pair test][t-collapsed]
  builds the replay at `:27303-27318` without the `synthetic` marker.

## Failure scenario

A shared-reference design flags `parsed` before `apply`. The historian's
message filter then drops the replayed pair on a full-array turn where HEAD
keeps it as a `BoundaryMsg` with zero blocks, so historian ordinals change. The
native attach's newest-assistant choice changes. A design that flags through
`mark_modified` or rebuilds the message emits `"synthetic":true` on the wire
where HEAD emits the harness bytes. Both are behavior changes relative to HEAD.

## Timing windows and dependencies

None in time. The observers' agreement depends on the request reaching the
handler in one of two shapes: the full-array shape, where the harness replays
the pair unflagged, and the delta shape, where the rebuilt prefix already
carries the flag. B5 records that the delta shape is the one a campaign must
reach.

## What a test must construct

A prior bust pass that froze a todo pair; an array that replays the pair as
ordinary messages; a historian firing on that pass; `serve_native` on. Capture
the `BoundaryMsg` list and `input_ordinals` the historian sees, the projection
cache's charged messages, the newest-assistant mid, and the served bytes of
the normalized message, and compare each against the HEAD reference on both a
full-array turn and a delta turn. The
[shared-input checks](../existing-checks.md#shared-input-equivalence) cover the
inside shadow ([`pending_rewrite_passes...`][t-pending]) and cache reuse on the
replayed pair ([`warm_cache_...`][t-collapsed]); none compares the two lanes.

## Investigation log

### Q: Should the historian see the replayed pair as zero blocks or not at all?

- Sources examined: [`cached_boundary_messages`][cached-boundary] filters at
  [`:16586-16588`][msg-filter] and [`:16597`][boundary-filter]; the
  [reattach meta copy][reattach-meta].
- Findings: Full-array lane: the message passes the message filter and every
  block fails the block filter, so a zero-block `BoundaryMsg` exists. Delta
  lane: the rebuilt shell carries `synthetic: true` and the message filter
  drops it. Both are HEAD behavior.
- Missing evidence: A statement of the intended historian view.
- Conclusion: needs human input.

### Q: Is the fingerprint-versus-bytes pairing in passthrough intended?

- Sources examined: [`pending_passthrough_messages`][pending-pass],
  [`Serialize for WireMessage`][ser-msg].
- Findings: Fingerprints derive from the typed flag (`eidnara_todo:` ids);
  bytes replay `original` without the flag.
- Missing evidence: A statement that the pairing is intended.
- Conclusion: needs human input.

[tc-synthetic]: ../../../daemon/transform/catalog.md#synthetic-strip-precedes-every-coverage-read
[normalize]: https://github.com/ahrav/eidnara/blob/9132344/crates/daemon/src/transform.rs#L2083-L2100
[apply-head]: https://github.com/ahrav/eidnara/blob/9132344/crates/daemon/src/transform.rs#L2836-L2866
[shadow-first]: https://github.com/ahrav/eidnara/blob/9132344/crates/daemon/src/transform.rs#L2855-L2856
[shadow]: https://github.com/ahrav/eidnara/blob/9132344/crates/daemon/src/transform.rs#L2951
[pending-pass]: https://github.com/ahrav/eidnara/blob/9132344/crates/daemon/src/transform.rs#L6626-L6654
[t-pending]: https://github.com/ahrav/eidnara/blob/9132344/crates/daemon/src/transform.rs#L19129
[t-collapsed]: https://github.com/ahrav/eidnara/blob/9132344/crates/daemon/src/transform.rs#L27269-L27270
[flatten]: https://github.com/ahrav/eidnara/blob/9132344/crates/daemon/src/wire.rs#L622-L685
[meta-clone]: https://github.com/ahrav/eidnara/blob/9132344/crates/daemon/src/wire.rs#L514
[reattach]: https://github.com/ahrav/eidnara/blob/9132344/crates/daemon/src/wire.rs#L146-L187
[reattach-meta]: https://github.com/ahrav/eidnara/blob/9132344/crates/daemon/src/wire.rs#L181
[arc-parsed]: https://github.com/ahrav/eidnara/blob/9132344/crates/daemon/src/lib.rs#L8115
[prepare-call]: https://github.com/ahrav/eidnara/blob/9132344/crates/daemon/src/lib.rs#L8245-L8247
[historian-fire]: https://github.com/ahrav/eidnara/blob/9132344/crates/daemon/src/lib.rs#L4994
[boundary-call]: https://github.com/ahrav/eidnara/blob/9132344/crates/daemon/src/lib.rs#L5074
[assemble]: https://github.com/ahrav/eidnara/blob/9132344/crates/daemon/src/lib.rs#L5234-L5238
[store-pc]: https://github.com/ahrav/eidnara/blob/9132344/crates/daemon/src/lib.rs#L4302-L4345
[native-attach]: https://github.com/ahrav/eidnara/blob/9132344/crates/daemon/src/lib.rs#L13082-L13094
[newest-assistant]: https://github.com/ahrav/eidnara/blob/9132344/crates/daemon/src/lib.rs#L13138-L13143
[cached-boundary]: https://github.com/ahrav/eidnara/blob/9132344/crates/daemon/src/lib.rs#L16566-L16626
[msg-filter]: https://github.com/ahrav/eidnara/blob/9132344/crates/daemon/src/lib.rs#L16586-L16588
[boundary-filter]: https://github.com/ahrav/eidnara/blob/9132344/crates/daemon/src/lib.rs#L16597
[todo-prefix]: ../../../../../crates/daemon/src/injection.rs#L187-L189
[tail-reclaim]: ../../../../../crates/daemon/src/healing.rs#L130-L139
[ser-msg]: ../../../../../crates/memory-store/src/lib.rs#L145-L161
[meta-doc]: ../../../../../crates/memory-store/src/lib.rs#L210-L216

## Pass-local view evidence

Implementation base: `bf6b9d5fad969fa29da852a1dd9f1de569732197`.
Preservation authority: [implementation ticket](https://github.com/ahrav/eidnara/issues/419)
and [parent specification](https://github.com/ahrav/eidnara/issues/350).

[`TransformIngress`][ingress-view] borrows the request. Its
[`MessageProjection`][message-view] owns a set of borrowed message IDs for
unflagged synthetic todo carriers. Projection rejects duplicate message IDs;
the override uses the same identity as the projection map. Both full and
incremental projection consult the view for block flags, identity membership
and message metadata. The set is pass-local, not a retained cache. The derived
projection retains normalized metadata and the handler may cache it. Prefix
reattachment restores those flags. The independent lineage ordinal rebase
still clones the request.

Request-dependent transform helpers read the view. Passthrough rendering
changes the output clone's typed flag without clearing retained ingress JSON.
The handler still passes the original request to boundary construction,
historian assembly, projection-cache accounting and native attachment. These
handler call sites have no production changes.

The [reference comparison][reference-test] uses decoded unflagged messages and
a clone with only the pair's typed flags set. Fresh, pending-rewrite and
lineage-passthrough cases compare canonical served bytes, complete projection
state (including digests), native attachment bytes and tag rows. Passthrough
also pins both `eidnara_todo:` fingerprint IDs and the unflagged ingress bytes.
The original request remains unchanged. The historian lane retains two empty
boundary messages; its chunk input retains ordinals `[90, 91, 92]`, and chunk
text, snapshot and metadata agree across the two transform inputs.

The [handler comparison][delta-parity-test] compares full and incremental
projection and expands the native response delta before comparing native
bytes. The [firing witness][delta-witness-test] establishes the required
delta-turn situation separately from the equivalence assertions.

### Focused execution, 2026-09-11

Before production edits, `cargo test -p daemon --lib --locked` with each of
these filters passed: `synthetic_ingress_matches_flagged_reference` (1),
`handler_delta_normalization_matches_full_when_reserved_todo_starts_at_frontier`
(1), `warm_cache_selection_bust_does_not_replay_collapsed_synthetic_todo_as_live`
(1), `dg_goldens` (2), and
`continued_lineage_tolerates_a_synthetic_head_like_the_seam_check` (1).
The firing-input and nonempty-native/tag assertions extend the reference
comparison after that baseline; they are not claimed as pre-edit checks.

After implementation, the same Cargo command passed filters `synthetic` (20),
`lineage` (15), `differential` (8), `golden` (32), `projection` (15),
`pending_rewrite` (4), `strip` (18), `temporal` (9) and `user_hint` (4).
The strengthened reference comparison, handler delta comparison and
`native_attachment_reuses_transform_tag_baseline_and_preserves_bytes` each
passed individually. Counts overlap; they are not a unique-test total.

Test adequacy remains unaudited. No production plugin trace, benchmark,
before/after measurement or A/A evidence is claimed. Whole-workspace gates
and independent reviews belong to the controller. The historical design
questions above are resolved only as preservation requirements: neither
observer semantics nor fingerprint identifiers change.

[ingress-view]: ../../../../../crates/daemon/src/transform.rs#L2091-L2140
[message-view]: ../../../../../crates/daemon/src/wire.rs#L378
[reference-test]: ../../../../../crates/daemon/src/transform.rs#L27359
[delta-parity-test]: ../../../../../crates/daemon/src/lib.rs#L23735
[delta-witness-test]: ../../../../../crates/daemon/src/lib.rs#L23468
[lineage-rebase-test]: ../../../../../crates/daemon/src/transform.rs#L28625

## Retention and observer-scope verification

The normalized metadata is not a new retention behavior. At
`bf6b9d5fad969fa29da852a1dd9f1de569732197`, the
[normalizer][baseline-normalizer] clones the request and sets the typed flag;
the [pass entry][baseline-projection-input] projects that clone; and the
[projector][baseline-projection-meta] copies `msg.ck.meta` into
`ProjectionMessageMeta`. Removing normalized flags from cached metadata would
change that baseline. The optimization removes the input clone and discards
the override set after the pass, not the derived projection's flags.

The handler's original request means its current, expanded ingress. An
unflagged replay in a full array or delta suffix is an empty boundary message
and contributes an input ordinal to historian chunk construction. When the
same replay enters a cached prefix, reattachment restores its normalized flag
and both historian filters omit it. The
[three-turn handler witness][delta-witness-test] checks this distinction;
it does not require full raw ingress and reattached ingress to be identical.
It compares actual producer prompts and native bytes against a full request
whose prefix is reconstructed from the clone-era typed-flag projection.

The [direct reference comparison][reference-test] intentionally gives both
transform variants the same original handler-observer request. Its tuple now
also compares all `BoundaryMsg` debug fields. Fresh mode excludes replay
carriers. Passthrough mode asserts typed flags are true while original input
flags and retained bytes stay unflagged. A nonempty carrier-targeted tag and
temporal overlay leaves the replay bytes and fingerprints unchanged; the live
control takes its tag. The carrier contains text so a guard that reads the
unflagged ingress metadata would permit a visible temporal edit.

The [lineage comparison][lineage-rebase-test] first establishes a valid
descent, then replays the edge as a non-subagent with a fresh-ordinal synthetic
head. The actual pass rebases ordinal 1 to 11, excludes the head from live
identity, and matches the typed-flag reference's projection, served bytes and
fingerprints. Rebasing changes ordinals and their metadata, not message or
tool IDs. Recomputing overrides after rebasing preserves classification while
borrowing IDs from the rebased request; no owned-string set is needed.

All non-indexed scans reached through `TransformIngress` that explicitly
exclude synthetic messages use `live_messages()`. The additive-only pass and
`clear_served_native_reasoning_from_iter` keep their raw predicates, matching
the baseline. The reverse scan that uses array positions keeps its indexed
predicate. `provisional_tail_mid` consumes only the message view and
`mid_turn`, and returns a reference with the message-data lifetime. Raw
immutable request forwarding through `Deref` remains deliberate for scalar
consumers. Normalized decisions use the view, as its source contract states.

### Follow-up focused execution

The strengthened direct comparison, three-turn handler witness and lineage
rebase test each passed before the local scan/helper cleanup. After cleanup,
`cargo test -p daemon --lib --locked` passed filters `synthetic` (21),
`lineage` (16), `projection` (15), `pending_rewrite` (4), `strip` (18),
`temporal` (9), `user_hint` (4), `additive` (2) and `differential` (8).
`cargo clippy -p daemon --lib --tests --locked -- -D warnings` passed.
These overlap and remain focused checks, not a whole-workspace verdict.

[baseline-normalizer]: https://github.com/ahrav/eidnara/blob/bf6b9d5fad969fa29da852a1dd9f1de569732197/crates/daemon/src/transform.rs#L2083-L2100
[baseline-projection-input]: https://github.com/ahrav/eidnara/blob/bf6b9d5fad969fa29da852a1dd9f1de569732197/crates/daemon/src/transform.rs#L2855-L2866
[baseline-projection-meta]: https://github.com/ahrav/eidnara/blob/bf6b9d5fad969fa29da852a1dd9f1de569732197/crates/daemon/src/wire.rs#L505-L519
