# synthetic-normalization-is-scoped-to-the-pass

Baseline: `913234433ae36a80a6e22c6aac14c7f9aab74386`, 2026-09-10.
The [scope and provenance](../catalog.md#scope-and-provenance) apply here.

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
[normalize]: ../../../../../crates/daemon/src/transform.rs#L2083-L2100
[apply-head]: ../../../../../crates/daemon/src/transform.rs#L2836-L2866
[shadow-first]: ../../../../../crates/daemon/src/transform.rs#L2855-L2856
[shadow]: ../../../../../crates/daemon/src/transform.rs#L2951
[pending-pass]: ../../../../../crates/daemon/src/transform.rs#L6626-L6654
[t-pending]: ../../../../../crates/daemon/src/transform.rs#L19129
[t-collapsed]: ../../../../../crates/daemon/src/transform.rs#L27269-L27270
[flatten]: ../../../../../crates/daemon/src/wire.rs#L622-L685
[meta-clone]: ../../../../../crates/daemon/src/wire.rs#L514
[reattach]: ../../../../../crates/daemon/src/wire.rs#L146-L187
[reattach-meta]: ../../../../../crates/daemon/src/wire.rs#L181
[arc-parsed]: ../../../../../crates/daemon/src/lib.rs#L8115
[prepare-call]: ../../../../../crates/daemon/src/lib.rs#L8245-L8247
[historian-fire]: ../../../../../crates/daemon/src/lib.rs#L4994
[boundary-call]: ../../../../../crates/daemon/src/lib.rs#L5074
[assemble]: ../../../../../crates/daemon/src/lib.rs#L5234-L5238
[store-pc]: ../../../../../crates/daemon/src/lib.rs#L4302-L4345
[native-attach]: ../../../../../crates/daemon/src/lib.rs#L13082-L13094
[newest-assistant]: ../../../../../crates/daemon/src/lib.rs#L13138-L13143
[cached-boundary]: ../../../../../crates/daemon/src/lib.rs#L16566-L16626
[msg-filter]: ../../../../../crates/daemon/src/lib.rs#L16586-L16588
[boundary-filter]: ../../../../../crates/daemon/src/lib.rs#L16597
[todo-prefix]: ../../../../../crates/daemon/src/injection.rs#L187-L189
[tail-reclaim]: ../../../../../crates/daemon/src/healing.rs#L130-L139
[ser-msg]: ../../../../../crates/memory-store/src/lib.rs#L145-L161
[meta-doc]: ../../../../../crates/memory-store/src/lib.rs#L210-L216
