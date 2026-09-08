import { createHash } from "node:crypto";
import { BoundedSessionMap } from "../../shared/bounded-session-map";
import { piModelRefToCanonical } from "../../shared/harness-provider-map";
import { sessionLog } from "../../shared/logger";
import { type PromptSurfaceConfig, resolvePromptSurface } from "../../shared/prompt-surface";
import { promptSurfaceHashMaterial } from "../../shared/prompt-surface-runtime";
import { resolveCtxReduceAvailability } from "./ctx-reduce-availability";
import {
    EIDNARA_INTERNAL_AGENT_SIGNATURES,
    INTERNAL_OPENCODE_AGENT_SIGNATURES,
} from "./internal-agent-signatures";
import { estimateTokens } from "./read-session-formatting";

/** The plugin's per-session view of the host system prompt, refreshed on every tracked pass. */
export interface SystemPromptState {
    /** Hexadecimal MD5 of the frozen prompt content and the prompt-surface preset; empty until the ctx_reduce verdict is frozen and the model is known. */
    systemPromptHash: string;
    systemPromptTokens: number;
    isSubagent: boolean;
}

/** Everything the handler remembers per session; one LRU entry bounds it all by `SYSTEM_PROMPT_STATE_CAPACITY`. */
interface SessionTracking {
    /** Sticky dates change only on cache-busting passes, preventing midnight cache rebuilds. */
    stickyDate?: string;
    prompt?: SystemPromptState;
}

/** One entry per tracked session; the LRU bound matches the ctx_reduce verdict caches. */
const SYSTEM_PROMPT_STATE_CAPACITY = 1000;

/**
 * The host emits `Today's date: ${new Date().toDateString()}`, e.g. `Today's date: Tue Sep 08 2026`. commentlint: allow(JUDGE)
 * Matches only complete date lines, excluding prose mentions and date-shaped examples mid-sentence.
 * The lookarounds keep the matched text to the phrase itself, so the rewrite preserves indentation.
 */
const DATE_LINE =
    /(?<=(?:^|\n)[ \t]*)Today's date: [A-Z][a-z]{2} [A-Z][a-z]{2} \d{2} \d{4}(?=[ \t]*(?:\n|$))/;
const DATE_LINE_ALL = new RegExp(DATE_LINE.source, "g");

/**
 * OpenCode joins the agent prompt first into one system segment (`session/llm/request.ts`), so a
 * built-in prompt's opening line is the opening of a segment. Only match segment openings so
 * custom agents quoting a signature are not classified as internal.
 */
function segmentOpensWith(
    systemSegments: readonly string[],
    signatures: readonly string[],
): boolean {
    return systemSegments.some((segment) => {
        const opening = segment.trimStart();
        return signatures.some((signature) => opening.startsWith(signature));
    });
}

/** Title, summary, and compaction calls share the main session id; tracking their hash would flush the main agent's cache. commentlint: allow(JUDGE) */
function isInternalOpenCodeAgent(systemSegments: readonly string[]): boolean {
    return segmentOpensWith(systemSegments, INTERNAL_OPENCODE_AGENT_SIGNATURES);
}

/** Hidden child agents use fixed prompts, so their hashes must not enter primary-session tracking. */
export function isEidnaraInternalAgent(systemSegments: readonly string[] | string): boolean {
    const segments = typeof systemSegments === "string" ? [systemSegments] : systemSegments;
    return segmentOpensWith(segments, EIDNARA_INTERNAL_AGENT_SIGNATURES);
}

export function createSystemPromptHashHandler(deps: {
    promptSurface?: PromptSurfaceConfig;
    /** `resolveModel` recovers the session's latest model when the transform input omits it. */
    resolveModel?: (sessionId: string) => { providerID: string; modelID: string } | undefined;
    isSubagentSession?: (sessionId: string) => boolean;
    /** Membership at handler entry marks the pass cache-busting; the handler drains the entry at exit. */
    systemPromptRefreshSessions: Set<string>;
    /** A hash change adds the session so history is re-read. */
    historyRefreshSessions: Set<string>;
    pendingMaterializationSessions: Set<string>;
    lastHeuristicsTurnId: Map<string, string>;
    /** When false, the plugin tracks no prompt hash for any agent. */
    injectionEnabled?: boolean;
    /** A call whose system prompt contains one of these substrings is not tracked. */
    injectionSkipSignatures?: string[];
    /** Prompt signatures identify hidden children before session-created tracking adds them to `internalChildSessions`. */
    internalChildSessions?: Set<string>;
}): {
    handler: (
        input: {
            sessionID?: string;
            model?: { providerID?: string; modelID?: string };
        },
        output: { system: string[] },
    ) => Promise<void>;
    promptStateFor: (sessionId: string) => SystemPromptState | undefined;
    clearSession: (sessionId: string) => void;
} {
    const isSubagentSession = deps.isSubagentSession ?? (() => false);

    const trackingBySession = new BoundedSessionMap<SessionTracking>(SYSTEM_PROMPT_STATE_CAPACITY);

    const handler = async (
        input: {
            sessionID?: string;
            model?: { providerID?: string; modelID?: string };
        },
        output: { system: string[] },
    ): Promise<void> => {
        const sessionId = input.sessionID;
        if (!sessionId) return;

        const fullPromptForDetection = output.system.join("\n");
        if (isInternalOpenCodeAgent(output.system)) {
            sessionLog(
                sessionId,
                "system-prompt-hash skipped (OpenCode internal agent: title/summary/compaction)",
            );
            return;
        }

        if (deps.internalChildSessions?.has(sessionId) || isEidnaraInternalAgent(output.system)) {
            sessionLog(sessionId, "system-prompt-hash skipped (Eidnara internal child)");
            return;
        }

        const injectionEnabled = deps.injectionEnabled !== false;
        const skipSignatures = deps.injectionSkipSignatures ?? [];
        if (!injectionEnabled) {
            sessionLog(sessionId, "system-prompt-hash skipped (injection globally disabled)");
            return;
        }
        if (skipSignatures.some((sig) => sig.length > 0 && fullPromptForDetection.includes(sig))) {
            sessionLog(
                sessionId,
                "system-prompt-hash skipped (matched system_prompt_injection.skip_signatures)",
            );
            return;
        }

        const availability = resolveCtxReduceAvailability(sessionId);
        const inputModel = input.model;
        const liveModel =
            inputModel?.providerID && inputModel.modelID
                ? { providerID: inputModel.providerID, modelID: inputModel.modelID }
                : deps.resolveModel?.(sessionId);
        const modelKey =
            liveModel?.providerID && liveModel.modelID
                ? piModelRefToCanonical(`${liveModel.providerID}/${liveModel.modelID}`)
                : undefined;
        const promptSurface = resolvePromptSurface(deps.promptSurface, modelKey);

        const isCacheBusting = deps.systemPromptRefreshSessions.has(sessionId);

        const liveSystemContent = output.system.join("\n");
        if (liveSystemContent.length === 0) return;
        const tracked = trackingBySession.get(sessionId) ?? {};
        const previousState = tracked.prompt;
        const previousHash = previousState?.systemPromptHash ?? "";
        const hasPersistedHash = previousHash !== "" && previousHash !== "0";
        // Parentage is immutable, so a recorded subagent stays one even when the lookup later fails or no longer knows the session.
        let isSubagent = previousState?.isSubagent ?? false;
        try {
            isSubagent = isSubagent || isSubagentSession(sessionId);
        } catch (error) {
            sessionLog(sessionId, "system-prompt-hash subagent lookup failed:", error);
        }
        // Every element containing a date line participates in freezing.
        // A host prompt with a matching date line must freeze that line too; otherwise its hash changes at midnight.
        const dateElementIndexes: number[] = [];
        let currentDate: string | undefined;
        for (let i = 0; i < output.system.length; i++) {
            const match = output.system[i].match(DATE_LINE);
            if (!match) continue;
            dateElementIndexes.push(i);
            currentDate ??= match[0];
        }
        const stickyDate = tracked.stickyDate;
        const stableCandidate =
            currentDate && stickyDate && currentDate !== stickyDate
                ? liveSystemContent.replace(DATE_LINE_ALL, stickyDate)
                : liveSystemContent;
        const stableCandidateHash = createHash("md5")
            .update(promptSurfaceHashMaterial(stableCandidate, promptSurface.preset))
            .digest("hex");
        const contentOrPresetChanged = hasPersistedHash && stableCandidateHash !== previousHash;
        const dateMayAdvance = isCacheBusting || contentOrPresetChanged;

        if (currentDate && !stickyDate) {
            tracked.stickyDate = currentDate;
            trackingBySession.set(sessionId, tracked);
        } else if (currentDate && stickyDate && currentDate !== stickyDate) {
            if (dateMayAdvance) {
                tracked.stickyDate = currentDate;
                trackingBySession.set(sessionId, tracked);
                sessionLog(
                    sessionId,
                    `system prompt date updated: ${stickyDate} → ${currentDate} (cache-busting pass)`,
                );
            } else if (dateElementIndexes.length > 0) {
                for (const index of dateElementIndexes) {
                    output.system[index] = output.system[index].replace(DATE_LINE_ALL, stickyDate);
                }
                sessionLog(
                    sessionId,
                    `system prompt date frozen: real=${currentDate}, using=${stickyDate} (defer pass)`,
                );
            }
        }

        const systemContent = output.system.join("\n");

        // The hash waits for a frozen ctx_reduce verdict and a known model; the classification does not, so a subagent's first turn is not reported as primary.
        const hashReady = availability.frozen && modelKey !== undefined;
        const currentHash = hashReady
            ? createHash("md5")
                  .update(promptSurfaceHashMaterial(systemContent, promptSurface.preset))
                  .digest("hex")
            : previousHash;

        // A Set does not record when an entry was added.
        let refreshRaisedThisPass = false;
        if (hashReady && hasPersistedHash && previousHash !== currentHash) {
            sessionLog(
                sessionId,
                `system prompt hash changed: ${previousHash} → ${currentHash} (len=${systemContent.length}), triggering flush`,
            );
            // A prompt-content or preset change must refresh history, adjuncts, and materialization together.
            deps.historyRefreshSessions.add(sessionId);
            deps.systemPromptRefreshSessions.add(sessionId);
            refreshRaisedThisPass = true;
            deps.pendingMaterializationSessions.add(sessionId);
            deps.lastHeuristicsTurnId.delete(sessionId);
        } else if (hashReady && !hasPersistedHash) {
            sessionLog(
                sessionId,
                `system prompt hash initialized: ${currentHash} (len=${systemContent.length})`,
            );
        }

        // Failed token estimation must not abort the LLM call; the hash still updates.
        if (currentHash !== previousHash || previousState === undefined) {
            let systemPromptTokens = previousState?.systemPromptTokens ?? 0;
            try {
                systemPromptTokens = estimateTokens(systemContent);
            } catch (error) {
                sessionLog(
                    sessionId,
                    "system prompt token estimate failed (using prior count):",
                    error,
                );
            }
            tracked.prompt = {
                systemPromptHash: currentHash,
                systemPromptTokens,
                isSubagent,
            };
            trackingBySession.set(sessionId, tracked);
        } else if (previousState.isSubagent !== isSubagent) {
            // An unchanged hash still records a changed classification without re-estimating tokens.
            tracked.prompt = { ...previousState, isSubagent };
            trackingBySession.set(sessionId, tracked);
        }

        // A pass that cannot persist the hash leaves the refresh flag for the one that can.
        if (!hashReady) return;

        // Drain only the refresh entry present at handler entry; one raised by this pass's hash change stays so the next pass is cache-busting.
        if (isCacheBusting && !refreshRaisedThisPass) {
            deps.systemPromptRefreshSessions.delete(sessionId);
        }
    };

    return {
        handler,
        promptStateFor: (sessionId: string) => trackingBySession.peek(sessionId)?.prompt,
        clearSession: (sessionId: string) => {
            trackingBySession.delete(sessionId);
        },
    };
}
