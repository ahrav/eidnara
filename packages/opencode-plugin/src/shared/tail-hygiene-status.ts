import type { TailHygieneStatus } from "./rpc-types";

export interface WireTailHygieneBaseline {
    u?: number;
    t?: number;
    severity?: number;
    evaluable?: boolean;
    generation_invalidated?: boolean;
    baseline_generation?: number;
    computed_at_ms?: number;
}

function finiteNumber(value: unknown, fallback = 0): number {
    return typeof value === "number" && Number.isFinite(value) ? value : fallback;
}

/** Clamps the wire baseline into `0 <= u <= t` and `0 <= severity <= 1`; zero-valued fields survive as zeros. */
export function resolveTailHygieneStatus(
    rustBaseline?: WireTailHygieneBaseline | null,
): TailHygieneStatus | undefined {
    if (rustBaseline === undefined || rustBaseline === null) return undefined;
    const t = Math.max(0, finiteNumber(rustBaseline.t));
    const u = Math.min(t, Math.max(0, finiteNumber(rustBaseline.u)));
    return {
        u,
        t,
        severity: Math.min(1, Math.max(0, finiteNumber(rustBaseline.severity, u / Math.max(t, 1)))),
        evaluable: rustBaseline.evaluable === true,
        generationInvalidated: rustBaseline.generation_invalidated === true,
        baselineGeneration: Math.max(0, finiteNumber(rustBaseline.baseline_generation)),
        computedAt: Math.max(0, finiteNumber(rustBaseline.computed_at_ms)),
    };
}

export function formatTailHygiene(status: TailHygieneStatus): string {
    const percentage = (status.severity * 100).toFixed(1);
    const state = status.evaluable ? "" : " · held until baseline refresh";
    return `${percentage}% · ${status.u.toLocaleString()} / ${status.t.toLocaleString()} tok${state}`;
}
