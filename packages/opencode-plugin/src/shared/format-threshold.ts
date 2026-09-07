/**
 * Behaviour:
 * Values less than 0.05 from an integer render without decimals.
 * Finite values at least 0.05 from an integer render with one decimal digit.
 *
 */
export function formatThresholdPercent(value: number | undefined | null): string {
    if (typeof value !== "number" || !Number.isFinite(value)) return "—";
    const rounded = Math.round(value);
    if (Math.abs(value - rounded) < 0.05) return String(rounded);
    return value.toFixed(1);
}

/**
 * The unit of `configuredValue` follows `mode`, so the message never labels a token count with `%`.
 * A non-positive `contextLimit` means the limit is unknown, and the tokens note omits it.
 */
export function formatThresholdClampNote(opts: {
    clamped?: boolean;
    mode: "tokens" | "percentage";
    /** configuredValue is the pre-clamp value: tokens in tokens mode and % in percentage mode. */
    configuredValue?: number;
    contextLimit: number;
    maxPercentage: number;
}): string {
    if (!opts.clamped || opts.configuredValue === undefined) return "";
    if (opts.mode === "tokens") {
        const limit = opts.contextLimit > 0 ? ` of ${opts.contextLimit.toLocaleString()}` : "";
        return ` [clamped: ${opts.configuredValue.toLocaleString()} > ${opts.maxPercentage}%${limit}]`;
    }
    return ` [clamped: ${opts.configuredValue}% > ${opts.maxPercentage}%]`;
}
