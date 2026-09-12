# route-and-typed-decode-are-independent-of-entry-path

Baseline: `913234433ae36a80a6e22c6aac14c7f9aab74386`, 2026-09-10.
The [scope and provenance](../catalog.md#scope-and-provenance) apply here.
The discovery and investigation sections describe that baseline. Their source
links are pinned to it. The implementation evidence below describes the live code.

## Discovery trigger

The audit proposes replacing the `Value` parse plus `from_value` pair with a
direct `from_slice::<TransformRequest>` on the unpaged lane. Routing today reads
the raw `Value`, and both lanes reach the same typed decode from a `Value`. A
replacement on one lane can derive the lane from a typed field, treat a `null`
page field as absent, or report duplicate keys and malformed JSON differently,
so the same body decodes two ways depending on which lane carried it.

## Evidence trail

- [`dispatch_value_with_inbound_bytes`][dispatch] reads `method` with
  `Value::as_str`, falls back to `kind` with `as_str`, so a non-string
  discriminator is absent. A body with `name` and `arguments` and no
  discriminator goes to the facade; anything else reaches
  [`unrecognized_request_error`][unrecognized], which reports
  `non-object JSON (<type>)` for a non-object root. A malformed body arrives as
  `Value::Null` from [`handle`][handle], so it reports `non-object JSON (null)`
  ([`json_type_name`][typename]).
- [`handle_transform_dispatch`][tdispatch] splits on
  [`has_transform_page_fields`][pagefields], which is `request.get(field)
  .is_some()` over the six [`TRANSFORM_PAGE_FIELDS`][pageconst]; a `null` value
  is present by key and selects the page lane.
- The unpaged lane reads [`request_observed_at_ms`][observed] from the raw
  `Value`, then runs [`from_value::<TransformRequest>`][fromvalue] with any
  error as `bad_request`. The page lane assembles a `Value` and calls the same
  function through [`handle_transform_unpaged_value`][pageapply].
- [`TransformRequestWire`][wirestruct] has `#[serde(default)] kind: String`,
  no `method` field, and no `deny_unknown_fields`; `session_id` and
  `render_config` are the only fields without a default. The
  [`Deserialize for TransformRequest`][typed-decode] copies `kind` without
  validating it. `Option` fields take `null` as `None`; defaulted non-`Option`
  fields reject `null`.
- [`WireMessage`][wiremsg] and [`WireBlock`][wireblock] each decode a `Value`
  first and retain it, so the typed decode is a `Value` round trip at every
  level, not only at the top.
- Over 1 MiB, [`enforce_request_byte_cap`][bytecap] runs the
  [`RequestMethodProbe`][probe], whose [`ProbeString`][probestr] keeps only a
  string of at most 64 bytes and skips arrays, objects, numbers, booleans, and
  `null`; [`is_transform_class`][class] reads `method` then `kind` and admits
  `transform`, `state_sync`, and `kernel.artifact.ingest.page`. The ceiling
  `body.len() <= MAX_TRANSFORM_FRAME_BYTES` is inclusive at 32 MiB.
- Tests enter at [`dispatch_value_for_test`][testentry] with a built `Value`.
  The `Value` map's last-wins behavior on duplicate keys and serde_derive's
  `duplicate field` error are cited from upstream source in the lens
  ([`Map::insert`][mapinsert], [serde_derive][derivedup]); neither ran here.

## Failure scenario

A direct typed decode derives the lane from `TransformRequest.kind`, so
`{"method":"transform","kind":"x"}` routes differently than the `Value` read.
It treats `{"kind":"transform","transform_page_id":null}` as unpaged where the
key test selects the page lane. It returns `duplicate field` where the round
trip keeps the last value. It reports malformed JSON as `bad_request` where the
`Value::Null` path reports `unrecognized_request_shape`. Each divergence is
silent: the client sees a different code or a different accepted request
depending on lane.

## Timing windows and dependencies

None in time. The dependencies are the two upstream behaviors (map insert
replacement, derived-struct duplicate detection) and the fact that both lanes
share one typed decode function today.

## What a test must construct

A fixed corpus fed through both lanes: `{"method":0,"kind":"transform"}`,
`{"method":"transform","kind":"x"}`,
`{"kind":"transform","transform_page_id":null}`, `{"name":"x","arguments":{}}`,
duplicate `method` and duplicate `session_id` keys, unknown fields, `null` on
`Option` and defaulted fields, missing `session_id`, malformed JSON, numeric
edge cases; bodies over 1 MiB with string, numeric, array, object, and 2 MiB
`method`; bodies of exactly 32 MiB and 32 MiB plus one. Compare route, code,
and the decoded `TransformRequest` between `decode_unpaged`, `decode_via_value`,
and [assembled pages][pageapply]. The
[ingress checks](../existing-checks.md#ingress-admission-and-decode)
cover `kind` routing, the unrecognized shape, the probe class set, and one
full envelope; none covers a `null` page field, duplicates, or malformed JSON
on either lane.

The reference is a frozen test-only copy of HEAD's routing read, probe, and
`Value` decode kept under `crates/daemon/tests/` on the pattern of
`historian_truncate_differential.rs:13-58`, or a recorded corpus of expected
route, code, and decoded request per body. The live reader cannot remain the
oracle once the change replaces it.

## Investigation log

### Q: Is last-wins on duplicate top-level keys a contract or an accident?

- Sources examined: [`from_value`][fromvalue],
  [`TransformRequestWire`][wirestruct],
  [§7.5.1][wire751], upstream [`Map::insert`][mapinsert].
- Findings: Nothing in the context module states a duplicate-key rule; the
  behavior follows from `Value` construction. Synapse rejects duplicates by
  contract. The probe refuses duplicates over 1 MiB, so the two size classes
  already differ.
- Missing evidence: A written rule for routed bodies.
- Conclusion: needs human input.

### Q: Must malformed JSON keep reporting `unrecognized_request_shape`?

- Sources examined: [`handle`][handle] (`unwrap_or(Value::Null)`),
  [`unrecognized_request_error`][unrecognized], [§6.3][wire63].
- Findings: The code is a consequence of mapping parse failure to `Null`; the
  wire contract leaves routed bodies opaque and names no code for this case.
- Missing evidence: A statement of the intended code.
- Conclusion: needs human input.

### Q: Do numeric edge cases decode equivalently on both paths?

- Sources examined: the `u64`, `usize`, and `f64` fields of
  [`TransformRequestWire`][wirestruct].
- Findings: Integers above `u64::MAX`, exponents, and `-0` pass through
  `Value` number normalization before the typed decode today; a direct decode
  would hand serde the token. No differential ran.
- Missing evidence: A differential run over the numeric corpus.
- Conclusion: unresolved, needs a differential run.


## Direct-decode evidence

Implementation base: `96709d0ef54bcfad2327878ab96e118fb8ba4969` plus the units
that precede it on the branch.
Preservation authority: [implementation ticket](https://github.com/ahrav/eidnara/issues/435)
and [parent specification](https://github.com/ahrav/eidnara/issues/350).

[`Handler::handle`][handle-live] reads one [entry probe][probe-live] from the
body: the `method` and `kind` discriminators and whether any
[`TRANSFORM_PAGE_FIELDS`][pageconst] key is present. A body over 1 MiB is
probed by the byte cap before the resident reservation, as it always was; a
body at or under 1 MiB is admitted by its length and probed after the
reservation, so no parse runs before it, as before this change; only a
discriminator the body spells literally selects the direct lane (the
[route read][route-name] keeps an escaped spelling apart, still widening the
cap with it), so the later [lane probe][lane-probe] runs only when the body's
bytes spell `transform` somewhere; a facade body pays its `Value` parse and
nothing more, as before the direct lane, and a body naming the route through
an escaped discriminator takes the tree lane whatever else its bytes spell. The probe's
one body-proportional cost is serde_json's unescape buffer for a long escaped
key or discriminator, so this ordering keeps that buffer inside admission
control for every body the cap does not need to classify. The probe's
[map visitor][probe-visitor] mirrors the tree dispatch rather than a derived
struct: a repeated key keeps its last value, a page key counts when present
whatever its value, `null` included, each key is [classified in place][probe-key]
so no key text is retained however long it is, and every other value is skipped
through `IgnoredAny`, the byte scan the byte-cap probe always used. The probe
selects the lane and the byte-cap class; it does not decide whether the tree
parses the body, so a body it routes to the direct lane is not admitted there on
the probe alone.

The same probe serves the byte cap: [`enforce_request_byte_cap`][cap-live]
returns the probe it read for the class, so a body over 1 MiB is probed once,
and it keeps the [class read][class-live] (an overlong `method` reads as absent for the
widening while [dispatch's read][route-resolve] takes any string `method` as
the route, so that body is admitted and then refused by shape). A body over
1 MiB whose discriminator the probe scans past a value the tree cannot parse is
still admitted under the wider cap and refused by shape, as before; its refusal
code does not move to the 1 MiB cap.

[`dispatch_body`][dispatch-body] is the one branch. When the probe names the
`transform` route with no page key, the body is first walked by
[`tree_decode_parses`][witness], which skips every value through
[`deserialize_any`][skipped] so every check serde_json applies to a `Value`
parse applies to the walk: the nesting limit, number range, string escapes, and
UTF-8. The walk also refuses any object holding the
[raw-value token][raw-token] as a key, in any position: serde_json's
`raw_value` feature, on in this workspace, makes a `Value` parse read an object
whose first key is the token as one boxed raw document (the value must be a
string holding a document and no key may follow), while a derived struct skips
it as an unknown field, so without the rule the direct decode accepted
`{"kind":"transform",...,"x":{"$serde_json::private::RawValue":1}}` where the
tree refused it. The tree lane also re-reads every retained `Value` (a
`tail_delta`, a `native_messages` element, a wire message's `original`) through
`from_value`, where the tree's map presents its keys sorted and the token sorts
before any letter, so a body that placed the token after another key under such
a field parses as a tree and is then refused with `bad_request` while the
direct decode accepts it; refusing the token at every key sends those bodies
down the tree lane too. A `true` from the walk is therefore proof that the tree
decode parses the body. Only then does the body decode with
`serde_json::from_slice::<TransformRequest>` and enter the
[direct lane][direct-lane]; on any decode error the body falls through to the
`Value` parse and the tree dispatch, so a refusal is always the tree decode's
refusal with the tree decode's message. Every other body takes the tree path
unchanged and pays only the byte scan for the lane choice. The branch reports
the lane it took, which the handler discards and the tests assert. The
[tree-decoded unpaged lane][tree-lane] and the [page apply][page-apply-live]
both decode their `Value` with `from_value` and enter the same
[typed handler][typed-entry] the direct lane enters; the
`request_observed_at_ms` read moved from the raw `Value` to the typed field,
which decodes to the same value wherever the typed decode succeeds, and
`request_observed_to_handler` still ends where the typed decode starts, the
[boundary test][t-observed] holding a decode begun ten seconds earlier out of
the span. The `handler_total` timing starts before the typed decode on both
lanes; on the direct lane that decode reads the body bytes, so the direct lane's
`handler_total` includes the byte parse that the tree lane's `Value` parse
precedes. The walk's only body-proportional cost is serde_json's scratch buffer
for one escaped string at a time, released with the walk, and it runs after the
resident reservation. The [byte-scan bound][copies-live] is retained and gains one
term: the direct decode retains what the tree lane retains, but serde_json
unescapes a string holding an escape into a scratch buffer that the direct
lane's deserializer keeps beside those copies until `from_slice` returns, where
the tree lane's `Value` parse releases it before `from_value` builds the typed
copies. The bound charges the longest escaped string at
[`UNESCAPE_SCRATCH_SLACK`][scratch-live] for that buffer's doubling growth.
The [peak test][t-peak] measures the direct decode below the tree decode's peak
on the dense native corpus, and the [escaped peak test][t-peak-escaped]
measures it within the charge on a 4 MiB text block holding one escape, where
before the term it peaked four copies against a three-copy charge.
[`handle_transform_for_test`][test-entry] serializes its request and enters at
the body branch, so the crate's transform tests run the direct lane.

The [corpus][t-corpus] holds 44 bodies: a valid body under `kind` and under
`method`, an unknown top-level field, `null` on an `Option` and on a defaulted
field, a wrong type, a float, an exponent, a negative and an above-`u64`
integer on integer fields, `-0` on a float field, a repeated top-level key, a
repeated discriminator, a repeated nested key, a missing required field, a
missing and an unknown serializer profile, a `null` and a lone page field, a
non-string and an overlong `method` beside `kind`, an escaped discriminator
alone and beside a field whose text is `transform`, another route, trailing
bytes, malformed JSON, array, string and empty bodies, `messages` as an object,
an out-of-range number, a lone surrogate and invalid UTF-8 under an ignored
field, the raw-value token opening an object under an ignored field, with a
sibling key, inside an ignored array, under the discriminator, holding a
document string, after another key under an ignored field, under `tail_delta`,
in a `native_messages` element, and inside a message, and nesting at and one
past the tree's depth limit. The [entry differential][t-entry-diff] runs every body through
`dispatch_body` and through the tree dispatch on two identical handlers and
asserts the same response with the timing block removed, or the same code and
message; it pins the nine bodies that took the direct lane and asserts the valid
body was served. The [decode differential][t-decode-diff] asserts for every
body that the walk implies the tree parses it, that where both decodes accept a
body they produce the same request (pinning the eighteen such bodies, with `-0`
compared by bit pattern), pins the bodies only the tree accepts to the three
repeated-key shapes (the derive refuses a repeated field; the handler's
fallback carries them), pins the bodies only the direct decode accepts to the
eight derive-leniency shapes under an ignored field or the discriminator and
the two retained-value shapes holding the token after another key, and asserts
the walk refuses each first, and decodes a two-page assembly of the
valid body to the one-slice request. The [probe test][t-probe] checks each page
key with `null`, the discriminator fallbacks, last-wins, the overlong-`method`
split between cap and route, that an over-deep body probes while the walk
refuses it, and that an escaped discriminator widens the cap but never selects
the direct lane, with the lane probe skipped when no byte spells `transform`. The [walk test][t-witness] checks the walk against a `Value` parse
on scalar, array, string, out-of-range, lone-surrogate, trailing, malformed, and
empty bodies and pins the raw-value rule, including the shapes the tree
accepts and the walk declines (the token holding a document string, and the
token after another key, which the tree's `from_value` re-read of that object
refuses; both then take the tree lane). The [token test][t-token] holds the pinned literal to
serde_json's behavior. The [cap test][t-cap-live] adds a body of exactly
`MAX_TRANSFORM_FRAME_BYTES` admitted, one byte more refused, and a 2 MiB
transform-class body nested past the depth limit still admitted. The
[allocation test][t-cap-alloc] passes a 900 KiB body whose one key holds an
escape through the byte cap and measures less than half the body's size
allocated; before the ordering above it measured the key's length.

The numeric open question is answered by the decode differential over the
corpus, not by construction: the tree lane parses numbers into `Value` and
`from_value` hands them to the typed visitors, while the direct lane hands the
typed visitors the token, and on the corpus a float or exponent on an integer
field, an integer above `u64::MAX`, a negative on an unsigned field, and `-0`
on a float field gave the same accept or refusal on both, `-0` compared by bit
pattern. The
repeated-key question stays open as a contract question; the behavior is
unchanged, last-wins through the tree.

### Focused execution, 2026-09-12

`cargo test -p daemon --locked` passed 1018 tests including the six above, the
two `dreamer_run_task_bounds_*` tests failing under full-suite load on the base
branch as well and passing in isolation;
`cargo test -p daemon --locked --features test-support --test parse_charge_covers_typed_decode`
passed; `cargo test -p daemon --locked --features direct-host-fixture --test direct_host`
passed 6, including the real unary transform through `Handler::handle`;
`bun scripts/verify-serialized-transform-pages.ts` in `packages/e2e-tests`
generated the 19-case serialized corpus and passed
`serialized_transform_corpus_preserves_host_admission_and_completion` through
the direct-host fixture, which compares the paged final `messages` bytes to the
one-slice control.

### Review follow-up, 2026-09-12

Two automated review findings on the merged head were reproduced with the
counting allocator before any production edit, then closed by the edits above.
`parse_charge_covers_escaped_text_direct_decode_peak` failed at
`4194304 text bytes (escaped: true) peaked at 16782827 bytes during the direct
decode but the parse charge reserved only 12589901`, and passes with the
scratch term. `byte_cap_admits_a_facade_sized_body_without_body_proportional_allocation`
failed at `the byte cap allocated 921610 bytes before the resident reservation
for a 921627 byte body` against a signature-only refactor of the merged head,
and passes with the cap probing only bodies over 1 MiB. Before the scratch term the
tree lane's peak on that message shape measured 54 to 57 bytes above the
charge at 64 KiB, 1 MiB, and 4 MiB of text, with and without the escape; the
overage is in the fixed headroom, predates this change, still stands for the
unescaped shape, and is recorded here, not fixed.
A third finding named the sorted re-read above: three corpus bodies placing
the token after another key under `tail_delta`, in a `native_messages`
element, and inside a message were added, and the entry differential failed on
the first with the direct lane serving `need_full_sync` where the tree dispatch
returned `bad_request` (`invalid type: integer `1`, expected raw value`); the
walk now refuses the token at every key and both differentials pass with the
lists re-pinned.
A fourth finding named a moved timing boundary: `request_observed_to_handler`
sampled its clock after the typed decode where it had sampled before the
`from_value` call. `request_observed_to_handler_ends_before_the_typed_decode`
failed at `request_observed_to_handler was 20000 ms; the decode span of 10 s
must not be counted in it` and passes with the sample backed to the decode
start.
`cargo test -p daemon --locked --no-fail-fast` then passed every test but the
two `dreamer_run_task_bounds_*` tests, which fail under full-suite load on the
base as well, and `publication_search_deadline_preserves_admission_without_recharging`,
which fails the same way on the base commit `d42838e3` alone.

Before the raw-value rule and the split into probe and walk, the six raw-value
corpus bodies failed both differentials: the body with the token under an
ignored field was served through `dispatch_body` on the direct lane while the
tree dispatch refused it with `unrecognized_request_shape`, and the probe
accepted it where the tree did not parse it. The rule and the split were added
against those failures.

[handle-live]: ../../../../../crates/daemon/src/lib.rs#L11910-L11930
[dispatch-body]: ../../../../../crates/daemon/src/lib.rs#L12667-L12689
[lane-probe]: ../../../../../crates/daemon/src/lib.rs#L15473-L15481
[route-name]: ../../../../../crates/daemon/src/lib.rs#L15685-L15702
[probe-live]: ../../../../../crates/daemon/src/lib.rs#L15463-L15471
[witness]: ../../../../../crates/daemon/src/lib.rs#L15483-L15486
[raw-token]: ../../../../../crates/daemon/src/lib.rs#L15453-L15460
[probe-visitor]: ../../../../../crates/daemon/src/lib.rs#L15497-L15530
[probe-key]: ../../../../../crates/daemon/src/lib.rs#L15542-L15566
[skipped]: ../../../../../crates/daemon/src/lib.rs#L15569-L15656
[route-resolve]: ../../../../../crates/daemon/src/lib.rs#L15659-L15665
[class-live]: ../../../../../crates/daemon/src/lib.rs#L15675-L15682
[cap-live]: ../../../../../crates/daemon/src/lib.rs#L15894-L15917
[direct-lane]: ../../../../../crates/daemon/src/lib.rs#L7976-L7988
[tree-lane]: ../../../../../crates/daemon/src/lib.rs#L7992-L8012
[typed-entry]: ../../../../../crates/daemon/src/lib.rs#L8019
[page-apply-live]: ../../../../../crates/daemon/src/lib.rs#L9531
[copies-live]: ../../../../../crates/daemon/src/lib.rs#L15790
[test-entry]: ../../../../../crates/daemon/src/lib.rs#L8606-L8616
[t-probe]: ../../../../../crates/daemon/src/lib.rs#L19402-L19461
[t-witness]: ../../../../../crates/daemon/src/lib.rs#L19466-L19496
[t-token]: ../../../../../crates/daemon/src/lib.rs#L19501-L19513
[t-corpus]: ../../../../../crates/daemon/src/lib.rs#L19515-L19709
[t-decode-diff]: ../../../../../crates/daemon/src/lib.rs#L19720-L19859
[t-entry-diff]: ../../../../../crates/daemon/src/lib.rs#L19877-L19920
[t-observed]: ../../../../../crates/daemon/src/lib.rs#L21513-L21541
[t-cap-live]: ../../../../../crates/daemon/src/lib.rs#L19323-L19400
[t-peak]: ../../../../../crates/daemon/tests/parse_charge_covers_typed_decode.rs#L89-L127
[t-peak-escaped]: ../../../../../crates/daemon/tests/parse_charge_covers_typed_decode.rs#L143-L164
[t-cap-alloc]: ../../../../../crates/daemon/tests/parse_charge_covers_typed_decode.rs#L166-L187
[scratch-live]: ../../../../../crates/daemon/src/lib.rs#L15792-L15793

[handle]: https://github.com/ahrav/eidnara/blob/9132344/crates/daemon/src/lib.rs#L11805-L11827
[bytecap]: https://github.com/ahrav/eidnara/blob/9132344/crates/daemon/src/lib.rs#L15472-L15488
[probe]: https://github.com/ahrav/eidnara/blob/9132344/crates/daemon/src/lib.rs#L15311-L15323
[probestr]: https://github.com/ahrav/eidnara/blob/9132344/crates/daemon/src/lib.rs#L15330-L15393
[class]: https://github.com/ahrav/eidnara/blob/9132344/crates/daemon/src/lib.rs#L15395-L15406
[dispatch]: https://github.com/ahrav/eidnara/blob/9132344/crates/daemon/src/lib.rs#L12558-L12652
[pagefields]: https://github.com/ahrav/eidnara/blob/9132344/crates/daemon/src/lib.rs#L12654-L12658
[unrecognized]: https://github.com/ahrav/eidnara/blob/9132344/crates/daemon/src/lib.rs#L12674-L12698
[typename]: https://github.com/ahrav/eidnara/blob/9132344/crates/daemon/src/lib.rs#L12700-L12709
[pageconst]: https://github.com/ahrav/eidnara/blob/9132344/crates/daemon/src/lib.rs#L745-L752
[tdispatch]: https://github.com/ahrav/eidnara/blob/9132344/crates/daemon/src/lib.rs#L7887-L7903
[observed]: https://github.com/ahrav/eidnara/blob/9132344/crates/daemon/src/lib.rs#L7926-L7932
[fromvalue]: https://github.com/ahrav/eidnara/blob/9132344/crates/daemon/src/lib.rs#L7933-L7941
[pageapply]: https://github.com/ahrav/eidnara/blob/9132344/crates/daemon/src/lib.rs#L9425-L9433
[testentry]: https://github.com/ahrav/eidnara/blob/9132344/crates/daemon/src/lib.rs#L12484-L12499
[wirestruct]: https://github.com/ahrav/eidnara/blob/9132344/crates/daemon/src/transform.rs#L801-L972
[typed-decode]: https://github.com/ahrav/eidnara/blob/9132344/crates/daemon/src/transform.rs#L909-L972
[wiremsg]: https://github.com/ahrav/eidnara/blob/9132344/crates/memory-store/src/lib.rs#L126-L143
[wireblock]: https://github.com/ahrav/eidnara/blob/9132344/crates/memory-store/src/lib.rs#L250-L264
[wire63]: https://github.com/ahrav/eidnara/blob/9132344/docs/host-wire-protocol.md#L308
[wire751]: https://github.com/ahrav/eidnara/blob/9132344/docs/host-wire-protocol.md#L440
[mapinsert]: https://docs.rs/serde_json/1.0.151/src/serde_json/map.rs.html#127-129
[derivedup]: https://docs.rs/serde_derive/1.0.229/src/serde_derive/de/struct_.rs.html#269
