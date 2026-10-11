import { describe, expect, it } from "bun:test";
import { chargeInvocation, harnessProfile, validateInvocation } from "./invocation-budget";

const BUDGET = { headroomPermille: 250, profile: "opencode-heuristic" } as const;

describe("invocation budget", () => {
    it("charges UTF-8 bytes with headroom and a fixed heuristic identity", () => {
        const lengths = [40, 120, 7];
        const charge = chargeInvocation(lengths, BUDGET);
        expect(charge.entries).toBe(3);
        expect(charge.bytes).toBe(167);
        expect(charge.estimatedTokens).toBe(Math.ceil(167 / 3.5));
        expect(charge.chargedTokens).toBe(Math.ceil((charge.estimatedTokens * 1250) / 1000));
        expect(charge.profile).toEqual({
            identity: "opencode-heuristic",
            revision: "utf8-bytes-div-3.5-v1",
            authority: "heuristic",
        });
        expect(harnessProfile("pi-heuristic").revision).toBe("utf8-bytes-div-3.5-v1");
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

    it("counts the host messages a decline serves before the window against a growing candidate", () => {
        const candidate = [40, 120, 7];
        const incoming = [40, 120];
        const budget = { ...BUDGET, maxTokens: 1 };
        expect(validateInvocation(candidate, incoming, budget, () => 7)).toMatchObject({
            ok: true,
            reason: "shrinks",
        });
        expect(validateInvocation(candidate, incoming, budget, () => 6).ok).toBe(false);
        expect(validateInvocation(candidate, incoming, budget, () => undefined).ok).toBe(false);
        let read = 0;
        validateInvocation(candidate, incoming, { ...budget, maxTokens: 1_000 }, () => ++read);
        expect(read).toBe(0);
    });

    it("asks the before-window reader only for the bytes the candidate grows by", () => {
        const needs: number[] = [];
        const result = validateInvocation(
            [40, 120, 7],
            [40, 120],
            { ...BUDGET, maxTokens: 1 },
            (needed) => {
                needs.push(needed);
                return needed;
            },
        );
        expect(result).toMatchObject({ ok: true, reason: "shrinks" });
        expect(needs).toEqual([7]);
    });

    it("gates nothing when the host has not reported a limit", () => {
        expect(
            validateInvocation([1 << 20], [], { ...BUDGET, maxTokens: undefined }),
        ).toMatchObject({ ok: true, reason: "limit_unknown" });
    });
});
