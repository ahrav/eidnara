# cf-guidance-capability-bound

## Discovery trigger

Date: 2026-09-19. Inspected HEAD:
`99f68bd37516ca5351f8aadb8b0b51cb0f13dcc8`.
An orchestrator-commissioned independent portfolio review identifies missing
U5/KTD5 coverage for producer and hint guidance. The
[settled contract][contract], "Implementation Decisions" KTD5 and "Milestone
boundaries and dependencies" U5 (historical plan lines 157,247-255),
requires correcting witnessed recovery overclaims at their text owners.
The plan and local code/docs/history are supplied; no additional incident logs
or related repositories are supplied. No tests or model calls run here.

## Evidence trail

- [System prompt:17,362][system] disclaims summarized-history and transcript
  recovery. [Lines 130,212][system] nevertheless say the history segment is
  recoverable through search and P4 detail would be recovered through search.
  These exact statements establish the conflicting premise at this HEAD.
- [prompt.rs:91-92][prompt] includes that text as
  `HISTORY_SUMMARIZER_SYSTEM_PROMPT`; [chunk.rs:850-867][chunk],
  `as_fire_request`, uses it as the producer's system field. It is not merely
  a design-doc example.
- [transform.rs:8286-8316][transform], `run_user_hint_lexical_search`, builds
  candidates from stored history content and P1-P4, not canonical memory rows.
- [transform.rs:8609-8634][transform], `render_user_hint`, emits those
  fragments with a footer that offers `eidnara_search` as a project-memory
  search; #926 removed the earlier promise that it retrieves full context.
- [tool-registry.ts:34-60][registry], `createToolRegistry`, exposes the search
  factory when the plugin is enabled and returns no tools when disabled.
- [search tools.ts:13-26,33-51][tools] permits only the `memory` source and
  dispatches to `executeEidnaraSearch`.
- [execute.ts:33-85,172-260][execute] rejects other source names and searches
  memory rows from the kernel client's `explicit_search` read. That registered
  path does not retrieve the history segment that supplied the hint.
- [Registry tests:32-44,258-305][registry-tests] pin four tool IDs and check
  memory-only guidance in four daemon assets. Those assets do not include the
  summarizer prompt or generated hint footer. Existing checks are unaudited.

Reachability is `test-only` for the proposed manual guidance-review and linkage
check. Its subjects are production text, but no automatic semantic guard runs
in production. The actual registered configuration must accompany the review.
Confidence is high for the inspected unsupported premise, not for a completed
corrective replay or an observed consumer failure.

## Failure scenario

A producer writes a thinner P4 assuming search can restore its history detail.
The consumer sees a matching history hint and follows its full-context promise,
but the registered search only returns canonical memory. The source text and
route establish the unsupported general promise; no consumer incident is claimed.

A competing explanation is that the daemon's internal history search, or a
separately admitted memory, supplies recovery. The registry-to-executor trace
rules out the first as the named model-facing route. The second may satisfy a
specific case only when that memory is actually available; it cannot justify
a general history-recovery promise or exact-transcript claim.

## Timing windows and dependencies

Bind manual review to the exact prompt/hint text and registry configuration.
Recheck on text, schema, or route changes; an old approved judgment must not be
attached to a different capability surface. U5 deterministic text corrections
still require U2/U3 evidence; prompt/meaning changes need U4 and fresh review.
Static evidence of this premise does not waive the settled dependency order.

## What a test must construct

Pair the unsupported history-recovery wording with a supported memory-search
statement and a disabled-registry control. Retain actual generated hint bytes,
schema/route references, and the human judgment linking each recovery claim to
its available capability or stated limitation. Deterministic checks validate
this linkage and completeness, not the meaning of arbitrary prose. Do not add
an automatic semantic classifier or a source-access API. None found for the
combined prompt/hint capability-review check at HEAD.

## Investigation log

### Q: Does the named registered search recover the hinted history?
- Sources examined: Hint builder, registry, search schema, and executor above.
- Findings: Hints use history rows; the registered route accepts only memory.
  Prompt lines 17/362 do not cancel the contradictory premises at 130/212.
- Missing evidence: A case-linked consumer/corrective replay; no incident is supplied.
- Conclusion: Resolved for the unsupported premise; runtime effect is unexercised.

### Q: What wording and review evidence should a correction use?
- Sources examined: Plan KTD5/U5 and the four-asset guidance test.
- Findings: Narrowing claims at their owners is authorized after witnesses and
  dependencies; a string assertion cannot supply semantic approval.
- Missing evidence: Approved wording, generated hint capture, and linked
  human review for the applicable registry configuration.
- Conclusion: Needs human input for wording; unresolved for the offline capture.

[contract]: https://github.com/ahrav/eidnara/issues/707
[system]: ../../../../../crates/daemon/testdata/history_summarizer-system-prompt.txt
[prompt]: ../../../../../crates/daemon/src/history_summarizer_prompt.rs
[transform]: ../../../../../crates/daemon/src/transform.rs
[registry]: ../../../../../packages/opencode-plugin/src/plugin/tool-registry.ts
[tools]: ../../../../../packages/opencode-plugin/src/tools/eidnara-search/tools.ts
[execute]: ../../../../../packages/opencode-plugin/src/tools/eidnara-search/execute.ts
[registry-tests]: ../../../../../packages/opencode-plugin/src/plugin/tool-registry.test.ts
[chunk]: ../../../../../crates/daemon/src/history_summarizer_chunk.rs
