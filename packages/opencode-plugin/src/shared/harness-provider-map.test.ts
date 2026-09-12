import { describe, expect, it } from "bun:test";
import {
    modelRefLookupOrder,
    ompModelRefToCanonical,
    piModelRefToCanonical,
    resolveModelRefForOmp,
    resolveModelRefForPi,
} from "./harness-provider-map";

describe("harness-provider-map", () => {
    describe("resolveModelRefForPi (canonical -> Pi, used when spawning)", () => {
        it("maps the diverging auth-plugin providers and passes every other ref through unchanged", () => {
            const cases: Array<[string, string]> = [
                ["openai/gpt-5.5", "openai-codex/gpt-5.5"],
                [
                    "google/antigravity-gemini-3.5-flash",
                    "google-antigravity/antigravity-gemini-3.5-flash",
                ],
                ["openai/some/nested/id", "openai-codex/some/nested/id"],
                ["anthropic/claude-opus-4-8", "anthropic/claude-opus-4-8"],
                ["cerebras/gpt-oss-120b", "cerebras/gpt-oss-120b"],
                ["openrouter/openai/gpt-5.5", "openrouter/openai/gpt-5.5"],
                ["openai-codex/gpt-5.5", "openai-codex/gpt-5.5"],
                [
                    "google-antigravity/antigravity-gemini-3.1-pro",
                    "google-antigravity/antigravity-gemini-3.1-pro",
                ],
                ["gpt-5.5", "gpt-5.5"],
                ["/gpt-5.5", "/gpt-5.5"],
                ["", ""],
            ];
            for (const [canonical, pi] of cases) {
                expect(resolveModelRefForPi(canonical), canonical).toBe(pi);
            }
        });
    });

    describe("piModelRefToCanonical (Pi -> canonical, used by Pi setup write)", () => {
        it("normalizes Pi-native provider ids to the OpenCode form and leaves other refs unchanged", () => {
            const cases: Array<[string, string]> = [
                ["openai-codex/gpt-5.5", "openai/gpt-5.5"],
                [
                    "google-antigravity/antigravity-gemini-3.5-flash",
                    "google/antigravity-gemini-3.5-flash",
                ],
                ["anthropic/claude-opus-4-8", "anthropic/claude-opus-4-8"],
                ["openai/gpt-5.5", "openai/gpt-5.5"],
            ];
            for (const [pi, canonical] of cases) {
                expect(piModelRefToCanonical(pi), pi).toBe(canonical);
            }
        });

        describe("modelRefLookupOrder (config read edge)", () => {
            it("tries canonical before the Pi-native spelling and keeps unknown prefixes as one key", () => {
                expect(modelRefLookupOrder("openai-codex/gpt-5.6-sol")).toEqual([
                    "openai/gpt-5.6-sol",
                    "openai-codex/gpt-5.6-sol",
                ]);
                expect(modelRefLookupOrder("openai/gpt-5.6-sol")).toEqual([
                    "openai/gpt-5.6-sol",
                    "openai-codex/gpt-5.6-sol",
                ]);
                expect(modelRefLookupOrder("custom-provider/model")).toEqual([
                    "custom-provider/model",
                ]);
            });
        });
    });
});

describe("OMP provider boundary", () => {
    const representativeRefs = [
        ["openai/gpt-5.5", "openai-codex/gpt-5.5"],
        ["google/antigravity/gemini-3.5-flash", "google-antigravity/antigravity/gemini-3.5-flash"],
        ["anthropic/claude-opus-4-8", "anthropic/claude-opus-4-8"],
        ["@scope/provider/nested/model", "@scope/provider/nested/model"],
    ] as const;

    it.each(
        representativeRefs,
    )("round-trips canonical %s through OMP selector %s", (canonical, omp) => {
        expect(resolveModelRefForOmp(canonical)).toBe(omp);
        expect(ompModelRefToCanonical(omp)).toBe(canonical);
    });

    it("normalizes an already-native OMP selector idempotently", () => {
        const selector = "openai-codex/team/nested/gpt-5.5";
        expect(resolveModelRefForOmp(selector)).toBe(selector);
        expect(resolveModelRefForOmp(ompModelRefToCanonical(selector))).toBe(selector);
    });

    it("passes through provider ids that collide with Object.prototype members", () => {
        for (const ref of [
            "constructor/model",
            "toString/model",
            "__proto__/model",
            "hasOwnProperty/model",
        ]) {
            expect(resolveModelRefForOmp(ref)).toBe(ref);
            expect(ompModelRefToCanonical(ref)).toBe(ref);
            expect(resolveModelRefForPi(ref)).toBe(ref);
            expect(piModelRefToCanonical(ref)).toBe(ref);
        }
    });
});
