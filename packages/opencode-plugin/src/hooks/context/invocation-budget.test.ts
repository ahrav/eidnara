import { afterEach, describe, expect, it } from "bun:test";
import { installTokenizerForTest, resetTokenEstimatorForTest } from "../../shared/token-estimator";
import {
    chargeInvocation,
    HARNESS_PROFILE_IDENTITY,
    harnessProfile,
    validateInvocation,
} from "./invocation-budget";

afterEach(() => {
    resetTokenEstimatorForTest();
});

describe("invocation budget", () => {
    it("charges every entry with headroom and labels the count heuristic under the estimator generation", () => {
        const lengths = [40, 120, 7];
        const charge = chargeInvocation(lengths, 250);
        expect(charge.entries).toBe(3);
        expect(charge.bytes).toBe(167);
        expect(charge.estimatedTokens).toBe(Math.ceil(167 / 3.5));
        expect(charge.chargedTokens).toBe(Math.ceil((charge.estimatedTokens * 1250) / 1000));
        expect(charge.profile.identity).toBe(HARNESS_PROFILE_IDENTITY);
        expect(charge.profile.authority).toBe("heuristic");
        expect(charge.profile.revision).toBe(harnessProfile().revision);
        expect(chargeInvocation([lengths[2]!], 250).chargedTokens).toBeLessThan(
            charge.chargedTokens,
        );
    });

    it("admits at the limit, refuses one below it when the surface grows, and never refuses a shrinking surface", () => {
        const candidate = [40, 120, 7];
        const incoming = [40, 120];
        const charged = chargeInvocation(candidate, 250).chargedTokens;
        expect(
            validateInvocation(candidate, incoming, { maxTokens: charged, headroomPermille: 250 }),
        ).toMatchObject({ ok: true, reason: "fits" });
        const over = validateInvocation(candidate, incoming, {
            maxTokens: charged - 1,
            headroomPermille: 250,
        });
        expect(over.ok).toBe(false);
        if (!over.ok) {
            expect(over.limit).toBe(charged - 1);
            expect(over.incoming.bytes).toBe(160);
            expect(over.candidate.bytes).toBe(167);
        }
        expect(
            validateInvocation(incoming, candidate, { maxTokens: 1, headroomPermille: 250 }),
        ).toMatchObject({ ok: true, reason: "shrinks" });
        expect(
            validateInvocation(candidate, candidate, { maxTokens: 1, headroomPermille: 250 }),
        ).toMatchObject({ ok: true, reason: "shrinks" });
    });

    it("gates nothing when the host has not reported a limit", () => {
        expect(
            validateInvocation([1 << 20], [], { maxTokens: undefined, headroomPermille: 250 }),
        ).toMatchObject({ ok: true, reason: "limit_unknown" });
    });

    it("moves its revision with the estimator generation", () => {
        const before = harnessProfile().revision;
        installTokenizerForTest(null);
        expect(harnessProfile().revision).not.toBe(before);
        expect(harnessProfile().authority).toBe("heuristic");
    });
});
