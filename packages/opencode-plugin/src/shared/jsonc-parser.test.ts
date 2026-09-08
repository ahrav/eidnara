import { describe, expect, it } from "bun:test";

import { parseConfigJsonc, parseJsonc } from "./jsonc-parser";

describe("parseConfigJsonc scalar top level", () => {
    it.each([
        ["42", 42],
        ['"hello"', "hello"],
        ["true", true],
        ["null", null],
    ] as Array<
        [string, unknown]
    >)("returns the primitive for %s without a spurious __proto__ rejection", (text, expected) => {
        const rejected: string[] = [];
        const parsed = parseConfigJsonc(text, {
            onRejectedKey: (path) => rejected.push(path.join(".")),
        });

        expect(parsed).toBe(expected);
        expect(rejected).toEqual([]);
    });
});

describe("parseJsonc prototype-pollution hardening", () => {
    it("rejects dangerous keys recursively, including inside arrays", () => {
        const rejected: string[] = [];
        const parsed = parseJsonc<Record<string, unknown>>(
            `{
                "safe": 1,
                "constructor": { "hidden": true },
                "nested": { "prototype": { "hidden": true }, "safe": 2 },
                "items": [
                    { "__proto__": { "polluted": true }, "safe": 3 },
                    { "safe": 4 }
                ]
            }`,
            { onRejectedKey: (path) => rejected.push(path.join(".")) },
        );

        expect(rejected).toEqual(["constructor", "nested.prototype", "items.0.__proto__"]);
        expect(Object.hasOwn(parsed, "constructor")).toBe(false);

        const nested = parsed.nested as Record<string, unknown>;
        expect(nested).toEqual({ safe: 2 });
        expect(Object.getPrototypeOf(nested)).toBe(Object.prototype);

        const items = parsed.items as Array<Record<string, unknown>>;
        expect(items).toEqual([{ safe: 3 }, { safe: 4 }]);
        expect(Object.getPrototypeOf(items[0])).toBe(Object.prototype);
        expect("polluted" in items[0]).toBe(false);
    });
});
