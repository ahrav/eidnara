# Property lens: version compatibility

Date: 2026-09-13. HEAD: `2e4433e6b511ae74944df8a9669c428e73915d29`.
Baseline: `e451a2b470ae8663b4613ca04f019a30b6d7df53`.
External leads: plan R3-R6 and issue 350's frozen-corpus contract.

## Findings

The old `wire-golden.json` has ten explicit-false tool blocks, so its entire
projection cannot remain byte-identical under the accepted false omission.
The actual plugin emits absent/true. Golden fixture membership must be
classified by shape and provenance, not by filename.

Old `raw_messages_json` enters the same message decoder
(`crates/daemon/src/lib.rs:16443-16461`). Old frozen synthetic pairs explicitly
discard retained originals on load (`crates/memory-store/src/lib.rs:1301-1319`).

## Candidates and handoff

Use separate old-plugin byte fixtures, typed-only normalization fixtures, and
old persisted rows. `historical-chunks-retain-readable-identity` promises
readability of known fields, not indefinite unknown-envelope replay.
The parent specification must reconcile accepted changes with older broad
frozen-byte statements. No versioned host-wire literal change is proposed.

## Missing evidence

The Appendix A corpus and an old-release database are not supplied as raw
artifacts. Existing tests are references, not executed upgrade evidence.
