import { describe, expect, it } from "bun:test";
import { parseRangeString } from "./range-parser";

describe("parseRangeString", () => {
    // Each row parses one accepted input shape into its expanded id list.
    it.each([
        ["parses a single number", "5", [5]],
        ["parses a range", "3-5", [3, 4, 5]],
        ["parses comma-separated numbers", "1,2,9", [1, 2, 9]],
        [
            "parses mixed ranges and individual numbers",
            "1-5,8,12-15",
            [1, 2, 3, 4, 5, 8, 12, 13, 14, 15],
        ],
        ["deduplicates repeated numbers", "1,1,2,3,3", [1, 2, 3]],
        ["handles whitespace around separators", " 3 - 5 , 8 ", [3, 4, 5, 8]],
        // The agent often pastes transcript §N§ tag markers verbatim; they
        // parse as their bare numbers instead of erroring.
        ["tolerates §N§ tag markers in a range", "§302§-§305§", [302, 303, 304, 305]],
        ["tolerates §N§ markers in a single id", "§5§", [5]],
        ["tolerates §N§ markers in a comma list", "§1§,§2§,§9§", [1, 2, 9]],
        ["tolerates mixed bare and marked ids", "1-3,§8§", [1, 2, 3, 8]],
    ] as Array<[string, string, number[]]>)("%s", (_title, input, expected) => {
        expect(parseRangeString(input)).toEqual(expected);
    });

    // Each row is one rejected input shape.
    it.each([
        ["throws on empty string", ""],
        ["throws on non-numeric input", "abc"],
        ["throws on reversed range", "5-3"],
        // 2^53 is where `i++` stops advancing, so an unrejected endpoint there would never terminate.
        ["throws on a single id above MAX_SAFE_INTEGER", "9007199254740992"],
        [
            "throws on a range whose endpoints are above MAX_SAFE_INTEGER",
            "9007199254740992-9007199254740992",
        ],
        [
            "throws on a range whose end is above MAX_SAFE_INTEGER",
            "9007199254740990-9007199254740992",
        ],
    ] as Array<[string, string]>)("%s", (_title, input) => {
        expect(() => parseRangeString(input)).toThrow();
    });

    it("accepts MAX_SAFE_INTEGER as an id", () => {
        expect(parseRangeString("9007199254740991")).toEqual([9007199254740991]);
    });

    it("throws on range of 1001 elements, naming the range and its size", () => {
        expect(() => parseRangeString("1-1001")).toThrow(
            'Range "1-1001" exceeds maximum size of 1000 elements (got 1001)',
        );
    });

    it("rejects many small ranges as soon as the total passes 1000 elements", () => {
        const input = Array.from({ length: 5 }, (_, i) => `${i * 1000}-${i * 1000 + 999}`).join(
            ",",
        );
        // A size of 1001 proves the parser stopped at the first excess id instead of expanding all 5000.
        expect(() => parseRangeString(input)).toThrow(
            "Total range size exceeds maximum of 1000 elements (got 1001)",
        );
    });

    it("dedupes overlapping ranges before applying the total cap", () => {
        expect(parseRangeString("1-1000,1-1000,500-1000")).toHaveLength(1000);
    });

    it("allows max valid range of 1000 elements", () => {
        //#given
        const input = "1-1000";
        //#when
        const result = parseRangeString(input);
        //#then
        expect(result).toHaveLength(1000);
        expect(result[0]).toBe(1);
        expect(result[999]).toBe(1000);
    });
});
