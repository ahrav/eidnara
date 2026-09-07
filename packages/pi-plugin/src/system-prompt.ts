import { createHash } from "node:crypto";
import { estimateTokens } from "@eidnara/opencode/hooks/context/read-session-formatting";
import { BoundedSessionMap } from "@eidnara/opencode/shared/bounded-session-map";
import { sessionLog } from "@eidnara/opencode/shared/logger";
import type { PromptSurfacePreset } from "@eidnara/opencode/shared/prompt-surface";
import { promptSurfaceHashMaterial } from "@eidnara/opencode/shared/prompt-surface-runtime";

/**
 * `stickyDateBySession` is module-scoped so `clearPiSystemPromptSession` can release its entries.
 */
const stickyDateBySession = new Map<string, string>();

export interface PiSystemPromptState {
    /** `systemPromptHash` covers prompt content and the prompt-surface preset. */
    systemPromptHash: string;
    systemPromptTokens: number;
}

// The bound evicts the least recently used session so long-lived processes stay flat.
const systemPromptStateBySession = new BoundedSessionMap<PiSystemPromptState>(1000);

export function piSystemPromptStateFor(sessionId: string): PiSystemPromptState | undefined {
    return systemPromptStateBySession.peek(sessionId);
}

export interface SystemPromptHashResult {
    /** `systemPrompt` is the prompt sent to the LLM and may contain a frozen date. */
    systemPrompt: string;
    /** `hashChanged` reports whether prompt content or the prompt-surface preset differs from the stored hash. */
    hashChanged: boolean;
    /** `currentHash` is the content-and-preset hash stored for the session. */
    currentHash: string;
}

const DATE_PATTERN = /Today's date: .+/;

/**
 * The stored per-session hash detects content and prompt-surface preset changes.
 * `processSystemPromptForCache` returns `hashChanged=true` when the stored hash changes.
 *
 * `processSystemPromptForCache` freezes `Today's date` unless `isCacheBusting` or a hash change busts the cache.
 * A cache-busting turn updates the sticky date to the live date.
 */
export function processSystemPromptForCache(args: {
    sessionId: string;
    systemPrompt: string;
    /** `isCacheBusting` means the caller has already determined that this turn busts the cache. */
    isCacheBusting: boolean;
    promptSurfacePreset?: PromptSurfacePreset;
}): SystemPromptHashResult {
    const { sessionId, systemPrompt, isCacheBusting } = args;

    const previousState = systemPromptStateBySession.get(sessionId);
    const previousHash = previousState?.systemPromptHash ?? "";
    const isFirstHash = previousHash === "" || previousHash === "0";

    // A content or preset change permits the date to advance in the same cache-busting pass.
    let frozenPrompt = systemPrompt;
    const dateMatch = systemPrompt.match(DATE_PATTERN);
    const liveDate = dateMatch ? dateMatch[0] : null;
    const stickyDate = stickyDateBySession.get(sessionId);
    const stableCandidate =
        liveDate && stickyDate && liveDate !== stickyDate
            ? systemPrompt.replace(DATE_PATTERN, stickyDate)
            : systemPrompt;
    const stableCandidateHash = createHash("md5")
        .update(promptSurfaceHashMaterial(stableCandidate, args.promptSurfacePreset))
        .digest("hex");
    const contentOrPresetChanged = !isFirstHash && stableCandidateHash !== previousHash;
    const dateMayAdvance = isCacheBusting || contentOrPresetChanged;

    if (liveDate && !stickyDate) {
        stickyDateBySession.set(sessionId, liveDate);
    } else if (liveDate && stickyDate && liveDate !== stickyDate) {
        if (dateMayAdvance) {
            stickyDateBySession.set(sessionId, liveDate);
            sessionLog(
                sessionId,
                `system prompt date updated: ${stickyDate} → ${liveDate} (cache-busting pass)`,
            );
        } else {
            frozenPrompt = systemPrompt.replace(DATE_PATTERN, stickyDate);
            sessionLog(
                sessionId,
                `system prompt date frozen: real=${liveDate}, using=${stickyDate} (cache-stable pass)`,
            );
        }
    }

    const currentHash = createHash("md5")
        .update(promptSurfaceHashMaterial(frozenPrompt, args.promptSurfacePreset))
        .digest("hex");
    const hashChanged = !isFirstHash && currentHash !== previousHash;

    if (hashChanged) {
        sessionLog(
            sessionId,
            `system prompt hash changed: ${previousHash} → ${currentHash} (len=${frozenPrompt.length})`,
        );
    } else if (isFirstHash) {
        sessionLog(
            sessionId,
            `system prompt hash initialized: ${currentHash} (len=${frozenPrompt.length})`,
        );
    }

    // The next turn compares against the stored hash; a token drift beyond 50 refreshes the estimate alone.
    const systemPromptTokens = estimateTokens(frozenPrompt);
    if (
        previousState === undefined ||
        currentHash !== previousHash ||
        Math.abs(previousState.systemPromptTokens - systemPromptTokens) > 50
    ) {
        systemPromptStateBySession.set(sessionId, {
            systemPromptHash: currentHash,
            systemPromptTokens,
        });
    }

    return {
        systemPrompt: frozenPrompt,
        hashChanged,
        currentHash,
    };
}

export function clearPiSystemPromptSession(sessionId: string): void {
    stickyDateBySession.delete(sessionId);
    systemPromptStateBySession.delete(sessionId);
}
