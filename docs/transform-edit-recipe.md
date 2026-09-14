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
- a value outside the `serde_json` domain, including non-finite numbers,
  unpaired UTF-16 surrogates, or more than 127 nested containers;
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

Reconstruction produces a new array of shared references and literal values,
each entry's canonical length for use as a later `previous` source, and the
array's measured canonical length. It never edits a source or splices the host
array in place.

Both appliers measure literals with the compact `serde_json` rule. The daemon
keeps a number parsed from `1.0` as a float and re-emits `1.0`; JavaScript has
already collapsed it to `1`. Acceptance still agrees, but sizes for non-integer
numeric content can differ by a few bytes, so a caller must not treat the two
measurements as interchangeable at the exact limit.

## Shared fixtures

`crates/daemon/tests/fixtures/transform-edit-recipe-v1.json` holds the cases
both languages must agree on. `crates/daemon/tests/edit_recipe_fixtures.rs` and
`packages/opencode-plugin/src/hooks/context/edit-recipe.test.ts` read the same
file. Each case states whether the recipe is accepted and, when it is, the exact
reconstructed output and its canonical length. Rejection reasons are not
standardized across languages; the TypeScript test pins its own code per case.
`crates/daemon/tests/edit_recipe_generated.rs` adds seeded generated valid plans
and single-fault mutations against an independent model.
