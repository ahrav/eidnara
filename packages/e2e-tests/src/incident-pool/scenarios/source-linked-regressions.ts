/**
 * Parity A1 verifies first-render tag stability during pure-defer growth.
 * Parity A3 verifies aged `ctx_reduce` prefix survival during defer growth.
 * The thinking-block drivers verify that signed reasoning never reaches the provider wire
 * after a drop and that dropping a tagged text block leaves provider roles well-formed.
 */

import { detectRustPrerequisites } from "../../../scripts/check-rust-prerequisites";
import { analyzePasses, formatBustReport, mainAgentRequests } from "../../cache-analysis";
import type { RustTestHarness, RustTestHarnessOptions } from "../../rust-harness";
import { DEFAULT_SCRIPTED_TOOL_USAGE } from "../../scripted-tool-call";
import type {
    CaseDriverContext,
    JsonValue,
    PreconditionOutcome,
    RegisteredIncidentCase,
} from "../registry";
import { adaptBoundSymbol } from "../registry";
import { createCaseHarness } from "../support/tool-loop";

export interface RegressionCheck {
    id: string;
    passed: boolean;
}

export interface RegressionResult {
    verdict: "pass" | "assertion_fail";
    checks: RegressionCheck[];
}

function resultFromChecks(checks: RegressionCheck[]): RegressionResult {
    return {
        verdict: checks.every((check) => check.passed) ? "pass" : "assertion_fail",
        checks,
    };
}

/* */
export function failedCheckIds(result: RegressionResult): string[] {
    return result.checks.filter((check) => !check.passed).map((check) => check.id);
}

// ---------------------------------------------------------------------------
// ---------------------------------------------------------------------------

const DEFER_USAGE = DEFAULT_SCRIPTED_TOOL_USAGE;

export const FIRST_RENDER_HARNESS_OPTIONS = {
    modelContextLimit: 100_000,
    eidnaraConfig: {
        execute_threshold_percentage: 20,
        protected_tags: 1,
        memory: {
            enabled: true,
            auto_promote: false,
            auto_search: { enabled: false },
            git_commit_indexing: { enabled: false },
        },
    },
} as const satisfies RustTestHarnessOptions;

export const FIRST_RENDER_A1_CHECKS = [
    "check-a1-defer-request-floor",
    "check-a1-zero-prefix-busts",
    "check-a1-cached-transitions",
    "check-a1-transform-served",
    "check-a1-pure-defer",
] as const;

export const FIRST_RENDER_A3_CHECKS = [
    "check-a3-defer-request-floor",
    "check-a3-reduce-on-wire",
    "check-a3-zero-prefix-busts",
    "check-a3-reduce-retained-final-wire",
    "check-a3-cached-transitions",
    "check-a3-transform-served",
    "check-a3-pure-defer",
] as const;

/** Fields shared by the A1 and A3 observations that prove the zero-bust result is not vacuous. */
export interface CacheStabilityEvidence extends Record<string, JsonValue> {
    mainRequestCount: number;
    bustCount: number;
    bustReport: string;
    /** Transitions whose previous request carried no `cache_control` breakpoint; the oracle cannot report a bust on them. */
    uncachedTransitionCount: number;
    /** Main requests whose wire carries the transform's `§N§` tag overlay; a raw-served request carries none, and internal-agent passes cannot stand in for it. */
    transformRenderedRequestCount: number;
    /** `rust pass:` lines the plugin logged during the drive. */
    rustPassCount: number;
    /** Passes whose `served_from` is `transform`; the plugin labels the fail-open fallback `raw`. */
    transformServedPassCount: number;
    /** Passes whose decision is `SOFT+`, the transform's defer label; a low-pressure drill that executes instead has left the pure-defer path it claims to measure. */
    deferredPassCount: number;
}

export interface FirstRenderDeferObservation extends CacheStabilityEvidence {}

export interface AgedCtxReduceObservation extends CacheStabilityEvidence {
    sawReduceOnWire: boolean;
    finalWireHasCtxReduce: boolean;
}

const TAG_OVERLAY_RE = /§\d+§ /u;

/** The transform prefixes every taggable block with `§N§ ` as it renders a request, so a served request shows the overlay on its user text. */
export function wireCarriesTagOverlay(body: Record<string, unknown>): boolean {
    return messagesOf(body).some(
        (message) =>
            Array.isArray(message.content) &&
            message.content.some(
                (block) => typeof block.text === "string" && TAG_OVERLAY_RE.test(block.text),
            ),
    );
}

/** The plugin writes each pass line after the provider response is captured, so the pass floor is awaited rather than read once. */
async function collectCacheStabilityEvidence(
    h: RustTestHarness,
    requests: ReturnType<typeof mainAgentRequests>,
    requestFloor: number,
): Promise<CacheStabilityEvidence> {
    const passes = await h.waitForRustPasses(requestFloor);
    const comparisons = analyzePasses(requests);
    const busts = comparisons.filter((comparison) => comparison.verdict === "BUST");
    return {
        mainRequestCount: requests.length,
        transformRenderedRequestCount: requests.filter((request) =>
            wireCarriesTagOverlay(request.body),
        ).length,
        bustCount: busts.length,
        bustReport: busts.length > 0 ? formatBustReport(busts) : "",
        uncachedTransitionCount: comparisons.filter(
            (comparison) => comparison.verdict !== "BASE" && !comparison.prevHadBreakpoint,
        ).length,
        rustPassCount: passes.length,
        transformServedPassCount: passes.filter((pass) => pass.servedFrom === "transform").length,
        deferredPassCount: passes.filter((pass) => pass.decision === "SOFT+").length,
    };
}

function cacheStabilityChecks(
    prefix: "a1" | "a3",
    observation: CacheStabilityEvidence,
    requestFloor: number,
): {
    busts: RegressionCheck;
    cached: RegressionCheck;
    served: RegressionCheck;
    deferred: RegressionCheck;
} {
    return {
        busts: {
            id: `check-${prefix}-zero-prefix-busts`,
            passed: observation.bustCount === 0,
        },
        cached: {
            id: `check-${prefix}-cached-transitions`,
            passed: observation.uncachedTransitionCount === 0,
        },
        served: {
            id: `check-${prefix}-transform-served`,
            passed:
                observation.transformRenderedRequestCount === observation.mainRequestCount &&
                observation.rustPassCount >= requestFloor &&
                observation.transformServedPassCount === observation.rustPassCount,
        },
        deferred: {
            id: `check-${prefix}-pure-defer`,
            passed:
                observation.rustPassCount > 0 &&
                observation.deferredPassCount === observation.rustPassCount,
        },
    };
}

export async function driveFirstRenderPureDeferStability(
    h: RustTestHarness,
): Promise<FirstRenderDeferObservation> {
    const sessionId = await h.createSession();
    for (let i = 1; i <= 6; i++) {
        h.mock.setDefault({ text: `A1 reply ${i}`, usage: DEFER_USAGE });
        await h.sendPrompt(sessionId, `A1 turn ${i}: low-pressure cache-stability probe.`);
    }
    const requests = mainAgentRequests(h.mock.requests());
    return collectCacheStabilityEvidence(h, requests, FIRST_RENDER_A1_FIXTURE.turns);
}

export function verifyFirstRenderPureDeferStability(
    observation: FirstRenderDeferObservation,
): RegressionResult {
    const stability = cacheStabilityChecks("a1", observation, FIRST_RENDER_A1_FIXTURE.turns);
    return resultFromChecks([
        {
            id: "check-a1-defer-request-floor",
            passed: observation.mainRequestCount >= FIRST_RENDER_A1_FIXTURE.turns,
        },
        stability.busts,
        stability.cached,
        stability.served,
        stability.deferred,
    ]);
}

function isRecordLike(value: unknown): value is Record<string, unknown> {
    return typeof value === "object" && value !== null && !Array.isArray(value);
}

function messageBlocks(message: unknown): Array<Record<string, unknown>> {
    if (!message || typeof message !== "object") return [];
    const content = (message as { content?: unknown }).content;
    if (!Array.isArray(content)) return [];
    return content.filter(
        (block): block is Record<string, unknown> =>
            block !== null && typeof block === "object" && !Array.isArray(block),
    );
}

/** The check requires the emitted `ctx_reduce` use/result pair, not its tool declaration. */
/** `toolName` and `drop` pin the tool use the fixture emitted, so a pair whose name or payload the transform rewrote does not count as retained. */
export function hasCtxReducePair(
    body: Record<string, unknown>,
    callId: string,
    drop: string,
    toolName: string,
): boolean {
    if (!Array.isArray(body.messages)) return false;
    for (let index = 0; index < body.messages.length - 1; index += 1) {
        const assistant = body.messages[index];
        const user = body.messages[index + 1];
        if (
            !assistant ||
            typeof assistant !== "object" ||
            (assistant as { role?: unknown }).role !== "assistant" ||
            !user ||
            typeof user !== "object" ||
            (user as { role?: unknown }).role !== "user"
        ) {
            continue;
        }
        const use = messageBlocks(assistant).some(
            (block) =>
                block.type === "tool_use" &&
                block.id === callId &&
                block.name === toolName &&
                isRecordLike(block.input) &&
                block.input.drop === drop,
        );
        const result = messageBlocks(user).some(
            (block) => block.type === "tool_result" && block.tool_use_id === callId,
        );
        if (use && result) return true;
    }
    return false;
}

/* */
/** `emittedName` reports the published tool name the `tool_use` carried, or null while no request has published a `ctx_reduce` tool. */
function emitCtxReduceOnce(
    h: RustTestHarness,
    drop: string,
    callId: string,
): { emittedName: () => string | null } {
    let emitted = false;
    let emittedName: string | null = null;
    h.mock.addMatcher((body) => {
        if (emitted) return null;
        const sys = JSON.stringify(body.system ?? "");
        if (!sys.includes("## Eidnara")) return null;
        const tools = Array.isArray(body.tools) ? body.tools : [];
        const name = tools
            .map((t) => (t && typeof t === "object" ? (t as { name?: unknown }).name : null))
            .find((n) => typeof n === "string" && /ctx_reduce/.test(n)) as string | undefined;
        if (!name) return null;
        emitted = true;
        emittedName = name;
        return {
            content: [
                {
                    type: "tool_use",
                    id: callId,
                    name,
                    input: { drop },
                },
            ],
            stop_reason: "tool_use" as const,
            usage: DEFER_USAGE,
        };
    });
    return { emittedName: () => emittedName };
}

export async function driveAgedCtxReduceSurvival(
    h: RustTestHarness,
): Promise<AgedCtxReduceObservation> {
    const sessionId = await h.createSession();
    h.mock.setDefault({ text: "A3 reply 1", usage: DEFER_USAGE });
    await h.sendPrompt(sessionId, "A3 turn 1: establish baseline content.");

    const reduce = emitCtxReduceOnce(
        h,
        FIRST_RENDER_A3_FIXTURE.drop,
        FIRST_RENDER_A3_FIXTURE.callId,
    );
    h.mock.setDefault({
        text: "A3 reply 2 (after ctx_reduce tool call)",
        usage: DEFER_USAGE,
    });
    await h.sendPrompt(sessionId, "A3 turn 2: this turn issues a ctx_reduce call.");
    // The pair is matched against the exact name the fixture emitted; an unpublished tool makes every later check false.
    const toolName = reduce.emittedName() ?? "";
    const retainedPair = (body: Record<string, unknown>): boolean =>
        toolName.length > 0 &&
        hasCtxReducePair(
            body,
            FIRST_RENDER_A3_FIXTURE.callId,
            FIRST_RENDER_A3_FIXTURE.drop,
            toolName,
        );

    // Pure-defer growth ages the `ctx_reduce` call past the protected window.
    let sawReduceOnWire = false;
    for (let i = 3; i <= 8; i++) {
        h.mock.setDefault({ text: `A3 defer reply ${i}`, usage: DEFER_USAGE });
        await h.sendPrompt(sessionId, `A3 turn ${i}: defer growth ages the ctx_reduce call.`);
        const body = h.mock.lastRequest()?.body;
        if (body && retainedPair(body)) {
            sawReduceOnWire = true;
        }
    }

    const requests = mainAgentRequests(h.mock.requests());
    const finalBody = requests.at(-1)?.body;
    return {
        sawReduceOnWire,
        finalWireHasCtxReduce: finalBody !== undefined && retainedPair(finalBody),
        ...(await collectCacheStabilityEvidence(h, requests, FIRST_RENDER_A3_FIXTURE.mainRequests)),
    };
}

export function verifyAgedCtxReduceSurvival(
    observation: AgedCtxReduceObservation,
): RegressionResult {
    const stability = cacheStabilityChecks("a3", observation, FIRST_RENDER_A3_FIXTURE.mainRequests);
    return resultFromChecks([
        {
            id: "check-a3-defer-request-floor",
            passed: observation.mainRequestCount >= FIRST_RENDER_A3_FIXTURE.mainRequests,
        },
        { id: "check-a3-reduce-on-wire", passed: observation.sawReduceOnWire },
        stability.busts,
        {
            id: "check-a3-reduce-retained-final-wire",
            passed: observation.finalWireHasCtxReduce,
        },
        stability.cached,
        stability.served,
        stability.deferred,
    ]);
}

// ---------------------------------------------------------------------------
// ---------------------------------------------------------------------------

/**
 * The suite disables `auto_search` because the dropped paste body must be absent from all user text.
 */
export const THINKING_BLOCK_HARNESS_OPTIONS = {
    eidnaraConfig: {
        execute_threshold_percentage: 80,
        memory: { auto_search: { enabled: false } },
    },
    modelContextLimit: 50_000,
} as const satisfies RustTestHarnessOptions;

export const THINKING_NUDGE_ANCHOR_CHECKS = [
    "check-thinking-a-no-nudge-in-signed-assistant",
    "check-thinking-a-signature-byte-stable",
    "check-thinking-a-nonvacuous-inspection",
] as const;

export const THINKING_DROPPED_SHELL_CHECKS = [
    "check-thinking-b-drop-emitted",
    "check-thinking-b-paste-body-absent",
    "check-thinking-b-shell-preserved",
    "check-thinking-b-signed-replay-intact",
    "check-thinking-b-turn-boundary-preserved",
] as const;

export const THINKING_IMAGE_SURVIVAL_CHECKS = [
    "check-thinking-c-drop-emitted",
    "check-thinking-c-dropped-text-absent",
    "check-thinking-c-shell-preserved",
    "check-thinking-c-image-part-survives",
] as const;

interface AnthropicContentBlock {
    type: string;
    text?: string;
    thinking?: string;
    signature?: string;
}

interface AnthropicMessage {
    role: string;
    content: AnthropicContentBlock[] | string;
}

function messagesOf(body: Record<string, unknown>): AnthropicMessage[] {
    return Array.isArray(body.messages) ? (body.messages as AnthropicMessage[]) : [];
}

function mainRequests(h: RustTestHarness): Array<{ body: Record<string, unknown> }> {
    return h.mock
        .requests()
        .filter((request) => JSON.stringify(request.body.system ?? "").includes("## Eidnara"));
}

function blocksOfRole(body: Record<string, unknown>, role: string): AnthropicContentBlock[] {
    return messagesOf(body)
        .filter((m) => m.role === role)
        .flatMap((m) => (Array.isArray(m.content) ? m.content : []));
}

function userText(body: Record<string, unknown>): string {
    return blocksOfRole(body, "user")
        .filter((b) => b.type === "text")
        .map((b) => b.text ?? "")
        .join("\n");
}

function findThinkingBlocks(body: Record<string, unknown>): AnthropicContentBlock[] {
    const out: AnthropicContentBlock[] = [];
    for (const msg of messagesOf(body)) {
        if (!Array.isArray(msg.content)) continue;
        for (const block of msg.content) {
            if (block.type === "thinking" || block.type === "redacted_thinking") out.push(block);
        }
    }
    return out;
}

function toolName(body: Record<string, unknown>, pattern: RegExp): string | null {
    const tools = body.tools;
    if (!Array.isArray(tools)) return null;
    for (const tool of tools) {
        if (!tool || typeof tool !== "object") continue;
        const name = (tool as { name?: unknown }).name;
        if (typeof name === "string" && pattern.test(name)) return name;
    }
    return null;
}

function emitThinkingCtxReduceOnce(h: RustTestHarness, tag: number): () => boolean {
    let emitted = false;
    h.mock.addMatcher((body) => {
        if (emitted || !JSON.stringify(body.system ?? "").includes("## Eidnara")) return null;
        const name = toolName(body, /^ctx_reduce$/);
        if (!name) return null;
        emitted = true;
        return {
            content: [
                {
                    type: "tool_use",
                    id: `toolu_reduce_${crypto.randomUUID().replace(/-/g, "").slice(0, 16)}`,
                    name,
                    input: { drop: String(tag) },
                },
            ],
            stop_reason: "tool_use" as const,
            usage: {
                input_tokens: 45_000,
                output_tokens: 20,
                cache_creation_input_tokens: 0,
                cache_read_input_tokens: 0,
            },
        };
    });
    return () => emitted;
}

/** The helper resolves the public §N§ handle for the message containing `needle`. */
/**
 * The daemon always emits the `<session-history>` wrapper, empty or not, and a compartment heading `## <start>-<end> ·` names the raw message ordinals it replaced.
 * Raw ordinals are 1-based over the session's user and assistant messages, so a driver knows the ordinal of a turn from its position in the prompts it sent.
 */
export function publishedHistoryCovers(text: string, ordinal: number): boolean {
    const block = text.match(/<session-history>([\s\S]*?)<\/session-history>/u);
    if (block === null) return false;
    for (const heading of (block[1] ?? "").matchAll(/^\s*## (\d+)-(\d+) /gmu)) {
        const start = Number(heading[1]);
        const end = Number(heading[2]);
        if (start <= ordinal && ordinal <= end) return true;
    }
    return false;
}

function tagForText(body: Record<string, unknown>, needle: string): number {
    for (const message of messagesOf(body)) {
        if (!Array.isArray(message.content)) continue;
        for (const block of message.content) {
            const text = block.text;
            if (typeof text !== "string" || !text.includes(needle)) continue;
            const match = text.match(/§(\d+)§/u);
            if (match) return Number(match[1]);
        }
    }
    throw new Error(`no §N§ tag found for ${JSON.stringify(needle)}`);
}

async function ageTagBeyondProtectedWindow(h: RustTestHarness, sessionId: string): Promise<void> {
    h.mock.reset();
    h.mock.setDefault({
        text: "aging response",
        usage: {
            input_tokens: 1_000,
            output_tokens: 20,
            cache_creation_input_tokens: 0,
            cache_read_input_tokens: 1_000,
        },
    });
    for (let turn = 0; turn < 6; turn += 1) {
        await h.sendPrompt(sessionId, `aging turn ${turn + 1}`);
    }
}

async function dropAndMaterialize(
    h: RustTestHarness,
    sessionId: string,
    tag: number,
): Promise<{ body: Record<string, unknown>; dropEmitted: boolean }> {
    h.mock.reset();
    const wasDropEmitted = emitThinkingCtxReduceOnce(h, tag);
    h.mock.setDefault({
        text: "after reduce",
        usage: {
            input_tokens: 45_000,
            output_tokens: 20,
            cache_creation_input_tokens: 0,
            cache_read_input_tokens: 0,
        },
    });
    await h.sendPrompt(sessionId, `mark §${tag}§ spent`);

    h.mock.reset();
    h.mock.setDefault({
        text: "after materialization",
        usage: {
            input_tokens: 1_000,
            output_tokens: 20,
            cache_creation_input_tokens: 0,
            cache_read_input_tokens: 1_000,
        },
    });
    await h.sendPrompt(sessionId, "inspect the reduced history");
    return {
        body: mainRequests(h).at(-1)!.body,
        dropEmitted: wasDropEmitted(),
    };
}

const NUDGE_MARKERS = [
    '<instruction name="context_',
    "context_iteration",
    "context_warning",
    "context_critical",
] as const;

export interface ThinkingNudgeAnchorObservation extends Record<string, JsonValue> {
    mainRequestCount: number;
    assistantCandidates: number;
    /** Inspected requests (every request after the establishing turn) whose wire carries no assistant; the nudge cannot be tested on such a request. */
    requestsWithoutAssistant: number;
    nudgeMarkerFound: boolean;
    thinkingBlockCount: number;
}

export async function driveThinkingNudgeAnchor(
    h: RustTestHarness,
): Promise<ThinkingNudgeAnchorObservation> {
    h.mock.reset();

    const signedThinking = "Let me work through this carefully step by step.";
    const signature = "opaque-provider-signature-bug-a";

    // The response includes thinking and text so the assistant carries a signed thinking block.
    // A response at 46% of 50K keeps the nudge anchoring logic live.
    h.mock.setDefault({
        content: [
            { type: "thinking", thinking: signedThinking, signature },
            { type: "text", text: "Here is the answer." },
        ],
        usage: {
            input_tokens: 23_000,
            output_tokens: 200,
            cache_creation_input_tokens: 0,
            cache_read_input_tokens: 0,
        },
    });

    const sessionId = await h.createSession();
    await h.sendPrompt(sessionId, "turn 1 — establish the thinking block");
    await h.sendPrompt(sessionId, "turn 2 — give nudge logic a chance to anchor");
    await h.sendPrompt(sessionId, "turn 3 — defer pass must not mutate signed msg");

    const reqs = mainRequests(h);
    // The mock accepts what Anthropic would reject, so a mutation on turn 2 that is cleared before turn 3 is visible only on turn 2's request.
    const inspected = reqs.slice(1).map((request) => request.body);
    const assistantsByRequest = inspected.map((body) =>
        messagesOf(body).filter((m) => m.role === "assistant"),
    );
    const assistants = assistantsByRequest.flat();

    const nudgeMarkerFound = assistants.some((asst) => {
        const serialized = JSON.stringify(asst.content);
        return NUDGE_MARKERS.some((marker) => serialized.includes(marker));
    });

    return {
        mainRequestCount: reqs.length,
        assistantCandidates: assistants.length,
        requestsWithoutAssistant: assistantsByRequest.filter((list) => list.length === 0).length,
        nudgeMarkerFound,
        thinkingBlockCount: inspected.reduce(
            (count, body) => count + findThinkingBlocks(body).length,
            0,
        ),
    };
}

export function verifyThinkingNudgeAnchor(
    observation: ThinkingNudgeAnchorObservation,
): RegressionResult {
    return resultFromChecks([
        {
            id: "check-thinking-a-no-nudge-in-signed-assistant",
            passed: !observation.nudgeMarkerFound,
        },
        {
            // The transform clears historical reasoning, so no signed thinking block reaches the wire.
            id: "check-thinking-a-signature-byte-stable",
            passed: observation.thinkingBlockCount === 0,
        },
        {
            // A request with no assistant cannot show the nudge, so every inspected request must carry one.
            id: "check-thinking-a-nonvacuous-inspection",
            passed:
                observation.mainRequestCount >= 3 &&
                observation.assistantCandidates > 0 &&
                observation.requestsWithoutAssistant === 0,
        },
    ]);
}

export interface ThinkingDroppedShellObservation extends Record<string, JsonValue> {
    dropEmitted: boolean;
    pasteBodyAbsent: boolean;
    shellPreserved: boolean;
    signedReplayIntact: boolean;
    turnBoundaryPreserved: boolean;
}

export async function driveThinkingDroppedShell(
    h: RustTestHarness,
): Promise<ThinkingDroppedShellObservation> {
    h.mock.reset();

    const signedThinkingA = "First thinking block for turn one.";
    const signedThinkingB = "Second thinking block for turn two.";
    const sigA = "sig-bug-b-turn-one";
    const sigB = "sig-bug-b-turn-two";

    h.mock.script([
        {
            content: [
                {
                    type: "thinking",
                    thinking: signedThinkingA,
                    signature: sigA,
                },
                { type: "text", text: "Response to turn 1." },
            ],
            usage: {
                input_tokens: 15_000,
                output_tokens: 100,
                cache_creation_input_tokens: 0,
                cache_read_input_tokens: 0,
            },
        },
        {
            content: [
                {
                    type: "thinking",
                    thinking: signedThinkingB,
                    signature: sigB,
                },
                { type: "text", text: "Response to turn 2." },
            ],
            usage: {
                input_tokens: 18_000,
                output_tokens: 100,
                cache_creation_input_tokens: 10_000,
                cache_read_input_tokens: 5_000,
            },
        },
    ]);
    h.mock.setDefault({
        content: [{ type: "text", text: "follow-up" }],
        usage: {
            input_tokens: 19_000,
            output_tokens: 50,
            cache_creation_input_tokens: 10_000,
            cache_read_input_tokens: 9_000,
        },
    });

    const sessionId = await h.createSession();
    await h.sendPrompt(sessionId, "please explain how the drop logic works");

    // The public §N§ handle avoids coupling either mode to its private store.
    const paste = `Here is a log of the failing session:\n${"ERROR: call_failed at line 42.\n".repeat(60)}`;
    await h.sendPrompt(sessionId, paste);

    const pasteTag = tagForText(
        mainRequests(h).at(-1)!.body,
        "Here is a log of the failing session:",
    );
    await ageTagBeyondProtectedWindow(h, sessionId);
    const reduced = await dropAndMaterialize(h, sessionId, pasteTag);
    const body = reduced.body;

    const allUserText = userText(body);
    const pasteBodyAbsent = !allUserText.includes("ERROR: call_failed at line 42.");
    // The transform replaces covered turns with one published history summary; the paste is the third raw message (prompt, reply, paste).
    const shellPreserved = publishedHistoryCovers(allUserText, 3);

    // Historical reasoning is cleared, so no signed thinking block may survive on the wire.
    const signedReplayIntact = findThinkingBlocks(body).length === 0;

    const messages = messagesOf(body);
    let turnBoundaryPreserved = true;
    for (let i = 1; i < messages.length; i++) {
        if (messages[i - 1]!.role === "assistant" && messages[i]!.role === "assistant") {
            turnBoundaryPreserved = false;
        }
    }

    return {
        dropEmitted: reduced.dropEmitted,
        pasteBodyAbsent,
        shellPreserved,
        signedReplayIntact,
        turnBoundaryPreserved,
    };
}

export function verifyThinkingDroppedShell(
    observation: ThinkingDroppedShellObservation,
): RegressionResult {
    return resultFromChecks([
        {
            id: "check-thinking-b-drop-emitted",
            passed: observation.dropEmitted,
        },
        {
            id: "check-thinking-b-paste-body-absent",
            passed: observation.pasteBodyAbsent,
        },
        {
            id: "check-thinking-b-shell-preserved",
            passed: observation.shellPreserved,
        },
        {
            id: "check-thinking-b-signed-replay-intact",
            passed: observation.signedReplayIntact,
        },
        {
            id: "check-thinking-b-turn-boundary-preserved",
            passed: observation.turnBoundaryPreserved,
        },
    ]);
}

export interface ThinkingImageSurvivalObservation extends Record<string, JsonValue> {
    dropEmitted: boolean;
    droppedTextAbsent: boolean;
    coveredByRustHistory: boolean;
    imageBlockCount: number;
    imagePayloadPreserved: boolean;
    placeholderPresent: boolean;
    userWithImagePresent: boolean;
}

export async function driveThinkingImageSurvival(
    h: RustTestHarness,
): Promise<ThinkingImageSurvivalObservation> {
    h.mock.reset();
    h.mock.setDefault({
        content: [{ type: "text", text: "I see the screenshot." }],
        usage: {
            input_tokens: 22_000,
            output_tokens: 50,
            cache_creation_input_tokens: 0,
            cache_read_input_tokens: 0,
        },
    });

    const sessionId = await h.createSession();

    // The request uses the raw client because the text-only prompt helper cannot include a file part.
    const sdk = await import("@opencode-ai/sdk");
    // The server accepts file parts although the published prompt type omits them.
    const rawClient = sdk.createOpencodeClient({
        baseUrl: h.opencode.url,
    }) as unknown as {
        session: {
            prompt: (opts: {
                path: { id: string };
                body: {
                    model: { providerID: string; modelID: string };
                    parts: Array<{
                        type: "text" | "file";
                        text?: string;
                        mime?: string;
                        url?: string;
                        filename?: string;
                    }>;
                };
            }) => Promise<unknown>;
        };
    };

    // The fixture uses a 1×1 transparent PNG data URL.
    const imageDataUrl =
        "data:image/png;base64,iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mNkYAAAAAYAAjCB0C8AAAAASUVORK5CYII=";

    await rawClient.session.prompt({
        path: { id: sessionId },
        body: {
            model: { providerID: "mock-anthropic", modelID: "mock-sonnet" },
            parts: [
                { type: "text", text: "see this screenshot for the bug" },
                {
                    type: "file",
                    mime: "image/png",
                    url: imageDataUrl,
                    filename: "bug.png",
                },
            ],
        },
    });

    // The drop targets only the text block through its public §N§ handle because the image is a sibling content block.
    const userTextTag = tagForText(mainRequests(h).at(-1)!.body, "see this screenshot for the bug");
    await ageTagBeyondProtectedWindow(h, sessionId);
    const reduced = await dropAndMaterialize(h, sessionId, userTextTag);
    const body = reduced.body;

    const allUserBlocks = blocksOfRole(body, "user");
    const imageBlocks = allUserBlocks.filter((b) => b.type === "image");
    // The test compares the surviving image payload with the sent payload because block counts cannot detect replacement, MIME rewrites, or re-encoding.
    const expectedImageBase64 = imageDataUrl.slice(imageDataUrl.indexOf(",") + 1);
    const imagePayloadPreserved = imageBlocks.some((block) => {
        const source = (block as { source?: unknown }).source;
        if (!source || typeof source !== "object") return false;
        const value = source as { media_type?: unknown; data?: unknown };
        return value.media_type === "image/png" && value.data === expectedImageBase64;
    });
    const allUserText = allUserBlocks
        .filter((block) => block.type === "text")
        .map((block) => block.text ?? "")
        .join("\n");

    return {
        dropEmitted: reduced.dropEmitted,
        droppedTextAbsent: !allUserText.includes("see this screenshot for the bug"),
        // The image prompt is the session's first raw message.
        coveredByRustHistory: publishedHistoryCovers(allUserText, 1),
        imageBlockCount: imageBlocks.length,
        imagePayloadPreserved,
        placeholderPresent: /\[dropped \u00a7\d+\u00a7\]/.test(allUserText),
        userWithImagePresent: messagesOf(body).some(
            (m) =>
                m.role === "user" &&
                Array.isArray(m.content) &&
                m.content.some((b) => b.type === "image"),
        ),
    };
}

export function verifyThinkingImageSurvival(
    observation: ThinkingImageSurvivalObservation,
): RegressionResult {
    // A Rust history range replaces the text shell and image for every raw block it covers.
    const covered = observation.coveredByRustHistory;
    return resultFromChecks([
        {
            id: "check-thinking-c-drop-emitted",
            passed: observation.dropEmitted,
        },
        {
            id: "check-thinking-c-dropped-text-absent",
            passed: observation.droppedTextAbsent,
        },
        {
            id: "check-thinking-c-shell-preserved",
            passed: covered || observation.placeholderPresent,
        },
        {
            id: "check-thinking-c-image-part-survives",
            passed: covered
                ? observation.imageBlockCount === 0
                : observation.imageBlockCount > 0 &&
                  observation.userWithImagePresent &&
                  observation.imagePayloadPreserved,
        },
    ]);
}

// A1 and A3 judge the prefix rendered by `transform.rs` and carried by `rust-mode-transform.ts`;
// `cache-analysis.ts` determines `mainRequestCount` and `bustCount`, so it is part of the digest.
// `scripted-tool-call.ts` supplies `DEFER_USAGE`, whose token counts decide whether each turn defers.
const RUST_CACHE_IMPLEMENTATION_FILES = [
    "packages/e2e-tests/src/incident-pool/scenarios/source-linked-regressions.ts",
    "packages/e2e-tests/src/incident-pool/support/tool-loop.ts",
    "packages/e2e-tests/src/rust-harness.ts",
    "packages/e2e-tests/src/rust-runner/hermetic-host.ts",
    "packages/e2e-tests/src/opencode-runner/spawn.ts",
    "packages/e2e-tests/src/cache-analysis.ts",
    "packages/e2e-tests/src/scripted-tool-call.ts",
    "packages/opencode-plugin/src/hooks/context/hook-handlers.ts",
    "packages/opencode-plugin/src/hooks/context/rust-mode-transform.ts",
    "crates/daemon/src/transform.rs",
];

export const FIRST_RENDER_A1_FIXTURE = {
    scenario: "pure-defer-growth",
    turns: 6,
    modelContextLimit: 100_000,
    executeThresholdPercentage: 20,
} as const;

export const FIRST_RENDER_A3_FIXTURE = {
    scenario: "aged-ctx-reduce-defer-growth",
    turns: 8,
    // Turn 2's `ctx_reduce` tool_use adds one continuation request carrying the tool result.
    mainRequests: 9,
    drop: "99999",
    callId: "toolu_incident_a3_ctx_reduce",
    requiredWireEvidence: "matching ctx_reduce tool_use and tool_result blocks",
    modelContextLimit: 100_000,
    executeThresholdPercentage: 20,
} as const;

function exactPrimitiveObservation(
    raw: JsonValue,
    kind: string,
    fields: Record<string, "boolean" | "number" | "string">,
): Record<string, JsonValue> {
    if (typeof raw !== "object" || raw === null || Array.isArray(raw)) {
        throw new Error(`${kind} observation must be an object`);
    }
    const keys = Object.keys(raw).sort();
    const expected = Object.keys(fields).sort();
    if (keys.join("\0") !== expected.join("\0")) {
        throw new Error(`${kind} observation fields do not match the contract`);
    }
    for (const [field, type] of Object.entries(fields)) {
        if (typeof raw[field] !== type) {
            throw new Error(`${kind}.${field} must be ${type}`);
        }
    }
    return raw;
}

function numberField(observation: Record<string, JsonValue>, field: string): number {
    const value = observation[field];
    if (typeof value !== "number") throw new Error(`${field} must be number`);
    return value;
}

function stringField(observation: Record<string, JsonValue>, field: string): string {
    const value = observation[field];
    if (typeof value !== "string") throw new Error(`${field} must be string`);
    return value;
}

function booleanField(observation: Record<string, JsonValue>, field: string): boolean {
    const value = observation[field];
    if (typeof value !== "boolean") throw new Error(`${field} must be boolean`);
    return value;
}

const CACHE_STABILITY_FIELDS = {
    mainRequestCount: "number",
    bustCount: "number",
    bustReport: "string",
    uncachedTransitionCount: "number",
    transformRenderedRequestCount: "number",
    rustPassCount: "number",
    transformServedPassCount: "number",
    deferredPassCount: "number",
} as const;

function cacheStabilityFields(value: Record<string, JsonValue>): CacheStabilityEvidence {
    return {
        mainRequestCount: numberField(value, "mainRequestCount"),
        bustCount: numberField(value, "bustCount"),
        bustReport: stringField(value, "bustReport"),
        uncachedTransitionCount: numberField(value, "uncachedTransitionCount"),
        transformRenderedRequestCount: numberField(value, "transformRenderedRequestCount"),
        rustPassCount: numberField(value, "rustPassCount"),
        transformServedPassCount: numberField(value, "transformServedPassCount"),
        deferredPassCount: numberField(value, "deferredPassCount"),
    };
}

function normalizeFirstRenderA1(raw: JsonValue): FirstRenderDeferObservation {
    return cacheStabilityFields(
        exactPrimitiveObservation(raw, "parity-a1", CACHE_STABILITY_FIELDS),
    );
}

function normalizeFirstRenderA3(raw: JsonValue): AgedCtxReduceObservation {
    const value = exactPrimitiveObservation(raw, "parity-a3", {
        sawReduceOnWire: "boolean",
        finalWireHasCtxReduce: "boolean",
        ...CACHE_STABILITY_FIELDS,
    });
    return {
        sawReduceOnWire: booleanField(value, "sawReduceOnWire"),
        finalWireHasCtxReduce: booleanField(value, "finalWireHasCtxReduce"),
        ...cacheStabilityFields(value),
    };
}

async function withCaseHarness<T extends JsonValue>(
    context: CaseDriverContext,
    options: RustTestHarnessOptions,
    run: (harness: RustTestHarness) => Promise<T>,
): Promise<T> {
    const harness = await createCaseHarness(context, options);
    try {
        return await run(harness);
    } finally {
        await harness.dispose();
    }
}

function rustPrerequisite(): { ok: true } | { ok: false; reason: string } {
    const result = detectRustPrerequisites();
    return result.ok ? { ok: true } : { ok: false, reason: result.missing.join("; ") };
}

function satisfiedPrecondition(): PreconditionOutcome {
    return { satisfied: true };
}

export function sourceLinkedRegressionIncidentCases(): RegisteredIncidentCase[] {
    return [
        {
            variantId: "var-parity-a1-pure-defer-stability",
            implementationFiles: RUST_CACHE_IMPLEMENTATION_FILES,
            fixtures: { ...FIRST_RENDER_A1_FIXTURE },
            driver: adaptBoundSymbol(
                driveFirstRenderPureDeferStability,
                (inner) => (context) =>
                    withCaseHarness(context, FIRST_RENDER_HARNESS_OPTIONS, (h) => inner(h)),
            ),
            normalizer: normalizeFirstRenderA1,
            precondition: satisfiedPrecondition,
            verifier: adaptBoundSymbol(
                verifyFirstRenderPureDeferStability,
                (inner) => (raw) => inner(normalizeFirstRenderA1(raw)).checks,
            ),
            binding: {
                driver: driveFirstRenderPureDeferStability,
                verifier: verifyFirstRenderPureDeferStability,
            },
            prerequisite: rustPrerequisite,
        },
        {
            variantId: "var-parity-a3-ctx-reduce-survival",
            implementationFiles: RUST_CACHE_IMPLEMENTATION_FILES,
            fixtures: { ...FIRST_RENDER_A3_FIXTURE },
            driver: adaptBoundSymbol(
                driveAgedCtxReduceSurvival,
                (inner) => (context) =>
                    withCaseHarness(context, FIRST_RENDER_HARNESS_OPTIONS, (h) => inner(h)),
            ),
            normalizer: normalizeFirstRenderA3,
            precondition: satisfiedPrecondition,
            verifier: adaptBoundSymbol(
                verifyAgedCtxReduceSurvival,
                (inner) => (raw) => inner(normalizeFirstRenderA3(raw)).checks,
            ),
            binding: {
                driver: driveAgedCtxReduceSurvival,
                verifier: verifyAgedCtxReduceSurvival,
            },
            prerequisite: rustPrerequisite,
        },
    ];
}
