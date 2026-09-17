# packing-heuristic-counts-never-carry-the-exact-label

## Discovery trigger

RP2.8 acceptance row AC7 requires any heuristic count to carry its authority
and headroom and never an exact label.

## Evidence trail

- `crates/daemon/src/packing/accounting.rs` `Authority` distinguishes `Exact`
  from `Heuristic { degradation, headroom_permille }`; `Charge` has private
  fields and is produced only by `AccountingProfile::charge`, which copies the
  profile's authority; `with_headroom` rounds the headroom up and adds nothing
  for `Exact`.
- The `compile_fail` doctest on `Charge` shows the struct literal rejected
  with the private-field error.
- `crates/daemon/tests/packing_accounting.rs` builds a heuristic profile and
  reads its charges' authority and headroom, and shows a heuristic that names
  the exact identity and vocabulary digest cannot obtain an exact-labelled
  count through the shared cache.

## Failure scenario

A bytes-over-four estimate labeled exact would pass a whole-invocation check
with zero headroom and overrun the provider limit by its error.

## Timing windows and dependencies

None.

## What a test must construct

- A heuristic profile with a named degradation and non-zero headroom.
- An attempt to construct a `Charge` directly.
