import { describe, expect, test } from "bun:test";
import { formatThresholdClampNote, formatThresholdPercent } from "./format-threshold";

describe("formatThresholdPercent", () => {
    test("renders near-integers without decimals and others with one", () => {
        expect(formatThresholdPercent(80)).toBe("80");
        expect(formatThresholdPercent(80.04)).toBe("80");
        expect(formatThresholdPercent(80.06)).toBe("80.1");
        expect(formatThresholdPercent(79.96)).toBe("80");
        expect(formatThresholdPercent(12.5)).toBe("12.5");
    });

    test("renders a dash for missing or non-finite input", () => {
        expect(formatThresholdPercent(undefined)).toBe("—");
        expect(formatThresholdPercent(null)).toBe("—");
        expect(formatThresholdPercent(Number.NaN)).toBe("—");
        expect(formatThresholdPercent(Number.POSITIVE_INFINITY)).toBe("—");
    });
});

describe("formatThresholdClampNote", () => {
    test("is empty when not clamped or when the configured value is unknown", () => {
        expect(
            formatThresholdClampNote({
                clamped: false,
                mode: "tokens",
                configuredValue: 200_000,
                contextLimit: 128_000,
                maxPercentage: 90,
            }),
        ).toBe("");
        expect(
            formatThresholdClampNote({
                clamped: true,
                mode: "tokens",
                contextLimit: 128_000,
                maxPercentage: 90,
            }),
        ).toBe("");
    });

    test("tokens mode names the token count and the limit it was clamped against", () => {
        expect(
            formatThresholdClampNote({
                clamped: true,
                mode: "tokens",
                configuredValue: 200_000,
                contextLimit: 128_000,
                maxPercentage: 90,
            }),
        ).toBe(" [clamped: 200,000 > 90% of 128,000]");
    });

    test("percentage mode names both values as percentages", () => {
        expect(
            formatThresholdClampNote({
                clamped: true,
                mode: "percentage",
                configuredValue: 95,
                contextLimit: 128_000,
                maxPercentage: 90,
            }),
        ).toBe(" [clamped: 95% > 90%]");
    });

    test("tokens mode with a zero context limit still renders a token count, not a percentage", () => {
        expect(
            formatThresholdClampNote({
                clamped: true,
                mode: "tokens",
                configuredValue: 200_000,
                contextLimit: 0,
                maxPercentage: 90,
            }),
        ).toBe(" [clamped: 200,000 > 90%]");
    });
});
