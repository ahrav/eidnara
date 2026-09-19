const BYTE_BUDGET_BYTES_PER_TOKEN = 3.5;
export const BYTE_BUDGET_REVISION = "utf8-bytes-div-3.5-v1";

/** A harness's own estimator validates the invocation locally; it is never the daemon's bound profile and never labeled exact. */
export type HarnessProfileIdentity = "opencode-heuristic" | "pi-heuristic";

export interface HarnessProfile {
    identity: HarnessProfileIdentity;
    revision: string;
    authority: "heuristic";
}

export function harnessProfile(identity: HarnessProfileIdentity): HarnessProfile {
    return {
        identity,
        revision: BYTE_BUDGET_REVISION,
        authority: "heuristic",
    };
}

export interface InvocationBudget {
    /** `undefined` when the host has not reported a context limit; nothing is gated then. */
    maxTokens: number | undefined;
    headroomPermille: number;
    profile: HarnessProfileIdentity;
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

/**
 * `entryLengths` are UTF-8 byte counts. This separate admission heuristic keeps its existing
 * byte ratio and never substitutes for native token counting.
 */
export function chargeInvocation(
    entryLengths: readonly number[],
    budget: Pick<InvocationBudget, "headroomPermille" | "profile">,
): InvocationCharge {
    let bytes = 0;
    for (const length of entryLengths) bytes += length;
    const estimatedTokens = Math.ceil(bytes / BYTE_BUDGET_BYTES_PER_TOKEN);
    return {
        profile: harnessProfile(budget.profile),
        entries: entryLengths.length,
        bytes,
        estimatedTokens,
        chargedTokens: Math.ceil((estimatedTokens * (1000 + budget.headroomPermille)) / 1000),
    };
}

/** A candidate with no more bytes than the incoming window is accepted over the limit, allowing an oversized window to compact. */
export function validateInvocation(
    candidateLengths: readonly number[],
    incomingLengths: readonly number[],
    budget: InvocationBudget,
): InvocationValidation {
    const candidate = chargeInvocation(candidateLengths, budget);
    if (budget.maxTokens === undefined) return { ok: true, candidate, reason: "limit_unknown" };
    if (candidate.chargedTokens <= budget.maxTokens) return { ok: true, candidate, reason: "fits" };
    const incoming = chargeInvocation(incomingLengths, budget);
    if (candidate.bytes <= incoming.bytes) return { ok: true, candidate, reason: "shrinks" };
    return { ok: false, candidate, incoming, limit: budget.maxTokens };
}
