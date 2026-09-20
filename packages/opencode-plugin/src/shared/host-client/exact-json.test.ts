import { describe, expect, test } from "bun:test";

import {
    exactCount,
    exactI64,
    exactU64,
    formatExactInteger,
    parseExactJson,
    rawJsonInteger,
} from "./exact-json";

describe("parseExactJson", () => {
    test("keeps safe integers as numbers, other integer lexemes as exact bigints, and every other value as JSON.parse does", () => {
        const value = parseExactJson(
            '{"a":9007199254740992,"b":9007199254740993,"c":-9007199254740993,"d":123,"e":-0,"f":0,"g":1.0,"h":1e21,"i":"12","j":true,"k":null,"l":[1,2.5,99999999999999999999]}',
        ) as Record<string, unknown>;
        expect(value.a).toBe(9007199254740992n);
        expect(value.b).toBe(9007199254740993n);
        expect(value.c).toBe(-9007199254740993n);
        expect(value.d).toBe(123);
        expect(value.e).toBe(-0);
        expect(value.f).toBe(0);
        expect(value.g).toBe(1);
        expect(value.h).toBe(1e21);
        expect(value.i).toBe("12");
        expect(value.j).toBe(true);
        expect(value.k).toBeNull();
        expect(value.l).toEqual([1, 2.5, 99999999999999999999n]);
    });

    test("a lexeme wider than any 64-bit integer refuses the whole body", () => {
        expect(parseExactJson("99999999999999999999")).toBe(99999999999999999999n);
        expect(parseExactJson("-99999999999999999999")).toBe(-99999999999999999999n);
        expect(() => parseExactJson("100000000000000000000")).toThrow(SyntaxError);
        // An exact double beyond 2^53 is still a bigint: the split is on safe integers, not on representability.
        expect(parseExactJson("1152921504606846976")).toBe(1152921504606846976n);
        expect(() => parseExactJson(`{"a":1,"b":1${"0".repeat(400)}}`)).toThrow(SyntaxError);
    });

    test("invalid JSON throws like JSON.parse", () => {
        expect(() => parseExactJson("{")).toThrow(SyntaxError);
    });
});

describe("domains", () => {
    test("exactCount admits 0 through 2^53 inclusive and nothing else", () => {
        expect(exactCount(0)).toBe(0);
        expect(exactCount(9007199254740992)).toBe(9007199254740992);
        expect(exactCount(9007199254740992n)).toBe(9007199254740992);
        expect(exactCount(9007199254740993n)).toBeNull();
        expect(exactCount(9007199254740994)).toBeNull();
        expect(exactCount(-1)).toBeNull();
        expect(exactCount(1.5)).toBeNull();
        expect(exactCount("1")).toBeNull();
        expect(exactCount(null)).toBeNull();
    });

    test("u64 and i64 follow their bounds and return numbers when safe", () => {
        expect(exactU64(18446744073709551615n)).toBe(18446744073709551615n);
        expect(exactU64(18446744073709551616n)).toBeNull();
        expect(exactU64(-1)).toBeNull();
        expect(exactU64(5)).toBe(5);
        expect(exactI64(-9223372036854775808n)).toBe(-9223372036854775808n);
        expect(exactI64(-9223372036854775809n)).toBeNull();
        expect(exactI64(9223372036854775807n)).toBe(9223372036854775807n);
        expect(exactI64(9223372036854775808n)).toBeNull();
    });
});

describe("output", () => {
    test("human and JSON output carry the exact digits", () => {
        expect(formatExactInteger(9007199254740993n)).toBe("9007199254740993");
        expect(formatExactInteger(42)).toBe("42");
        expect(JSON.stringify({ n: rawJsonInteger(9007199254740993n), m: rawJsonInteger(3) })).toBe(
            '{"n":9007199254740993,"m":3}',
        );
    });
});
