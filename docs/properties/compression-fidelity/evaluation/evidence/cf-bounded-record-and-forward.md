# cf-bounded-record-and-forward

## Discovery trigger

U4 in the [settled contract](https://github.com/ahrav/eidnara/issues/707),
"Milestone boundaries and dependencies" and "Resource and security invariants"
(historical plan lines 234-241), specifies optional record-and-forward on the
existing Messages provider. It requires the selected real request, returned
response, and actual registered-tool loop, with synthetic data, private
artifacts, no credentials, and fixed whole-run limits.

Inspected HEAD: `0380a83a70747d2114268ac0b7e3d17551fbcf00`, 2026-10-09, the
head of the U4 change ([#953](https://github.com/ahrav/eidnara/pull/953)).
The first inspection, at `99f68bd37516ca5351f8aadb8b0b51cb0f13dcc8` on
2026-09-19, found no forwarding path; its reuse constraints are kept below
where they still describe the surrounding code.
[Catalog provenance](../catalog.md#scope-and-evidence-boundary) records drift.
Reachability is test-only: the forwarding path exists only behind the e2e
mock's explicit `forward` construction argument. Exercise: partial. Every
check below ran against an in-process provider double; no authorized live
provider run has been made, and no external model call was made for this
inspection.

## Evidence trail

- [Construction](../../../../../packages/e2e-tests/src/mock-provider/forward.ts#L147-L188)
  (`validateForwardConfig`) requires an `https:` URL ending in `/messages`
  with no user info, query, or fragment; a model; a whole-number context
  limit; the reviewed corpus digest; positive input and output prices; a
  credential callback; and four positive limits (`maxCalls`,
  `maxOutputTokens`, `timeoutMs`, `spendCapUsd`), then freezes them.
  [`MockProvider`](../../../../../packages/e2e-tests/src/mock-provider/server.ts#L139-L146)
  refuses `forward` together with `script`, and every scripting method throws
  on a forwarding mock, so script and forward modes are exclusive with no
  silent matcher or default fallback.
- [Admission](../../../../../packages/e2e-tests/src/mock-provider/forward.ts#L462-L486)
  checks, before each send, that the run has not stopped, the body is a JSON
  object, the model matches, `max_tokens` is within `maxOutputTokens`, the
  call count is below `maxCalls`, and the reservation (body bytes as input
  tokens plus `max_tokens` as output, at the configured prices) fits under
  `spendCapUsd`. The corpus digest is a construction-time attestation; the
  forwarder reads no message content (see the open question below).
- [Send and response](../../../../../packages/e2e-tests/src/mock-provider/forward.ts#L532-L632)
  forwards the received bytes unchanged with only `content-type`, `accept`,
  `anthropic-version`, and `anthropic-beta` copied from the inbound request
  and the callback's headers added outbound; `redirect: "error"`; one
  deadline per send that also fires when the mock stops
  (`close`, line 489); at most 16 MiB of response read
  ([`readBounded`](../../../../../packages/e2e-tests/src/mock-provider/forward.ts#L354)).
  The response's usage, stop reason, model, `tool_use` ids, and Messages
  envelope are read; a non-2xx, an unfinished or errored stream, a body that
  is not a Messages message, a response naming another model or none, or
  usage costing more than the reservation stops the run. Captured headers are
  [redacted](../../../../../packages/e2e-tests/src/mock-provider/forward.ts#L195).
- [Report](../../../../../packages/e2e-tests/src/mock-provider/forward.ts#L635-L678)
  records `mode: "forward"`, the upstream URL, the frozen limits and prices,
  every exchange, attempted sends apart from acknowledged responses, spend,
  the stop reason, refusal count and up to 32 reasons, and `complete` only
  when nothing stopped the run, no send is in flight, no capture is
  truncated, every cost is known, every acknowledged response states a stop
  reason, spend is within the cap, and every `tool_use` id is answered by a
  later request's `tool_result`.
- [Publication](../../../../../packages/e2e-tests/src/mock-provider/forward.ts#L710-L740)
  writes the report `0600` through the atomic publisher into a `0700`
  owner-only directory outside the repository, with every existing ancestor
  owned by this user or root and closed to group and other writes unless
  sticky, checked before and after creation.
- [Harness](../../../../../packages/e2e-tests/src/rust-harness.ts#L267-L287)
  refuses `forward` beside `mockDefault`, runs OpenCode on `forward.model` at
  `forward.contextLimit` with `forward.limits.maxOutputTokens` as the output
  limit, writes the mock's per-mock `inboundKey` into `opencode.json`, and
  starts a forwarding run's OpenCode serve API behind a random Basic
  credential held only in the child's environment and the SDK client's
  header. The mock checks the inbound key
  [before reading a body](../../../../../packages/e2e-tests/src/mock-provider/server.ts#L276).
- [Real producer capture](../../../../../crates/daemon/src/compression_fidelity_replay_tests.rs#L1684-L1805)
  (`capture_sources`, behind the ignored
  [`real_producer_capture_of_every_corpus_source`](../../../../../crates/daemon/src/compression_fidelity_replay_tests.rs#L1996))
  folds each corpus source through the host's existing producer execution
  under one per-source wait; an unsettled source waits for firings before
  their first start, then
  [cancels](../../../../../crates/daemon/src/compression_fidelity_replay_tests.rs#L1840-L1909)
  every unconfirmed run through a second connection and purges the session
  of an unproven start error, stopping the capture when the host confirms
  neither. Records are written through
  [`private_capture_dir`](../../../../../crates/daemon/src/compression_fidelity_replay_tests.rs#L1934-L1988)
  with the same ancestor rule as the TypeScript publisher.
- Prior art the forwarding path reuses, unchanged from the first inspection:
  [scripted tool flow](../../../../../packages/e2e-tests/src/scripted-tool-call.ts#L94-L145)
  for the local loop, the
  [atomic publisher](../../../../../packages/e2e-tests/src/atomic-publish.ts#L5-L16)
  with an explicit `mode: 0o600`, and the incident runner's loopback and
  secret guards
  ([tests](../../../../../packages/e2e-tests/src/incident-pool/runner.test.ts#L804-L881)),
  which the forwarding path leaves as they were.

## Failure scenario

A "forwarded" sample is actually a scripted continuation, the model is replaced
after OpenCode constructs its request, or tool results are synthesized outside
the registered-tool loop. Separately, copying incoming headers leaks a real
credential, or each retry resets the call/time/spend allowance.

Against this implementation: a scripted continuation is impossible in a
forwarding mock by construction; a response from another model stops the run
and is recorded per exchange; tool results come from OpenCode's own tool loop
and are correlated by `tool_use` id; only four named headers are copied
outbound and credential headers are redacted from captures; limits are frozen
at construction and every send, retry, and refusal counts against them.

## Timing windows and dependencies

The real model is selected before capture through `forward.model`. Credentials
enter only the outbound request; the report, `opencode.json`, and logs are
checked for a canary. The explicit HTTPS target and the construction-time
corpus digest constrain admission. Redirects are refused by the request
option. Recorded script and forward modes are exclusive. Forward failure stops
the run rather than falling back to a matcher, script, or default.

The loop consumes the fixed limits owned by
[cf-evaluation-cost-completeness](cf-evaluation-cost-completeness.md), including
retries and ambiguous attempts. Exhaustion, unsupported shape, and an unfinished
loop produce an incomplete report. Truncated captures are marked.

## What a test must construct

1. Require `always` check `cf-eval-offline-zero-outbound` on every default/scripted
   run and `sometimes` marker `cf-eval-offline-default-invoked`. Only enabled
   forwarding uses `always-or-unreached`; default safety is not optional.
2. With explicit forwarding enabled, bind each actual selected-model request to
   the returned response, then bind provider tool-call IDs to registered-tool
   results in subsequent captured requests through loop completion.
3. Use credential canaries and inspect captures, config, and logs for absence;
   observe outbound authentication separately and private JSON mode 0600.
4. Challenge each frozen limit across multiple turns and retries, plus provider
   errors, unsupported response shape, and missing tool results. No bound extends.
5. Keep usage, prices/estimator identity, serving cost, generation cost, and
   recovery cost separate from fidelity verdicts and from synthetic mock counts.
6. Challenge `cf-eval-mode-origin-consistent` with a forged model string and a
   silent scripted fallback. Neither can receive a real-model evidence label.

Items 1 through 6 are exercised by
`packages/e2e-tests/src/mock-provider/forward.test.ts` and
`packages/e2e-tests/tests/compression-fidelity-forwarding.test.ts` against an
in-process double, as the catalog's existing-check line lists; the redirect
boundary is asserted only as the outbound `redirect: "error"` option.

## Investigation log

### Q: How does the real selected model enter the captured request?
- Sources examined: `rust-harness.ts:267-287`, `spawn.ts:185-230`,
  `forward.ts:462-486`.
- Findings: `RustTestHarness.create({ forward })` writes `forward.model`,
  `forward.contextLimit`, and `forward.limits.maxOutputTokens` into the model
  OpenCode runs, so OpenCode constructs every request against the selected
  model; `admit` refuses a request naming any other model, and the response's
  model is recorded and checked per exchange.
- Conclusion: Resolved at this HEAD by the harness and admission path; no
  multi-provider adapter was added and incident isolation is unchanged.

### Q: How will recorded mode prevent forged real-model evidence?
- Sources examined: `server.ts:139-158`, `forward.ts:635-678`.
- Findings: a forwarding mock cannot be scripted; the report records
  `mode: "forward"`, the upstream URL, and each response's own `model`, and
  a response naming another model stops the run.
- Conclusion: Resolved at this HEAD for origin. Cost accounting is owned by
  `cf-evaluation-cost-completeness`; neither check proves batch reachability.

### Q: Does the reviewed corpus digest bind the forwarded content?
- Sources examined: `forward.ts:147-188,462-486`,
  `tests/compression-fidelity-forwarding.test.ts`.
- Findings: the digest is compared with the compiled-in constant at
  construction and is not tied to any request body; the forwarding e2e test
  sends a non-corpus prompt through the double. Forwarding is never enabled
  without an operator-supplied credential callback.
- Conclusion: Unresolved; open as a design decision on
  [#953](https://github.com/ahrav/eidnara/pull/953) between a per-request
  content allowlist drawn from the corpus and an attestation-only reading
  with the guarantee reworded.
