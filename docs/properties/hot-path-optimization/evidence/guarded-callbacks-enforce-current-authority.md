# guarded-callbacks-enforce-current-authority

Baseline: `913234433ae36a80a6e22c6aac14c7f9aab74386`, 2026-09-10.
The [scope and provenance](../catalog.md#scope-and-provenance) apply here.
The discovery and investigation sections describe that baseline. Their source
links are pinned to it. The mode-gate evidence below describes the live store.

## Discovery trigger

Callback setup and statement reauthorization cost can motivate caching. The
contract to preserve is authorization at use, not a mandatory preparation rate.

## Evidence trail

- [storage/lib.rs:195-209][lock] acquires a synchronous mutex and rejects
  same-thread reentry. [229-245][read] installs a read-only callback scope.
- [290-316][write] prechecks and claims the fence, pins durability, and commits
  only after the guarded callback and successful scope release.
- [624-705][scope] checks temp shadows, installs the authorizer, compares
  infrastructure names to detect rename effects, and restores scope state.
- [487-498][cache] documents prepared-statement expiry on scope installation.
- [memory-store/lib.rs:5563-5586][facade] installs caller/domain/route scopes
  inside the connection-locked callback; [5999-6025][notes] does the same for
  note callers. [4772-4823][scope-owners] records thread ownership and restoration.

## Failure scenario

A statement cached under a writable or different facade scope runs after the
scope changes without equivalent authorization. A maintenance-created shadow
redirects a later unqualified query, or unwind leaves the next caller's scope
incorrect. These are effect-level failures, even if statement caching succeeds.

## Timing windows and dependencies

Maintenance can change connection-local schema between independent callbacks.
Installing a non-NULL authorizer expires statements; clearing is not assumed
to do so. The optimized mechanism must account for changed authority regardless
of whether SQLite prepares again. Nonfacade calls legitimately lack facade scope.

## What a test must construct

Alternate read, fenced write, and maintenance calls; warm cached SQL, then
change authority. Include preexisting shadows, infrastructure rename attempts,
callback errors, panic, and contended facade callers. Compare results, refusals,
durable effects, and the next call's restored state against the baseline.
[Existing checks](../existing-checks.md#guarded-store) remain unaudited.
At the discovery baseline no authority-transition experiment ran.

## Investigation log

### Q: What makes reduced setup safe after authority or maintenance changes?

- Sources examined: [Scope installation][scope], [cache documentation][cache],
  and [facade scope placement][facade].
- Findings: The baseline re-establishes scope per callback and uses current
  same-thread facade ownership. Caching alone does not replace that contract.
- Missing evidence: A proposed invalidation mechanism and discriminating tests
  are not supplied.
- Conclusion: Observable authority is the resolved requirement; setup elision
  remains unresolved. No schema ledger or weakened durability is authorized.

## Mode-gate evidence

Implementation base: `96709d0ef54bcfad2327878ab96e118fb8ba4969`; bundled SQLite
3.51.3 through rusqlite 0.39.0.
Preservation authority: [implementation ticket](https://github.com/ahrav/eidnara/issues/428)
and [parent specification](https://github.com/ahrav/eidnara/issues/350).

The store installs [one authorizer per connection][gate-install] at open. The
authorizer reads a [connection mode][mode] under the store's lock:
`Unrestricted` allows everything, `Guarded` applies `deny_scope_escapes`
against the main-schema names captured at callback entry, and `Baseline`
applies `deny_baseline_escapes`. The four situations the specification names
map onto three policies: a read-only callback and a fenced write share the
`Guarded` policy and differ only by `query_only`, which SQLite checks when a
statement runs. A callback [enters its mode][scope-install] after the
temp-shadow scan, which stays uncached, and a [drop guard][mode-hold] returns
the connection to `Unrestricted` on release and on unwind; entering a mode
while one is held is an assertion in every build. The [read path][live-read] still toggles
`query_only` around the mode; the [fenced path][live-write] still prechecks the
fence, pins durability, and claims inside the immediate transaction. The
[baseline DDL][apply] on a pristine file runs under `Baseline`; the marker and
version writes that follow run under `Unrestricted`.

SQLite evaluates the authorizer at prepare time, and a statement taken from
the cache is not re-authorized. The gate keeps that safe by construction:

- Only guarded callbacks populate the cache. `MaintenanceConn` exposes no
  cached preparation, the store's own fence and pragma statements run through
  uncached `execute` and `query_row`, and the [maintenance path flushes the
  cache][flush] through a [drop guard][flush-guard] when its callback ends,
  on return and on unwind. The [internal gate tests][gate-tests]
  pin both halves: an unrestricted-prepared fence upsert is reused without
  re-authorization when the cache is not flushed, and is refused with
  `not authorized` after the flush. The [surface test][surface-test] shows no
  fence or pin statement is found in the cache after an open.
- Both guarded callback kinds share one prepare-time policy, so a statement
  one caches may run in the other; the [cached-statement test][cached-test]
  shows the cached write still meets `query_only` in the read callback.
- The only verdict that depends on the entry snapshot is the temp-shadow
  denial. A temp object can shadow a main name only after main gained that
  name; any temp DDL on the connection expires every prepared statement, and a
  foreign main-schema change is seen by the next callback's entry snapshot,
  whose stale main cookie resets the temp schema as well, so every cached
  statement re-prepares under that callback's snapshot. The
  [statement-reuse probe][probe] prepares `CREATE TEMP TABLE late (x)` after
  its own temp DDL and never runs it, so the cached program is valid when a
  second connection creates main `late`; the next callback refuses it
  `not authorized`, creates no temp `late`, and reads the new table.
- Main DDL on the store's own connection is the case the cookie does not
  cover. `sqlite3EndTable` emits `ChangeCookie` and `ParseSchema` but no
  `OP_Expire`, and it keeps the in-memory main cookie equal to the file's, so
  no schema reset reaches the temp schema; a cached `CREATE TEMP TABLE`
  statement verifies only the temp schema when it runs and stays valid. The
  maintenance flush is what discards it, and the [flush-on-unwind
  test][flush-test] pins that a maintenance callback that creates main `late`
  and then panics still leaves the next fenced callback's cached
  `CREATE TEMP TABLE late (x)` refused `not authorized`, with no temp `late`
  created. Before the drop guard the statement ran and created the shadow.

The [probe][probe] reads `SQLITE_STMTSTATUS_REPREPARE` on a cached insert: it is
zero across two consecutive fenced callbacks, and it rises after the foreign
DDL above the count the temp DDL left. The [read-path witness][read-witness]
pins the remaining expiry: `query_only` is a flag pragma and SQLite expires
every statement on the connection for each one, so the counts over read,
read, fenced, fenced calls are `0, >=1, higher, unchanged`. The
[temp-write test][temp-write] shows `query_only` is the read path's write
barrier for the temp database as well as main. Removing the toggle is the
open question of the connection-open unit
([#430](https://github.com/ahrav/eidnara/issues/430)); both tests name the
behavior that unit would change.

The [restoration test][restore-test] shows maintenance regaining its pragma
writes after a panicking read and a panicking fenced callback, `query_only`
restored, the partial fenced write rolled back, and the next fenced write
re-pinning `synchronous=FULL`. The [baseline test][baseline-test] bypasses the
scratch check and shows the store connection's own gate refusing a pragma
write, an `ATTACH`, a `BEGIN`, a `SAVEPOINT`, a fence-row insert, and a
format-marker delete inside baseline text, then opens the same pristine file
with benign text. The [internal baseline table][gate-tests] covers the full
denial set of `deny_baseline_escapes` plus its allowed DDL.

The permanently installed authorizer is invoked per resolved column reference
on every prepared statement, including the store's own. This record proves
the mechanism; the latency effect is a W1 measurement and is not claimed here.

### Focused execution, 2026-09-12

`cargo test -p storage --locked` passed 68 tests after the change: the 58 that
passed before it, four internal gate tests, and six store-level tests added
here. The denial-matrix tests that existed at the baseline pass unchanged.

After the flush moved into a drop guard, `cargo test -p storage --locked`
passed 72 tests at `perf/mode-gated-authorizer` merged with `origin/main`:
the 68 above, three tests the merge brought in, and the flush-on-unwind test.
Review-time verification: the new test failed with `Ok(())` in place of
`not authorized` before the guard and passes with it. The anchors below are to
the live tree at that state.

[lock]: https://github.com/ahrav/eidnara/blob/9132344/crates/storage/src/lib.rs#L195-L209
[read]: https://github.com/ahrav/eidnara/blob/9132344/crates/storage/src/lib.rs#L229-L245
[write]: https://github.com/ahrav/eidnara/blob/9132344/crates/storage/src/lib.rs#L290-L316
[live-read]: ../../../../crates/storage/src/lib.rs#L305-L321
[live-write]: ../../../../crates/storage/src/lib.rs#L369-L433
[scope]: https://github.com/ahrav/eidnara/blob/9132344/crates/storage/src/lib.rs#L624-L705
[cache]: https://github.com/ahrav/eidnara/blob/9132344/crates/storage/src/lib.rs#L487-L498
[facade]: https://github.com/ahrav/eidnara/blob/9132344/crates/memory-store/src/lib.rs#L5563-L5586
[notes]: https://github.com/ahrav/eidnara/blob/9132344/crates/memory-store/src/lib.rs#L5999-L6025
[scope-owners]: https://github.com/ahrav/eidnara/blob/9132344/crates/memory-store/src/lib.rs#L4772-L4823
[mode]: ../../../../crates/storage/src/lib.rs#L709-L720
[gate-install]: ../../../../crates/storage/src/lib.rs#L1317
[flush]: ../../../../crates/storage/src/lib.rs#L334-L343
[flush-guard]: ../../../../crates/storage/src/lib.rs#L522-L535
[scope-install]: ../../../../crates/storage/src/lib.rs#L867-L969
[mode-hold]: ../../../../crates/storage/src/lib.rs#L770-L779
[apply]: ../../../../crates/storage/src/lib.rs#L1824-L1862
[gate-tests]: ../../../../crates/storage/src/lib.rs#L1985-L2087
[probe]: ../../../../crates/storage/src/lib.rs#L4472-L4562
[read-witness]: ../../../../crates/storage/src/lib.rs#L4564-L4598
[temp-write]: ../../../../crates/storage/src/lib.rs#L4600-L4628
[restore-test]: ../../../../crates/storage/src/lib.rs#L4630-L4686
[baseline-test]: ../../../../crates/storage/src/lib.rs#L4688-L4722
[surface-test]: ../../../../crates/storage/src/lib.rs#L4724-L4749
[flush-test]: ../../../../crates/storage/src/lib.rs#L4751-L4793
[cached-test]: ../../../../crates/storage/src/lib.rs#L4418-L4460
