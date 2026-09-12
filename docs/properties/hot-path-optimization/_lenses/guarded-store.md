# Guarded-store surface

This summarizes separate prior read-only surface discovery supplied in the
task, not a new test run or portfolio evaluation. Anchors are rechecked in
`/local/home/ahrav/scratch/eidnara` at
`913234433ae36a80a6e22c6aac14c7f9aab74386` on 2026-09-10. The
[external-evidence scope](../catalog.md#scope-and-provenance) is pending final
confirmation; historical citations and exercise are not carried forward.

[The connection mutex][mutex] is synchronous. [Read callbacks][read] open a
deferred snapshot; [fenced writes][write] claim authority inside an immediate
transaction. [Scope installation][scope] checks preexisting shadows and installs
the authorizer; release checks infrastructure names to catch rename effects.

[The cache documentation][cache] says installation expires prepared statements.
This is a baseline mechanism, not an obligation to reprepare every statement
on every optimized call. Installing a non-NULL authorizer expires statements;
clearing it is not assumed to do so. S1 instead checks observable authority.

[Facade scopes][facade] are installed on the executing thread while the
connection lock is held. Their absence is legitimate for nonfacade operations.
S2 preserves [within-call snapshot and next-call freshness][snapshot]. Neither
record permits batching unrelated operations into one snapshot, weaker
durability, or a schema/version ledger.

[mutex]: ../../../../crates/storage/src/lib.rs#L240-L277
[read]: ../../../../crates/storage/src/lib.rs#L305-L321
[write]: ../../../../crates/storage/src/lib.rs#L369-L433
[scope]: ../../../../crates/storage/src/lib.rs#L887-L969
[cache]: ../../../../crates/storage/src/lib.rs#L722-L730
[facade]: ../../../../crates/memory-store/src/lib.rs#L5563-L5586
[snapshot]: ../../../../crates/storage/src/lib.rs#L4796-L4833
