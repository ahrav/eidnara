# sc-capture-state-has-one-durable-store

Abbreviations follow [`../catalog.md`](../catalog.md): `hs.rs`, `hs-tests`,
`sc.rs`, and `sc-tests` name the harness-source and source-capture modules
and their tests.

## Discovery trigger

The specification's stop conditions include "A second durable store for
capture state appears." The fresh-context portfolio evaluation (finding E4
in [`../portfolio-evaluation.md`](../portfolio-evaluation.md)) found that the
crosswalk mapped this stop condition to a record whose check never names a
durable store.

## Evidence trail

- `sc.rs` keeps the memo (`Memo`, bounded by `MEMO_BUDGET_BYTES`), the
  publication slot, the delete epoch, and the counters in process; a restart
  empties them and Kernel receipts answer the next pass.
- `hs.rs` `publish_batch` and `publish_screened` write through
  `KernelStore::commit` and `Envelope` methods; descriptor and evidence
  receipts are the durable record of capture.
- `crates/daemon/src/project_docs.rs` reads documents through
  `config::read_bounded_bytes` and writes nothing.
- `sc.rs` unit test `capture_state_has_one_durable_store` reads the four
  modules with `include_str!`, keeps the text before each module's test
  block, and asserts it contains none of `rusqlite::`, `OpenOptions`,
  `fs::write`, `File::create`, `create_dir`, `MemoryStore::open`, or
  `KernelStore::open`.

## Failure scenario

A change persists the memo or pending units to a side file or SQLite
database to survive restarts. Capture state then lives in two stores whose
retention, deletion, and recovery diverge: `session.delete` retires Kernel
descriptors while the side store still holds the session's units.

## Timing windows and dependencies

None; the property is structural.

## What a test must construct

Nothing at runtime: the source scan reads the production code of the
capture modules at test time.

## Investigation log

### Q: Does the scan cover every way to open a durable store?
- Sources examined: the four capture modules and the store openers in
  `crates/kernel` and `crates/memory-store`.
- Findings: the named forms cover direct SQLite, file creation and writes,
  and opening either store. A store opened elsewhere and passed into
  capture, or a write through a helper in another module, passes the scan.
- Missing evidence: a scan of the handler code in `crates/daemon/src/lib.rs`
  that builds the offer.
- Conclusion: unresolved, needs a `/testing:test-strategy` decision on
  widening the scan; confidence stays medium.
