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
Preservation authority: implementation tickets
[#428](https://github.com/ahrav/eidnara/issues/428) (mode gate) and
[#429](https://github.com/ahrav/eidnara/issues/429) (schema-version keyed
snapshot and once-per-connection durability pin), and
[#430](https://github.com/ahrav/eidnara/issues/430) (connection-open pragmas
and statement-cache capacity), and the
[parent specification](https://github.com/ahrav/eidnara/issues/350).

The store installs [one authorizer per connection][gate-install] at open. The
authorizer reads a [connection mode][mode] under the store's lock:
`Unrestricted` allows everything, `Guarded` applies `deny_scope_escapes`
against the main-schema names of the [schema snapshot][snapshot] the callback
took at entry, and `Baseline` applies `deny_baseline_escapes`. The four situations the specification names
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

The [snapshot][snapshot] is [keyed][snapshot-cache] on the header schema
version and the pager data version, read at callback entry inside the
callback's transaction. Any main-schema DDL on any connection bumps the schema
version, so the [rename test][rename-test] shows the callback after a foreign
`ALTER TABLE ... RENAME` denying a temp shadow of the new name and allowing the
old one. The schema version alone is not an integrity key: it is a stored
header field a foreign connection can write back, and a `writable_schema`
edit moves it not at all. Two mechanisms close that. The data version is
computed by the pager and stored nowhere, and every foreign commit moves it,
so the [forged-version test][forge-test] shows a rename followed by a write of
the old schema version still rescanned. SQLite expires a cached statement only
when the schema cookie moves, so that rescan also [flushes the statement
cache][snapshot-cache] when it changes the main-schema names under an unchanged
schema version; the [rescan-flush test][rescan-flush-test] shows a cached
`CREATE TEMP TABLE late (x)` refused `not authorized` after a foreign
`CREATE TABLE late` that wrote the old schema version back, with no temp
`late` created. The same forged cookie leaves SQLite's parsed schema stale,
so that rescan also runs `PRAGMA writable_schema = RESET`; the
[parsed-schema test][parsed-schema-test] shows a callback after a foreign
rename plus a written-back version resolving `kv2` and refusing `kv`. An
oversized snapshot also clears the retained one, so the
comparison never runs against a policy older than the cached statements; the
[unretained-policy test][unretained-policy-test] caches the statement under an
oversized foreign schema, restores the cookie the store last saw, and shows
the same refusal. On the store connection
[defensive mode][defensive] turns `PRAGMA schema_version = N` and
`PRAGMA writable_schema = ON` into no-ops, which the
[defensive test][defensive-test] pins. The temp-schema shadow scan stays
uncached: the temp schema has no version to key on, it is normally empty, and
the rename test shows a shadow that maintenance leaves is still refused. At
release the [infrastructure comparison][infra-check] is satisfied by an
unchanged schema version and rescans otherwise; the guarded policy denies every
statement that could move the version inside a callback, so the rescan branch
is a defense against an authorizer gap, and the [release test][release-test]
exercises both branches directly.

The durability pin [runs once per connection][pin]: at open, and again only
after the maintenance path, whose [exit guard][maintenance-exit] discards the
snapshot, re-arms the pin, and flushes the statement cache, including when the
callback unwinds. The [pin test][pin-test] shows a fenced write leaving a
raw-connection `synchronous=OFF` alone and the first fenced write after
maintenance re-pinning `FULL`; the [unwind test][unwind-test] shows a
panicking maintenance callback still re-arming the pin, discarding the
snapshot, and leaving its temp shadow to be refused. The
[read-durability test][durability-test] keeps the maintenance-lowers,
fenced-re-pins case, and the journal-mode refusal is unchanged. Another
connection cannot leave WAL while this one holds the database open, and
`synchronous` is connection-local, so the pin holds between maintenance
callbacks; the [foreign-WAL test][foreign-wal-test] accepts a second
connection's `PRAGMA journal_mode = DELETE` returning the unchanged `wal` mode
or a `DatabaseBusy` error while the store is idle between callbacks, and
shows the next fenced write still in WAL. A snapshot is retained only within
[`SCHEMA_SNAPSHOT_RETAINED_BYTES_BOUND`][bound], measured over the collections'
heap including hashbrown buckets, the trailing control group, and the `Arc`
counts, which the daemon adds to its declared
retained-resident total once per storage connection; the
[bound test][bound-test] shows an oversized snapshot serving its callback
without being kept.

SQLite evaluates the authorizer at prepare time, and a statement taken from
the cache is not re-authorized. The gate keeps that safe by construction:

- Only guarded callbacks populate the cache. `MaintenanceConn` exposes no
  cached preparation, the store's own fence and pragma statements run through
  uncached `execute` and `query_row`, and the [maintenance path flushes the
  cache][flush] through a [drop guard][maintenance-exit] when its callback ends,
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
barrier for the temp database as well as main. The connection-open unit
([#430](https://github.com/ahrav/eidnara/issues/430)) resolved the open
question with those two tests: `query_only` blocks temp-database writes, it is
the read callback's only write barrier because `deny_scope_escapes` allows
DML on every non-infrastructure table, and the read path therefore keeps its
two pragma statements.

The connection-open path owns the resource pragmas. The memory store's
[connection profile][profile] reads `PRAGMA page_size` and the
`MAX_MMAP_SIZE` compile option, derives `cache_size` in pages from a byte
budget and the measured page size, caps `mmap_size` at the compile-time
maximum, and reads back what took effect; the [profile test][profile-test]
checks each derivation. `temp_store` stays at its default: SQLite bounds a
sorter's in-memory list at `cache_size` and spills the rest only to a
file-backed temp store, so a temp store held in memory removes the bound and
keeps every sorted row in the heap. The [sort-spill test][sort-spill] sorts
four page-cache budgets of rows through a read callback and shows the library
retaining about one budget. The
[resource-pragma test][resource-pragmas] shows read and fenced callbacks denied
`cache_size`, `temp_store`, and `mmap_size` writes while the maintenance-set
values stand. The memory store sets an explicit
[statement-cache capacity][capacity] covering its distinct cached texts, with
the kernel store's capacity as precedent. The [eviction probe][eviction-probe]
extends the statement-reuse probe: a handle handed out with no runs after a
returned handle of the same text had run means the cache re-created it, and an
undersized cache shows exactly that. The [steady-pass test][pass-probe] shows a
warm pass and four steady passes on one growing session preparing more distinct
texts than the default capacity would hold, fewer than the configured capacity
with a quarter of it as headroom, and re-creating no statement; the distinct
count is the load-bearing claim, since the cache evicts only on insertion past
capacity. The budgets are declared, not measured; the W1 measurement contract
owns their effect.

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

`cargo test -p storage --locked` passed 68 tests after the mode gate: the 58
that passed before it, four internal gate tests, and six store-level tests. The
denial-matrix tests that existed at the baseline pass unchanged. After the
schema-version keyed snapshot and the once-per-connection pin the same command
passed 76 tests: seven internal tests (key reuse and replacement, defensive
mode, forged version, retained bound, release comparison, pin, maintenance
unwind) and the rename test were added. After the connection-open pragmas and
the statement-cache capacity it passed 81 tests: the 76 above, the three
tests the `origin/main` merge brought in, the resource-pragma denial rows, and
the eviction probe (an earlier revision counted 78 and omitted the three merged
tests); `cargo test -p memory-store --locked`
gained the connection-profile test and `cargo test -p daemon --locked` the
steady-pass eviction test.

After the flush moved into a drop guard, `cargo test -p storage --locked`
passed 72 tests at `perf/mode-gated-authorizer` merged with `origin/main`:
the 68 above, three tests the merge brought in, and the flush-on-unwind test.
Review-time verification: the new test failed with `Ok(())` in place of
`not authorized` before the guard and passes with it. On this branch the
flush is one action of the maintenance exit guard, and the same command
passes 80 tests: the 76 above, the three from `origin/main`, and the
flush-on-unwind test. With the rescan flush, the parsed-schema reload, the
unretained-policy check, and the foreign-WAL check it passes 84 tests. Merged
with this branch's resource-pragma rows and eviction probe
(`perf/schema-version-keyed-callback-cache` at `3958bb8`), the same command
passes 86 tests. The anchors below are to that merged tree.

[lock]: https://github.com/ahrav/eidnara/blob/9132344/crates/storage/src/lib.rs#L195-L209
[read]: https://github.com/ahrav/eidnara/blob/9132344/crates/storage/src/lib.rs#L229-L245
[write]: https://github.com/ahrav/eidnara/blob/9132344/crates/storage/src/lib.rs#L290-L316
[live-read]: ../../../../crates/storage/src/lib.rs#L343-L360
[live-write]: ../../../../crates/storage/src/lib.rs#L409-L473
[scope]: https://github.com/ahrav/eidnara/blob/9132344/crates/storage/src/lib.rs#L624-L705
[cache]: https://github.com/ahrav/eidnara/blob/9132344/crates/storage/src/lib.rs#L487-L498
[facade]: https://github.com/ahrav/eidnara/blob/9132344/crates/memory-store/src/lib.rs#L5563-L5586
[notes]: https://github.com/ahrav/eidnara/blob/9132344/crates/memory-store/src/lib.rs#L5999-L6025
[scope-owners]: https://github.com/ahrav/eidnara/blob/9132344/crates/memory-store/src/lib.rs#L4772-L4823
[mode]: ../../../../crates/storage/src/lib.rs#L810-L822
[gate-install]: ../../../../crates/storage/src/lib.rs#L1639
[flush]: ../../../../crates/storage/src/lib.rs#L373-L383
[scope-install]: ../../../../crates/storage/src/lib.rs#L1182-L1284
[mode-hold]: ../../../../crates/storage/src/lib.rs#L1085-L1087
[apply]: ../../../../crates/storage/src/lib.rs#L2146-L2184
[gate-tests]: ../../../../crates/storage/src/lib.rs#L2307-L2646
[probe]: ../../../../crates/storage/src/lib.rs#L5027-L5124
[read-witness]: ../../../../crates/storage/src/lib.rs#L5313-L5342
[temp-write]: ../../../../crates/storage/src/lib.rs#L5349-L5372
[restore-test]: ../../../../crates/storage/src/lib.rs#L5378-L5430
[baseline-test]: ../../../../crates/storage/src/lib.rs#L5436-L5466
[surface-test]: ../../../../crates/storage/src/lib.rs#L5473-L5493
[cached-test]: ../../../../crates/storage/src/lib.rs#L4980
[snapshot]: ../../../../crates/storage/src/lib.rs#L830-L840
[snapshot-cache]: ../../../../crates/storage/src/lib.rs#L995-L1017
[infra-check]: ../../../../crates/storage/src/lib.rs#L1257-L1272
[bound]: ../../../../crates/storage/src/lib.rs#L864
[rename-test]: ../../../../crates/storage/src/lib.rs#L5131-L5171
[pin-test]: ../../../../crates/storage/src/lib.rs#L2545-L2583
[bound-test]: ../../../../crates/storage/src/lib.rs#L2494-L2515
[durability-test]: ../../../../crates/storage/src/lib.rs#L4278-L4343
[forge-test]: ../../../../crates/storage/src/lib.rs#L2458-L2488
[defensive]: ../../../../crates/storage/src/lib.rs#L944
[defensive-test]: ../../../../crates/storage/src/lib.rs#L2432-L2452
[release-test]: ../../../../crates/storage/src/lib.rs#L2521-L2537
[pin]: ../../../../crates/storage/src/lib.rs#L1023-L1039
[maintenance-exit]: ../../../../crates/storage/src/lib.rs#L565-L574
[unwind-test]: ../../../../crates/storage/src/lib.rs#L2588-L2637
[profile]: ../../../../crates/memory-store/src/lib.rs#L496-L538
[profile-test]: ../../../../crates/memory-store/src/lib.rs#L15582-L15609
[sort-spill]: ../../../../crates/memory-store/src/lib.rs#L15616-L15641
[capacity]: ../../../../crates/memory-store/src/lib.rs#L478
[resource-pragmas]: ../../../../crates/storage/src/lib.rs#L5176-L5214
[eviction-probe]: ../../../../crates/storage/src/lib.rs#L5220-L5279
[pass-probe]: ../../../../crates/daemon/src/lib.rs#L24656-L24807
[flush-test]: ../../../../crates/storage/src/lib.rs#L5495-L5537
[rescan-flush-test]: ../../../../crates/storage/src/lib.rs#L5581-L5620
[foreign-wal-test]: ../../../../crates/storage/src/lib.rs#L5544-L5574
[unretained-policy-test]: ../../../../crates/storage/src/lib.rs#L5665-L5711
[parsed-schema-test]: ../../../../crates/storage/src/lib.rs#L5626-L5659
