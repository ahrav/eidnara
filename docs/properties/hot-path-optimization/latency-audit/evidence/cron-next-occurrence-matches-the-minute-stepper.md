# cron-next-occurrence-matches-the-minute-stepper

Baseline: `913234433ae36a80a6e22c6aac14c7f9aab74386`, 2026-09-10.
The [scope and provenance](../catalog.md#scope-and-provenance) apply here.

## Discovery trigger

The cron search steps one epoch minute at a time up to a four-year cap, on
the scheduler task. A field-jumping replacement is the obvious optimization.
The wildcard pass asked what the stepper's answer depends on and found three
things a jumper can get wrong: DST handling through local civil fields, Vixie
day semantics, and unsatisfiable-but-valid expressions that must return
`None` at the cap rather than an instant or a panic.

## Evidence trail

- [`parse_cron`][parse] accepts five numeric fields with day-of-month `1..31`
  independent of month, and sets `dom_restricted` and `dow_restricted` from a
  leading `*`. [`matches_day`][vixie] implements Vixie OR semantics when both
  are restricted.
- [`next_occurrence`][stepper], kept verbatim at HEAD as the test-only
  `frozen_minute_stepper`, starts at the minute boundary after `after_ms`,
  caps at `after_ms + min(MAX_SEARCH_MS, max_search_ms)`, evaluates
  `tz.timestamp_millis_opt(cursor_ms).single()?` per minute so an
  unrepresentable instant returns `None`, and matches minute, hour, month, and
  day on the local civil fields. Its doc says evaluating each epoch minute by
  civil fields handles DST transitions.
- [`MAX_SEARCH_MS`][cap] is `4 * 366 * 24 * 60 * MINUTE_MS`, so an
  unsatisfiable expression pays 2_108_160 iterations per call.
- [`next_cron_occurrence`][occurrence] applies the parser's own cap and
  filters `ms != 0`; [`is_valid_conditional_note_cron`][valid] is
  `parse_cron(...).is_some()`. Conditional Notes use a smaller ceiling through
  `next_due_at_ms` at [`:649-653`][note-cap] with the same zero filter.
- The memory_classifier scheduler's [`next_due`][sched-due] calls
  `next_cron_occurrence(schedule, now_ms, &chrono::Local)` and treats `None`
  as `i64::MAX`. The schedule [defaults to `None`][sched-default]; the config
  key accepts any string [`is_valid_conditional_note_cron`][sched-accept] accepts
  and warns otherwise.
- Existing checks: [Vixie `*`-prefix semantics][t-vixie], [`None` at `i64`
  extremes][t-extreme], and the [fixture-timezone golden][t-golden-cron].

## Failure scenario

A jumper computes the next civil match and converts once to an instant. On a
spring-forward day `30 2 * * *` has no civil 02:30, which the stepper skips
by construction; a jumper that builds the instant from fields may fire at an
adjacent hour or a different day. On fall-back the stepper returns the earlier
of the two instants with the same civil time; a jumper may return the later.
For `0 0 30 2 *` the stepper returns `None` after the cap; a jumper that
searches by field may loop or return a February 30 that normalizes into
March. The scheduler consumes the value directly as its due time.

## Timing windows and dependencies

None in time inside the function. The stepper runs synchronously on the
scheduler task, so an unsatisfiable schedule costs the full cap on every
`next_due` call; the property forbids running past that cap.

## What a test must construct

A differential against [`next_occurrence`][stepper] as frozen at HEAD over
`Local` with a DST-bearing zone and over `Utc`, for `30 2 * * *` on
spring-forward, `0 0 30 2 *`, `0 0 31 4,6,9,11 *`, `0 0 29 2 *`, and
`after_ms` at the `i64` extremes, comparing `Option<i64>`. The
[wildcard checks](../existing-checks.md#wildcard-and-cross-cutting) list the
Vixie test, the extreme-instant test, and the golden; none covers DST or an
unsatisfiable expression. Reachability is explicit-config-only: the memory_classifier
schedule defaults to `None` and conditional notes need a cron on the note.

The frozen copy is a test-only function kept because the change replaces
`next_occurrence` itself; the live function cannot remain the oracle.

## Investigation log

### Q: Should validation reject calendar-impossible dates instead?

- Sources examined: [`parse_cron`][parse], [`sched-accept`][sched-accept],
  [`is_valid_conditional_note_cron`][valid].
- Findings: Validation is a grammar check; `0 0 30 2 *` is accepted and never
  fires. Rejecting it changes config acceptance and the warning surface,
  which is outside a latency change.
- Missing evidence: A product decision on config strictness.
- Conclusion: needs human input.

### Q: Does the field-jumping search match the stepper, and is it worth it? (at the field-jumping head)

- Sources examined: [`next_occurrence`][jump-head], the
  [`CronZone`][zone-head] claims, [`advance_while_offset_holds`][advance-head],
  and the [differential tests][diff-head].
- Findings: The search keeps the stepper's first cursor, cap, and
  unrepresentable-instant `None`, and skips only minutes whose civil fields
  cannot match while the offset holds. Each zone type claims how its offsets
  change: UTC and fixed offsets never change; IANA zones keep each offset at
  least an hour, so probes every hour and a bisection stop a skip at the
  first minute with another offset; `Local` claims the hour bound only when
  `TZ` is unset or names a zone file, because a POSIX rule in `TZ` can place
  changes minutes apart, and then the search visits every minute. A review
  built that counterexample (`TZ="STD0DST,J60/1:10,J60/2:40"`, `15 2 * * *`):
  an hourly probe at both ends of a 30-minute daylight window jumps past the
  match. The test zone `DailyDaylight` reproduces it and shows the
  `Unbounded` claim equals the stepper while an overclaimed `HourApart` does
  not. `zdump` over tzdata 2026c's 599 zones and a parse of chrono-tz
  0.10.4's tables put the closest transitions about 6.9 days apart; a review
  also ran 31 million cases around 42,957 transitions of every chrono-tz zone
  from 1800 to 2045 with no difference. Negative controls, run locally and
  reverted: dropping the offset check fails the transition sweep and the
  shift test, returning the probe instead of the bisected minute fails the
  shift test, searching an `Unbounded` zone with hourly probes fails the
  daylight-window test, and an off-by-one in the midnight or next-minute skip
  fails four to six tests.
- Payoff: An uncommitted probe on one host, release build, `taskset -c 5`, 21
  start instants per arm, three runs. Medians before then after:
  `0 0 30 2 *` at the full cap 25.1 ms to 34.2 us in UTC and 78.4 ms to
  1.12 ms in New York; `30 2 * * 1` 175-177 us to 2.9 us; `0 9 1 * *`
  1.55-1.58 ms to 23.1 us; `*/15 * * * *` 0.3 us to 0.2 us. The ranges do not
  overlap for the first four, so the change stays. A `Local` zone from a POSIX
  rule keeps the stepper's cost.
- Missing evidence: Custom zone files with offset runs shorter than an hour.
- Conclusion: resolved with answer - the search matches the stepper on every
  tested input, and every zone claims only the bound its data supports.

### Q: How close does the search run to the cost of its zone lookups? (at the lookup-floor head)

- Sources examined: [`next_occurrence`][floor-search], the
  [date bound][floor-bound], the [month scan][floor-month], the
  [impossible-date check][floor-never], the [byte parser][floor-parse],
  [`deterministic_jitter_ms`][floor-jitter], the
  [component tests][floor-tests], and chrono 0.4.45's `offset/local/unix.rs`
  and `tz_info/timezone.rs`.
- Findings: Most of a search's cost is `Local` lookups. Each one reads
  `SystemTime::now` (about 26 ns) to check chrono's one-second zone cache,
  borrows a thread-local, and binary-searches the zone's transitions: 81 ns
  in New York, 63 ns for a UTC zone file, and 105 ns for a POSIX rule on the
  probe host. The search now returns `None` without a lookup when no date can
  match; takes a minute-precise date bound before its first lookup, and again
  only once the bound's civil minute is out of reach; scans dates a listed
  month at a time with day bitmasks; parses the five fields in one byte scan;
  hashes the jitter seed without a `String`; stops `HourApart` skips at the
  bound's horizon and at the cap, and lets one skip cross two days; and
  rejects an `Unbounded` minute by its time of day before reading its date.
  A counting zone shows what remains: two lookups for `*/15 * * * *`, about one
  per hour until the match for daily and weekday schedules, and 26 to 34 for
  weekly through yearly ones, because an offset of almost a day could carry
  any minute of the preceding day onto the first listed civil time. Measured
  New York and UTC costs are 91 to 98 percent of lookups times lookup cost
  plus the parse and the `TZ` read; a POSIX-rule zone costs about one lookup
  per minute. Under the `HourApart` claim a 60-minute probe gap is the widest
  that cannot hide a run of another offset, and `FixedOffset`'s range fixes
  the one-day window, so fewer lookups need a stronger claim.
- Payoff: An uncommitted probe on one host, release build, pinned to one
  CPU, paired against the previous head in two worktrees over 44 scenarios in
  New York, a UTC zone file, and a POSIX rule. The geometric mean of
  per-scenario time ratios went from 0.99 (the same code against itself) to
  0.46; in New York `0 0 30 2 *` went from 1.87 us to 35 ns and the other
  searches shortened by about a quarter to two fifths, so the change stays.
  These are local probe numbers, not approved bench evidence.
- Negative controls (run locally, then reverted): rounding the bound's epoch
  minute a minute late, excluding the current minute from the bound, a
  lookahead skip one day too far, a 28-day February in the impossible-date
  check, a weekday rotation off by one, a truncating remainder for the time of
  day of an `Unbounded` minute, reversed jitter digits, and an unfolded
  day-of-week `7` each fail at least one test.
- Missing evidence: A run length beyond one hour for zone files and POSIX
  rules. A probe every six days (tzdb's closest transitions are 6.9 days
  apart) measured New York's weekly search at about 0.48 us, and treating a
  POSIX rule with month-long runs as `HourApart` measured its weekly search at
  about 3.3 us. Both strengthen what `CronZone` claims.
- Conclusion: resolved with answer - under the current claims the search
  runs within about 10 percent of its lookup cost; a stronger zone claim is a
  design decision (needs human input).

[cap]: ../../../../../crates/daemon/src/conditional_note_evaluation.rs#L31-L34
[parse]: ../../../../../crates/daemon/src/conditional_note_evaluation.rs#L122-L175
[vixie]: ../../../../../crates/daemon/src/conditional_note_evaluation.rs#L188-L197
[stepper]: ../../../../../crates/daemon/src/conditional_note_evaluation.rs#L2078-L2101
[valid]: ../../../../../crates/daemon/src/conditional_note_evaluation.rs#L622-L632
[occurrence]: ../../../../../crates/daemon/src/conditional_note_evaluation.rs#L630-L632
[note-cap]: ../../../../../crates/daemon/src/conditional_note_evaluation.rs#L649-L653
[t-golden-cron]: ../../../../../crates/daemon/src/conditional_note_evaluation.rs#L1557
[t-extreme]: ../../../../../crates/daemon/src/conditional_note_evaluation.rs#L2568
[t-vixie]: ../../../../../crates/daemon/src/conditional_note_evaluation.rs#L2582
[sched-due]: ../../../../../crates/daemon/src/memory_classifier_scheduler.rs#L735-L737
[sched-default]: ../../../../../crates/daemon/src/config.rs#L155
[sched-accept]: ../../../../../crates/daemon/src/config.rs#L1249-L1267
[jump-head]: ../../../../../crates/daemon/src/conditional_note_evaluation.rs#L254-L366
[advance-head]: ../../../../../crates/daemon/src/conditional_note_evaluation.rs#L567-L606
[diff-head]: ../../../../../crates/daemon/src/conditional_note_evaluation.rs#L2078-L2565
[zone-head]: ../../../../../crates/daemon/src/conditional_note_evaluation.rs#L202-L245
[floor-search]: ../../../../../crates/daemon/src/conditional_note_evaluation.rs#L247-L366
[floor-bound]: ../../../../../crates/daemon/src/conditional_note_evaluation.rs#L450-L518
[floor-month]: ../../../../../crates/daemon/src/conditional_note_evaluation.rs#L402-L448
[floor-never]: ../../../../../crates/daemon/src/conditional_note_evaluation.rs#L377-L393
[floor-parse]: ../../../../../crates/daemon/src/conditional_note_evaluation.rs#L51-L181
[floor-jitter]: ../../../../../crates/daemon/src/conditional_note_evaluation.rs#L673-L701
[floor-tests]: ../../../../../crates/daemon/src/conditional_note_evaluation.rs#L2623-L2791
