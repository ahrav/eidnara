/** A `Denial::code` the daemon reports: lowercase words joined by underscores, at most 32 bytes. */
const SEARCH_ADMISSION_REASON = /^[a-z_]{1,32}$/;

/**
 * The `search_admission` block of the daemon's session status as one line: `admitted`, or
 * `refused (<code>)` with a reason outside the code shape shown as `unknown`. `null` when the
 * status carries no well-formed block, so a renderer shows nothing for an older daemon.
 */
export function formatSearchAdmission(value: unknown): string | null {
    if (!value || typeof value !== "object") return null;
    const { state, reason } = value as Record<string, unknown>;
    if (state === "admitted") return "admitted";
    if (state !== "refused") return null;
    const code =
        typeof reason === "string" && SEARCH_ADMISSION_REASON.test(reason) ? reason : "unknown";
    return `refused (${code})`;
}
