import { describe, expect, it } from "bun:test";
import { fakeContext } from "./__tests__/test-utils";
import { renderStatusText, setEidnaraRecompActive } from "./status-line";

function reservedWindowContext(sessionId: string, tokens: number) {
    return {
        ...fakeContext(sessionId),
        model: { provider: "anthropic", id: "claude", contextWindow: 100_000, maxTokens: 20_000 },
        getContextUsage: () => ({ tokens, percent: tokens / 1_000, contextWindow: 100_000 }),
    };
}

describe("Pi footer status", () => {
    it("reports live usage against the output-reserved usable window", () => {
        const sessionId = "ses-footer-usable";
        const text = renderStatusText(reservedWindowContext(sessionId, 50_000) as never, sessionId);
        expect(text).toContain("eidnara: 50K (63%)");
        expect(text).not.toContain("(50%)");
        expect(text).toEndWith("· idle");
    });

    it("preserves valid zero-valued usage fields", () => {
        const sessionId = "ses-footer-zero";
        const text = renderStatusText(reservedWindowContext(sessionId, 0) as never, sessionId);
        expect(text).toContain("eidnara: 0 (0%)");
        expect(text).not.toContain("--");
    });

    it("renders placeholders when Pi exposes no usage", () => {
        const sessionId = "ses-footer-no-usage";
        const ctx = { ...fakeContext(sessionId), getContextUsage: undefined };
        expect(renderStatusText(ctx as never, sessionId)).toBe("eidnara: -- (--) · idle");
    });

    it("shows the recomp state while a recomp is active for the session", () => {
        const sessionId = "ses-footer-recomp";
        setEidnaraRecompActive(sessionId, true);
        try {
            const text = renderStatusText(
                reservedWindowContext(sessionId, 50_000) as never,
                sessionId,
            );
            expect(text).toEndWith("· recomp");
        } finally {
            setEidnaraRecompActive(sessionId, false);
        }
    });
});
