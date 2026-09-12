# worker-thread-panics-stay-inside-the-redaction-boundary

Baseline: `913234433ae36a80a6e22c6aac14c7f9aab74386`, 2026-09-10.
The [scope and provenance](../catalog.md#scope-and-provenance) apply here.

## Discovery trigger

The area's [portfolio evaluation](../portfolio-evaluation.md#gaps-queued)
queued this as gap 1: the redaction guard the host wraps around every
handler callback is a thread-local, and the daemon already runs kernel work
on `spawn_blocking` workers that never enter it. W8 and W11 frame the
worker thread as a future relocation; this record states the diagnostics
obligation that boundary carries at HEAD and that a transform relocation
widens to every pass.

## Evidence trail

- The guard is the thread-local `CALLBACK_POLL_DEPTH` at
  [`panic_boundary.rs:11-13`][pb-tls], initialised to `0`. Only
  [`redact_sync`][pb-sync] and [`redact`][pb-async] raise it, through
  `CallbackPollGuard::enter` at [`:15-22`][pb-guard].
- The hook installed at [`:36-50`][pb-hook] reads the depth on the panicking
  thread ([`callback_is_polling`][pb-polling]). Depth non-zero prints
  [`REDACTED_DIAGNOSTIC`][pb-redacted] and nothing else (`:44`); depth zero
  calls `previous(info)` with the full panic info (`:46`). `previous` is
  whatever `take_hook` returned at the single install
  ([`runtime.rs:602`][install]); the only other `std::panic::set_hook` in the
  tree is the test at [`tests/dispatch.rs:635`][t-panic-child], so in the
  daemon it is the Rust default, which prints the thread name, location, and
  payload text.
- The host wraps the request handler at [`dispatch.rs:928-934`][wrap]:
  `redact_sync` around `handler.handle(ctx)` and `redact` around the returned
  future, both on the runtime worker that polls the task. A panic there is
  caught by Tokio as a `JoinError` and mapped at [`:985-989`][terminal] to
  `Terminal::Error { code: "internal_error", message: "handler request task
  failed" }` ([`CODE_INTERNAL_ERROR`][code]).
- [`kernel_routes::blocking`][blocking] at [`mod.rs:462-468`][blocking] calls
  `tokio::task::spawn_blocking(work)` with no guard entry and maps the
  `JoinError` to `KernelOutcome::unavailable(StoreUnavailable)` (`:467`). Its
  doc at [`:460-461`][blocking-doc] states the intent: a panic inside the
  store closure is reported as an unavailable store rather than taking the
  handler down. The route returns that outcome as a `Response` body tagged
  `{"kind":"unavailable","reason":"store_unavailable"}`
  ([`UnavailableReason`][reason], serialized `snake_case`; the read route's
  `state_only` arms at [`read.rs:296-322`][read-arms]).
- The eight `blocking` call sites: [`commit.rs:1013`][blk-commit-preview],
  [`:1038`][blk-commit-run], [`egress.rs:201`][blk-egress],
  [`eligibility.rs:346`][blk-eligibility], [`ingest.rs:722`][blk-ingest-decode]
  (base64 page decode, holding the page bytes), [`:797`][blk-ingest-finish]
  (`finish_upload`, which calls [`store.ingest_artifact`][route-ingest] with
  the upload payload at `:577`), [`read.rs:291`][blk-read-gate], and
  [`:311`][blk-read-rows].
- Three direct `spawn_blocking` calls: [`health.rs:224`][spawn-health];
  [`mod.rs:358`][spawn-kernel-open], where a `JoinError` is printed with
  `eprintln!` and mapped to `KernelError::Fault` (`:360-362`); and
  [`lib.rs:3808`][spawn-store-open], where a `JoinError` re-panics on the
  caller (`:3810`).
- The evaluation names `routing.rs:591` as a production tenant of the pool.
  That call sits inside `#[cfg(test)] mod tests`
  ([`routing.rs:458-459`][routing-tests]) and is not production code; the
  record does not list it.
- The other production `thread_local!` the boundary crosses is the
  token-cache counter block at [`token_cache.rs:57`][tc-local] (W2). The
  third in the inspected crates is `#[cfg(test)]`
  ([`transform.rs:487-490`][tl-test]).
- The host-runtime catalog's
  [every-callback-invocation-is-inside-the-redaction-guard][hr-redact]
  inventories the host's own call sites and
  [the-panic-hook-cannot-itself-fail][hr-hook] covers the hook's failure
  modes; neither reaches a thread the daemon spawns.

## Failure scenario

A `blocking` closure panics with a message built from request bytes, an
`expect` on a value derived from the payload, or with `RUST_BACKTRACE` set.
The hook runs on the blocking-pool thread, finds depth `0`, and forwards to
the default hook, which writes the message and location to stderr. The
daemon's stderr is a log file, so the bytes persist. The request settles
with `{"kind":"unavailable","reason":"store_unavailable"}`, so nothing in
the wire exchange signals a panic. After a transform relocation the same
path carries the whole `TransformRequest`, and the terminal changes from
today's `internal_error` to whatever the relocation chooses.

## Timing windows and dependencies

None in time. The decision is made on the panicking thread at panic time,
before Tokio's `catch_unwind` in the blocking task sees the payload, so the
`JoinError` mapping cannot undo the print. The property depends on which
thread runs the closure, on the guard depth of that thread, and on which
hook `install` preserved as `previous`.

## What a test must construct

A child process running the daemon or a reduced host with a `kernel.*` route
whose store closure panics with a unique long sentinel, on the pattern of
[`handler_panic_payload_is_redacted_from_process_stderr`][t-panic-stderr]
and its [child role][t-panic-child]. The parent asserts the child's stderr
contains [`REDACTED_DIAGNOSTIC`][pb-redacted] and not the sentinel, and
records the terminal frame beside it. A second run panics on the runtime
worker for the same route as the control; at HEAD that control settles as
`internal_error` while the kernel-route form settles as a
`store_unavailable` response, and the test records both rather than
asserting equality until the open question is answered. The injection point
does not exist at HEAD: `blocking` takes an opaque closure, so the test needs
a test-only store that panics on a chosen call or a `#[cfg(test)]` branch
inside one closure. No existing daemon check panics on a worker thread.

## Investigation log

### Q: Which terminal is the contract for a worker-thread panic?

The host settles an in-handler panic as `internal_error`; the kernel routes
settle a worker panic as an `unavailable` response by documented intent.

- Sources examined: [`dispatch.rs:985-989`][terminal],
  [`mod.rs:460-468`][blocking], [`read.rs:296-322`][read-arms],
  [`mod.rs:358-362`][spawn-kernel-open], [`lib.rs:3808-3810`][spawn-store-open].
- Findings: Three distinct mappings exist for a `JoinError` from a worker
  panic: an `internal_error` terminal (host), a `store_unavailable` response
  (kernel routes), and a re-panic (store open). The kernel-route mapping is a
  stated design, not an omission. A relocated transform has no mapping yet.
- Missing evidence: The specification's choice for the transform, and
  whether the kernel routes keep theirs.
- Conclusion: needs human input.

### Q: Does the default hook print the payload?

- Sources examined: [`panic_boundary.rs:36-50`][pb-hook]; the standard
  library's default hook behaviour, which prints the panic message and
  location and, with `RUST_BACKTRACE`, a backtrace.
- Findings: `previous(info)` receives the unmodified `PanicHookInfo`; the
  redacting hook does not alter it. Whether a given closure's panic message
  contains request bytes depends on the panic site; the class is open because
  `expect` and `unwrap` on payload-derived values format the value.
- Missing evidence: No run; the standard-library behaviour is cited, not
  reproduced here.
- Conclusion: resolved with answer - the payload text reaches stderr through
  the default hook whenever the panicking thread's depth is `0`.

[pb-redacted]: ../../../../../crates/host-runtime/src/panic_boundary.rs#L7
[pb-tls]: ../../../../../crates/host-runtime/src/panic_boundary.rs#L11-L13
[pb-guard]: ../../../../../crates/host-runtime/src/panic_boundary.rs#L15-L22
[pb-polling]: ../../../../../crates/host-runtime/src/panic_boundary.rs#L30-L34
[pb-hook]: ../../../../../crates/host-runtime/src/panic_boundary.rs#L36-L50
[pb-sync]: ../../../../../crates/host-runtime/src/panic_boundary.rs#L52-L55
[pb-async]: ../../../../../crates/host-runtime/src/panic_boundary.rs#L60-L72
[install]: ../../../../../crates/host-runtime/src/runtime.rs#L602
[wrap]: ../../../../../crates/host-runtime/src/dispatch.rs#L928-L934
[terminal]: ../../../../../crates/host-runtime/src/dispatch.rs#L985-L989
[code]: ../../../../../crates/host-runtime/src/control.rs#L17
[blocking-doc]: ../../../../../crates/daemon/src/kernel_routes/mod.rs#L460-L461
[blocking]: ../../../../../crates/daemon/src/kernel_routes/mod.rs#L462-L468
[reason]: ../../../../../crates/daemon/src/kernel_routes/state.rs#L38-L44
[read-arms]: ../../../../../crates/daemon/src/kernel_routes/read.rs#L296-L322
[blk-commit-preview]: ../../../../../crates/daemon/src/kernel_routes/commit.rs#L1013
[blk-commit-run]: ../../../../../crates/daemon/src/kernel_routes/commit.rs#L1038
[blk-egress]: ../../../../../crates/daemon/src/kernel_routes/egress.rs#L201
[blk-eligibility]: ../../../../../crates/daemon/src/kernel_routes/eligibility.rs#L346
[blk-ingest-decode]: ../../../../../crates/daemon/src/kernel_routes/ingest.rs#L722
[blk-ingest-finish]: ../../../../../crates/daemon/src/kernel_routes/ingest.rs#L797
[route-ingest]: ../../../../../crates/daemon/src/kernel_routes/ingest.rs#L577
[blk-read-gate]: ../../../../../crates/daemon/src/kernel_routes/read.rs#L291
[blk-read-rows]: ../../../../../crates/daemon/src/kernel_routes/read.rs#L311
[spawn-health]: ../../../../../crates/daemon/src/kernel_routes/health.rs#L224
[spawn-kernel-open]: ../../../../../crates/daemon/src/kernel_routes/mod.rs#L358-L362
[spawn-store-open]: ../../../../../crates/daemon/src/lib.rs#L3815-L3817
[routing-tests]: ../../../../../crates/host-runtime/src/routing.rs#L458-L459
[tc-local]: ../../../../../crates/daemon/src/token_cache.rs#L57-L76
[tl-test]: ../../../../../crates/daemon/src/transform.rs#L495-L498
[t-panic-stderr]: ../../../../../crates/host-runtime/tests/dispatch.rs#L603-L628
[t-panic-child]: ../../../../../crates/host-runtime/tests/dispatch.rs#L631-L660
[hr-redact]: ../../../host-runtime/catalog.md#every-callback-invocation-is-inside-the-redaction-guard
[hr-hook]: ../../../host-runtime/catalog.md#the-panic-hook-cannot-itself-fail
