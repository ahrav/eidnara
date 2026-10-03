import { describe, expect, it } from "bun:test";
import {
    type AntiMemoryPayload,
    MemoryInputError,
    parseAntiMemoryContent,
    renderAntiMemoryContent,
} from "./anti-memory";

const payload: AntiMemoryPayload = {
    trigger: "session caching",
    rejectedStrategy: "Redis",
    rejectionReason: "it creates split ownership",
    saferAlternative: "the embedded store",
    expiresAt: 4_102_444_800_000,
};

function parseError(content: string): string {
    try {
        parseAntiMemoryContent(content);
    } catch (error) {
        expect(error).toBeInstanceOf(MemoryInputError);
        return (error as Error).message;
    }
    throw new Error("expected a parse error");
}

describe("parseAntiMemoryContent", () => {
    it("round-trips rendered content", () => {
        expect(parseAntiMemoryContent(renderAntiMemoryContent(payload))).toEqual({
            trigger: "session caching",
            rejectedStrategy: "Redis",
            rejectionReason: "it creates split ownership",
            saferAlternative: "the embedded store",
            preconditions: null,
            attemptedApproach: null,
            observedFailure: null,
            rootCause: null,
            recovery: null,
            nonApplicableWhen: null,
            expiresAt: 4_102_444_800_000,
        });
    });

    it("collapses every whitespace run in required and optional fields to one space", () => {
        const parsed = parseAntiMemoryContent(
            [
                "Trigger: session\t caching",
                "Rejected strategy:   Redis\u00a0\u00a0cluster  ",
                "Rejection reason: split\u3000ownership",
                "Recovery: move  the\vstate",
            ].join("\r\n"),
        );
        expect(parsed.trigger).toBe("session caching");
        expect(parsed.rejectedStrategy).toBe("Redis cluster");
        expect(parsed.rejectionReason).toBe("split ownership");
        expect(parsed.recovery).toBe("move the state");
    });

    it("keeps text that is already collapsed unchanged", () => {
        const parsed = parseAntiMemoryContent(
            "Trigger: a b c\nRejected strategy: x\nRejection reason: 漢字 🎉 text",
        );
        expect(parsed.trigger).toBe("a b c");
        expect(parsed.rejectionReason).toBe("漢字 🎉 text");
    });

    it("refuses a missing or whitespace-only required field", () => {
        expect(parseError("Trigger: t\nRejected strategy: s")).toBe(
            "anti-memory rejectionReason must be non-empty",
        );
        expect(parseError("Trigger: \u00a0\nRejected strategy: s\nRejection reason: r")).toBe(
            "anti-memory trigger must be non-empty",
        );
    });

    it("reports line errors before unknown labels and unknown labels before field errors", () => {
        expect(parseError("Bogus: x\nTrigger: t\nTrigger: u")).toBe(
            "duplicate anti-memory Trigger",
        );
        expect(parseError("Bogus: x\nno separator")).toBe("invalid anti-memory content line");
        expect(parseError("Bogus: x\nTrigger: ")).toBe("unknown anti-memory field Bogus");
    });
});
