# dense-rescore-reads-only-accepted-rows-from-their-pinned-layers

## Discovery trigger

RP2.6 R3 and #613 AC3: load only accepted logical identities from their
matching pinned original-f32 files, preserving bindings across promotion and
prune.

## Evidence trail

- `rank_compressed` (`crates/daemon/src/vector_reader.rs:697`) selects the
  pool through `select_candidates` over the view's own layers, then rescores
  through `rescore_pool` with a read closure that names
  `view.layers[winner.layer]` and `winner.row`.
- `PinnedLayer::original` (`vector_reader.rs:171`) reads one row at its offset
  through the descriptor verification hashed.
- Nothing in the rescore reads the selector.

## Failure scenario

A rescore that re-resolves through the current selector reads a newer
generation's row for an occurrence the scan accepted from an older one.

## Timing windows and dependencies

Between selection and the first original read.

## What a test must construct

- A delta that supersedes and masks; a publication and prune after selection.

## Investigation log

### Q: Can a prune remove a file the rescore reads?

- Sources examined: `secure` and `hand_off` in `vector_reader.rs`.
- Findings: the view holds shared locks on the record and every member until
  it drops, and prune skips locked generations.
- Missing evidence: none.
- Conclusion: resolved with answer - no.
