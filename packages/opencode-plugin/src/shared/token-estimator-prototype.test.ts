import { describe, expect, it } from "bun:test";
import { createRequire } from "node:module";
import { estimateTokens, preloadTokenizer } from "./token-estimator";

type ReferenceTokenizer = { encode: (text: string, allowedSpecial: string) => number[] };
type ReferenceModule = { default: new (encoding: unknown) => ReferenceTokenizer };

const requireFromTest = createRequire(import.meta.url);

/** Reference counts come from `ai-tokenizer` driven with a null-prototype `stringEncoder`. */
function referenceTokenizer(): ReferenceTokenizer {
    const { default: Tokenizer } = requireFromTest("ai-tokenizer") as ReferenceModule;
    const claude = requireFromTest("ai-tokenizer/encoding/claude") as {
        stringEncoder: Record<string, number>;
    };
    return new Tokenizer({
        ...claude,
        stringEncoder: Object.assign(Object.create(null), claude.stringEncoder),
    });
}

describe("estimateTokens with Object.prototype member names", () => {
    it("counts inherited-member names as ordinary text", async () => {
        expect(await preloadTokenizer()).toBe(true);
        const reference = referenceTokenizer();
        for (const text of [
            "valueOf",
            "hasOwnProperty",
            "__proto__",
            "isPrototypeOf",
            "obj.hasOwnProperty(k)",
            "if (a.hasOwnProperty(b)) return a.valueOf();",
        ]) {
            const expected = reference.encode(text, "all");
            expect(expected.every(Number.isInteger)).toBe(true);
            expect(estimateTokens(text)).toBe(expected.length);
        }
    });
});
