# cf-unavailable-evidence-no-credit

Repository: `/local/home/ahrav/scratch/eidnara`.
Inspected revision: `99f68bd37516ca5351f8aadb8b0b51cb0f13dcc8`.
Date: 2026-09-19. Exercise: not yet. Existing checks are unaudited.

## Discovery trigger

R3's omission rule separates visible, discoverable, and unavailable evidence.
The [settled contract](https://github.com/ahrav/eidnara/issues/707), "Materiality
and minimum meaning by tier" and "Evidence and review gate"
(historical plan lines 78-87, 263-269, 318-319),
forbids using safe abstention as preservation credit. Recovery, degradation,
resource, and security lenses found several distinct non-success outcomes.
These are code-supported possibilities, not reported fidelity incidents.

This record owns the canonical disposition predicate for each declared
material-obligation/follow-up/scenario tuple. Require exactly one result:
visible when the complete pre-recovery invocation includes the qualified
obligation; otherwise discoverable when a permitted registered-tool replay
with visible-derived arguments delivers it within the declared budget;
otherwise unavailable. A missing invocation capture fails the prerequisite;
it is not an unavailable-evidence observation. Delivery consumes this predicate
and owns pressure setup, rather than defining another disposition oracle.

## Evidence trail

- [Native publication errors, lines 408-434](../../../../../crates/daemon/src/harness_sources.rs#L408)
  distinguish artifact refusal, identity reuse, descriptor refusal, and
  kernel failure. They are not alternative successful source values.
- [Daemon refusal test, lines 1459-1483](../../../../../crates/daemon/tests/harness_sources.rs#L1459)
  offers a synthetic credential-shaped output, expects `ExactBytesRewritten`,
  and checks no new inventory or commit. It also checks content is not exposed
  in formatted errors. This catalog does not change scanner behavior.
- [Artifact read, lines 50-112](../../../../../crates/kernel/src/cas/read.rs#L50)
  rejects malformed digests, absent live evidence, tombstones, inaccessible
  objects, and corrupt objects. A privileged handle does not force success.
  Descriptor succession alone does not cause this refusal: the
  [verified native selector](cf-native-reopen-bytes.md#investigation-log)
  distinguishes the invalidated observation from still-live evidence metadata.
  Explicit evidence retirement, purge, or an actual read refusal still earns
  no recovery credit; historical selection cannot bypass those guards.
- [SourceRow, lines 84-101](../../../../../crates/kernel/src/source_export.rs#L84)
  distinguishes absent text from an empty string. The daemon
  [inventory helper, lines 731-744](../../../../../crates/daemon/tests/harness_sources.rs#L731)
  maps absence to empty text, so that helper alone cannot observe this
  distinction for a C6 result.
- [Search execution, lines 151-175, 230-260](../../../../../packages/opencode-plugin/src/tools/eidnara-search/execute.ts#L151)
  distinguishes invalid state, complete delivery, and packed results. An
  explicit empty sources list completes without reading. Unresolved IDs and
  truncated reads carry notes rather than proof of a complete search.
- [Search fallback, lines 230-238](../../../../../packages/opencode-plugin/src/tools/eidnara-search/execute.ts#L230)
  retries only `no_required_consumer` ungated and adds a freshness note. Do
  not classify that first refusal as terminal if the permitted second read
  actually delivers supporting evidence.
- [Search tests, lines 350-425](../../../../../packages/opencode-plugin/src/tools/eidnara-search/tools.test.ts#L350)
  cover stale, no-consumer fallback, busy, disabled, and absent-daemon cases.
  [Lines 473-508](../../../../../packages/opencode-plugin/src/tools/eidnara-search/tools.test.ts#L473)
  distinguish pre-pack omissions from delivered results and complete-empty
  from invalid execution. The backing kernel in these tests is a fake.

Reachability is `test-only`: the property governs the proposed replay's
evidence accounting at the captured-invocation boundary. Production routes
already emit the outcomes above; no production fidelity classifier is claimed.

## Failure scenario

The only material source leaves the prompt. An internal artifact read
succeeds but no registered tool exposes it, or a registered search returns
an error, no hit, or a packed result lacking the obligation. The consumer
abstains. A report credits recovery because the store contains evidence,
the search completed, or the answer avoided a false statement.

The competing explanation is that the evidence remains elsewhere in the
actual invocation. Inspect the live tail, included memory, hints, and tool
results before declaring unavailable. Candidate memory and hidden fixture
state are not included evidence. Refusal must not invite bypassing policy.

## Timing windows and dependencies

Observe the complete invocation before recovery and the actual results after
bounded tool execution. Preserve artifacts before a helper resets captured
requests. A result that arrived after the declared limit cannot retroactively
satisfy the bounded witness. No unbounded retry or new expansion API is needed.

## What a test must construct

1. Cover visible and discoverable controls as well as unavailable cases.
   Require one disposition per declared tuple, without missing or duplicate
   rows. Reuse delivery's pressure setup to remove the obligation from every
   delivered representation; absence from one history tier is insufficient.
2. Exercise a refused native publication and an unavailable read using the
   existing boundaries, without weakening scanner, egress, or admission.
3. Exercise search no-hit and target-omitted results as distinct controls;
   do not infer them solely from `status: complete` or `prePack` contents.
4. Require unavailable, no recovery credit, and no useful-preservation credit
   when neither visibility nor a replayed allowed route supplies the
   obligation. Keep the useful-preservation failure even if abstention is
   safe. Empty search results and privileged reads alone earn no recovery.
5. Record consumer safety independently: an explicit limitation and no
   unsupported conclusion/action is a safe response, not recovered evidence.
   Scripted responses do not establish real-model semantic behavior.

## Investigation log

### Q: Which unavailable C6 rows permit abstention?

- Sources examined: the plan's omission and acceptance rules, native refusal,
  and search outcomes above.
- Findings: unavailability never earns recovery credit. An absent material
  constraint is a preservation failure even when abstention is safe. The plan
  permits abstention only where the scenario declares it acceptable.
- Missing evidence: the human-reviewed C6 scenario annotations and baseline.
- Conclusion: needs human input for those scenario judgments. The accounting
  rule is settled; neither a test result nor an implementation convenience
  may redefine the intended material obligation or relax access policy.
