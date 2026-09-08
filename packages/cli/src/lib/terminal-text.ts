/**
 * C0 and C1 controls can move the cursor, erase lines, or forge terminal output. The ranges
 * match zero-width and bidi marks, line and paragraph separators, bidi overrides and isolates,
 * and the BOM. commentlint: allow(JUDGE)
 */
export const TERMINAL_CONTROL_CHARS =
    // biome-ignore lint/suspicious/noControlCharactersInRegex: the security boundary intentionally matches C0/C1 ranges
    /[\u0000-\u001f\u007f-\u009f\u200b-\u200f\u2028-\u202e\u2060-\u2064\u2066-\u2069\ufeff]/g;

/**
 * CSI (`ESC [ ... final`) and OSC (`ESC ] ... BEL|ST`) sequences are removed whole; stripping only the
 * ESC byte would leave their parameter bytes (`[2K`) in the output.
 */
const ANSI_ESCAPE_SEQUENCES =
    // biome-ignore lint/suspicious/noControlCharactersInRegex: the security boundary intentionally matches ESC-led sequences
    /\u001b\[[0-?]*[ -/]*[@-~]|\u001b\][^\u0007\u001b]*(?:\u0007|\u001b\\)?/g;

export function printableLine(text: string, maxLength: number): string {
    const flat = text
        .replace(ANSI_ESCAPE_SEQUENCES, " ")
        .replace(TERMINAL_CONTROL_CHARS, " ")
        .replace(/\s+/g, " ")
        .trim();
    if (flat.length <= maxLength) return flat;
    return `${flat.slice(0, Math.max(0, maxLength - 3))}...`;
}

/** Multi-line text keeps its line and tab structure; every other control is replaced. */
export function printableBlock(text: string): string {
    return text
        .replace(ANSI_ESCAPE_SEQUENCES, " ")
        .replace(TERMINAL_CONTROL_CHARS, (char) => (char === "\n" || char === "\t" ? char : " "));
}
