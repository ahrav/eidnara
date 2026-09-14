export const CONDITIONAL_NOTE_CHECK_POLICY_VERSION = 1;

export const CONDITIONAL_NOTE_CHECK_FLOOR_MS = 5 * 60 * 1000;
export const CONDITIONAL_NOTE_CHECK_CEILING_MS = 24 * 60 * 60 * 1000;
export const CONDITIONAL_NOTE_CHECK_DEFAULT_INTERVAL_MS = 60 * 60 * 1000;
export const CONDITIONAL_NOTE_CHECK_MAX_STALENESS_MS = 7 * 24 * 60 * 60 * 1000;
export const CONDITIONAL_NOTE_CHECK_LIVENESS_RECHECK_MS = 24 * 60 * 60 * 1000;

export type ConditionalNoteCapabilityName =
    | "readFile"
    | "gitHeadSha"
    | "gitTag"
    | "gitLog"
    | "httpGet";

export type ConditionalNoteCheckStatus = "uncompiled" | "compiled" | "failing" | "fallback";

export interface ConditionalNoteCheckManifest {
    capabilities: ConditionalNoteCapabilityName[];
    readFiles?: string[];
    hosts?: string[];
    urls?: string[];
    signals?: string[];
    summary?: string;
}

export interface ConditionalNoteCheckResult {
    met: boolean;
}

export interface ConditionalNoteNetworkErrorOptions {
    terminal?: boolean;
}

export class ConditionalNoteNetworkError extends Error {
    readonly isConditionalNoteNetworkError = true;
    readonly terminal: boolean;

    constructor(message: string, options: ConditionalNoteNetworkErrorOptions = {}) {
        super(message);
        this.name = "ConditionalNoteNetworkError";
        this.terminal = options.terminal ?? false;
    }
}

export class ConditionalNoteSecurityError extends Error {
    readonly isConditionalNoteSecurityError = true;

    constructor(message: string) {
        super(message);
        this.name = "ConditionalNoteSecurityError";
    }
}

export function isConditionalNoteNetworkError(error: unknown): boolean {
    return (
        error instanceof ConditionalNoteNetworkError ||
        (error instanceof Error &&
            (error.name === "ConditionalNoteNetworkError" ||
                error.message.includes("ConditionalNoteNetworkError") ||
                error.message.includes("CONDITIONAL_NOTE_NETWORK")))
    );
}

export function conditionalNoteAbortError(signal: AbortSignal): ConditionalNoteNetworkError {
    return signal.reason instanceof ConditionalNoteNetworkError
        ? signal.reason
        : new ConditionalNoteNetworkError("CONDITIONAL_NOTE_NETWORK: aborted");
}

export function isTerminalConditionalNoteNetworkError(
    error: unknown,
): error is ConditionalNoteNetworkError {
    return error instanceof ConditionalNoteNetworkError && error.terminal;
}

export function parseConditionalNoteManifest(json: string | null): ConditionalNoteCheckManifest {
    if (!json) return { capabilities: [] };
    try {
        const parsed = JSON.parse(json) as Partial<ConditionalNoteCheckManifest>;
        const capabilities = Array.isArray(parsed.capabilities)
            ? parsed.capabilities.filter(
                  (c): c is ConditionalNoteCapabilityName =>
                      typeof c === "string" &&
                      ["readFile", "gitHeadSha", "gitTag", "gitLog", "httpGet"].includes(c),
              )
            : [];
        return {
            capabilities,
            readFiles: stringArray(parsed.readFiles),
            hosts: stringArray(parsed.hosts),
            urls: stringArray(parsed.urls),
            signals: stringArray(parsed.signals),
            summary: typeof parsed.summary === "string" ? parsed.summary : undefined,
        };
    } catch {
        return { capabilities: [] };
    }
}

function stringArray(value: unknown): string[] | undefined {
    if (!Array.isArray(value)) return undefined;
    const arr = value.filter((item): item is string => typeof item === "string");
    return arr.length > 0 ? arr : undefined;
}
