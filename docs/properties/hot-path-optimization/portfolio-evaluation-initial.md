# Initial portfolio evaluation

This retained review describes the initial 14-record draft. Its open findings
are historical review evidence, not the status of the revised 16-record
supplement. See [the current disposition](portfolio-evaluation.md) for applied
corrections, the independent recheck, and remaining owner gates.

Baseline: `913234433ae36a80a6e22c6aac14c7f9aab74386`.

Evaluation date: 2026-09-10.

Evaluator independence: This is a fresh-context evaluation. I had not seen the
catalog's discovery reasoning before starting. I read `METHOD.md`, the property
README, and the evaluation reference first. I then read the supplied catalog,
lens summaries, evidence, tests, and source as evaluation evidence.

## Harness fit

The eleven `always` and three `sometimes` labels reported in
`catalog.md:54-71` match the record index. Most guarantee sentences use the
right semantics. Three records state campaign situations and correctly use
`sometimes`. The remaining records state per-evaluation preservation rules and
mostly use `always`.

- **K1, UNCERTAIN.** `always` matches exact result preservation. Ordered rows,
  bytes, revision, truncation, `known_as_of`, and availability are literal
  assertions. The oracle is not executable until an independent baseline is
  selected, which the record leaves open at `catalog.md:82-101`.
- **K2, VERIFIED.** `always` matches the metamorphic irrelevance guarantee.
  Equality of selected IDs, order, content, revision, and truncation is directly
  assertable for paired stores (`catalog.md:108-128`).
- **K3, VERIFIED for semantics, REFUTED as complete cap coverage.** `sometimes`
  correctly describes situation coverage, and seed count, target rank, payload
  length, and eligibility can be recorded before execution. Its row-or-byte
  disjunction lets one cap remain unexercised (`catalog.md:135-153`). This is
  finding 3.
- **E1, UNCERTAIN.** `always` matches the cleanup gate. Start, completion,
  route-gone, and reuse are assertable events. No transform-worker event or
  independent physical-work ledger exists at this baseline
  (`catalog.md:162-180`).
- **E2, REFUTED as written.** Charge ownership and per-class sums are assertable,
  but "each reservation returns once" adds unbounded eventual release to an
  `always` safety check (`catalog.md:187-206`). This is finding 2.
- **E3, VERIFIED for semantics, UNCERTAIN for observation.** `sometimes` matches
  the required start-close-completion ordering. The predicate is literal once a
  physical start and completion seam exists (`catalog.md:213-230`).
- **S1, UNCERTAIN.** `always` matches current-authority preservation. Results,
  refusals, effects, and post-call connection state can be asserted, but the
  differential baseline is not identified and the read-only and restoration
  subcases are not stated precisely (`catalog.md:239-258`). Findings 1 and 4
  address this.
- **S2, VERIFIED with one bridge refinement.** `always` matches snapshot and
  transaction-boundary preservation. The read sequence and durable effect sets
  are literal assertions (`catalog.md:265-284`). Finding 8 makes the retained
  durability obligation explicit for moved or folded writes.
- **H1, UNCERTAIN.** `always` matches byte-for-byte preservation, and final bytes
  plus direct and cached token counts are literal assertions. The pre-change
  reference and golden provenance remain unnamed (`catalog.md:293-312`).
- **H2, VERIFIED.** `always` matches the direct-renderer, wrapped-retry, and
  replay boundaries. Output cost, retry count, threshold, and bytes are
  directly observable with an injected counting estimator
  (`catalog.md:319-339`).
- **H3, VERIFIED.** `sometimes` correctly checks independent multi-demotion
  pressure rather than candidate loop coverage. Both reference costs and
  remaining demotability are literal inputs (`catalog.md:346-363`).
- **R1, UNCERTAIN only on the differential oracle.** `always` matches the policy
  matrix. Output, refusal, field metadata, owner links, and actions are literal
  assertions, but "the baseline" is not an identified artifact
  (`catalog.md:372-390`).
- **R2, VERIFIED.** `always` matches no append and no commit on each refusal.
  In-memory scan length and durable before/after projections are separate,
  literal observations (`catalog.md:397-414`).
- **R3, VERIFIED.** `always` matches metadata and effect parity. Normalized graph
  equality and exact synthetic-sentinel absence are literal assertions
  (`catalog.md:421-441`). Allocation reduction correctly remains outside this
  functional check.

Routing all records through `/testing:test-strategy` is plausible. K, H, and R
have cheaper deterministic unit or SQLite integration forms. S has controlled
SQLite interleavings. E needs deterministic scheduling only after physical-work
instrumentation exists. The conditional DST handoff at `catalog.md:498-500` is
therefore plausible, but DST cannot manufacture missing ownership events.

**Finding 1, baseline oracle bias.** The repository commit freezes source, but
it does not say which executable reference, trace schema, fixture outputs, or
goldens remain independent after candidate code replaces the old path. K1, S1,
H1, and R1 all compare with an unnamed baseline. K1 and H1 acknowledge the
choice; S1 and R1 do not. A human must choose and retain the four oracle
artifacts before these checks become executable.

**Finding 2, E2 refinement.** Replace E2's `Check:` with this wording:

> Check: `always` - For every charge identity and resource class, a live
> protected resource has exactly one live reservation owner. A transfer changes
> that owner atomically without changing the charged amount. A release occurs
> at most once and only after the protected lifetime ends. Per-class reserved
> totals never exceed capacity. At each explicitly quiescent test endpoint, no
> protected resource or reservation remains.

This keeps the invariant safety-shaped. Any promise that work eventually
releases a reservation needs a separate bounded liveness record.

**Finding 3, K3 gap.** Row pressure and byte pressure are distinct failure
classes in `crates/daemon/src/kernel_routes/read.rs:181-224`. One disjunctive
marker can pass while the other cap is never stressed. Targeted discovery must
split the two precondition witnesses or require two fixed campaign markers.

## Coverage balance

Every named Rust surface has records: K covers canonical read, E covers
placement, S covers storage callbacks, H covers history rendering, and R covers
prepared fields. Coverage is not balanced inside those surfaces.

1. **Read-only write rejection, REFUTED as a self-contained S check.** S1's
   broad "read/write authority" wording and the retained canonical
   `read-callbacks-cannot-write` record point at the obligation. S1 does not
   explicitly require a cached write or writable pragma to run after setup is
   reused or elided. Existing source proves this is constructible:
   `crates/storage/src/lib.rs:3901-3941` runs cached writes in a read callback.
2. **Cancellation before transform commit, REFUTED.** E1 protects cleanup, E2
   protects accounting, and E3 witnesses overlap. None states that work which
   observes cancellation before its commit point cannot later commit. The
   unconditional form "Cancel means no effect" is not the existing contract:
   cancellation is best effort in `docs/host-wire-protocol.md:765-768`.
3. **Durability, UNCERTAIN only at the optimization bridge.** The relationship
   map retains `protected-transactions-pin-fence-durability`
   (`catalog.md:456-461`). That canonical record requires WAL and
   `synchronous=FULL` for each fenced transaction
   (`../shared-primitives/catalog.md:721-733`). It does not itself prove that a
   moved or folded write remains in the protected write set, and it does not
   inject power loss.
4. **Plugin and shared-memory transport, VERIFIED absent.** None of the 14
   records covers either. They are **not in this catalog's scope** as framed by
   the five Rust-side surfaces at `catalog.md:25-29`. The boundary matters: the
   production TypeScript transform calls the module transport at
   `packages/opencode-plugin/src/hooks/context/rust-mode-transform.ts:1383-1389`.
   That transport owns a five-second transform ceiling and request cancellation
   at `packages/opencode-plugin/src/hooks/context/module-transport.ts:719-844`.
   Its connection uses `ShmFrameChannel`
   (`packages/opencode-plugin/src/shared/host-client/connection.ts:1-17,43`),
   which imports `@eidnara/shm-native` at
   `packages/opencode-plugin/src/shared/host-client/shm-frame-channel.ts:1-8`.
   Finding 9 asks a human to confirm that no optimization crosses this boundary.
5. **Situation coverage, REFUTED.** S1, S2, and R1-R3 have only `always`
   checks. The only `sometimes` records are K3, E3, and H3
   (`catalog.md:54-71`). Required-state prose is useful, but it is not a
   campaign assertion. METHOD requires coverage checks to assert independent
   preconditions (`../METHOD.md:77-86`).

**Finding 4, S1 refinement.** Append this exact text to S1's `Check:`:

> For both prior `query_only` values, and after callback success, error, or
> panic, assert `query_only_after == query_only_before`. After any callback
> setup reuse or elision, a cached `INSERT` and a writable pragma executed in a
> read callback return an error and leave durable state unchanged. A following
> callback observes only its own current facade scope.

Append this exact text to `Required faults and enabling state:`:

> Warm the statement cache under writable authority, then reuse the SQL in a
> read callback after the optimized setup path. Run the restoration matrix with
> prior `query_only` OFF and ON, and with success, error, and panic exits.

**Finding 5, guarded-store coverage gap.** S1 needs a situation witness for
cached SQL crossing an authority change or setup-elision boundary. S2 needs
separate witnesses for a second-connection commit inside one callback, a commit
between callbacks, and adjacent independent success and failure. These markers
assert the schedules, not a stale read or rollback defect.

**Finding 6, redaction coverage gap.** R1-R3 have no campaign assertions that
all policy/layer states occur. Targeted discovery should define independent
markers for clean and detected input across all six R1 pairs, each R2 refusal
after an earlier successful field, and R3's payload-free path with multi-owner,
replay, and rollback states. Deterministic parameterized tests may satisfy these
as mandatory case accounting, but that decision belongs in test strategy.

**Finding 7, cancellation-observation gap.** Current daemon handling parses and
dispatches without passing `RequestCtx` cancellation into the operation
(`crates/daemon/src/lib.rs:11821-11826,12558-12592`). It checks cancellation
only while settling an already prepared response
(`crates/daemon/src/lib.rs:12042-12060`). Moving work off-worker changes that
shape because host cancellation can abort the waiter while physical work
continues (`crates/host-runtime/src/dispatch.rs:937-954`). Discovery must define
transform commit points and state the conditional rule, if intended: work that
has observed cancellation before a named commit point does not cross that
point. Post-commit cancellation must remain compatible with best-effort and
unknown-outcome rules.

**Finding 8, S2 refinement.** Append this wording to S2's guarantee and check:

> Guarantee: Every resulting durable effect remains inside a fence-checked
> transaction with the durability settings required by the retained storage
> contract.
>
> Check: For every moved or folded durable write, the protected-write inventory
> maps the effect to `with_conn_fenced`, and the connection is pinned to WAL and
> `synchronous=FULL` immediately before its `BEGIN IMMEDIATE` transaction.

If the optimization changes crash boundaries rather than only callback count,
route the power-loss window to crash-consistency and failpoint test design.

**Finding 9, server-side scope bias.** The stated scope stops at the five Rust
surfaces while the actual per-turn path crosses TypeScript timeout,
cancellation, retry, and shared-memory publication code. A human must confirm
that the optimization changes none of those contracts. If that assumption is
false, cross-layer discovery is required before implementation.

## Implementability

Construction verdicts follow existing infrastructure, not hypothetical helper
APIs.

- **K1, partial.** `crates/daemon/tests/transform_canonical_memory.rs:43-205`
  constructs canonical transform output, authority changes, metadata, and
  absence. The independent frozen oracle does not exist.
- **K2, yes.** `KernelDaemon` exposes a real kernel store and bound transform
  route (`crates/daemon/tests/support/kernel_daemon.rs:30-116,159-169`), so two
  isolated stores and controlled lineage facts are feasible.
- **K3, yes but expensive.** Existing tests seed more than 8192 rows and a
  payload over the byte budget at
  `crates/daemon/tests/kernel_routes.rs:2002-2056,2156-2195`.
- **E1, no for the proposed worker.** The transform closure runs synchronously
  at `crates/daemon/src/lib.rs:8139-8204`. Host tracking covers the callback at
  `crates/host-runtime/src/dispatch.rs:873-934`, not a future detached worker.
- **E2, partial.** Existing host permits and daemon ingest reservations provide
  examples, but no transform physical-work class or owner ledger exists.
- **E3, no for transform-specific overlap.** Host tests can order cancellation
  and callback completion (`crates/host-runtime/tests/dispatch.rs:357-400`), but
  no transform-worker start or completion barrier exists.
- **S1, yes.** Storage tests already construct cached write denial, panic
  restoration, maintenance changes, shadows, and infrastructure rename
  attempts (`crates/storage/src/lib.rs:3371-3544,3594-3696,3900-3942`).
- **S2, yes.** A raw second connection commits between two reads, then a later
  callback observes the commit at `crates/storage/src/lib.rs:3946-3981`.
- **H1, partial.** Renderer goldens, SHA-256, real tokenizer costs, and cache
  parity exist (`crates/daemon/src/decay_render.rs:613-807` and
  `crates/daemon/src/token_cache.rs:187-205`). The independently retained
  pre-change reference is not selected.
- **H2, yes.** Renderer and outer retry take an injected estimator, so budgets,
  retry count, wrapper cost, and output bytes are unit-test observable
  (`crates/daemon/src/decay_render.rs:289-338` and
  `crates/daemon/src/m0_compose.rs:178-215`).
- **H3, yes.** Test fixtures can call the renderer with the production tokenizer
  and record initial and first-demotion costs. Existing `fired` accounting does
  not provide that witness (`crates/daemon/src/decay_render.rs:765-805`).
- **R1, yes.** In-crate tests can reach private `PreparedWrite`, and an existing
  test observes preserve and substitute actions
  (`crates/memory-store/src/lib.rs:18168-18212`).
- **R2, yes in an in-crate unit test.** `PreparedWrite.scans` is private, but the
  source test module can inspect it. Existing output-growth construction is at
  `crates/memory-store/src/lib.rs:22689-22725`.
- **R3, partial.** Integration infrastructure observes the full audit graph and
  durable bytes (`crates/memory-store/tests/production_redaction.rs:67-183`),
  and its helper counts all audit tables
  (`crates/memory-store/tests/support/scan_audit.rs:9-61`). The payload-free
  candidate path does not exist yet.

Existing seam checks run successfully at this baseline. These runs verify
fixture availability, not catalog adequacy:

- `cargo test --locked -p storage
  a_read_callback_observes_one_snapshot_across_its_statements`: 1 passed.
- `cargo test --locked -p storage
  cached_statements_run_under_the_callback_scope`: 1 passed.
- `cargo test --locked -p storage
  a_panicking_read_does_not_strand_the_connection_read_only`: 1 passed.
- `cargo test --locked -p host-runtime --test dispatch
  cancel_and_completion_settle_exactly_once`: 1 passed.
- `cargo test --locked -p memory-store --test production_redaction
  active_note_scan_audit_is_atomic_complete_and_opaque`: 1 passed.

**Finding 10, future-topology bias.** E1-E3 project generic host lifecycle rules
onto a transform worker that does not exist. The catalog acknowledges this at
`catalog.md:41-45`, but still labels all three records `default-production` from
the generic dispatch path. A human must choose the worker owner, join behavior,
cancellation handoff, resource class, and physical completion event. Until
then, the E semantics are plausible but their transform-specific reachability
and harness are unverified.

### Anchor verification

| Anchor | Verified/off | Note |
| --- | --- | --- |
| `catalog.md` `[pass-read]`, `daemon/src/lib.rs:8133-8202` | VERIFIED | Pins one canonical read and invokes the synchronous transform closure. |
| `catalog.md` `[memory-read]`, `daemon/src/canonical_memory.rs:141-212` | VERIFIED | Captures tip/lag, reads selected rows, filters, trims, and builds snapshot. |
| `catalog.md` `[dispatch]`, `host-runtime/src/dispatch.rs:823-934` | VERIFIED | Acquires both permit classes and starts tracked callback work. |
| `catalog.md` `[close]`, `host-runtime/src/dispatch.rs:1237-1268` | VERIFIED | Contains wait, abort, second wait, fatal refusal, and successful gate. |
| `catalog.md` `[read-callback]`, `storage/src/lib.rs:229-245` | VERIFIED | Opens one deferred transaction and releases scope before finishing it. |
| `catalog.md` `[write-callback]`, `storage/src/lib.rs:290-316` | VERIFIED | Pins durability, claims fence, releases scope, and commits. |
| `catalog.md` `[history-render]`, `daemon/src/decay_render.rs:296-338` | VERIFIED | Joins bodies and performs bounded oldest-first demotion. |
| `catalog.md` `[core-prep]`, `memory-store/src/lib.rs:3428-3459` | VERIFIED | Uses durable-layer prepared fields for core state. |
| `catalog.md` `[transaction-prep]`, `memory-store/src/lib.rs:3462-3494` | VERIFIED | Uses transaction-layer prepared fields for core state. |
| `catalog.md` `[tokenizer-dependency]`, `daemon/Cargo.toml:21-32` | VERIFIED | `tokenizer` is a normal dependency at line 32. |
| `_lenses/guarded-store.md` `[cache]`, `storage/src/lib.rs:487-498` | VERIFIED | Documents expiry on scope installation and per-callback reuse. |
| `_lenses/history-render.md` `[outer]`, `daemon/src/m0_compose.rs:178-215` | VERIFIED | Counts wrapped history and retries at most three times over 105 percent. |
| `_lenses/canonical-read.md` `[sql]`, `kernel/src/admission.rs:3113-3279` | VERIFIED | Query has admission joins, scope prefilter, object-ID order, and Vec collection. |
| `_lenses/redaction-ownership.md` `[json]`, `memory-store/src/lib.rs:2133-2143` | VERIFIED | Records empty retained text with detections and ownership metadata. |

No sampled anchor is off.

## Wildcard

- **Infrastructure rename detection, VERIFIED.** `CallbackScope::install` records
  infrastructure objects, and release compares them before commit at
  `crates/storage/src/lib.rs:624-679`. S1 explicitly includes maintenance,
  rename, stale statements, and failure (`catalog.md:246-250`). Observable
  baseline parity therefore protects this behavior even if schema scans are
  cached. Finding 5 still requires a non-vacuous cached-scan schedule.
- **Per-call statement expiry, VERIFIED as mechanism rather than contract.**
  Installing the authorizer expires cached statements
  (`crates/storage/src/lib.rs:443-498`). S1 correctly protects current authority
  instead of requiring a preparation count. No mechanism-specific property is
  needed.
- **`query_only` restoration, REFUTED as precise S1 coverage.** The source saves
  the prior value and restores it on install failure, release, and drop
  (`crates/storage/src/lib.rs:604-695`). S1 says "restored connection state" but
  does not enumerate prior ON, prior OFF, success, error, and panic. Finding 4
  supplies literal wording.
- **H reachability, VERIFIED.** The premise that decay rendering runs only on
  HARD is false. A fresh shape selects `PassPlan::Hard`
  (`crates/context-core/src/lib.rs:15-16,79-83`), and the HARD arm composes m0
  (`crates/daemon/src/transform.rs:4031-4058`). Composition calls the history
  renderer (`crates/daemon/src/m0_compose.rs:258-280` and
  `crates/daemon/src/memory_render.rs:191-198`). A SOFT pressure refold also
  composes m0 at `crates/daemon/src/transform.rs:4310-4344`. The H-group
  `default-production` label is supported; pressure still needs workload
  construction.

## Finding disposition

| # | Finding | Class (gap/refinement/bias) | Records | Proposed action |
| --- | --- | --- | --- | --- |
| 1 | Differential baselines are not retained oracle artifacts. | bias | K1, S1, H1, R1 | Human selects executable references, trace schemas, fixtures, and provenance before test design. |
| 2 | E2's `always` check includes unbounded eventual release. | refinement | E2 | Replace `Check:` with the charge-identity and quiescent-endpoint wording above. |
| 3 | K3's row-or-byte marker can leave one cap unexercised. | gap | K3 | Discover two independent coverage witnesses and deduplicate against K1/K2 cases. |
| 4 | S1 does not state write denial and `query_only` restoration literally. | refinement | S1 | Append the exact `Check:` and enabling-state text above. |
| 5 | Guarded-store risk states have no campaign coverage checks. | gap | S1, S2 | Add independent schedule markers or mandatory deterministic case accounting. |
| 6 | Redaction policy and refusal states have no campaign coverage checks. | gap | R1-R3 | Add matrix, refusal, and payload-free-path precondition accounting. |
| 7 | No property binds observed cancellation to transform commit points. | gap | E1-E3 | Discover commit points and add a conditional cancellation property after topology selection. |
| 8 | Retained durability is not explicitly bridged to moved or folded writes. | refinement | S2 | Append the protected-write inventory and WAL/FULL wording above. |
| 9 | Stated scope ends before TypeScript and SHM portions of the live path. | bias | Scope, E1-E3 | Human confirms no cross-boundary contract changes or commissions discovery. |
| 10 | E records rely on a future worker topology for reachability and observation. | bias | E1-E3 | Human chooses ownership, join, cancellation, accounting, and event seams first. |

## Gaps queued

1. Split K3 pressure discovery into independently witnessed row-count and byte
   pressure, then deduplicate each witness against K1 and K2.
2. Discover situation-coverage records or mandatory case counters for S1 and
   S2. Markers describe cache/authority and commit-boundary schedules, not
   violations.
3. Discover situation-coverage records or mandatory case counters for R1-R3.
   Keep expected metadata independent of candidate preparation code.
4. After execution topology is chosen, discover cancellation-observation and
   commit-point properties. Do not turn best-effort cancellation into an
   unconditional no-effect promise.

## Biases for a human

1. **Baseline artifacts.** Decide what remains frozen for K1, S1, H1, and R1.
   A commit hash identifies old source but does not by itself provide an
   independent executable oracle after replacement.
2. **Cross-layer scope.** Confirm that TypeScript timeout/cancellation/retry
   behavior and shared-memory publication remain unchanged and out of scope. If
   not, add them before treating the set as a complete per-turn preservation
   contract.
3. **Execution topology.** Decide who owns and joins worker work, which token it
   observes, which resources it charges, and which event proves physical
   completion. E1-E3 cannot settle these design facts by test wording.
