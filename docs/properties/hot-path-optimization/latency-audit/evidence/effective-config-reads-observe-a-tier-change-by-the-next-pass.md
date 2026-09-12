# effective-config-reads-observe-a-tier-change-by-the-next-pass

Baseline: `913234433ae36a80a6e22c6aac14c7f9aab74386`, 2026-09-10.
The [scope and provenance](../catalog.md#scope-and-provenance) apply here.

## Discovery trigger

The historian fire decision merges the user and project config tiers on every
pass, with a deep clone of the result and an unconditional read of the
guidance override file. A cache of the merged result is the obvious change.
The wildcard pass asked what staleness contract the current code gives and
what a merged cache could break: the one-pass edit latency, and the
privilege rules that a project tier must never bypass.

## Evidence trail

- [`effective_config`][eff-cfg] locks the handler's single `ConfigCache` and
  calls [`effective_for_project`][eff-proj], which resolves the user path
  from `XDG_CONFIG_HOME` and `HOME` per call and delegates to
  [`effective_with_warnings`][eff-warn].
- [`effective_with_warnings`][eff-warn] calls [`read_tier_cached`][tier-cached]
  twice (one `fs::metadata` each; a same-path, same-mtime, no-warning hit
  clones the cached `Value`), runs [`merge_tiers_with_warnings`][merge],
  calls [`resolve_user_guidance_override`][guidance] (a `metadata` and a full
  bounded read of the override file on every call, no mtime gate), and
  returns `self.effective.clone()` at [`:288`][eff-clone]. Its doc at
  [`:266-267`][eff-warn-doc] says tier read failures are reported on every
  load so a long-running daemon keeps surfacing them.
- One [`ConfigCache`][cache-struct] holds one `user` and one `project`
  [`TierConfig`][tier-struct] (path, mtime, value, warning). A different
  `project_root` changes `path`, so two roots alternating re-read the file
  every call.
- [`merge_tiers_with_warnings`][merge] applies user values first, then
  project values per key class; [`ProjectRaiseOnly`][raise-only] applies a
  project value only when it tightens the user value.
- Per-pass callers: [`maybe_spawn_reattach`][call-reattach],
  [`prepare_historian_fire`][call-fire] after the state load, pending-rewrite,
  and live-historian early returns ([`:5013-5051`][fire-early]), and the
  wrapup path at [`:5368`][call-wrapup]. [`bind`][call-bind] freezes a copy
  into `SessionBinding`, whose doc at [`:216-217`][binding-doc] says config
  can change while the route stays open.
- The mtime test at [`:2165-2202`][t-mtime] rewrites the user file with the
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

Use passes that reach the `prepare_historian_fire` call at `lib.rs:5051`:
not a subagent pass (`:8234`), state load succeeds, no `pending_rewrite`, and
no live historian completion pending (`:5013-5051`); the check is on each
call. Unit tests built with `fixed_config` (`lib.rs:3859`, returned at
`:4557-4560`) bypass the cache and cannot exercise this record.

## Investigation log

### Q: Keep reporting tier read failures on every load?

- Sources examined: [`:266-267`][eff-warn-doc], [`:283-285`][eff-warn],
  [`read_tier_cached:370-371`][tier-cached].
- Findings: The cache bypasses its hit path when `warning` is set, so a
  failing file is re-read and re-warned on every call. The doc states this as
  intent. A merged cache that stores the result silences the repeat unless it
  also stores and re-emits the warning.
- Missing evidence: A decision on the warning surface.
- Conclusion: needs human input.

### Q: Should the historian read the bind-frozen config instead?

- Sources examined: [`:5051`][call-fire], [`:11788`][call-bind],
  [`:216-217`][binding-doc].
- Findings: The binding doc freezes the fallback history budget because
  config can change while the route stays open; the historian deliberately
  reads a fresh merge per pass. Switching it to the frozen copy removes the
  per-pass cost and the one-pass edit latency together.
- Missing evidence: A specification decision.
- Conclusion: needs human input.

[eff-cfg]: ../../../../../crates/daemon/src/lib.rs#L4581-L4590
[binding-doc]: ../../../../../crates/daemon/src/lib.rs#L223-L224
[call-reattach]: ../../../../../crates/daemon/src/lib.rs#L4834
[fire-early]: ../../../../../crates/daemon/src/lib.rs#L5057-L5095
[call-fire]: ../../../../../crates/daemon/src/lib.rs#L5095
[call-wrapup]: ../../../../../crates/daemon/src/lib.rs#L5412
[call-bind]: ../../../../../crates/daemon/src/lib.rs#L11841
[tier-struct]: ../../../../../crates/daemon/src/config.rs#L222-L228
[cache-struct]: ../../../../../crates/daemon/src/config.rs#L230-L235
[eff-proj]: ../../../../../crates/daemon/src/config.rs#L242-L245
[eff-warn-doc]: ../../../../../crates/daemon/src/config.rs#L266-L267
[eff-warn]: ../../../../../crates/daemon/src/config.rs#L268-L288
[eff-clone]: ../../../../../crates/daemon/src/config.rs#L288
[tier-cached]: ../../../../../crates/daemon/src/config.rs#L368-L398
[guidance]: ../../../../../crates/daemon/src/config.rs#L414-L496
[merge]: ../../../../../crates/daemon/src/config.rs#L716
[raise-only]: ../../../../../crates/daemon/src/config.rs#L740
[t-mtime]: ../../../../../crates/daemon/src/config.rs#L2165-L2202
