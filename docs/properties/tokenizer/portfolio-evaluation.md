# Tokenizer portfolio evaluation

Discovery seeks properties; evaluation seeks flaws in the set. This pass was run
at U3 against `catalog.md`, `existing-checks.md`, and `fault-map.md` in this
directory. It was not an independent fresh-context evaluation: it was performed
in the same review pass that corrected the catalog, driven by review findings on
the initial three-record set. A fresh-context pass is queued below.

Lenses applied: harness fit, coverage balance, implementability, and whether the
part's framing (a bit-faithful port with one oracle) survives contact with the
code.

## Disposition summary

| Category | Count | Status |
| --- | --- | --- |
| refinement | 4 | applied to the catalog |
| gap | 3 | queued |
| bias | 2 | require human judgment, listed below |

## Refinements applied

1. **All records reclassified `test-only`.** No workspace crate depends on
   `tokenizer` and nothing outside `crates/tokenizer` calls `encode_ordinary`
   or `estimate_tokens`; the initial `default-production` labels rested on an
   intended consumer that is not in this tree.
2. **Parity scope narrowed to the crate's stated contract.** The scope note
   now lists the three deliberate divergences from stock `ai-tokenizer@1.0.6`
   (chunk seams above `MAX_PIECE_BYTES`, `Object.prototype` member-name pieces,
   BOM-leading byte slices) and how each is pinned, so a campaign cannot flag
   the corrections as regressions.
3. **Byte-sequence uniqueness added to the vocabulary record.** The generator
   checks it; the Rust `FxHashMap` build would silently replace a duplicated
   sequence's rank while rank uniqueness and single-byte coverage still held.
4. **`tokenizer-over-long-pieces-are-chunked-and-bounded` added.** The
   bounded-work contract and its permitted seam divergence were absent from the
   initial set, so a campaign could regress worst-case latency or demand oracle
   equality for a long unpunctuated input without violating the catalog.

## Gaps queued

1. A Rust test over the committed `assets/claude.tiktoken` (unique ranks,
   unique token byte sequences, 256 single-byte tokens). This is the only
   record with no Rust check; the generator is trusted at generation time only.
2. A time-bounded assertion or benchmark for the chunking path. The bound is
   asserted structurally; nothing measures it.
3. A fresh-context evaluation once a production caller exists, because the
   caller decides which inputs matter (budget estimates over model context
   versus arbitrary text) and therefore which golden cases are load-bearing.

## Biases requiring human judgment

1. **Reclassification trigger.** The records are `test-only` because the tree
   has no caller. The wave that adds the first production caller must
   reclassify them; nothing mechanical will flag that. Should the catalog
   checker warn when a `test-only` record's cited API gains a non-test caller?
2. **Oracle authority.** The golden is produced from `ai-tokenizer@1.0.6`
   with a patched encoder. If upstream fixes the prototype-name or BOM defects
   in a later version, the patched oracle and the stock oracle converge and the
   scope exceptions become dead text; if upstream changes the vocabulary, the
   golden must be regenerated and the crate's ids change. Which upstream
   version is the contract, and who decides when to move it?

## Verdict

The three records state the crate's contract as the code and its docs define
it: exact parity below the cap with named exceptions, bounded work above it, and
a complete embedded vocabulary. Two of the three are exercised by committed
tests; the vocabulary record is trusted from the generator. The set is complete
for a crate with no consumer; it is not yet evaluated against the consumer that
would make its reachability `default-production`.
