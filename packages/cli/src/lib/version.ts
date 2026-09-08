/**
 * Returns 0 when either input has no `X.Y.Z` triple, so an unrecognized version banner never blocks setup.
 * A prerelease (`1.15.0-beta.1`) orders below its release, as in SemVer, so it does not satisfy a `>=1.15.0` floor.
 */
export function compareVersionStrings(a: string, b: string): number {
    const parse = (
        value: string,
    ): { triple: [number, number, number]; pre: string | null } | null => {
        const match = value.match(/(\d+)\.(\d+)\.(\d+)(?:-([0-9A-Za-z.-]+))?/);
        if (!match) return null;
        return {
            triple: [Number(match[1]), Number(match[2]), Number(match[3])],
            pre: match[4] ?? null,
        };
    };
    const left = parse(a);
    const right = parse(b);
    if (!left || !right) return 0;
    for (let i = 0; i < 3; i += 1) {
        if (left.triple[i] < right.triple[i]) return -1;
        if (left.triple[i] > right.triple[i]) return 1;
    }
    if (left.pre === right.pre) return 0;
    if (left.pre === null) return 1;
    if (right.pre === null) return -1;
    return left.pre < right.pre ? -1 : 1;
}
