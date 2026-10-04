import { describe, expect, it } from "bun:test";
import { createCountingPi, fakeContext } from "./__tests__/test-utils";
import { registerStatusLine, renderStatusText, setEidnaraRecompActive } from "./status-line";

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

    it("shows the recomp state while active and repaints the footer under the eidnara key at both transitions", () => {
        const sessionId = "ses-footer-recomp";
        const { ctx, statuses, keys } = statusRecordingContext(sessionId, 50_000);
        setEidnaraRecompActive(ctx as never, sessionId, true);
        try {
            expect(renderStatusText(ctx as never, sessionId)).toEndWith("· recomp");
            expect(statuses.at(-1)).toBe("eidnara: 50K (63%) · recomp");
        } finally {
            setEidnaraRecompActive(ctx as never, sessionId, false);
        }
        expect(renderStatusText(ctx as never, sessionId)).toEndWith("· idle");
        expect(statuses).toEqual(["eidnara: 50K (63%) · recomp", "eidnara: 50K (63%) · idle"]);
        expect(new Set(keys)).toEqual(new Set(["eidnara"]));
    });

    it("scopes the recomp state to the session that started it", () => {
        const active = statusRecordingContext("ses-footer-recomp-a", 50_000);
        const other = statusRecordingContext("ses-footer-recomp-b", 50_000);
        setEidnaraRecompActive(active.ctx as never, "ses-footer-recomp-a", true);
        try {
            expect(renderStatusText(other.ctx as never, "ses-footer-recomp-b")).toEndWith("· idle");
        } finally {
            setEidnaraRecompActive(active.ctx as never, "ses-footer-recomp-a", false);
        }
    });
});

describe("Pi footer per-turn work", () => {
    it("paints from the reported response usage without reading the branch", async () => {
        const sessionId = "ses-footer-reported";
        const counting = createCountingPi();
        registerStatusLine(counting.pi as never, { projectIdentity: "" });
        const { ctx, statuses } = statusRecordingContext(sessionId, 50_000);
        let usageReads = 0;
        const counted = {
            ...ctx,
            getContextUsage: () => {
                usageReads += 1;
                return ctx.getContextUsage();
            },
        };
        const messageEnd = counting.handlers.get("message_end") as (
            event: unknown,
            context: unknown,
        ) => Promise<void>;
        const agentEnd = counting.handlers.get("agent_end") as (
            event: unknown,
            context: unknown,
        ) => Promise<void>;
        await messageEnd(
            {
                message: {
                    role: "assistant",
                    stopReason: "stop",
                    usage: {
                        input: 10,
                        output: 5,
                        cacheRead: 30_000,
                        cacheWrite: 0,
                        totalTokens: 30_015,
                    },
                },
            },
            counted,
        );
        await agentEnd({}, counted);
        expect(statuses.at(-1)).toBe("eidnara: 30K (38%) · idle");
        expect(usageReads).toBe(0);
    });
});

function statusRecordingContext(sessionId: string, tokens: number) {
    const statuses: Array<string | undefined> = [];
    const keys: string[] = [];
    const base = reservedWindowContext(sessionId, tokens);
    const ctx = {
        ...base,
        ui: {
            ...base.ui,
            setStatus: (key: string, text: string | undefined) => {
                keys.push(key);
                statuses.push(text);
            },
        },
    };
    return { ctx, statuses, keys };
}
