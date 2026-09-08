/**
 * The version from the first line that is nothing but a version (an optional
 * `v` prefix, `X.Y.Z`, an optional pre-release or build suffix), or `null`.
 * A probe's warnings can carry their own version numbers, so a match anywhere
 * in the output is not enough.
 */
export function standaloneVersion(text: string | null): string | null {
    if (text === null) return null;
    for (const line of text.split(/\r?\n/)) {
        const match = /^v?(\d+\.\d+\.\d+(?:[-+][0-9A-Za-z.-]+)?)$/.exec(line.trim());
        if (match) return match[1];
    }
    return null;
}
