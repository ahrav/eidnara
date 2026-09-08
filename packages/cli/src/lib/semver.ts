/** The first `X.Y.Z` in `text`, or `null`; a probe's warnings never enter a report this way. */
export function firstSemver(text: string | null): string | null {
    return text === null ? null : (/\d+\.\d+\.\d+/.exec(text)?.[0] ?? null);
}
