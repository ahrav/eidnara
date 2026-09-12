# admission-chain-charges-before-decode-and-refuses-effect-free

Baseline: `913234433ae36a80a6e22c6aac14c7f9aab74386`, 2026-09-10.
The [scope and provenance](../catalog.md#scope-and-provenance) apply here.
The discovery and investigation sections describe that baseline. Their source
links are pinned to it. The implementation evidence below describes the live code.

## Discovery trigger

The audit proposes fusing the `Value` parse with the typed decode so a body
is read once. That moves the resident charge relative to allocation and moves
the admission decision relative to the post-parse side effects. The contract
to preserve is the order of the four pre-dispatch steps, the pool the charge
draws on, the two refusal codes, and the absence of any dispatch-side effect
on refusal. The parent's [E2][e2] owns the charge's lifetime; this record owns
its position and magnitude.

## Evidence trail

- [`Handler::handle`][handle] runs, in order:
  [`enforce_request_byte_cap`][bytecap],
  [`value_footprint_bound`][footprint], `ctx.try_reserve_resident(footprint)`,
  then `serde_json::from_slice::<Value>` with failure mapped to `Value::Null`.
  The charge binding `_parse_charge` lives until the function returns, so it
  spans dispatch and [`settle_prepared`][settle].
- [`try_reserve_resident`][reserve] is `self.scratch.try_charge(bytes)`; its
  doc says reserving after allocation lets the allocation escape the envelope.
  [`resident_capacity`][capacity] is `self.scratch.capacity()`. The scratch
  pool is one host-wide [`ByteBudget`][pools] sized by
  [`SCRATCH_RESERVED_BYTES`][scratchconst]; the constant evaluates to
  184,878,336 bytes, about 176 MiB, with `MAX_BODY_LEN` at 64 MiB.
- [`ByteBudget::try_charge`][try-charge] uses `try_acquire_many_owned` and
  returns `None` on a `u32` overflow or a short pool; it never awaits.
- The refusal helpers are [`request_too_large_error`][toolarge], which emits
  `invalid_params`, and [`resident_capacity_error`][queuefull], which emits
  `queue_full`. `handle` selects the first when the footprint scan overflows or
  when `footprint > resident_capacity()`, and the second otherwise.
- The footprint arithmetic is `nodes * size_of::<Value>() * VALUE_NODE_SLACK +
  string_bytes * RETAINED_STRING_COPIES + VALUE_ENVELOPE_BYTES` with
  [slack 2, envelope 4096, copies 3][copies]; the copies doc names the three
  holders: the decoded `Value`, the `WireMessage` `original`, and the
  `WireBlock` `original`. [`WireMessage`][wiremsg] and [`WireBlock`][wireblock]
  decode through a `Value` and keep it as `original`.
- The post-parse side effects sit inside the transform arm:
  [`freeze_prompt_surface_selection`][freeze] and the
  [`transform_route_channels` insert][routechan] run before
  [`ticket.accept()`][accept]; the [`TransformDispatchTicket`][ticket] is
  created by `handle_transform_dispatch`, which a refused body never reaches.
- [`RequestOutcome::error`][outcome] sets `retry_after_ms: None`. The wire
  contract at [§6.3][wire63] names `invalid_params` for the 1 MiB and 32 MiB
  limits; [§7.5.1][wire751] scopes the `queue_full` retry hint and the
  `schema_violation` code to Synapse. The string `request_too_large` appears
  only in the [direct-host fixture control channel][fixture].
- The plugin [pages bodies over 512 KiB][paging], so the over-cap arms need
  constructed input; the plugin sends both [`method` and `kind`][plugin].

## Failure scenario

A fused design starts the typed decode before the charge is held. A body that
fits the cap but not the pool allocates its `WireMessage` trees, then fails
the reservation; the allocation escaped the envelope while peers were
admitted. A design that decodes before deciding reaches the prompt freeze and
the route-channel insert for a body the current code refuses, so a refused
request leaves session state behind. A design that awaits the reservation
parks while holding the pending and task permits, changing admission order for
every peer.

## Timing windows and dependencies

None in time for the ordering clause. The `queue_full` arm depends on
concurrent holders of the shared scratch pool, which is A3's situation. The
charge magnitude depends on `RETAINED_STRING_COPIES` matching what the typed
decode retains; a change to either `Deserialize` impl changes the truth of the
bound without changing the constant.

## What a test must construct

Drive `Handler::handle`, not `dispatch_value`; `RequestCtx` is transport-private
so [`dispatch_value_for_test`][testentry] enters after admission. Bodies: one
over 1 MiB without a transform-class discriminator; one over 32 MiB with one;
one whose footprint exceeds about 176 MiB (a scalar-dense body bounds far above
its wire size); one admitted while a barrier holds a concurrent charge. Assert
the code, that no ticket, route channel, prompt freeze, page staging, or store
row changed, and that the pool's available bytes return when the future ends.
For the magnitude clause, decode a large text block and count owned copies in
the resulting `TransformRequest`. The
[ingress checks](../existing-checks.md#ingress-admission-and-decode)
cover the cap and footprint as pure functions; none enters at `handle`.

The real-host fixture `FixtureProcess` in
`crates/daemon/tests/support/direct_host.rs`, used by `direct_host.rs:48-70`,
reaches `Handler::handle` over the ring and is the seam. Count copies
structurally (the `Value` node, the `WireMessage` `original`, and the
`WireBlock` `original` for one known block) or through a counting allocator,
because the decoded type exposes no copy count.

## Investigation log

### Q: Does the context module owe a `retry_after_ms` on `queue_full`?

- Sources examined: [`RequestOutcome::error`][outcome], [§7.5.1][wire751],
  [§6.3][wire63], [`resident_capacity_error`][queuefull].
- Findings: The helper carries no hint and the outcome constructor fixes
  `retry_after_ms: None`. The contract's hint rule is written for Synapse
  `embed.query`; routed transform bodies are opaque to the contract.
- Missing evidence: A written rule for the context module.
- Conclusion: needs human input.

### Q: Is the page staging budget the intended cover for the assembled whole?

- Sources examined: the [page apply][pageapply] call into
  `handle_transform_unpaged_value`, the [512 KiB page cap][hostpagecheck], and
  the staging constants at [`:735-736`][hostpage] (128 MiB staged maximum).
- Findings: The assembled `Value` reaches the typed decode with the per-page
  reservation history only; no `value_footprint_bound` runs on the whole.
- Missing evidence: A statement of which budget covers the assembled tree.
- Conclusion: needs human input.

### Q: Is sharing the Synapse-sized scratch slice intended?

- Sources examined: [`SCRATCH_RESERVED_BYTES`][scratchconst] and its doc,
  the [pool split][pools].
- Findings: The doc lists Synapse terms only; the transform footprint is not
  in the sizing or in `validate_serving_limits`.
- Missing evidence: A sizing statement that names the transform footprint.
- Conclusion: needs human input.

[e2]: ../../catalog.md#request-work-accounting-covers-retained-resources

## Metered-decode evidence

Implementation base: `96709d0ef54bcfad2327878ab96e118fb8ba4969` plus the units
that precede it on the branch.
Preservation authority: [implementation ticket](https://github.com/ahrav/eidnara/issues/436)
and [parent specification](https://github.com/ahrav/eidnara/issues/350).

The byte pre-scan no longer reserves. [`Handler::handle`][handle-live] runs the
byte cap, then creates a [`ResidentMeter`][meter] over the request's scratch
reserve and hands it to [`dispatch_body`][dispatch-body]. There a
[byte-derived floor][floor] refuses, before either decode and with no charge
taken, every body whose value count alone proves its footprint exceeds the
capacity: the scan counts one value per opening quote, `[`, `{`, number start,
or literal start outside a string, which for well-formed JSON is the count the
meter visits, and leaves string bytes out, so the floor never exceeds the
decoded footprint and refuses no body the meter would admit; a body too short
to reach the capacity is not scanned. Every decode of the body, typed or tree,
then runs through [`decode_metered`][decode]. The meter's
[visitor][visitor] wraps the one the decode supplies: each value the
deserializer hands to a visitor, object keys included, adds one node charge,
each string adds its length times the retained-copy count, and the fixed
envelope is added on the first value, so the footprint is the
[same arithmetic][constants] computed from parsed values rather than from
separators. A value a derived struct ignores is [skipped through the
meter][ignored] rather than through serde_json's own skip, so its contents are
counted and depth-checked; the two lanes therefore count one footprint for the
same bytes and refuse the same bodies. The meter [charges the reserve][need]
as the footprint grows, in batches: a batch is the footprint reached so far
until that reaches [one mebibyte][step], then one mebibyte, trimmed to the
capacity, so a small body holds at most twice its footprint and a large one
pays one acquisition per mebibyte; a batch the pool cannot grant is halved
toward the exact shortfall, which is the admission decision, so a nearly
drained pool is taken in a few acquisitions rather than one per value. A
footprint above the capacity is refused without asking the pool for more; a
footprint that fits but finds the pool held is refused as transient. The decode
stops at that visit, and a refused decode [releases][release] every held byte at
once, so the pool is not held while the refusal is classified and sent. An
admitted body's charges live in the meter, which outlives
[`settle_prepared`][handle-live], so they are released when the future ends, as
the single charge was.

Two costs follow from charging inside the decode rather than before it, and
the module documents both. A body whose footprint exceeds the capacity holds
pool bytes for the values it built until the footprint crosses the capacity,
so a doomed decode that reaches the pool occupies it for the length of its
failing parse, where the pre-scan refused it before decoding. The floor
refuses every such body whose value count proves the excess, so a doomed
decode that reaches the pool owes its excess to string bytes, which the byte
cap bounds to three copies of one transform body; a body of scalars cannot
reach it. And serde_json unescapes a string into its scratch buffer before the
visitor sees it, so an escaped string is allocated once, up to the body's
length, before its charge is taken; the buffer is reused across strings.

The refusal codes are the ones the pre-scan produced.
[`resident_refusal`][refusal-live] maps a permanent refusal to
[`request_too_large_error`][toolarge-live] and classes a transient one by
[counting the body against the capacity][exceeds] without charging, after the
refused decode released its bytes; the count stops at the value that crosses
the capacity, so a body far above it is not parsed to its end. Above the
capacity it is too large, otherwise
[`resident_capacity_error`][queuefull-live]. The refusal returns from
`dispatch_body` in the lane that saw it, before any dispatch: no
`TransformDispatchTicket`, no `transform_route_channels` entry, no prompt
freeze, page staging, or store row. A transform body the typed decode cannot
decode falls to the tree decode with the meter restarted, so the bytes the
failed decode held cover the tree decode before it charges more; a refusal
does not fall through.

The one behavior that moves: the pre-scan bounded a body before it was parsed,
so a malformed body with enough separators was refused as too large. The floor
keeps that outcome for a malformed body whose value count alone exceeds the
capacity; a malformed body under the floor is refused by the tree decode as
`unrecognized_request_shape` unless the footprint of its well-formed prefix
crosses the capacity first. Every body over 1 MiB is still gated by the byte
cap before either applies.

The [footprint test][t-footprint] shows separators inside strings not counted
as values, a large text block charged for three copies, a scalar-dense body
far above its wire size with string bytes not charged as values, and a cut
body counting its decoded part. The [meter test][t-meter] shows a fitting
decode holding at least its footprint and less than two steps more, a
footprint above the capacity refused as permanent with every byte released,
and a restarted meter reusing held bytes. The [small-body test][t-small] shows
a body under one mebibyte holding at most twice its footprint, and the
[acquisition test][t-acquire] shows a body decoded against a pool with less
than a batch free refused as transient in under a hundred reservation attempts.
The [count test][t-count] shows the capacity-bound count stopping within one
node of the capacity and agreeing with the footprint on either side of it. The
[floor test][t-floor] shows the floor equal to the footprint on string-free
bodies, including nesting to the depth limit, equal to the footprint less the
retained string copies on a body with strings, and the floor refusal agreeing
with the footprint on either side of the capacity; the [corpus floor
test][t-floor-corpus] shows the floor at most the footprint for every
well-formed corpus body. The [doomed body test][t-doomed] runs a paged and an
unpaged body of two hundred thousand values through `dispatch_body` against a
pool of half their footprint and shows the too-large refusal with the pool
never asked and no value counted. The [lane test][t-lanes] runs every
corpus body, one with twenty thousand values under an ignored field among
them, through both lanes with a pool one byte short of its footprint and with
a pool that just fits, and shows one outcome, one counted footprint, and the
too-large refusal on the short pool. The [drained-pool test][t-drain] is the
A3 witness. The [effect test][t-effect] runs a permanent and a transient
refusal through `dispatch_body` and shows the dispatch health counters, the
route channel, and the store row untouched, then the same body served with
room. The [ring test][t-ring] sends, through the direct-host fixture to
`Handler::handle`, a body over the facade cap, one over the transform cap, and
two under both caps whose two million values exceed the scratch pool, one
carrying a page key and one a complete unpaged request, both refused from
their bytes before either decode; each ends in one `host.invalid_params`
terminal, the session's `status` then shows no `pass_trace` and no
`row_version`, and a transform on the same route is served and counted once.
The [peak test][t-peak] shows the footprint covering the tree decode's heap
peak on the dense native corpus, the direct decode's peak at or below it, and
the direct lane's own metered count covering its peak with the values under an
ignored field.

### Focused execution, 2026-09-12

`cargo test -p daemon --locked` passed 1024 tests including the eleven above,
the two `dreamer_run_task_bounds_*` tests failing under full-suite load on the
base branch as well and passing in isolation;
`cargo test -p daemon --locked --features direct-host-fixture --test direct_host`
passed 7 including the ring test;
`cargo test -p daemon --locked --features test-support --test parse_charge_covers_typed_decode`
passed.

[handle-live]: ../../../../../crates/daemon/src/lib.rs#L11899-L11914
[dispatch-body]: ../../../../../crates/daemon/src/lib.rs#L12654-L12692
[refusal-live]: ../../../../../crates/daemon/src/lib.rs#L15713-L15721
[toolarge-live]: ../../../../../crates/daemon/src/lib.rs#L15723-L15728
[queuefull-live]: ../../../../../crates/daemon/src/lib.rs#L15730-L15735
[meter]: ../../../../../crates/daemon/src/metered_decode.rs#L131-L138
[need]: ../../../../../crates/daemon/src/metered_decode.rs#L212-L272
[release]: ../../../../../crates/daemon/src/metered_decode.rs#L196-L202
[decode]: ../../../../../crates/daemon/src/metered_decode.rs#L295-L315
[exceeds]: ../../../../../crates/daemon/src/metered_decode.rs#L328-L338
[floor]: ../../../../../crates/daemon/src/metered_decode.rs#L340-L401
[visitor]: ../../../../../crates/daemon/src/metered_decode.rs#L643-L748
[ignored]: ../../../../../crates/daemon/src/metered_decode.rs#L522-L528
[constants]: ../../../../../crates/daemon/src/metered_decode.rs#L57
[step]: ../../../../../crates/daemon/src/metered_decode.rs#L62
[t-footprint]: ../../../../../crates/daemon/src/lib.rs#L19737-L19771
[t-meter]: ../../../../../crates/daemon/src/lib.rs#L19777-L19810
[t-small]: ../../../../../crates/daemon/src/lib.rs#L19814-L19831
[t-acquire]: ../../../../../crates/daemon/src/lib.rs#L19836-L19864
[t-count]: ../../../../../crates/daemon/src/metered_decode.rs#L911-L939
[t-floor]: ../../../../../crates/daemon/src/metered_decode.rs#L941-L976
[t-floor-corpus]: ../../../../../crates/daemon/src/lib.rs#L19963-L19975
[t-doomed]: ../../../../../crates/daemon/src/lib.rs#L19923-L19958
[t-lanes]: ../../../../../crates/daemon/src/lib.rs#L19644-L19690
[t-drain]: ../../../../../crates/daemon/src/lib.rs#L19870-L19918
[t-effect]: ../../../../../crates/daemon/src/lib.rs#L19980-L20031
[t-ring]: ../../../../../crates/daemon/tests/direct_host.rs#L437-L558
[t-peak]: ../../../../../crates/daemon/tests/parse_charge_covers_typed_decode.rs#L94-L153

[handle]: https://github.com/ahrav/eidnara/blob/9132344/crates/daemon/src/lib.rs#L11805-L11827
[bytecap]: https://github.com/ahrav/eidnara/blob/9132344/crates/daemon/src/lib.rs#L15472-L15488
[footprint]: https://github.com/ahrav/eidnara/blob/9132344/crates/daemon/src/lib.rs#L15427-L15455
[copies]: https://github.com/ahrav/eidnara/blob/9132344/crates/daemon/src/lib.rs#L15408-L15417
[toolarge]: https://github.com/ahrav/eidnara/blob/9132344/crates/daemon/src/lib.rs#L15458-L15463
[queuefull]: https://github.com/ahrav/eidnara/blob/9132344/crates/daemon/src/lib.rs#L15465-L15470
[freeze]: https://github.com/ahrav/eidnara/blob/9132344/crates/daemon/src/lib.rs#L8036-L8037
[routechan]: https://github.com/ahrav/eidnara/blob/9132344/crates/daemon/src/lib.rs#L8045-L8048
[accept]: https://github.com/ahrav/eidnara/blob/9132344/crates/daemon/src/lib.rs#L8066
[ticket]: https://github.com/ahrav/eidnara/blob/9132344/crates/daemon/src/lib.rs#L574-L631
[pageapply]: https://github.com/ahrav/eidnara/blob/9132344/crates/daemon/src/lib.rs#L9425-L9433
[hostpage]: https://github.com/ahrav/eidnara/blob/9132344/crates/daemon/src/lib.rs#L735-L736
[hostpagecheck]: https://github.com/ahrav/eidnara/blob/9132344/crates/daemon/src/lib.rs#L9317-L9323
[settle]: https://github.com/ahrav/eidnara/blob/9132344/crates/daemon/src/lib.rs#L12072-L12087
[testentry]: https://github.com/ahrav/eidnara/blob/9132344/crates/daemon/src/lib.rs#L12484-L12499
[wiremsg]: https://github.com/ahrav/eidnara/blob/9132344/crates/memory-store/src/lib.rs#L126-L143
[wireblock]: https://github.com/ahrav/eidnara/blob/9132344/crates/memory-store/src/lib.rs#L250-L264
[reserve]: https://github.com/ahrav/eidnara/blob/9132344/crates/host-runtime/src/handler.rs#L474-L484
[capacity]: https://github.com/ahrav/eidnara/blob/9132344/crates/host-runtime/src/handler.rs#L486-L491
[outcome]: https://github.com/ahrav/eidnara/blob/9132344/crates/host-runtime/src/handler.rs#L230-L235
[pools]: https://github.com/ahrav/eidnara/blob/9132344/crates/host-runtime/src/runtime.rs#L814-L822
[scratchconst]: https://github.com/ahrav/eidnara/blob/9132344/crates/host-runtime/src/config.rs#L21-L31
[try-charge]: https://github.com/ahrav/eidnara/blob/9132344/crates/host-runtime/src/wire.rs#L430-L442
[paging]: https://github.com/ahrav/eidnara/blob/913234433ae36a80a6e22c6aac14c7f9aab74386/packages/opencode-plugin/src/hooks/context/module-wire.ts#L635-L640
[plugin]: ../../../../../packages/opencode-plugin/src/hooks/context/rust-mode-transform.ts#L759-L761
[fixture]: https://github.com/ahrav/eidnara/blob/9132344/crates/daemon/tests/direct_host.rs#L285-L290
[wire63]: https://github.com/ahrav/eidnara/blob/9132344/docs/host-wire-protocol.md#L308
[wire751]: https://github.com/ahrav/eidnara/blob/9132344/docs/host-wire-protocol.md#L440
