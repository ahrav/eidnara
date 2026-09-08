/**
 * Shared prerequisites, gates, and drivers for the rust-mode e2e scenarios.
 *
 * `rustPrereqs` is evaluated once at import so every suite guards with the same verdict.
 * Optional scenarios stay gated behind explicit environment switches and print why they skip.
 */

import { RustTestHarness } from "./rust-harness";

export const rustPrereqs = RustTestHarness.detectPrereqs();

export function foldInfraEnabled(): boolean {
    return process.env.EIDNARA_RUST_E2E_FOLD === "1";
}

export const FOLD_SKIP_REASON =
    "requires broad Rust fold qualification beyond the focused direct " +
    "backend fixture; set EIDNARA_RUST_E2E_FOLD=1 to run it";

export function duplicateIdInfraEnabled(): boolean {
    return process.env.EIDNARA_RUST_E2E_DUPLICATE_IDS === "1";
}

export const DUPLICATE_ID_SKIP_REASON =
    "requires broad duplicate-ID qualification beyond the focused direct " +
    "backend fixture; set EIDNARA_RUST_E2E_DUPLICATE_IDS=1 to run it";

/** Gated scenarios print the skip reason to avoid silent skips. */
export function printSkip(scenario: string, reason: string): void {
    console.log(`[rust-e2e] ${scenario} SKIPPED: ${reason}`);
}

/**
 * Drives one pass that lands the initial snapshot plus `deferPasses` growing turns, then waits
 * until the transform has logged a pass for each, so callers observe a warm steady state.
 */
export async function driveToSteadyState(
    h: RustTestHarness,
    sessionId: string,
    deferPasses = 4,
): Promise<void> {
    for (let i = 1; i <= 1 + deferPasses; i += 1) {
        h.mock.setDefault({
            text: `steady assistant ${i}`,
            usage: {
                input_tokens: 2_000 * i,
                output_tokens: 20,
                cache_creation_input_tokens: 1_000,
            },
        });
        await h.sendPrompt(sessionId, `steady turn ${i}: ${h.ballast(400)}`);
    }
    await h.waitForRustPasses(1 + deferPasses);
}

/** Placeholders only ever stand in for content on the plugin side; one on the provider wire is a leak. */
export function assertMessagesHaveNoPlaceholders(
    messages: readonly unknown[],
    lineageKey: string,
): void {
    if (lineageKey.length === 0) throw new Error("placeholder assertion requires a lineage key");
    const serializedMessages = JSON.stringify(messages);
    if (/\[dropped §\d+§\]|\[truncated §\d+§\]/.test(serializedMessages)) {
        throw new Error(`placeholder found in messages[] for lineage ${lineageKey}`);
    }
}

export function sessionLogLines(h: RustTestHarness, sessionId: string): string[] {
    if (sessionId.length === 0) throw new Error("log assertion requires a session id");
    return h
        .diagnosticLog()
        .split("\n")
        .filter((line) => line.includes(`[${sessionId}]`));
}

/** The transform serves the input unchanged on failure and logs it; a silent failure is the defect. */
export function assertLoudModuleFailure(h: RustTestHarness, sessionId: string): string[] {
    const lines = sessionLogLines(h, sessionId);
    if (!lines.some((line) => line.includes("rust transform failed"))) {
        throw new Error(`module failure was not logged for lineage ${sessionId}`);
    }
    return lines;
}
