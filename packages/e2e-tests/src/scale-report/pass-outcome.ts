import { isHostCallError } from "@eidnara/opencode/shared/host-client/errors";

import type { PassOutcome, RefusalReason } from "./rows";

export interface PassObservation {
    ok: boolean;
    error?: unknown;
}

/** The transform call timed out after its request may have been sent. */
function isDeadline(error: unknown): boolean {
    if (!(error instanceof Error)) return false;
    const code = (error as { code?: unknown }).code;
    return (
        (code === "ETIMEDOUT" && !/while queued/.test(error.message)) ||
        /request deadline expired after a possible send/.test(error.message)
    );
}

export function classifyPass(pass: PassObservation): {
    outcome: PassOutcome;
    refusal: RefusalReason | null;
} {
    if (pass.ok) return { outcome: "completed", refusal: null };
    if (isDeadline(pass.error)) return { outcome: "censored", refusal: null };
    if (pass.error === undefined) return { outcome: "refused", refusal: "declined" };
    const answered = isHostCallError(pass.error) && pass.error.kind === "terminal";
    return { outcome: "refused", refusal: answered ? "daemon_error" : "transport_error" };
}
