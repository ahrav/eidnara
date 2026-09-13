# Property lens: resource boundaries

Provenance: HEAD `2e4433e6b511ae74944df8a9669c428e73915d29`, 2026-09-13.

Candidate: `envelope-decode-has-no-retained-tree`.

The accepted structural condition forbids full message/block Value envelopes
and retained originals. It permits explicitly retained payload Values and
serde enum buffering. A successful serialization test alone cannot establish
this representation condition.

HEAD counterevidence is direct: `crates/memory-store/src/lib.rs:131-140` and
`:255-262` clone trees and retain them. Do not declare the prospective property
exercised on this source.

Heap peaks, string-copy constants, reservation refusal, and numerical payoff
remain A1/A3 and the separate accounting workstream. This lens records that
boundary rather than duplicating their budget properties or running a bench.
