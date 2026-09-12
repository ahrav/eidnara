import { describe, expect, test } from "bun:test";
import { stableStringify } from "./stable-json";

describe("stableStringify", () => {
    test("primitives match JSON.stringify, undefined renders literally, and empty containers stay empty", () => {
        expect(stableStringify("hello")).toBe('"hello"');
        expect(stableStringify(42)).toBe("42");
        expect(stableStringify(true)).toBe("true");
        expect(stableStringify(false)).toBe("false");
        expect(stableStringify(null)).toBe("null");
        expect(stableStringify(undefined)).toBe("undefined");
        expect(stableStringify({})).toBe("{}");
        expect(stableStringify([])).toBe("[]");
    });

    test("object keys sort by code-point order, not locale, for ASCII and non-ASCII keys", () => {
        // 'Z' (0x5a) sorts before 'a' (0x61) by code-point.
        // localeCompare can sort 'a' before 'Z'.
        expect(stableStringify({ Z: 1, a: 2 })).toBe('{"Z":1,"a":2}');
        // 'ä' (U+00E4) sorts AFTER 'z' (U+007A) by code-point.
        // localeCompare can sort 'ä' before 'z'.
        expect(stableStringify({ z: 1, ä: 2 })).toBe('{"z":1,"ä":2}');
    });

    test("nested objects sort recursively", () => {
        const input = { b: { y: 1, x: 2 }, a: { z: 3, w: 4 } };
        expect(stableStringify(input)).toBe('{"a":{"w":4,"z":3},"b":{"x":2,"y":1}}');
    });

    test("arrays preserve order", () => {
        const input = [3, 1, 2];
        expect(stableStringify(input)).toBe("[3,1,2]");
    });

    test("arrays of objects sort keys per element", () => {
        const input = [
            { b: 1, a: 2 },
            { d: 3, c: 4 },
        ];
        expect(stableStringify(input)).toBe('[{"a":2,"b":1},{"c":4,"d":3}]');
    });

    test("circular references through objects and arrays render as a marker instead of throwing", () => {
        const a: Record<string, unknown> = { x: 1 };
        a.self = a;
        expect(stableStringify(a)).toBe('{"self":"[Circular]","x":1}');
        const arr: unknown[] = [];
        arr.push(arr);
        expect(stableStringify(arr)).toBe('["[Circular]"]');
    });

    test("sparse arrays keep their length instead of collapsing to []", () => {
        expect(stableStringify(new Array(1))).toBe("[undefined]");
        expect(stableStringify(new Array(2))).toBe("[undefined,undefined]");
        // A hole between values serializes like an explicit `undefined` element.
        const holed: unknown[] = [1];
        holed[2] = 3;
        expect(stableStringify(holed)).toBe(stableStringify([1, undefined, 3]));
        expect(stableStringify(new Array(1))).not.toBe(stableStringify([]));
    });

    test("special string characters JSON-escaped in keys", () => {
        const input = { 'with "quotes"': 1 };
        expect(stableStringify(input)).toBe('{"with \\"quotes\\"":1}');
    });

    test("deterministic across multiple calls", () => {
        const input = { c: 3, a: 1, b: 2 };
        const first = stableStringify(input);
        const second = stableStringify(input);
        const third = stableStringify(input);
        expect(first).toBe(second);
        expect(second).toBe(third);
    });
});
