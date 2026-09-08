/** Returns 0 when either input has no `X.Y.Z` triple, so an unrecognized version banner never blocks setup. */
export function compareVersionStrings(a: string, b: string): number {
    const parse = (value: string): [number, number, number] | null => {
        const match = value.match(/(\d+)\.(\d+)\.(\d+)/);
        return match ? [Number(match[1]), Number(match[2]), Number(match[3])] : null;
    };
    const left = parse(a);
    const right = parse(b);
    if (!left || !right) return 0;
    for (let i = 0; i < 3; i += 1) {
        if (left[i] < right[i]) return -1;
        if (left[i] > right[i]) return 1;
    }
    return 0;
}
