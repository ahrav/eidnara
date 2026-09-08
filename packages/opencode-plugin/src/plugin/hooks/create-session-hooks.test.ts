/// <reference types="bun-types" />

import { describe, expect, it } from "bun:test";
import { buildEidnaraHookConfig } from "./create-session-hooks";

describe("buildEidnaraHookConfig", () => {
    it("threads toast_duration_ms into the per-session hook config", () => {
        const config = buildEidnaraHookConfig({
            enabled: true,
            protected_tags: 10,
            cache_ttl: "5m",
            toast_duration_ms: 30_000,
        } as never);

        expect(config.toast_duration_ms).toBe(30_000);
    });

    it("passes toast_duration_ms = 0 through unchanged (disables toasts)", () => {
        const config = buildEidnaraHookConfig({
            enabled: true,
            toast_duration_ms: 0,
        } as never);

        expect(config.toast_duration_ms).toBe(0);
    });

    it("leaves toast_duration_ms undefined when unset (consumer applies default)", () => {
        const config = buildEidnaraHookConfig({ enabled: true } as never);

        expect(config.toast_duration_ms).toBeUndefined();
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
