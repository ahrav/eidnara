import { describe, expect, test } from "bun:test";
import { ANTI_MEMORY_CATEGORY } from "../../shared/kernel-client/anti-memory";
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
});
