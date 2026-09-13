# Property lens: concurrency

Date: 2026-09-13. HEAD: `2e4433e6b511ae74944df8a9669c428e73915d29`.
Baseline: `e451a2b470ae8663b4613ca04f019a30b6d7df53`.
External lead: latency-audit B1/B2 and the plan's mutation section.

## Finding

Shared immutable shells at `crates/daemon/src/wire.rs:540-559,781-784`
coexist with accessor-driven invalidation at
`crates/memory-store/src/lib.rs:203-208,306-310`.
Removing invalidation changes the representation, not the ownership promise.

## Candidate and construction

`sibling-mutation-preserves-untouched-bytes` needs two aliases to an ingress
shell, an owned edited clone, and a sibling carrying nontrivial typed data.
The marker records alias presence and selection of one block before mutation.
It never requires observing sibling corruption.

## Narrow nonapplicability

No unsafe sharing, memory ordering, or concurrent writer protocol is added.
An interleaving simulator is not automatically needed for immutable alias
noninterference. Test strategy owns the cheapest sufficient form.
