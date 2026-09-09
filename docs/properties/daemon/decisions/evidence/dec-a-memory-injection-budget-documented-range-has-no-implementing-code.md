# dec-a-memory-injection-budget-documented-range-has-no-implementing-code

## Discovery trigger

The source-catalog contract describes a `500-20000` memory-injection budget,
but the Rust parser applies a floor of `1` and no ceiling. This is a user-tier
range property. A project-tier fixture exercises trust-policy rejection instead.

Source snapshot: working tree based on
`735f58dcb1002505c7aeb8a96505a4d210c4782b`, verified
2026-09-09. Rust paths below are relative to `crates/daemon/src/`.

## Evidence trail

The historical contract quotes `CONFIGURATION.md:591`, inside the `memory`
table. That source-catalog document is absent at HEAD; the quote is a retained
claim under test, not a newly verified documentation source:

> `injection_budget_tokens` | `number` (500–20000) | `4000` | Token budget for
> memory injection into `<session-history>`.

The default is `4_000.0` at `config.rs:23`, applied at `:123`. The standard
key's `apply_key` arm (`:847-851`) stores `budget.max(1.0)`. The legacy arm
(`:852-864`) uses the same floor when the primary key has no parsed numeric
value, and emits a deprecation warning. `number_at` (`:963-968`) accepts finite
JSON numbers. Neither assignment enforces `500` or `20000`.

Both keys are `UserOnly` (`:664-672`). The user loop invokes their parsers
(`:717-720`); the project loop emits an ignored-key warning without invoking
them (`:723-733`). There is no project-tier budget assignment. A project-only
out-of-range value therefore leaves the default budget intact and emits a
warning, which would make the range oracle pass for the wrong reason.

`Handler::project_memory_read` (`lib.rs:4832-4845`) passes the effective budget
to `canonical_memory::read_project_memory` when memory is enabled. The reader
applies its visible-row read before memory trimming (`canonical_memory.rs:163-174`,
`:195-212`). Increasing this budget does not remove the separate row and byte
caps of the visible-row read.

## Failure scenario

A user-tier `eidnara.jsonc` contains:

```
{ "memory": { "injection_budget_tokens": 200000 } }
```

With the project tier absent, the standard-key arm accepts `200000.0` without
a range warning. A value of `10` likewise remains `10`, not `500`. These are
source-traced outcomes of `config.rs:847-851`, not recorded campaign results.
They violate the retained range contract while preserving the user-only policy.

## Timing windows and dependencies

No fault or timing window is needed. Reachability is `explicit-config-only`
because an out-of-range user value is required; the default is inside the range.
The in-memory merge is enough to expose the mismatch. A file-based check uses
`ConfigCache::effective_with_warnings` (`config.rs:262-282`) to read and merge
the user tier.

## What a test must construct

For each value in `[200000, 10]`, construct a user object containing only
`memory.injection_budget_tokens` and call
`merge_tiers_with_warnings(Some(&user), None)`. Assert that the effective budget
is in `[500, 20000]` or that a warning names `/memory/injection_budget_tokens`.
Do not supply a project tier or the deprecated key: either can add a warning
without checking the standard key's range.

Keep project rejection separate: with a user budget of `3000` and a project
budget of `200000`, require `3000` and an ignored-project-key warning. That
checks authority, not the range.

Existing checks, each `unaudited`: `config.rs:1311-1314` pins the default;
`:1317-1350` checks standard/legacy precedence and project rejection;
`:1353-1387` checks the other user-only budget leaves. The fallback cases at
`:1885-1903` accept `128` but do not assert the standard-key range. No full
range-check exercise is claimed.

## Investigation log

### Historical discovery notes

The two entries below are preserved from the
[pre-refresh evidence](https://github.com/ahrav/eidnara/blob/c73ed613dc9c48f4ad789925034f6eb081fca260/docs/properties/daemon/decisions/evidence/dec-a-memory-injection-budget-documented-range-has-no-implementing-code.md).
Their references and tier-policy conclusions describe discovery-time source,
not HEAD. The current disposition follows them.

### Q: Is the project tier supposed to be able to write this key at all?

- Sources examined: `config.rs:6-7` (the header's allow-list: "may override
  trusted memory, auto-search, caveman, promotion, and privacy settings. User-profile
  and historian budgets remain user-tier only"); `:526-528` (project parse);
  `:539` (the user-profile budget's project-tier warning); `:538` (the deprecated
  `/memory/budget_tokens` project-tier warning); `CONFIGURATION.md:591` (source-catalog path, not present at HEAD), which
  carries no user-only marker for this key.
- Findings: the header's phrasing is compatible with the injection budget being
  project-writable, because it names only the user-profile and historian budgets
  as user-tier. So the tiering is intentional. The missing range is a separate
  question from the tiering.
- Missing evidence: none needed for the record. The record's guarantee is about
  the range, not the tier.
- Conclusion: resolved with answer. The tiering is deliberate; the absent range is
  the defect. Reachability is `explicit-config-only` because the key must be
  present in a config file for the divergence to have any effect; the default
  `4000` is inside the documented range.

### Q: Does the deprecated key share the missing range?

- Sources examined: `config.rs:443-445` (the `/memory/budget_tokens` fallback,
  user tier only), `:446-451` (its deprecation warning), `:538` (the project-tier
  ignore).
- Findings: yes, the same `.max(1.0)` applies. The deprecated key does at least
  produce a warning naming itself, which is the one place in this area where the
  caller is told something.
- Missing evidence: none.
- Conclusion: resolved with answer. The record's check covers both keys because
  both write the same field, and the assertion is on the resolved field.

### Q: Which tier must a range campaign exercise?

- Sources examined: `config.rs:664-672`, `:717-744`, `:847-864`, and
  `:1317-1350` at the source snapshot above.
- Findings: both budget keys are user-only. A project value never reaches the
  budget parser, and its rejection warning can satisfy a range-or-warning
  assertion vacuously. A legacy-key deprecation warning can do the same.
- Missing evidence: an independent standard-key range test using only the user
  tier.
- Conclusion: resolved with answer. Exercise the standard user-tier key with
  no project tier. Treat project rejection and legacy deprecation as separate
  properties; the missing range remains a user-tier concern.
