# Transform edit recipe

This document describes the application contract that a successful `transform`
response carries in place of a complete output array. It does not change the
host wire protocol: framing, versioning, authentication, and body limits stay as
[`host-wire-protocol.md`](host-wire-protocol.md) defines them, and generic host
clients transport the recipe as opaque JSON.

Source: the Transform Edit Responses specification
([#525](https://github.com/ahrav/eidnara/issues/525)).

## Two sources, two operations

A recipe rebuilds the daemon's approved output from whole messages taken from two
sources plus complete literal values:

- `input` is this request's complete selected-representation input. For CK it is
  `request.messages[i].ck`; for native output it is the complete captured
  `native_messages` array.
- `previous` is one previously applied output the caller advertised.

Operation order is output order. Omission deletes. There is no implicit trailing
keep, nested path, replace or move opcode, or string offset.

```json
{
  "base_revision": "opaque-input-snapshot-token",
  "output_revision": "opaque-output-token",
  "previous_output_revision": "opaque-previous-output-token",
  "operations": [
    { "op": "keep", "source": "previous", "start": 0, "count": 2 },
    { "op": "insert", "values": [{ "...": "one complete message" }] },
    { "op": "keep", "source": "input", "start": 3, "count": 1 }
  ]
}
```

`previous_output_revision` is present exactly when some operation keeps from
`previous`.

## Revisions

Revision tokens are opaque, nonempty strings of at most 128 UTF-8 bytes. They
are neither hashes nor authorization. `base_revision` names the exact input
snapshot the recipe was built for; `output_revision` names exactly one final
ordered array; `previous_output_revision` echoes the caller's advertised
revision. Row versions, boundary versions, ordinals, and input fingerprints are
not revision tokens.

## Validation

An applier rejects the whole recipe, and publishes nothing, when any of these
fail:

- unknown `op` or `source`, a missing required field, or an unknown field on an
  operation;
- `start` or `count` whose numeric value is not a nonnegative integer at or
  below 2^53 - 1. The check is on the value, not the lexical form: JavaScript
  cannot tell `1.0` from `1`, so `1.0`, `1e3`, and `-0` read as integers in
  both languages;
- `count` of zero, or an `insert` with no values;
- a `keep` range that repeats, overlaps, or moves backward within its source.
  Cursors advance independently per source;
- a range end past the source, computed with checked arithmetic;
- `base_revision` that does not name the held input, a `previous` keep without a
  held previous output, or a `previous_output_revision` that does not name it;
- a reconstructed array whose canonical JSON length, counting brackets and
  commas, exceeds 64 MiB. Kept entries contribute lengths recorded when they
  were captured or serialized; literals are measured once with the compact
  `serde_json` rule. This limit is checked before allocation and separately
  from the wire frame limit.

Reconstruction produces a new array of shared references and literal values
together with its measured canonical length. It never edits a source or splices
the host array in place.

Both appliers measure literals with the compact `serde_json` rule. The daemon
keeps a number parsed from `1.0` as a float and re-emits `1.0`; JavaScript has
already collapsed it to `1`. Acceptance still agrees, but sizes for non-integer
numeric content can differ by a few bytes, so a caller must not treat the two
measurements as interchangeable at the exact limit.

## Building a recipe

The daemon builds a recipe from the final approved array after every policy,
codec, healing, and cleanup pass. Provenance keys (cache keys, fingerprints,
message identities) only nominate candidates; every keep is confirmed by full
value equality of the selected representation. For each output message the
builder prefers an equal `previous` message, then an equal `input` message, and
otherwise inserts the complete value. Cursors advance independently per source,
so a message that repeats or moves backward relative to its source's last keep
becomes a literal. Each output message compares against at most
`MAX_CONFIRM_PROBES` (8) same-key candidates at or after the cursor; a match
further out also becomes a literal. That bound holds for any key distribution,
so a key shared by many differing messages costs a longer recipe, never a
quadratic build. Near-unique keys keep every reachable match. Adjacent keeps of
one source and adjacent inserts coalesce. The recipe names
`previous_output_revision` only when a `previous` keep was used.

## Wire integration

A `transform` request names its input snapshot with `base_revision`; the daemon
refuses a request without one (`transform_base_revision_missing`). A request
may also carry `previous_output_revision`, the `output_revision` of the last
recipe the caller applied for this session. The daemon offers that output as
the `previous` source only when the revision the caller names is the one it
retained; otherwise the recipe addresses the input alone.

A `status: ok` response carries `base_revision`, a fresh `output_revision`,
`operations`, and `previous_output_revision` when a `previous` keep was used.
The served CK array and the native array never cross the wire as whole
arrays: the response has no `messages`, `native_messages`, or native suffix
field. `need_full_sync` carries neither a recipe nor an output revision and
cannot be applied. An `ok` response without `operations` is an invalid recipe
on both sides: the daemon refuses to emit one
(`transform_recipe_omitted`), and the client treats one it receives as a
failed pass, nacks that attempt's deliveries, and serves the input unchanged.
Recomputed output gets a new revision; the daemon allocates revisions as
`<pid>-<start>-<counter>` and refuses a pass when the counter is exhausted
(`transform_output_revision_exhausted`).

Inserted literals are written straight into the response body from the
daemon's retained values against lengths measured when they were encoded, and
kept messages contribute no literal bytes. The frame limit and the 64 MiB
reconstructed-array limit stay independent
(`transform_output_too_large`).

The OpenCode plugin measures the canonical length of each submitted native
message once, reuses the acknowledged prefix's lengths on delta passes, and
applies the recipe against the complete captured native array. The applied
output, its lengths, and its revision are retained per session under a
separate 64 MiB optional-output budget with least-recently-retained eviction;
the 64-session wire cache bound still applies. Eviction, refusal, wire
invalidation, and session clear drop only the `previous` source: the next pass
still applies, from the input alone.

## Shared fixtures

`crates/daemon/tests/fixtures/transform-edit-recipe-v1.json` holds the cases
both languages must agree on. `crates/daemon/tests/edit_recipe_fixtures.rs` and
`packages/opencode-plugin/src/hooks/context/edit-recipe.test.ts` read the same
file. Each case states whether the recipe is accepted and, when it is, the exact
reconstructed output and its canonical length. Rejection reasons are not
standardized across languages; the TypeScript test pins its own code per case.
`crates/daemon/tests/edit_recipe_generated.rs` adds seeded generated valid plans
and single-fault mutations against an independent model, and checks that a
built recipe round-trips through the applier with coalesced operations.
