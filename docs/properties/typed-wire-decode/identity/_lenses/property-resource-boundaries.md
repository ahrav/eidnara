# Property lens: resource boundaries

Date: 2026-09-13. HEAD: `2e4433e6b511ae74944df8a9669c428e73915d29`.
Baseline: `e451a2b470ae8663b4613ca04f019a30b6d7df53`.
External leads: plan R8-R11, latency-audit A1/A3, and invalidated W1.

## Finding

The identity basis includes serialized lengths (`crates/daemon/src/transform.rs:198-200`)
and historian lengths (`crates/daemon/src/historian_chunk.rs:426-430`).
Lengths must be exact UTF-8 byte counts, independently of admission charges.

## Candidate

Include non-ASCII and escaped content in `block-byte-consumers-share-canonical-basis`
and `historical-chunks-retain-readable-identity`. Check byte length rather
than character count or allocation capacity.

## Narrow nonapplicability

Retained-copy accounting, allocation ceilings, and scratch reservation belong
to sibling decode work. W1 is invalidated and has no measurement ownership.
The targeted byte-policy report adds exact token/fold/protection preservation,
not a performance property or benchmark. The synthetic tool omission is exactly
26 compact bytes per false member with its comma, not a memory saving bound.
