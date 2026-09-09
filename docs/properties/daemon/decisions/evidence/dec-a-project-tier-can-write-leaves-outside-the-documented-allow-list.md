# dec-a-project-tier-can-write-leaves-outside-the-documented-allow-list

## Discovery trigger

The discovery compared a prose allow-list with a separate project-tier read
block. The active property instead checks the explicit permissions in the
[catalog record](../catalog.md#dec-a-project-tier-can-write-leaves-outside-the-documented-allow-list)
against the classified merge. An implementation-derived allow-list cannot serve
as the oracle for an accidental policy change.

Source snapshot: `eccca05ec111fdf39df3795f08b7ca31ee713d96`, verified
2026-09-09. Rust references below name `crates/daemon/src/config.rs`.
TypeScript references name files under `packages/opencode-plugin/src/config/`.
The [PR's decisions](https://github.com/ahrav/eidnara/pull/330) establish the
intended tier policy; source and tests establish implementation evidence.
The absent source-catalog `CONFIGURATION.md` is not current evidence.

## Evidence trail

`ConfigKey::pointer()` (`config.rs:588-618`) names 25 consumed keys.
`tier_class()` (`:657-693`) assigns 14 to `UserOnly`, nine to
`ProjectAllowed`, and two to `ProjectRaiseOnly`. These counts describe the
implementation, not the expected policy used by the check.

`merge_tiers_with_warnings` (`:710-755`) applies the user tier first, then
examines each supplied project key. A user-only key warns without calling
`apply_key`. An allowed key uses the same parser as the user tier. A raise-only
key is applied to a candidate and accepted only when its tightening predicate
holds. A changed but rejected candidate warns; an unchanged candidate does not.
The effective threshold is clamped to `[1, 90]` after the merge.

The predicates at `:682-692` require a higher threshold or a gate transition
from open to closed. The schedule is user-only (`:667`), so a project cannot
reopen the gate through `dreamer.tasks.review-user-memories.schedule`.
`dreamer.inject_docs` is also user-only (`:669`), in agreement with the
catalog's configuration-table row. `smart_drops` and `temporal_awareness` are
project-allowed (`:680-681`); a project may override either boolean direction,
not only its default value.

The nine privileged keys are named at `:627-637`. The `const` assertion at
`:696-708` forbids `ProjectAllowed` for a privileged key. It does not enforce
the complete policy: changing a user-only key to raise-only, or changing an
unprivileged key's class, needs an independent expected result to detect it.

The daemon reads project config itself (`:262-274`), so TypeScript filtering
does not replace the Rust policy. TypeScript independently removes project
`cache_ttl` (`project-security.ts:386-391`) and
`memory.injection_budget_tokens` (`:400-406`). The guidance path is resolved
after the merge from the user tier only (`config.rs:278-279`, `:408-419`);
its `apply_key` arm intentionally does nothing (`:921-923`).

## Failure scenario

A regression classifies `dreamer.inject_docs` as project-allowed and removes
its privileged mark. A project setting it to `true` then overrides a user
setting it to `false`. The documented user-only rule must fail even if an
implementation-derived permission list accepts the change. This is a regression
scenario, not observed behavior at the source snapshot.

## Timing windows and dependencies

No injected fault is needed. Reachability is `explicit-config-only`: construct
a project `.eidnara/eidnara.jsonc` with a value that differs from the user tier.
`effective_with_warnings` reads that path and merges the tiers at
`config.rs:262-274`. User-only versus user-plus-project comparisons must use
the same user config and defaults.

## What a test must construct

Use the catalog's fixed key lists to construct distinct user and project values,
including both boolean directions, both threshold directions, and the schedule
alias. Assert final values and warning keys independently of `tier_class`,
`privileged`, and `ALL`. Include the user-only guidance-path resolution, not
only the in-memory merge. Unknown keys must not change the effective config.

Existing checks remain `unaudited`:

- `config.rs:1297-1302`, `:1317-1350`, and `:1353-1387` assert specific
  threshold and budget outcomes with literal expectations.
- `:1407-1446` asserts auto-search and caveman project overrides.
- `:1458-1480` asserts docs-injection rejection and a temporal override;
  `:1905-1920` asserts that project config cannot turn user docs injection off.
- `:1637-1666` asserts that project flags and schedules cannot open a closed
  user-memory gate, and a no-op closed gate emits no warning.
- `:1671-1697` pins nine privileged names and rejects `ProjectAllowed` for
  them, but does not pin each key's exact class.
- `:1702-1804` supplies all 25 keys and asserts selected outputs; its warning
  count at `:1775-1785` is derived from the implementation's classes.
- `:1809-1844` checks pointer registration and prohibits raw `.pointer("`
  reads. It does not establish permissions or detect a `Value::get` read or a
  runtime-built pointer that bypasses the table.

These checks justify partial exercise, not a complete independent policy oracle.

## Investigation log

### Historical discovery notes

The two entries below retain the original investigation from the
[pre-refresh evidence snapshot](https://github.com/ahrav/eidnara/blob/eccca05ec111fdf39df3795f08b7ca31ee713d96/docs/properties/daemon/decisions/evidence/dec-a-project-tier-can-write-leaves-outside-the-documented-allow-list.md).
Their line references and conclusions describe discovery-time sources, not
the source snapshot verified above. They do not support the active confidence
claim; the current disposition follows them.

### Q: Is `smart_drops` intended to be project-overridable?

- Sources examined: `config.rs:6-7` (the allow-list, which omits it);
  `config.rs:467-469` (user tier) and `:541-543` (project tier), which are
  symmetric and therefore look deliberate; `CONFIGURATION.md:752` (source-catalog path, not present at HEAD) (default
  `false`) and `:767` (the "stays off while cache stability is being validated"
  rationale); `selection.rs:1229` and `:1236` (the gates).
- Findings: the code is symmetric and tested, which argues for deliberate. The
  header and `CONFIGURATION.md:767` (source-catalog path, not present at HEAD) argue that enabling it is a decision the user
  makes knowingly. Those two positions are in tension for a repository-supplied
  config, which the user does not author.
- Missing evidence: whether the TypeScript leg applies the same tiering. The
  repository has a `packages/plugin/src/config/project-security.ts` (source-catalog path, not present at HEAD) whose name
  suggests a per-leaf project policy exists there; reading it is outside 4f scope
  and would settle whether the two legs agree.
- Conclusion: needs human input. Either the header should name the three leaves or
  the project block should stop applying `smart_drops`.

### Q: Are the other two leaves harmful from a project tier?

- Sources examined: `config.rs:132-133` (both default `true`); the consumers,
  `inject_docs` gating the `<project-docs>` sub-block (`project_docs.rs` per the
  scope map at `:342`) and `temporal_awareness` gating the temporal gap overlay
  (`transform.rs:8172-8206` `temporal_gap_prefix`).
- Findings: a project can only turn them off, which removes content rather than
  adding it. `inject_docs: false` from a project means the repository declines to
  have its own `ARCHITECTURE.md` and `STRUCTURE.md` injected, which is a coherent
  thing for a repository to want.
- Missing evidence: none.
- Conclusion: resolved with answer. Both are outside the stated policy but neither
  is an escalation, so the record's impact statement rests on `smart_drops` and
  the memory budget.

### Q: Does the classified merge settle the discovery's policy questions?

- Sources examined: the PR's explicit decision to retain `smart_drops` and
  `temporal_awareness` as project-allowed; `config.rs:657-693`; the merge at
  `:710-755`; the tests listed above.
- Findings: the policy explicitly permits both flags, denies project docs
  injection and schedules, and only lets a project close the user-memory gate.
  The catalog's explicit permissions are the expected contract; the class table
  is the implementation checked against it.
- Missing evidence: a full independent per-key policy regression check.
- Conclusion: resolved with answer for policy intent. Exercise remains partial;
  existing tests are recorded as unaudited rather than claimed as full proof.
