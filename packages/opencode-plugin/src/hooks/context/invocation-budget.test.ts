import { describe, expect, it } from "bun:test";
import {
    BYTE_BUDGET_REVISION,
    chargeInvocation,
    harnessProfile,
    validateInvocation,
} from "./invocation-budget";

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
            revision: BYTE_BUDGET_REVISION,
            authority: "heuristic",
        });
        expect(harnessProfile("pi-heuristic").revision).toBe(BYTE_BUDGET_REVISION);
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
});
