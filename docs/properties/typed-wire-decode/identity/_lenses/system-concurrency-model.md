# System lens: concurrency model

Date: 2026-09-13. HEAD: `2e4433e6b511ae74944df8a9669c428e73915d29`.
Baseline: `e451a2b470ae8663b4613ca04f019a30b6d7df53`.
External leads: plan mutation paths and issue 438, read as design context.

## Observations

- `crates/daemon/src/wire.rs:540-559,781-784` shares immutable message shells
  through `Arc` and addresses a block by index.
- `crates/memory-store/src/lib.rs:203-208,306-310` invalidates originals on
  mutable access. Unchanged siblings retain independent originals at HEAD.
- `crates/daemon/src/transform.rs:171-191` builds its fallback digest index
  in a request-local `OnceCell`; it is not a shared global identity registry.
- `crates/daemon/src/lib.rs:2947-2948,3687-3688` owns output/native caches
  behind mutexes. These line references come from HEAD, not the dirty worktree.

## Contract and candidate

Mutation of one owned clone must not change a shared source or sibling bytes.
Digest candidate selection must preserve positional precedence and equality
rechecks. Ownership topology does not make byte equality automatic.

## Narrow nonapplicability and missing evidence

No new atomic protocol, lock order, or thread-scheduling mechanism is proposed
in this identity slice. Blocking-pool relocation belongs to issue 438.
No concurrent mutation execution is claimed; alias and clone cases need witnesses.
