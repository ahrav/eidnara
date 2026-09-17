import { afterEach, describe, expect, it } from "bun:test";
import {
    estimateTokens,
    HEURISTIC_CHARS_PER_TOKEN,
    installTokenizerForTest,
    resetTokenEstimatorForTest,
} from "../../shared/token-estimator";
import { chargeInvocation, harnessProfile, validateInvocation } from "./invocation-budget";

const BUDGET = { headroomPermille: 250, profile: "opencode-heuristic" } as const;

afterEach(() => {
    resetTokenEstimatorForTest();
});

describe("invocation budget", () => {
    it("charges every entry with headroom and labels the count heuristic under the estimator generation", () => {
        const lengths = [40, 120, 7];
        const charge = chargeInvocation(lengths, BUDGET);
        expect(charge.entries).toBe(3);
        expect(charge.bytes).toBe(167);
        expect(charge.estimatedTokens).toBe(Math.ceil(167 / HEURISTIC_CHARS_PER_TOKEN));
        expect(charge.chargedTokens).toBe(Math.ceil((charge.estimatedTokens * 1250) / 1000));
        expect(charge.profile.identity).toBe("opencode-heuristic");
        expect(charge.profile.authority).toBe("heuristic");
        expect(charge.profile.revision).toBe(harnessProfile("opencode-heuristic").revision);
        expect(chargeInvocation([lengths[2]!], BUDGET).chargedTokens).toBeLessThan(
            charge.chargedTokens,
        );
    });

    it("admits at the limit, refuses one below it when the surface grows, and never refuses a shrinking surface", () => {
        const candidate = [40, 120, 7];
        const incoming = [40, 120];
        const charged = chargeInvocation(candidate, BUDGET).chargedTokens;
        expect(
            validateInvocation(candidate, incoming, { ...BUDGET, maxTokens: charged }),
        ).toMatchObject({ ok: true, reason: "fits" });
        const over = validateInvocation(candidate, incoming, {
            ...BUDGET,
            maxTokens: charged - 1,
        });
        expect(over.ok).toBe(false);
        if (!over.ok) {
            expect(over.limit).toBe(charged - 1);
            expect(over.incoming.bytes).toBe(160);
            expect(over.candidate.bytes).toBe(167);
        }
        expect(validateInvocation(incoming, candidate, { ...BUDGET, maxTokens: 1 })).toMatchObject({
            ok: true,
            reason: "shrinks",
        });
        expect(validateInvocation(candidate, candidate, { ...BUDGET, maxTokens: 1 })).toMatchObject(
            { ok: true, reason: "shrinks" },
        );
    });

    it("gates nothing when the host has not reported a limit", () => {
        expect(
            validateInvocation([1 << 20], [], { ...BUDGET, maxTokens: undefined }),
        ).toMatchObject({ ok: true, reason: "limit_unknown" });
    });

    it("moves its revision with the estimator generation", () => {
        const before = harnessProfile("pi-heuristic").revision;
        installTokenizerForTest(null);
        expect(harnessProfile("pi-heuristic").revision).not.toBe(before);
        expect(harnessProfile("pi-heuristic")).toMatchObject({
            identity: "pi-heuristic",
            authority: "heuristic",
        });
    });

    it("charges by the same heuristic the estimator falls back to", () => {
        installTokenizerForTest(null);
        for (const length of [1, 3, 4, 167, 4_096, 358_401]) {
            expect(chargeInvocation([length], BUDGET).estimatedTokens).toBe(
                estimateTokens("x".repeat(length)),
            );
        }
    });
});
