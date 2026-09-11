# Portfolio evaluation

Baseline: `913234433ae36a80a6e22c6aac14c7f9aab74386`, evaluated 2026-09-10.

This evaluation is written with fresh context. The evaluator read
`METHOD.md`, `README.md`, `catalog.md`, `existing-checks.md`,
`fault-map.md`, and every file under `evidence/` before opening `_lenses/`,
and did not open `portfolio-evaluation-initial.md`. Every source line cited
below was read at HEAD by the evaluator. The task brief describes fourteen
records; the catalog holds sixteen (K4 and H4 are present), and all sixteen
are evaluated here. Five leads from an earlier read-only pass were supplied
without their reasoning; each is confirmed, narrowed, or refuted below on
independent evidence. This document evaluates the property set as a
preservation contract. It does not judge whether any optimization is worth
doing.

## Harness fit

**Differential `always` records depend on one unpackaged oracle.** K1, K2,
S1, S2, H1, R1, R2, and R3 all assert agreement with a baseline. Only K1 and
H1 name a frozen identity (`catalog.md:60-71`: the commit, the four JSON
goldens). S1 (`catalog.md:298-301`), S2 (`:324-327`), R1 (`:454-458`), and
R3 (`:504-509`) say "baseline" or "the reference" with no artifact named, and
the "Fixed reference identity" section covers only K1 and H1. The catalog
defers the executable to `/testing:test-strategy` (`catalog.md:73-75`). That
is a legitimate handoff, but until it lands none of these `always` checks is
assertable in code, and all sixteen records read `Exercised: not yet`. The
repository already holds the pattern the handoff needs: a frozen in-tree copy
of the baseline module used as a proptest oracle
(`crates/daemon/tests/selection_differential.rs:1-5`). The catalog's rule that
"a copied candidate helper cannot serve as the independent oracle"
(`catalog.md:75`) is consistent with that precedent only if the copy is taken
at the baseline commit and never regenerated; the record should say so.

**E2's check is one-sided.** The check asserts
`observed_live_units[c] <= reserved_units[c] <= configured_capacity[c]`
(`catalog.md:245-250`), but Impact names "missed release strands capacity"
(`catalog.md:259`). A leaked reservation makes `reserved_units` exceed
`observed_live_units`, which satisfies the inequality. The check cannot fail
on the second failure it claims to cover. A bounded quiescent clause is
needed (see the disposition table for wording).

**E1 and E3 each carry two guarantees.** E1's guarantee is "the current
callback completion gate precedes route cleanup" plus "relocation must extend
that gate to owned physical work" (`catalog.md:217-218`). E3 pairs "close
during a live current callback" with "extends this witness to physical work
if execution moves" (`catalog.md:270-271`). The first halves are observable
at HEAD; the second halves have no observable because no worker exists
(`catalog.md:44-50`, confirmed by `crates/daemon/src/lib.rs:8139-8205`,
which runs `run_transform()` synchronously inside the async handler). The
records label this BLOCKED honestly, but a single `always` check over two
guarantees with different constructibility invites a vacuous pass on the
current half being read as coverage of the future half. Lead (5) is
confirmed as stated.

**Check semantics match the guarantees elsewhere.** K3, K4, H3, and H4 assert
seeded or reference-derived preconditions, not candidate output, and name
constant markers (`fault-map.md:47-55`). They satisfy the coverage-check rule.
E3 correctly avoids requiring premature cleanup. The eleven `always` checks
are safety statements whose violation is a single false evaluation. No record
invents a liveness deadline (`catalog.md:101-103`).

**H2 mixes reachability classes.** H2 relies on "direct-API nonpositive
budgets disable that guard" (`catalog.md:376-378`). At HEAD the request path
filters `history_budget_tokens` with `is_finite() && *budget >= 0.0`
(`crates/daemon/src/lib.rs:8156-8159`), so a zero budget is
default-production reachable through the request, and a negative budget is
reachable only by calling `render_decayed_compartments` directly. The record
is labelled `default-production` as a whole (`catalog.md:371`). METHOD rule 4
asks for the label per record with its evidence; the negative clause needs
its own note.

**Routing recommendation.** K1, K2, H1, H2, R1, R2, R3, S1, and S2 are
value-comparison properties that a unit or integration test answers once the
reference is packaged; none needs timing exploration. K3, K4, H3, and H4 are
fixture-certification markers. E1, E2, and E3 need concurrency and a
non-yielding or detached fault and belong with
`/testing:deterministic-simulation-testing` after the worker topology exists.

## Coverage balance

Probe results, in the order the brief lists them.

**(a) Elided setup still denies writes inside read-only callbacks.** The
denial is connection state set per callback: `CallbackScope::read_only`
reads and sets `PRAGMA query_only` (`crates/storage/src/lib.rs:606-617`),
`install` snapshots `infrastructure_objects` and `main_schema_names`, checks
for temp shadows, and installs an authorizer closure that captures
`main_names` (`:624-650`), and `release` re-reads the infrastructure set and
restores the pragma (`:653-662`, `:684-695`). Every one of these is a
per-call cost that "reduced callback setup" (S1's guarantee,
`catalog.md:296-297`) would target. S1's check compares "restored connection
state" without naming what that state is. The existing test
`cached_statements_run_under_the_callback_scope`
(`crates/storage/src/lib.rs:3901-3943`) already pins that a cached write and a
cached pragma write are refused in a read scope; S1 should name it as the
warm-statement case rather than leaving it implicit under "reuse SQL after
authority changes" (`catalog.md:304-306`). Lead (2) is confirmed as a
refinement, not a gap: the canonical obligations
`read-callbacks-cannot-write` and `callback-scope-is-restored-after-unwind`
(`docs/properties/shared-primitives/catalog.md:691-719`) already hold the
denial and restoration invariants; S1 needs to name the observables it
compares.

**(b) Off-worker execution preserves cancellation observation.** At HEAD the
transform never observes cancellation. `handle` receives `RequestCtx`, whose
`cancel` token is `pub(crate)` (`crates/host-runtime/src/handler.rs:433-445`),
and forwards only `ctx.route` into `dispatch_value_with_inbound_bytes`
(`crates/daemon/src/lib.rs:11823-11825`, `:12558-12563`). The only
cancellation check on the transform path is in `settle_prepared_with`, after
the transform has returned (`:12057-12062`, wired at `:12072-12078`). The
host checks the token once before the handler starts
(`crates/host-runtime/src/dispatch.rs:888`) and otherwise aborts the task
(`:937-941`), which a synchronous transform does not observe. The wire
contract makes cancellation best effort with `outcome_unknown`
(`docs/host-wire-protocol.md:767`), and the catalog keeps that normative
(`catalog.md:534-536`). So lead (1), phrased as "a cancelled request does not
commit its transform", describes a property HEAD does not have and is
refuted as phrased. The real exposure is different and is not covered: see
the wildcard section on commit-versus-bookkeeping separation. E3 should also
add "cancel arrives while work is queued but not started" to its schedules,
because a worker that checks the token at dequeue changes which requests
commit even though the contract permits it.

**(c) fsync and durability when writes are folded or moved.** Every fenced
write runs `pin_fence_durability`, which sets `synchronous = FULL` and
re-asserts WAL because maintenance can lower them between transactions
(`crates/storage/src/lib.rs:299`, `:903-918`). This is two pragma statements
per write and an obvious elision target. The obligation is held by the
retained canonical record `protected-transactions-pin-fence-durability`
(`docs/properties/shared-primitives/catalog.md:721-734`), whose check
`always(synchronous == FULL && journal_mode == wal)` at the start of every
fenced transaction (`:728`) would catch an elided pin if the trace includes a
maintenance lowering between fenced writes. The hot-path catalog retains that
record by link (`catalog.md:540`). No new record is needed; S1 should list
`synchronous` and `journal_mode` among the compared connection state.

**(d) Plugin/TypeScript and shared-memory transport.** The scope names five
surfaces, all in the daemon and storage crates (`catalog.md:28-30`), and the
existing-checks deferrals name the host catalog for framing and connection
setup (`existing-checks.md:221-223`). Neither passage names
`packages/opencode-plugin/` or `crates/shm-transport/`. The transport is
therefore outside this catalog's declared scope by omission rather than by
statement. One dependency crosses that line: the K3/K4 byte threshold is
`MAX_READ_ROW_BYTES = MAX_WIRE_BODY_BYTES / 8`
(`crates/daemon/src/kernel_routes/read.rs:26`), where `MAX_WIRE_BODY_BYTES`
is `host_runtime::MAX_FRAME_BODY_LEN` (`crates/daemon/src/dispatch.rs:8`),
which is `shm_transport::MAX_FRAME_BYTES`
(`crates/host-runtime/src/wire.rs:38`), defined as 64 MiB
(`crates/shm-transport/src/arena.rs:4`). K3 and K4 hard-code "8 MiB"
(`catalog.md:172`, `:194-196`). A transport frame-cap change silently moves
both witnesses. The scope section should state the exclusion and the records
should name the constant chain.

**(e) `sometimes` witnesses for the S and R groups.** Confirmed: S1, S2, R1,
R2, and R3 are all `always` (`catalog.md:91-99`) and no record witnesses
their preconditions. The fault map substitutes "explicit case accounting"
(`fault-map.md:57-60`), which is the same intent without a marker. The K, E,
and H groups each carry one or two `sometimes` records; the S and R groups
carry none. The preconditions that need witnesses are concrete: an external
commit landed between two reads of one callback; a maintenance call lowered
`synchronous` or created a temp shadow between two guarded calls; a
statement warmed under a writable scope was reused under a read scope; a
detected input reached each of the six policy and layer pairs; a refusal
followed an earlier successful preparation in the same `PreparedWrite`.
Lead (3) is confirmed. This is a gap for targeted discovery, and its
distribution is also a bias (see below).

**(f) Infrastructure-rename detection under cached schema scans.**
`require_infrastructure_unchanged` compares the infrastructure-object set
read at `install` with the set read at `release`
(`crates/storage/src/lib.rs:628`, `:669-680`), and the authorizer closure
captures the `main_names` read at install (`:629`, `:645-647`). If a
statement-caching optimization caches either scan across callbacks, a
maintenance rename or `CREATE TABLE` between callbacks is either missed
(both cached) or reported as a false failure (one cached). S1's fault angle
names "rename" and "temp shadows" (`catalog.md:302-303`) but the check does
not name the two scans as per-call observations. This is a refinement to S1
folded into the same wording change as (a) and (c).

**(g) H-group reachability given `decay_render` runs only on HARD passes.**
`render_decayed_compartments` is reached in production only through
`render_m0` (`crates/daemon/src/memory_render.rs:191-193`), which
`compose_m0_for_context` calls on `PassPlan::Hard | MigrateHard`
(`crates/daemon/src/transform.rs:4031-4058`) and on pressure refold inside
the SOFT branch (`:4317-4344`). HARD and refold are scheduler decisions with
no configuration flag, so `default-production` is the right class. But the
transform-level states that H2's check names, "ordinary SOFT", "pressure
refold", and "pure Defer/SoftPlus" (`catalog.md:379-381`), have no
`sometimes` witness anywhere in the catalog; H3 and H4 witness renderer-level
pressure only (`catalog.md:405-408`, `:429-431`). A renderer-direct campaign
satisfies every H clause while never producing a refold or a replay pass.
That is a coverage gap, separate from the reachability label.

**Additional high-risk area with no record.** The SOFT branch computes the
pressure-refold predicate by tokenizing the frozen m0 payload and the m1 body
on every SOFT pass with the uncached `tokenizer::estimate_tokens`
(`crates/daemon/src/transform.rs:4298-4309`) and compares the counts against
`memory_update_count > 40`, `0.20 * history_budget_tokens`, and
`0.15 * m0_tokens` with `m0_tokens >= 500` (`:4310-4316`). This is
history-budget arithmetic on the per-turn hot path, and the comment at
`:1857-1859` that the estimator is HARD-only does not describe these two
calls. No record freezes the predicate, its constants, or its estimator. H2
takes the SOFT-versus-refold classification as given and constrains only
what each class preserves; an optimization that caches or approximates these
two counts changes which class a pass lands in, and every H record still
passes within each class. This is the largest gap the catalog's frame
misses.

**Type balance.** Eleven safety and five reachability records, no liveness.
The absence of liveness is deliberate and defensible (`catalog.md:101-103`).
Migration and restart scenarios are not in scope for a preservation
supplement and are correctly deferred to the canonical catalogs.

## Implementability

**Storage (S1, S2).** The second-connection snapshot test
(`crates/storage/src/lib.rs:3946-3983`) opens a raw `rusqlite::Connection`
on the same file, commits inside a `with_conn` callback, and observes
stability then freshness. `a_read_scope_restores_the_query_only_value_it_found`
(`:1689`) shows maintenance changing `query_only` between guarded calls, and
`cached_statements_run_under_the_callback_scope` (`:3901-3943`) warms a
cached statement in a fenced write and reuses it in a read scope. S1 and S2
are constructible today with `open_sqlite`, `tmp()`, `KV_BASELINE`, and a raw
second connection. The trace comparison itself still needs the packaged
reference.

**Redaction (R1, R2, R3).** `PreparedWrite.scans` is a private field
(`crates/memory-store/src/lib.rs:1953-1955`), and production code reads
`write.scans.len()` directly (`:7141`). R2's before-and-after scan
observation is constructible as an in-crate unit test beside the existing
ones at `:18171` and `:22694`. Twenty-one policy tests exist in
`crates/memory-store/tests/production_redaction.rs` (verified from `:68` to
`:1512`). R1's six-pair matrix has fixtures to reuse; R3's metadata-only
comparison needs the candidate, so it is blocked on the same packaging as
the other differentials.

**Canonical read (K1 to K4).** `kernel_route_fixtures` is a `test-support`
module for request and store-row builders
(`crates/daemon/src/lib.rs:12091-12092`), and
`crates/daemon/tests/kernel_routes.rs` has row-cap, byte-cap, and proptest
full-sort comparison tests (`:2002`, `:2122-2152`, `:2156-2195`).
K3 needs more than 8192 admitted visible candidates; K4 needs more than 8 MiB
of stored payload before a target. Both are seed-size questions, not
mechanism questions. K1's owner gate is real: `visible_as_of_in_scope`
decodes every candidate row inside `query_map` before the caller applies
domain and kind selection (`crates/kernel/src/admission.rs:3215-3221`,
`:3276-3277`), so a pushdown removes the error path for excluded corrupt
rows. The catalog states this correctly (`catalog.md:129-132`).

**History (H1 to H4).** The four goldens exist under
`crates/daemon/testdata/` and the real-estimator test at
`crates/daemon/src/decay_render.rs:765-807` compares bytes; its `fired`
counter increments on final fit or empty output (`:798-800`), exactly as H3's
evidence says. H3 and H4 need only the reference renderer and tokenizer,
which are the HEAD functions themselves until the reference is packaged.

**Execution lifecycle (E1 to E3).** This is where construction fails today.
The host test double supports `hang`, `await_cancel`, and `await_completion`
modes, all of which yield
(`crates/host-runtime/tests/support/mod.rs:514-531`), so `inner.abort()` at
`crates/host-runtime/src/dispatch.rs:940` takes effect and the tracker
empties. The fault E1 names, "a worker that has not observed abort"
(`catalog.md:222-223`), requires a handler that blocks its thread or
detaches a blocking task. The double has that pattern only on the bind path,
`BindPolicy::BlockThread`
(`crates/host-runtime/tests/support/mod.rs:416-420`), not on `handle`. Route
close with a non-yielding handler waits `route_close_budget` twice and trips
the fatal latch (`crates/host-runtime/src/dispatch.rs:1239-1259`; the budget
is five seconds at `crates/host-runtime/src/config.rs:185`). No test
exercises that branch in the inspected scope. E1's "independent ledger" of
live request-owned work does not exist as an observable; the `TaskTracker`
is internal. E2's byte classes need an allocation observer the harness does
not have; its count classes are observable through the double's
`dispatch_count()` and completion gate
(`crates/host-runtime/tests/support/mod.rs:187`, `:518-527`). A useful
existing seam for the commit-versus-bookkeeping gap below is the `cfg(test)`
hook `between_transform_and_prepare` (`crates/daemon/src/lib.rs:8224-8232`),
which runs test code between the first transform and historian preparation.

### Anchor verification

Anchors are cited as they appear in `catalog.md` or the evidence files and
were read at HEAD.

| Anchor | Status | Note |
| --- | --- | --- |
| `crates/daemon/src/lib.rs:8133-8202` (pass-read) | verified | Comment at 8133, closure 8139-8193, first call 8202. |
| `crates/daemon/src/lib.rs:8264-8375` (reruns) | verified | Reruns at 8264, 8291, 8317, 8372; awaits at 8263, 8289, 8315. |
| `crates/daemon/src/lib.rs:8155-8159` (validation) | verified | Filter is `is_finite() && >= 0.0`; zero passes. |
| `crates/daemon/src/lib.rs:11805-11826` (parse admission) | verified | `_parse_charge` at 11813; only `ctx.route` forwarded at 11824. |
| `crates/daemon/src/lib.rs:15408-15454` (footprint) | verified | Doc at 15408, `VALUE_ENVELOPE_BYTES` at 15454. |
| `crates/daemon/src/canonical_memory.rs:141-212` | verified | `read_project_memory` 141-175, `injectable_snapshot` 184-212. |
| `crates/daemon/src/kernel_routes/read.rs:23-26`, `:159-248` | verified | Caps at 23 and 26; `read_visible` 159-248; cutoff loop 208-224. |
| `crates/kernel/src/admission.rs:3113-3279` | verified | SQL 3113-3204, `ORDER BY o.object_id` at 3202, decode 3215-3277. |
| `crates/kernel/src/admission.rs:2880-2910` | verified | `visible_as_of_in_scope`. |
| `crates/kernel/src/slice/read.rs:140-158` | verified | Payload-size query doc and return. |
| `crates/daemon/src/config.rs:122` | verified | `memory_enabled: true`. |
| `crates/host-runtime/src/dispatch.rs:823-855`, `:857-934`, `:937-954`, `:1226-1268` | verified | Permits, tracked spawn, cancel arm, close gate. |
| `crates/host-runtime/tests/dispatch.rs:832-887` | verified | `closing_a_route_settles_its_admitted_work`. |
| `crates/host-runtime/tests/dispatch.rs:357-399` | verified | `cancel_and_completion_settle_exactly_once`. |
| `crates/host-runtime/src/handler.rs:277-295`, `:474-490` | verified | `InputBuffer`; `resident_capacity`. |
| `crates/host-runtime/src/wire.rs:430-481` | verified | `try_charge` through transfer. |
| `docs/host-wire-protocol.md#L765-L781` (E1 evidence) | verified, note | Link is 765-781; the evidence text at `evidence/route-cleanup-waits-for-request-owned-physical-work.md:20` says 767-781. Section heading is 765, cancellation text 767. Harmless. |
| `crates/storage/src/lib.rs:195-209`, `:229-245`, `:290-316` | verified | `lock_conn`, `with_conn`, `with_conn_fenced`. |
| `crates/storage/src/lib.rs:487-498` | verified | `prepare_cached` doc on scope expiry. |
| `crates/storage/src/lib.rs:604-705`, `:624-649`, `:652-705` | verified | `CallbackScope` install, release, restore, Drop. |
| `crates/storage/src/lib.rs:3901`, `:3946-3981`, `:3986`, `:4116` | verified | Cached-statement, snapshot, read-tx, rollback tests. |
| `crates/memory-store/src/lib.rs:2204-2243`, `:2245-2271`, `:2350-2424` | verified | `prepare_field`, `execute`, audit writer. |
| `crates/memory-store/src/lib.rs:3428-3459`, `:3462-3494`, `:5563-5586`, `:8306`, `:8921-8931` | verified | Core and transaction preparation, facade scope, callers. |
| `crates/memory-store/src/lib.rs:465`, `:2133-2143`, `:18171-18212`, `:22694-22725` | verified | Bound constant, JSON scans, receipt test, growth test. |
| `crates/memory-store/tests/production_redaction.rs` 21 anchors | verified | All 21 line numbers open `fn` declarations. |
| `crates/daemon/src/decay_render.rs:296-338`, `:289-338`, `:301-303`, `:321-338` | verified | Renderer and guard loop. |
| `crates/daemon/src/decay_render.rs:765-800` (tight golden) | verified, note | Test body runs to 807; `fired` at 770 and 798-800; final assertion 802-806 is outside the cited range. |
| `crates/daemon/src/m0_compose.rs:178-215` | verified | Retry loop 205-213; multiplier `*= 1.15` at 210. |
| `crates/daemon/src/memory_render.rs:9`, `:191-200` | verified | `M0_EMPTY_BODY`; effective budget at 191. |
| `crates/daemon/src/token_cache.rs:165-180`, `:188-205` | verified | `cached_estimate_tokens`; parity test. |
| `crates/tokenizer/src/lib.rs:123-149` / `:123-155` | verified | `estimate_tokens` is at 148-150 within both ranges. |
| `crates/daemon/src/transform.rs:4031-4058`, `:4310-4344`, `:4455-4507` | verified | HARD compose, refold predicate and compose, SOFT units. |
| `crates/cache-stability/src/lib.rs:221-287` | verified | Defer and SOFT transitions. |
| `crates/daemon/Cargo.toml:21-32` | verified | `tokenizer = { workspace = true }` at 32. |
| `crates/daemon/src/kernel_routes/ingest.rs` 20 anchors | verified | Constants, budget, guards, decode, finish, and 12 tests. |

No anchor is off. Two carry range notes above.

## Wildcard

**The catalog asserts an evaluation that did not exist.** `catalog.md:19-23`
says the separate review, local dispositions, and independent recheck "are
recorded in portfolio-evaluation.md", and `catalog.md:586-587` says "The
independent recheck is complete." Before this file was written the linked
file did not exist; the directory held `portfolio-evaluation-initial.md`,
which the brief does not mention and this evaluator did not read. Three
evidence files also cite "the independent review" as the trigger for K4 and
H4 and for K3's resolution
(`evidence/canonical-memory-byte-pressure-is-exercised.md:8`,
`evidence/history-outer-retry-pressure-is-exercised.md:8`,
`evidence/canonical-memory-selection-pressure-is-exercised.md:52`). The
catalog therefore incorporates corrections from a review whose text is not
on disk under the name the catalog links, and describes this document's
contents before it was written. METHOD rules 1 and 2 forbid stating an
unverified claim as fact. This is a provenance problem for a human, not a
record defect.

**Commit-versus-bookkeeping separation under relocation.** In the ordinary
(non-Emergency) path there is no `.await` between the transform commit at
`crates/daemon/src/lib.rs:8202` and the in-memory bookkeeping that follows:
lineage-root insertion (`:8209-8214`), projection-cache store (`:8387`), and
`guidance_dates` removal on `response.committed` (`:8398-8403`). The awaits
at `:8263`, `:8289`, and `:8315` sit only inside the Emergency95 branch. So
at HEAD a host abort cannot separate a committed transform from its
bookkeeping on the common path. Moving `run_transform` to `spawn_blocking`
introduces an `.await` immediately after the commit, and a blocking task
cannot be cancelled once started, so an abort landing there leaves the
transform committed and the bookkeeping skipped. Lineage roots self-heal
from a durable table (`:2936-2939`, `:4489-4520`). `guidance_dates` does not:
`guidance_date_for_transform` returns the pinned entry until it is removed
(`:4648-4655`), so the next pass's `ProducerContext.guidance_date`
(`:8175`) can carry a stale line. This is the failure class behind lead (1),
stated correctly: not "a cancelled request must not commit", which HEAD does
not guarantee, but "a committed transform's derived in-memory state is
either applied or provably recomputed on the next pass". No record covers
it. E1 and E2 talk about route state and resource charges; neither names
these three structures.

**The rerun structure is a relocation hazard the E group does not name.**
`run_transform` is a closure over borrowed request state that the handler
invokes up to four times per request (`:8202`, `:8264`, `:8291`, `:8317`,
`:8372`), each time reusing the pinned `project_memory` (`:8133-8138`). A
`spawn_blocking` relocation needs `'static` captures, so `parsed`,
`project_memory`, and the projection-cache input must be cloned or shared.
K1's evidence records the reruns and the pinning; E2's ledger says "No
transform-worker transfer exists here" for parse residency
(`evidence/request-work-accounting-covers-retained-resources.md:25`) but
does not say that an uncharged clone of the request body is the concrete
way the charge would be lost. A one-line addition to E2's fault angle closes
that.

**The baseline route-close observable changes under relocation.** With a
synchronous transform, route close cannot observe abort, waits two budgets,
and trips fatal (`dispatch.rs:1239-1259`; the comment at `:1245-1246` says
so). With a detached worker the tracker empties promptly and cleanup runs
while work continues. E1's check describes the second hazard. It should also
state the first as the baseline behavior being traded away, because a
"correct" relocation that no longer trips fatal on a long transform is a
behavior change that a preservation test comparing only cleanup ordering
would report as an improvement rather than as a difference to adjudicate.

## Finding disposition

| # | Finding | Class | Records | Proposed action |
| --- | --- | --- | --- | --- |
| 1 | Differential records other than K1 and H1 name no frozen reference; even K1 and H1 defer the executable. | refinement | K2, S1, S2, R1, R2, R3 | Extend "Fixed reference identity" to every differential record: "The reference for this record is the corresponding function set at commit `913234433ae36a80a6e22c6aac14c7f9aab74386`, packaged as a frozen in-tree copy taken at that commit and never regenerated, following `crates/daemon/tests/selection_differential.rs`." |
| 2 | E2's inequality cannot fail on a missed release, which Impact names. | refinement | E2 | Append to Check: "and after a bounded fault-free drain (no admitted work, all completions observed, one full `route_close_budget` elapsed), `reserved_units[c] == 0` for every class `c`." |
| 3 | E1 and E3 each fold a current observable and a future unobservable into one guarantee. | refinement | E1, E3 | Split each Guarantee into two sentences and mark the second: "Current: <first half>. Relocation: <second half> - not constructible at HEAD; see the BLOCKED note." Keep `always` and `sometimes`; the Exercised line already says not yet. |
| 4 | H2's negative-budget clause is direct-API only; zero is request-reachable. | refinement | H2 | Reachability note: "default-production for zero budgets through `parsed.history_budget_tokens` (`crates/daemon/src/lib.rs:8156-8159` admits `>= 0.0`); the negative-budget clause is reachable only through the direct renderer API and is test-only." |
| 5 | S1's "restored connection state" and per-call setup steps are unnamed; covers probes (a), (c), (f) and lead (2). | refinement | S1 | Replace "restored connection state" with: "restored connection state, meaning `query_only`, authorizer presence, `synchronous`, `journal_mode`, and an infrastructure-object set and `main_names` read at this callback's install rather than from a cache (`crates/storage/src/lib.rs:606-650`, `:669-680`, `:903-918`); include a statement warmed under a fenced scope and reused under a read scope (`:3901-3943`)." |
| 6 | H2's outer clause omits the tightening step that determines rerender output. | refinement | H2 | After "at-most-three rerenders" add: "each rerender divides the inner budget by a multiplier that grows by 1.15 per attempt (`crates/daemon/src/m0_compose.rs:210`, `crates/daemon/src/memory_render.rs:191`)". |
| 7 | K3 and K4 hard-code 8 MiB; the constant derives from the transport frame cap. | refinement | K3, K4 | Replace "8 MiB" with "`MAX_READ_ROW_BYTES` (`MAX_WIRE_BODY_BYTES / 8`, currently 8 MiB, derived from `shm_transport::MAX_FRAME_BYTES`)" and add to the scope section: "The TypeScript plugin and `shm-transport` are outside this catalog except where a constant derives from them." |
| 8 | No `sometimes` witness for any S or R precondition. | gap | S1, S2, R1, R2, R3 | Targeted discovery for five witness records, one per precondition listed under probe (e); dedupe against the fault map's case accounting. |
| 9 | Commit-versus-bookkeeping separation is a new failure class under relocation; lead (1) refuted as phrased. | gap | E group | Targeted discovery: "a committed transform's derived in-memory state (`transform_session_roots`, projection cache, `guidance_dates`, serialized-output cache) is applied or recomputed on the next pass"; use the `between_transform_and_prepare` seam. |
| 10 | SOFT-path pressure-refold predicate tokenizes m0 and m1 every pass and is unconstrained. | gap | H group | Targeted discovery: freeze the predicate at `crates/daemon/src/transform.rs:4298-4316` (constants 40, 0.20, 0.15, 500; uncached `tokenizer::estimate_tokens` on frozen m0 and m1 body) as a classification-preservation record, plus a `sometimes` witness that a pass crossed each threshold. |
| 11 | No `sometimes` witness that a campaign produced a HARD pass, a pressure refold, or a pure Defer/SoftPlus pass. | gap | H2 | Targeted discovery for three transform-level witnesses so H2's SOFT, refold, and replay clauses cannot pass vacuously. |
| 12 | E3 lacks the "cancel while queued" schedule that a worker makes possible. | refinement | E3 | Add to Required faults: "include a cancel that arrives after admission and before the worker dequeues the work; the contract permits either outcome (`docs/host-wire-protocol.md:767`), and the witness records which occurred." |
| 13 | Host test double has no non-yielding or detached handler mode, so E1/E3's named fault is not constructible. | refinement | E1, E3 | Add to Required faults: "a handler mode that blocks its thread or detaches a `spawn_blocking` task past cancellation, analogous to `BindPolicy::BlockThread` (`crates/host-runtime/tests/support/mod.rs:416-420`); `hang` yields and does not construct this fault (`:528-531`)." |
| 14 | E1 does not state the baseline route-close observable (fatal trip after two budgets) that relocation trades away. | refinement | E1 | Add to Fault/timing angle: "At HEAD a non-yielding transform makes route close wait two `route_close_budget` intervals and trip fatal (`crates/host-runtime/src/dispatch.rs:1239-1259`); a relocation that instead empties the tracker while work continues changes this observable and must be adjudicated, not silently accepted." |
| 15 | E2's ledger does not name the uncharged request-body clone as the concrete loss mechanism under relocation. | refinement | E2 | Add to Fault/timing angle: "a `'static` clone of `parsed` or `project_memory` for a blocking worker is an uncharged copy unless `_parse_charge` moves with it (`crates/daemon/src/lib.rs:11813`)." |
| 16 | Catalog claims a completed recheck recorded in a file that did not exist and pre-describes this document. | bias | all | Human decision on provenance wording; see below. |
| 17 | Every `always` record is a differential against an unpackaged oracle; sixteen of sixteen are `Exercised: not yet`. | bias | all | Human decision on sequencing; see below. |
| 18 | `sometimes` records exist only where an earlier review pointed; S and R have none. | bias | K3, K4, H3, H4, S, R | Human decision on a systematic witness pass; see below. |

Counts: 4 gaps (8, 9, 10, 11), 11 refinements (1 to 7, 12 to 15), 3 biases
(16 to 18).

## Gaps queued

1. S and R precondition witnesses (finding 8). Five `sometimes` records, one
   each for: external commit between two reads of one read callback;
   maintenance change (`synchronous`, temp shadow, or rename) between two
   guarded calls; warm statement reused across a scope change; detected input
   reaching each policy and layer pair; refusal after an earlier successful
   preparation in the same `PreparedWrite`. Dedupe against
   `fault-map.md:57-60`.
2. Commit-versus-bookkeeping consistency under abort (finding 9). One safety
   record with `always` semantics over the four in-memory structures named
   in the wildcard section, plus a `sometimes` witness that an abort landed
   between commit and bookkeeping. Constructible at HEAD only in the
   Emergency95 branch; constructible everywhere once a blocking worker
   exists.
3. SOFT pressure-refold predicate preservation (finding 10). One safety
   record freezing the predicate and its estimator, and one `sometimes`
   record per threshold crossing.
4. Transform-level pass-class witnesses (finding 11). Three `sometimes`
   records: a HARD pass composed m0 with nonempty compartments; a SOFT pass
   took the pressure-refold branch; a pure Defer/SoftPlus pass replayed a
   retained prefix after a HARD.

Re-evaluation is warranted if item 3 or 2 lands, because each opens a
category the current catalog does not have (a classification-preservation
record; a derived-state consistency record).

## Biases for a human

**Provenance (finding 16).** `catalog.md:19-23` and `:586-587` state that
an independent recheck is complete and recorded in `portfolio-evaluation.md`.
That file did not exist before this evaluation, and this evaluation is not
the document those sentences describe. A file named
`portfolio-evaluation-initial.md` exists and is not referenced by the
catalog. Judgment required: either rename and link the initial file as the
prior review and rewrite the two passages to say what it is, or delete the
claim. Evidence files citing "the independent review" should name it.

**Sequencing (finding 17).** All eleven `always` records are differentials,
and none can execute until the reference is packaged (`catalog.md:73-75`).
The catalog is internally consistent about this, but the practical effect is
that the entire preservation contract is unexecutable until one packaging
task lands, and that task is routed to a skill rather than tracked as a
blocker. Judgment required: whether to make reference packaging an explicit
gate before any optimization surface (M1, M5, or the others) is opened.

**Orientation of coverage witnesses (finding 18).** K4 and H4 were added,
and K3 was narrowed, in response to a prior review
(`evidence/canonical-memory-byte-pressure-is-exercised.md:8`,
`evidence/history-outer-retry-pressure-is-exercised.md:8`,
`evidence/canonical-memory-selection-pressure-is-exercised.md:52`). The S and
R groups, which no review pointed at, have no witnesses. The H lens followed
the estimator into `decay_render` and did not examine the two direct
tokenizer calls on the SOFT path (finding 10). Judgment required: whether to
run one systematic precondition-witness pass over all sixteen records rather
than accept the four queued gaps as sufficient.
