import { estimateTokensFromLength, tokenEstimatorGeneration } from "../../shared/token-estimator";

/** The plugin's estimator validates the invocation locally; it is never the daemon's bound profile and never labeled exact. */
export const HARNESS_PROFILE_IDENTITY = "opencode-heuristic";

export interface HarnessProfile {
    identity: typeof HARNESS_PROFILE_IDENTITY;
    revision: string;
    authority: "heuristic";
}

export function harnessProfile(): HarnessProfile {
    return {
        identity: HARNESS_PROFILE_IDENTITY,
        revision: `generation:${tokenEstimatorGeneration()}`,
        authority: "heuristic",
    };
}

export interface InvocationBudget {
    /** `undefined` when the host has not reported a context limit; nothing is gated then. */
    maxTokens: number | undefined;
    headroomPermille: number;
}

export interface InvocationCharge {
    profile: HarnessProfile;
    entries: number;
    bytes: number;
    estimatedTokens: number;
    chargedTokens: number;
}

export type InvocationValidation =
    | { ok: true; candidate: InvocationCharge; reason: "fits" | "shrinks" | "limit_unknown" }
    | { ok: false; candidate: InvocationCharge; incoming: InvocationCharge; limit: number };

export function chargeInvocation(
    entryLengths: readonly number[],
    headroomPermille: number,
): InvocationCharge {
    let bytes = 0;
    for (const length of entryLengths) bytes += length;
    const estimatedTokens = estimateTokensFromLength(bytes);
    return {
        profile: harnessProfile(),
        entries: entryLengths.length,
        bytes,
        estimatedTokens,
        chargedTokens: Math.ceil((estimatedTokens * (1000 + headroomPermille)) / 1000),
    };
}

/** A candidate with no more bytes than the incoming window is accepted over the limit, allowing an oversized window to compact. */
export function validateInvocation(
    candidateLengths: readonly number[],
    incomingLengths: readonly number[],
    budget: InvocationBudget,
): InvocationValidation {
    const candidate = chargeInvocation(candidateLengths, budget.headroomPermille);
    if (budget.maxTokens === undefined) return { ok: true, candidate, reason: "limit_unknown" };
    if (candidate.chargedTokens <= budget.maxTokens) return { ok: true, candidate, reason: "fits" };
    const incoming = chargeInvocation(incomingLengths, budget.headroomPermille);
    if (candidate.bytes <= incoming.bytes) return { ok: true, candidate, reason: "shrinks" };
    return { ok: false, candidate, incoming, limit: budget.maxTokens };
}
