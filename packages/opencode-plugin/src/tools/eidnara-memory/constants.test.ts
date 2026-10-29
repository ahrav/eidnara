import { describe, expect, test } from "bun:test";
import { ANTI_MEMORY_CATEGORY } from "../../shared/kernel-client/anti-memory";
import { unwrapImitatedReducedArgs } from "../unwrap-imitated-reduced-args";
import {
    EIDNARA_MEMORY_DESCRIPTION,
    EIDNARA_MEMORY_UNWRAP_RULES,
    V2_MEMORY_CATEGORIES,
    WRITABLE_MEMORY_CATEGORIES,
} from "./constants";

describe("eidnara_memory description", () => {
    test("names every category the schema accepts and no other", () => {
        const [, positiveLine] = EIDNARA_MEMORY_DESCRIPTION.match(
            /^Positive categories \(exact names\): (.+)\.$/m,
        ) ?? [undefined, undefined];
        expect(positiveLine).toBeDefined();
        const named = [...(positiveLine ?? "").matchAll(/([A-Z_]+) \(/g)].map((match) => match[1]);
        expect(named).toEqual([...V2_MEMORY_CATEGORIES]);
        expect(EIDNARA_MEMORY_DESCRIPTION).toContain(
            `${ANTI_MEMORY_CATEGORY} requires antiMemory, not content.`,
        );
        const mentioned = new Set(EIDNARA_MEMORY_DESCRIPTION.match(/\b[A-Z][A-Z_]{3,}\b/g) ?? []);
        expect([...mentioned].sort()).toEqual([...WRITABLE_MEMORY_CATEGORIES].sort());
    });

    test("the schema enum is the same list the description is built from", () => {
        expect(EIDNARA_MEMORY_UNWRAP_RULES.category).toEqual({
            type: "enum",
            values: WRITABLE_MEMORY_CATEGORIES,
        });
    });

    test("a reduced candidates call admits the schema's null first-page cursor", () => {
        const firstPage = { action: "candidates", objectId: "mem_labeled", after: null };
        const outer = { reduced: true, summary: JSON.stringify(firstPage) };
        expect(unwrapImitatedReducedArgs(outer, ["action"], EIDNARA_MEMORY_UNWRAP_RULES)).toEqual(
            firstPage,
        );
        const notACursor = { ...firstPage, after: 7 };
        const wrong = { reduced: true, summary: JSON.stringify(notACursor) };
        expect(unwrapImitatedReducedArgs(wrong, ["action"], EIDNARA_MEMORY_UNWRAP_RULES)).toBe(
            wrong,
        );
    });
});
