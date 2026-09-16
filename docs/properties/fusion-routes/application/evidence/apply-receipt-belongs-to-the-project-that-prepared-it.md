# apply-receipt-belongs-to-the-project-that-prepared-it

## Discovery trigger

A review of the first two RP2.7.U4 commits found that `with_receipts` discarded
the `RouteScope` its `kernel_request` call returned and that the store keyed
receipts by `preparation_id` alone, while one daemon binds many project routes
and the kernel routes' durable receipts are prefixed by the project digest
(`crates/daemon/src/kernel_routes/project.rs` `ProjectBinding::operation_key`).

Repository: `/local/home/ahrav/scratch/eidnara`; base `rp27/u3c-dense-lane` at
`f318c4a4`; inspected 2026-09-16.

## Evidence trail

- `crates/daemon/src/edit_receipts.rs`: `ReceiptStore` holds a
  `BTreeMap<String, ProjectReceipts>` keyed by `ProjectBinding::scope_id`;
  `prepare`, `apply`, and `confirm` take the project first; `with_receipts`
  reads `scope.project.scope_id()` and passes it to the closure.
- `crates/daemon/src/kernel_routes/project.rs` `ProjectBinding::scope_id` and
  `operation_key`: the per-project prefix convention this store follows.
- `docs/host-wire-protocol.md` Section 7.8: receipts belong to the bound
  project and the count bound is per project.
- `crates/daemon/tests/edit_receipts.rs` `a_receipt_belongs_to_the_project_that_prepared_it`.

## Failure scenario

A caller bound to one project could consume, reclassify, or evict another
project's receipts on the same daemon.

## Timing windows and dependencies

None.

## What a test must construct

A second route bound to another project root on one `KernelDaemon`, and a
receipt limit set with `max_keys = 1` so the per-project bound is observable.

## Investigation log

### Q: Is the cross-project refusal distinguishable from an evicted key?

- Sources examined: `ReceiptStore::apply` and `confirm` in
  `crates/daemon/src/edit_receipts.rs`; Section 7.8's `receipt_unavailable`
  row.
- Findings: both answer `receipt_unavailable`; the wire does not say why the
  project holds no receipt, and Section 7.8 lists "made under another project"
  as one of the causes.
- Missing evidence: none.
- Conclusion: resolved with answer; the refusal is deliberately the same so a
  key never authorizes a replay on any route that lacks its record.
