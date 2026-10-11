# Fault map: lexical retrieval

Fault classes the ten records require, where each is available, and the
cheapest valid oracle. The records are reconstructed from #391's acceptance
criteria: AC1 routes to the two literal-probe records, AC2 to ordering, AC3 and
AC4 to eligibility and the one-policy scan, AC5 to bounded work, AC6 to the
original budget, AC7 to payload isolation, AC8 to host application, and AC10 to
the single index. AC9, the RP2.9 corpus run, has no record; see gap 2 in
`portfolio-evaluation.md`. Every safety record runs while its fault is active; the
reachability record needs a host campaign. Locations are verified against the
#391 branch at `b3b9b7a80`.

## Fault classes and availability

| Class | Injection seam | Available in |
| --- | --- | --- |
| Query syntax in request text | request strings holding `OR`, `NEAR`, `*`, `"`, parentheses, and column filters; `probe_matches` (`crates/retrieval/tests/lexical_engine.rs:80`) for the engine, `probes` (`crates/retrieval/tests/lexical_retrieval.rs:164`) for `retrieve` | `lexical_engine.rs`, `lexical_retrieval.rs`, `lexical_analysis.rs` |
| Unquoted control | the raw expression passed to `matches` (`lexical_engine.rs:71`) | `lexical_engine.rs:213` |
| Zero atoms | separator-only and empty request text | `lexical_retrieval.rs:582`, `lexical_engine.rs:268` |
| Equal raw ranks across probes, objects, or storage orders | `corpus()` rows (`lexical_retrieval.rs:228`) and `project_bulk` (`:958`) with identical text; permuted and duplicated probe lists | `lexical_retrieval.rs:641`, `:682`, `:1950`, `:2253` |
| Dead rows inside and past the scan bound | `tombstone_raw` (`lexical_retrieval.rs:974`); an orphaned lexical row | `lexical_retrieval.rs:867`, `:885`, `:985`, `:1038`, `:1987`, `:2199`, `:2470`, `:2605` |
| Ineligible leader | a scope decision that hides the comparator's first occurrence; `batch_rows` of one | `lexical_retrieval.rs:716`, `:750`, `:2253` |
| Retirement, correction, or snapshot move after a grant | `retrieve_with_hook_for_test` (`crates/retrieval/src/lexical/retrieve.rs:331`) between admission batches; a kernel restore; a foreign project | `lexical_retrieval.rs:1150`, `:1173`, `:1203`, `:1243`; `crates/daemon/tests/claim_eligibility.rs:184` |
| A second eligibility caller | a planted `judge_eligibility` mention in any workspace member's `src` | `claim_eligibility.rs:456` (the scan; the plant is manual) |
| Match counts at and past the thresholds | `project_thresholds` (`lexical_retrieval.rs:1549`), `small_thresholds` (`:2136`), a scan bound one below the match count, a result cap of one | `lexical_retrieval.rs:789`, `:1069`, `:1576`, `:2145`, `:2312`, `:2381` |
| Cancellation or deadline inside a phase | `before_phase` hooks in the route (`crates/daemon/src/query_route.rs:1383` for `Phase::Lexical`) | `crates/daemon/tests/query_route.rs:74` |
| Engine interrupt mid-statement | `with_conn_interruptible` with a stop closure the progress handler polls every `READ_INTERRUPT_STEPS` (`crates/storage/src/lib.rs:191`) | `lexical_retrieval.rs:1407`, `:2049` |
| Budget that ends between probes or holds a kernel reader | an `EvalBudget` deadline between probes | `lexical_retrieval.rs:1300`, `:1516` |
| Payload table absent | `ALTER TABLE payloads RENAME` before `retrieve` | `lexical_retrieval.rs:1275` |
| Schema drift | the frozen inventory in `schema_inventory.rs` against `crates/retrieval/baseline.sql:94` | `crates/retrieval/tests/schema_inventory.rs:312` |
| Prefix or expansion path | the prefix-operator control | `lexical_engine.rs:248`, `:233`, `:340` |
| One million occurrences under open-loop load | `lexical_scan_p99_at_one_million_occurrences` (`lexical_retrieval.rs:1724`); `#[ignore]`, release, D21 host | one `ubuntu-latest` run, failed the 50 ms gate |
| Host application through fusion and packing | none in the repository | needs host owners (AC8) |

## Required faults per record

| Record | Required faults and enabling state | Witnessed |
| --- | --- | --- |
| `literal-probes-never-operate` | operator and syntax-shaped request text; the unquoted control | yes |
| `zero-terms-run-no-match` | separator-only and empty text; a nonempty control | yes |
| `rank-then-occurrence-order` | several probes hitting one occurrence; equal ranks from distinct probes; a large equal-rank group at the bound; dead rows inside a distinct-rank probe | yes |
| `eligibility-before-accepted-slots` | a hidden leader; equal-byte occurrences of distinct objects; `batch_rows` of one | yes |
| `one-kernel-eligibility-policy` | retirement and correction after a grant; a foreign project; a planted caller outside the allowed files | agreement yes; the planted caller is a manual check, run once at `b3b9b7a80` |
| `lexical-work-is-bounded-and-observed` | a repeated probe; a scan bound one below the match count; a result cap of one; one more matching row under each threshold; 32 more rows past the common thresholds | counters yes; no approved bound to compare with |
| `original-budget-stops-sql` | cancellation and a passed deadline before each phase; an interrupt during each statement | yes |
| `retrieval-reads-no-payload` | a projection whose `payloads` table is renamed | yes |
| `no-alternative-lexical-index` | none; the schema is static | yes |
| `host-application-matrices` | real OpenCode and Pi application capability; the production f32 oracle; the RP2.9 corpus | no |

## Coverage checks to add

- A planted-caller test for the architecture scan: write a temporary `.rs`
  file under a workspace member's `src`, run the scan function against it, and
  require the failure. Today the scan is proven only by a manual plant.
- A cancellation that lands inside the in-memory rank sort
  (`retrieve.rs:731`), to time the window between the last row read and the
  next `budget.check()` under `production_bounds`.
- An absolute bound on `sql_steps` and on allocation events per probe, once
  RP2.9 approves one. Awaits the open question in
  `lexical-work-is-bounded-and-observed`.
- A `lexical_scan_p99_at_one_million_occurrences` run on the D21 host, which
  is the only run the D26b gate accepts.
- A host matrix cell for each of OpenCode and Pi that applies a lexical
  contribution and captures the applied context. Awaits the open question in
  `host-application-matrices`.

## Oracle cost ranking

Cheapest valid oracle first.

1. AC1 and AC10 on the bare engine: an in-memory FTS5 connection, a handful of
   rows, hand-written term sets; milliseconds.
2. AC1, AC2, AC4, AC7 on a projected store: `open_store` plus `corpus()`, 64-row
   bounds, kernel judgments through the batch adapter; tens of milliseconds per
   test.
3. AC5 counters: the same store with `project_thresholds` fixtures built to the
   D26b counts; the allocation test installs a recording global allocator and
   warms up once.
4. AC6: the daemon route with per-phase hooks and an interruptible connection;
   a tokio multi-thread runtime per test.
5. AC3 architecture scan: reads every `.rs` file under every workspace member;
   about half a second.
6. AC5 scale gate: a one-million-occurrence projection built in about 700 s,
   600 open-loop queries, release build, the D21 host.
7. AC8: a host campaign with real application capability; not runnable here.
