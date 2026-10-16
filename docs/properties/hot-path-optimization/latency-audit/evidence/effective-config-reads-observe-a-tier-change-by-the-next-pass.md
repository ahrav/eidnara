# effective-config-reads-observe-a-tier-change-by-the-next-pass

Baseline: `913234433ae36a80a6e22c6aac14c7f9aab74386`, 2026-09-10.
The [scope and provenance](../catalog.md#scope-and-provenance) apply here.

## Discovery trigger

The history_summarizer fire decision merges the user and project config tiers on every
pass, with a deep clone of the result and an unconditional read of the
guidance override file. A cache of the merged result is the obvious change.
The wildcard pass asked what staleness contract the current code gives and
what a merged cache could break: the one-pass edit latency, and the
privilege rules that a project tier must never bypass.

## Evidence trail

- [`effective_config`][eff-cfg] locks the handler's single `ConfigCache` and
  calls [`effective_for_project`][eff-proj], which resolves the user path
  from `XDG_CONFIG_HOME` and `HOME` per call and delegates to
  [`effective_from_sources`][eff-warn].
- At the baseline, `effective_with_warnings` called
  [`read_tier_cached`][tier-cached] twice (one `fs::metadata` each; a
  same-path, same-mtime, no-warning hit clones the cached `Value`), ran
  [`merge_tiers_with_warnings`][merge], called
  [`resolve_user_guidance_override`][guidance] (a `metadata` and a full
  bounded read of the override file on every call, no mtime gate), and
  returned a clone of the merged config on every call. At HEAD that work is
  the memo miss path of [`effective_from_sources`][eff-warn], which returns
  a new `Arc` at [`:636`][eff-clone]. Its doc at [`:515-519`][eff-warn-doc]
  says tier read failures are reported on every load so a long-running
  daemon keeps surfacing them.
- One [`ConfigCache`][cache-struct] holds one `user` and one `project`
  [`TierConfig`][tier-struct] (path, mtime, value, warning). A different
  `project_root` changes `path`, so two roots alternating re-read the file
  every call.
- [`merge_tiers_with_warnings`][merge] applies user values first, then
  project values per key class; [`ProjectRaiseOnly`][raise-only] applies a
  project value only when it tightens the user value.
- Per-pass callers: [`maybe_spawn_reattach`][call-reattach],
  [`prepare_history_summarizer_fire`][call-fire] after the state load, pending-rewrite,
  and live-history_summarizer early returns ([`:5462-5549`][fire-early]), and the
  wrapup path at [`:5956`][call-wrapup]. [`bind`][call-bind] freezes a copy
  into `SessionBinding`, whose doc at [`:248-249`][binding-doc] says config
  can change while the route stays open.
- The mtime test at [`:2644-2682`][t-mtime] rewrites the user file with the
  original mtime restored and asserts the old value is returned, then
  asserts a new mtime reloads; that is the staleness contract at HEAD.

## Failure scenario

A merged-result cache keyed by nothing but the user path returns another
project's merged config to a second route, and a project tier gains a
privileged value it could not set. A cache that skips the override read
serves stale guidance for the life of the daemon. A cache that stores the
merged result and re-applies a fresh project tier onto it, instead of merging
fresh user and project contents, lets `ProjectRaiseOnly` compare against an
already-raised value.

## Timing windows and dependencies

An edit between two passes; two routes on different project roots alternating
passes; the override edited without touching a tier. The same-mtime edit is
already ignored at HEAD and stays ignored.

## What a test must construct

Rewrite the user and project tiers with later mtimes between passes and
compare the next `effective_config` result to a fresh merge; configure
`prompt_surface.guidance_override_path`, edit the file alone, and check the
next pass; bind two project roots and alternate them; assert the bound
`SessionBinding.config` is unchanged throughout. The
[wildcard checks](../existing-checks.md#wildcard-and-cross-cutting) cover the
mtime cache and three privilege tests; none covers override staleness or two
roots sharing one cache.

Use passes that reach the `prepare_history_summarizer_fire` call at `lib.rs:5550`:
not a subagent pass (`:9067`), state load succeeds, no `pending_rewrite`, and
no live history_summarizer completion pending (`:5462-5549`); the check is on each
call. Unit tests built with `fixed_config` (`lib.rs:4276`, returned at
`:4881-4884`) bypass the cache and cannot exercise this record.

## Investigation log

### Q: Keep reporting tier read failures on every load?

- Sources examined: [`:515-519`][eff-warn-doc], [`:520-638`][eff-warn],
  [`read_tier_cached:725-761`][tier-cached].
- Findings: The cache bypasses its hit path when `warning` is set, so a
  failing file is re-read and re-warned on every call. The doc states this as
  intent. A merged cache that stores the result silences the repeat unless it
  also stores and re-emits the warning.
- Missing evidence: A decision on the warning surface.
- Conclusion: needs human input. Superseded by the memo entry below.

### Q: Should the history_summarizer read the bind-frozen config instead?

- Sources examined: [`:5550`][call-fire], [`:13073-13074`][call-bind],
  [`:248-249`][binding-doc].
- Findings: The binding doc freezes the fallback history budget because
  config can change while the route stays open; the history_summarizer deliberately
  reads a fresh merge per pass. Switching it to the frozen copy removes the
  per-pass cost and the one-pass edit latency together.
- Missing evidence: A specification decision.
- Conclusion: needs human input. Superseded by the memo entry below.

### Q: How does the merged memo keep warnings and tier changes? (decision at the memo head)

- Sources examined: the [memo][memo-head], [`SourceIdentity`][identity-head],
  [`effective_from_sources`][eff-warn-head], and the [memo tests][t-memo-head].
- Findings: The daemon maintainer's decisions are to keep per-load warning
  re-emission and to keep the history_summarizer reading a fresh merge per
  pass rather than the bind-frozen config. The memo therefore stores the
  merge's warnings and returns them on every hit, and `effective_for_project`
  emits them each load. A tier with a read failure is never memoized, so it is
  re-read and re-warned on every load as the doc states. The memo is one
  entry keyed on the identities of the user tier, the project tier (whose
  path names its root), and the guidance override; each identity is the path
  plus device, inode, length, mtime, and ctime from `lstat`, matching the
  `NOFOLLOW` reads. Every write and attribute change
  moves the ctime, and a merge is memoized only when every source's ctime is
  more than one second before the load, so a second rewrite inside one
  coarse timestamp tick is still read; the mtime takes no part in that gate,
  so a tier stamped into the future by a tool or a skewed clock is memoized
  once its ctime settles; the override's change-on-every-read behavior
  holds. A tier or override whose read failed is never memoized. A source
  is also memoized only when its pre-read `lstat` stamp equals the stamp of
  the file the read opened: `read_bounded_file` returns the descriptor's
  `fstat`, `read_tier_cached` keeps it as the tier's stamp and on a
  same-mtime hit takes the followed `stat` instead, and
  `resolve_user_guidance_override` returns it on success. A dangling final
  symlink keeps its own stamp when its target is created, a file created or
  removed between the stamp and the read would key the merge on an entry it
  did not read, a link placed over a cached tier whose target carries the
  cached mtime is served from the tier cache's value, and a directory
  exchanged under the path between the stamp and the read leaves both
  files' inodes and ctimes untouched, so a memo keyed on the pre-read stamp
  would replay the other file once the directories swap back; none of these
  is memoized.
  Alternating roots replace the entry, so admitted-tier state for a root is
  read only on a miss, as before. The retained bytes are bounded by
  `MERGED_CONFIG_RETAINED_BYTES_BOUND` (4 MiB), added to
  `DECLARED_RETAINED_RESIDENT_BYTES`. The estimate counts each vector at its
  capacity, because deduplicating the model chain shortens it in place and
  keeps its buffer, and charges each `cache_ttl` map entry three inline
  entry sizes for its share of a partially filled B-tree node.
- Payoff: An uncommitted probe timed `effective_with_warnings` on one host,
  release build, `taskset -c 5`, with a user tier naming a 4 KiB guidance
  override and a small project tier, 200 samples of 50 calls, three runs.
  Before, medians were 54.83-54.96 us; after the review fixes, 11.60-11.61
  us. The payoff holds while one project root is active; alternating roots
  replace the one-entry memo on every call. The p10-p90
  ranges do not overlap, so the change stays.
- Missing evidence: None for this record. A same-ctime rewrite is outside
  the identity and also outside the tier contract at HEAD.
- Conclusion: resolved with answer - warnings re-emit on every load and the
  per-pass merge stays, served from the memo while every source identity
  matches.

[eff-cfg]: ../../../../../crates/daemon/src/lib.rs#L4880-L4889
[binding-doc]: ../../../../../crates/daemon/src/lib.rs#L248-L249
[call-reattach]: ../../../../../crates/daemon/src/lib.rs#L5141
[fire-early]: ../../../../../crates/daemon/src/lib.rs#L5462-L5549
[call-fire]: ../../../../../crates/daemon/src/lib.rs#L5550
[call-wrapup]: ../../../../../crates/daemon/src/lib.rs#L5956
[call-bind]: ../../../../../crates/daemon/src/lib.rs#L13073-L13074
[tier-struct]: ../../../../../crates/daemon/src/config.rs#L256-L266
[cache-struct]: ../../../../../crates/daemon/src/config.rs#L269-L275
[eff-proj]: ../../../../../crates/daemon/src/config.rs#L471-L481
[eff-warn-doc]: ../../../../../crates/daemon/src/config.rs#L515-L519
[eff-warn]: ../../../../../crates/daemon/src/config.rs#L520-L638
[eff-clone]: ../../../../../crates/daemon/src/config.rs#L636
[tier-cached]: ../../../../../crates/daemon/src/config.rs#L725-L761
[guidance]: ../../../../../crates/daemon/src/config.rs#L794-L858
[merge]: ../../../../../crates/daemon/src/config.rs#L1085
[raise-only]: ../../../../../crates/daemon/src/config.rs#L885-L887
[t-mtime]: ../../../../../crates/daemon/src/config.rs#L2644-L2682
[memo-head]: ../../../../../crates/daemon/src/config.rs#L371-L468
[identity-head]: ../../../../../crates/daemon/src/config.rs#L312-L369
[eff-warn-head]: ../../../../../crates/daemon/src/config.rs#L515-L638
[t-memo-head]: ../../../../../crates/daemon/src/config.rs#L2701-L3106
