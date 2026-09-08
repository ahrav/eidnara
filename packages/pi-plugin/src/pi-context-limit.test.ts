import { afterEach, describe, expect, test } from "bun:test";
import { setOutputReserveConfig } from "@eidnara/opencode/shared/models-dev-cache";
import { resolvePiUsableContextLimit, resolvePiWindowGeometry } from "./pi-context-limit";

describe("resolvePiUsableContextLimit", () => {
    afterEach(() => setOutputReserveConfig(undefined));

    test("honors the reporter's default output_reserve instead of the 25% fallback", () => {
        setOutputReserveConfig({ default: 16_384 });
        expect(
            resolvePiUsableContextLimit({
                rawContextWindow: 272_000,
                model: {
                    provider: "openai-codex",
                    id: "gpt-5.6-sol",
                    contextWindow: 272_000,
                    maxTokens: 128_000,
                },
                persistedInputTokens: 139_400,
                persistedPercentage: (139_400 / 204_000) * 100,
            }),
        ).toBe(272_000 - 16_384);
    });

    test("reserves Pi model maxTokens on shared-window providers", () => {
        expect(
            resolvePiUsableContextLimit({
                rawContextWindow: 122_880,
                model: {
                    provider: "openai",
                    id: "reporter-model",
                    contextWindow: 122_880,
                    maxTokens: 16_384,
                },
            }),
        ).toBe(106_496);
    });

    test("keeps Google Antigravity's separate output quota unchanged", () => {
        expect(
            resolvePiUsableContextLimit({
                rawContextWindow: 1_048_576,
                model: {
                    provider: "google-antigravity",
                    id: "gemini-2.5-pro",
                    maxTokens: 65_536,
                },
            }),
        ).toBe(1_048_576);
    });

    test("applies detected wire truth before output reservation", () => {
        expect(
            resolvePiUsableContextLimit({
                rawContextWindow: 200_000,
                detectedContextLimit: 120_000,
                model: { provider: "anthropic", id: "claude", maxTokens: 20_000 },
            }),
        ).toBe(100_000);
    });

    test("a detected limit is the window when no catalog window or persisted sample exists", () => {
        const geometry = resolvePiWindowGeometry({
            detectedContextLimit: 120_000,
            model: { provider: "anthropic", id: "claude", maxTokens: 20_000 },
        });
        expect(geometry?.derivation.window).toBe(120_000);
        expect(geometry?.usableSoft).toBe(100_000);
    });

    test("a persisted estimate above the detected cap leaves the capped geometry intact", () => {
        const geometry = resolvePiWindowGeometry({
            rawContextWindow: 272_000,
            detectedContextLimit: 120_000,
            model: { provider: "anthropic", id: "claude", maxTokens: 20_000 },
            persistedInputTokens: 139_400,
            persistedPercentage: (139_400 / 204_000) * 100,
        });
        expect(geometry?.derivation.window).toBe(120_000);
        expect(geometry?.usableSoft).toBe(100_000);
        expect(geometry?.usableHard).toBe(120_000 - 4_096);
    });

    test("a persisted estimate above the runtime window leaves the derived geometry intact", () => {
        const geometry = resolvePiWindowGeometry({
            rawContextWindow: 120_000,
            model: { provider: "anthropic", id: "claude", maxTokens: 20_000 },
            persistedInputTokens: 139_400,
            persistedPercentage: (139_400 / 204_000) * 100,
        });
        expect(geometry?.derivation.window).toBe(120_000);
        expect(geometry?.usableSoft).toBe(100_000);
        expect(geometry?.usableHard).toBe(120_000 - 4_096);
    });

    test("a persisted estimate below the hard wall refines the soft threshold only", () => {
        const geometry = resolvePiWindowGeometry({
            rawContextWindow: 272_000,
            detectedContextLimit: 120_000,
            model: { provider: "anthropic", id: "claude", maxTokens: 20_000 },
            persistedInputTokens: 55_000,
            persistedPercentage: 50,
        });
        expect(geometry?.usableSoft).toBe(110_000);
        expect(geometry?.usableHard).toBe(120_000 - 4_096);
    });

    test("a low-token persisted sample still infers the usable window for an unknown model", () => {
        expect(
            resolvePiUsableContextLimit({
                persistedInputTokens: 5_000,
                persistedPercentage: 5,
            }),
        ).toBe(100_000);
    });

    test("a persisted sample whose inferred window is implausible is ignored", () => {
        expect(
            resolvePiUsableContextLimit({
                persistedInputTokens: 50,
                persistedPercentage: 50,
            }),
        ).toBeUndefined();
    });
});
