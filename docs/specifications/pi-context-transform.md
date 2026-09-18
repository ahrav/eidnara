# Specification draft: Pi context transform

## Problem

The Pi plugin has no message transform. It registers `session_start`, `before_agent_start`,
`tool_execution_start`, `session_before_compact`, `message_end`, `session_shutdown`, and
`session_before_switch`, but nothing rewrites the messages Pi sends to the provider. Until this
change it also cancelled `session_before_compact` ("eidnara owns compaction"), so a Pi session
with Eidnara enabled overflowed at the model's limit with nothing folding it. Pi now runs in
compaction-off mode unconditionally: native compaction proceeds and `eidnara_reduce` is not
registered. Memory, search, notes, todo overlay, guidance and status work; folding, tags,
reduction and `<session-history>` do not.

Live evidence: `eidnara-evaluation/20260917-36f09a3/findings.md` (gate 0c, Pi memory create
and cross-session search recall pass; no transform passes; notes fail `session_unresolved`).

## Seam

Pi fires `context` before every provider call with a structured clone of the messages and
accepts `{ messages }` back (`core/extensions/runner.js` `emitContext`; `docs/extensions.md`
"context"). This is the analogue of OpenCode's `experimental.chat.messages.transform` and is
non-destructive: the session file keeps the raw history, only the provider request changes.
Pi also fires `before_provider_request` with the serialized payload, which is the wrong layer
(provider-specific, after tool pairing rules are applied).

The daemon already knows the Pi harness: `SerializerProfile::Pi` exists with healing coverage
`drops_empty_content`, `autofills_reasoning`, no consecutive-assistant merging, and tail
reclamation enabled (`crates/daemon/src/healing.rs`); the model-execution backend runs Pi
children; the harness closure for Pi 0.80.2 on Node 24.18.0 is published.

## Design

1. **Ingress adapter** (`packages/pi-plugin/src/transform/pi-wire.ts`): map `AgentMessage[]` to
   the daemon's `ck` ingress the way `encodeOpenCodeMessagesToCk` does for OpenCode.
   - `user` text and image content → `text` / `image` kinds.
   - `assistant` `text`, `thinking` (with `signature`), `toolCall` → `text`, `reasoning`,
     `tool_use {id, name, input}`; carry `provider`, `model`, `api`, `usage`, `stopReason` in
     `meta` so egress can rebuild a faithful assistant message.
   - `toolResult` → `tool_result {tool_use_id, content, is_error}`.
   - Message identity: Pi messages carry no id. Use `mid = pi-<role>-<timestamp>-<hash8>` where
     the hash covers the first content block; timestamps are per message and stable, and the hash
     separates same-millisecond messages. Ordinals are positions (`annotateOrdinals` memo keyed
     by `mid`, as today).
   - `serializer_profile: "pi"`.
2. **Driver** (`packages/pi-plugin/src/transform/pi-transform.ts`): a Pi-shaped copy of the
   OpenCode driver's pass loop, without host-array publication: build the paged payload
   (`buildPagedModuleTransformPayloads`), call `transform` through the existing
   `HostModuleClient` route for the Pi session, apply the edit recipe with
   `hooks/context/context-application.ts`, and return `{ messages }` from the `context` handler.
   Fail open: any error returns the input unchanged and logs once per session; the daemon's
   `history_summarizer` fires from the same pass as it does for OpenCode.
3. **Egress adapter**: canonical output → `AgentMessage`. Untouched inputs are returned as the
   original objects (looked up by `mid`) so provider-specific fields survive verbatim; daemon
   inserted blocks become `user` messages with `text` content; a rewritten assistant message
   keeps the original `api/provider/model/usage/stopReason` and only replaces `content`.
   Thinking blocks are never rewritten (the Anthropic 400 regression class the OpenCode E2E
   guards).
4. **Usage and limits**: `before_agent_start`/`agent_end` events and the assistant message
   `usage` feed the context-usage map the trigger reads; the model's context window comes
   from Pi's model registry (`ctx.model.contextWindow`).
5. **Compaction ownership**: with a transform in place, `session_before_compact` is cancelled
   again (current behaviour restored) and `eidnara_reduce` registers; `compaction.enabled=false`
   keeps the opt-out.
6. **Notes**: `resolve_facade_scope` in the daemon vouches for an OpenCode session through
   `module_knows_transform_session`; extend the same lineage check to `pi` once transform passes
   commit for Pi sessions, which removes the Claude-Code-worded `session_unresolved` failure.
7. **Guidance**: the system-prompt handler in Pi resolves the prompt-surface preset only; wire
   `guidance.get` through the module client exactly as the OpenCode handler now does.

## Verification

- Unit: ingress/egress round-trip for text, thinking with signature, tool call and result,
  image; identity stability across passes; tool pairing preserved after a fold.
- E2E: a Pi lane in `packages/e2e-tests` mirroring `rust-smoke`, `fold-under-pressure`,
  `thinking-block-safety` with the mock provider through Pi RPC (the `pi-smoke` harness exists).
- Live: the campaign's `gate0-pressure2` profile against Pi with Bedrock: folds land, no
  overflow across 16 turns, recall of the corrected constraint from `<session-history>`.

## Estimate

Two to three focused days: adapters and driver (1), tests and E2E lane (1), live qualification
and fixes (0.5–1). Ships as its own PR after the current fix set.
