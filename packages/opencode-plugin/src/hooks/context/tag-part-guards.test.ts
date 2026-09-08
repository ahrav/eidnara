import { describe, expect, it } from "bun:test";

import {
    buildFileSourceContent,
    isFilePart,
    isTextPart,
    isToolPartWithOutput,
} from "./tag-part-guards";

describe("isToolPartWithOutput", () => {
    const base = { type: "tool", callID: "call-1" };

    it("accepts a tool part with a string output and no input", () => {
        expect(isToolPartWithOutput({ ...base, state: { output: "ok" } })).toBe(true);
    });

    it("accepts a tool part whose input is a record", () => {
        expect(isToolPartWithOutput({ ...base, state: { output: "ok", input: { a: 1 } } })).toBe(
            true,
        );
    });

    it.each([
        ["null", null],
        ["an array", []],
        ["a string", "str"],
        ["a number", 7],
    ])("rejects a tool part whose present input is %s", (_label, input) => {
        expect(isToolPartWithOutput({ ...base, state: { output: "ok", input } })).toBe(false);
    });

    it.each([
        ["no output", { ...base, state: {} }],
        ["a non-string output", { ...base, state: { output: 1 } }],
        ["a null state", { ...base, state: null }],
        ["no callID", { type: "tool", state: { output: "ok" } }],
        ["another type", { type: "text", callID: "c", state: { output: "ok" } }],
        ["a primitive", "tool"],
        ["null", null],
    ])("rejects %s", (_label, part) => {
        expect(isToolPartWithOutput(part)).toBe(false);
    });
});

describe("isTextPart and isFilePart", () => {
    it("narrow on type and the required string field", () => {
        expect(isTextPart({ type: "text", text: "hi" })).toBe(true);
        expect(isTextPart({ type: "text", text: 1 })).toBe(false);
        expect(isFilePart({ type: "file", url: "file:///a" })).toBe(true);
        expect(isFilePart({ type: "file" })).toBe(false);
    });
});

describe("buildFileSourceContent", () => {
    it("joins text parts with tag prefixes removed and returns null when empty", () => {
        expect(
            buildFileSourceContent([
                { type: "text", text: "§1§ first" },
                { type: "tool", callID: "c", state: { output: "skip" } },
                { type: "text", text: "second" },
            ]),
        ).toBe("first\nsecond");
        expect(buildFileSourceContent([{ type: "text", text: "§1§ " }])).toBeNull();
    });
});
