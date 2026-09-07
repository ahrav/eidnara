import { describe, expect, it } from "bun:test";

import {
    extractTexts,
    extractToolCallSummaries,
    mergeCommitHashes,
} from "./read-session-formatting";

describe("extractTexts", () => {
    it("skips ignored text parts and keeps the user's own text", () => {
        const parts = [
            { type: "text", text: "## Claude Routing Status", ignored: true },
            { type: "text", text: "fix the bug" },
        ];

        expect(extractTexts(parts)).toEqual(["fix the bug"]);
    });

    it("returns nothing for an ignored-only message", () => {
        expect(extractTexts([{ type: "text", text: "## Claude Quotas", ignored: true }])).toEqual(
            [],
        );
    });

    it("trims and drops blank text parts", () => {
        expect(
            extractTexts([
                { type: "text", text: "  hello  " },
                { type: "text", text: "   " },
            ]),
        ).toEqual(["hello"]);
    });
});

describe("extractToolCallSummaries", () => {
    it("truncates a key argument on a code point boundary", () => {
        const filePath = `${"a".repeat(59)}😀/rest`;
        const [summary] = extractToolCallSummaries([
            { type: "tool", tool: "read", state: { input: { filePath } } },
        ]);

        expect(summary).toBe(`TC: read(${"a".repeat(59)}😀…)`);
        expect((summary as string).isWellFormed()).toBe(true);
    });

    it("leaves a key argument at the limit untouched", () => {
        const filePath = "a".repeat(60);
        expect(
            extractToolCallSummaries([
                { type: "tool", tool: "read", state: { input: { filePath } } },
            ]),
        ).toEqual([`TC: read(${filePath})`]);
    });
});

describe("mergeCommitHashes", () => {
    const five = ["a1b2c3d", "b2c3d4e", "c3d4e5f", "d4e5f6a", "e5f6a7b"];

    it("does not exceed the per-block cap when existing is already full", () => {
        expect(mergeCommitHashes(five, ["f6a7b8c"])).toEqual(five);
    });

    it("fills up to the cap and stops", () => {
        expect(mergeCommitHashes(five.slice(0, 4), ["f6a7b8c", "0000000"])).toEqual([
            ...five.slice(0, 4),
            "f6a7b8c",
        ]);
    });

    it("deduplicates without consuming cap slots", () => {
        expect(mergeCommitHashes(five.slice(0, 3), ["a1b2c3d", "f6a7b8c"])).toEqual([
            ...five.slice(0, 3),
            "f6a7b8c",
        ]);
    });

    it("returns existing unchanged when next is empty", () => {
        expect(mergeCommitHashes(five, [])).toBe(five);
    });
});
