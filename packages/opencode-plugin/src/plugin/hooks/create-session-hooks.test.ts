/// <reference types="bun-types" />

import { describe, expect, it } from "bun:test";
import { buildEidnaraHookConfig } from "./create-session-hooks";

describe("buildEidnaraHookConfig", () => {
    // 0 disables toasts and unset lets the consumer apply its default, so neither may be coerced.
    it("threads toast_duration_ms through unchanged, including 0 and unset", () => {
        expect(
            buildEidnaraHookConfig({
                enabled: true,
                protected_tags: 10,
                cache_ttl: "5m",
                toast_duration_ms: 30_000,
            } as never).toast_duration_ms,
        ).toBe(30_000);
        expect(
            buildEidnaraHookConfig({ enabled: true, toast_duration_ms: 0 } as never)
                .toast_duration_ms,
        ).toBe(0);
        expect(
            buildEidnaraHookConfig({ enabled: true } as never).toast_duration_ms,
        ).toBeUndefined();
    });

    // buildEidnaraHookConfig must preserve hook-consumed plugin-config fields.
    it("passes through every hook-consumed field, not a hand-maintained subset", () => {
        const config = buildEidnaraHookConfig({
            enabled: true,
            smart_drops: true,
            language: "de",
            caveman_text_compression: { enabled: true, min_chars: 800 },
            transform_mode: "rust",
            temporal_awareness: true,
        } as never) as Record<string, unknown>;

        expect(config.smart_drops).toBe(true);
        expect(config.language).toBe("de");
        expect(config.caveman_text_compression).toEqual({ enabled: true, min_chars: 800 });
        expect(config.transform_mode).toBe("rust");
        expect(config.temporal_awareness).toBe(true);
    });

    it("still applies the two defaulted fields when unset", () => {
        const config = buildEidnaraHookConfig({ enabled: true } as never);

        expect(config.protected_tags).toBeGreaterThan(0);
        expect(config.execute_threshold_percentage).toBeDefined();
    });
});
