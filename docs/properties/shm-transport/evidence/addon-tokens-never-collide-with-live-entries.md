# addon-tokens-never-collide-with-live-entries

## Discovery trigger

Round 17 review of the PR: `allocate_token` issues every JavaScript-facing
identity in the addon and its test checks wraparound and collision avoidance, but
no record required those identities to avoid live entries.

## Evidence trail

- `packages/shm-native/src/lib.rs:435-446`: `allocate_token` advances a counter
  with `wrapping_add` (`:440`), skips zero and any key present in `in_use`
  (`:441`), and bounds itself to `in_use.len() + 2` attempts (`:438-439`); the
  doc comment gives the counting argument for why that bound always reaches a
  free token.
- Callers: channel ids in `insert_channel` (`:427`), pending setups (`:872`),
  producer reservations (`:1179`), receive leases (`:1472`). Each inserts the
  returned token into the table it was checked against.
- Test: `token_allocation_wraps_and_skips_outstanding_tokens` (`:1294-1309`)
  positions the counter at `u32::MAX - 1` with `u32::MAX` and `1` live and
  asserts `2` then `3`; a fresh counter issues `1`.

## Failure scenario

After wraparound a bare increment reissues a live token; the insertion replaces
the live entry, and the JavaScript handle that held the old token now releases
or closes the new resource. No error is raised on either side.

## Timing windows and dependencies

None; single-threaded registry access under the addon's `RefCell`.

## What a test must construct

A counter just below wraparound with outstanding entries at the revisited
values, present at unit level. Exhausting a table through the public API needs
`u32::MAX` allocations and is not a practical test.

## Investigation log

### Q: Is every JavaScript-facing identity issued through this function?

- Sources examined: every `insert` into `registry.channels`, `registry.pending`,
  `channel.producers`, and `channel.active` in `lib.rs`.
- Findings: all four go through `allocate_token`.
- Missing evidence: none.
- Conclusion: the guarantee covers every token the addon hands out.
