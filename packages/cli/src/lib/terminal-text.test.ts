import { describe, expect, it } from "bun:test";
import { printableBlock, printableLine } from "./terminal-text";

describe("printableLine", () => {
    it("replaces terminal controls and bidi overrides with spaces and collapses whitespace", () => {
        expect(printableLine("fix\u001b[2K\rlogin\n\tbug\u202e", 50)).toBe("fix login bug");
    });

    it("cuts text past the limit with an ellipsis and returns the empty string for control-only input", () => {
        expect(printableLine("a".repeat(60), 50)).toBe(`${"a".repeat(47)}...`);
        expect(printableLine("\u0007\u200b", 50)).toBe("");
    });
});

describe("printableBlock", () => {
    it("keeps newlines and tabs but replaces escape sequences and other controls", () => {
        expect(printableBlock("## Log\n\tok\u001b[2K\rforged\u202e line\n")).toBe(
            "## Log\n\tok  forged  line\n",
        );
    });
});
