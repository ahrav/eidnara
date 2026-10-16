import { FLUSH_INTERVAL_MS } from "@eidnara/opencode/shared/logger";
import type { RustPassLine } from "./rust-harness";

export interface RustPassLog {
    readRustPasses(): RustPassLine[];
    waitFor<T>(
        predicate: () => T | null | undefined | false,
        opts: { timeoutMs?: number; intervalMs?: number; label?: string },
    ): Promise<T>;
}

export interface RustPassCursorTiming {
    flushIntervalMs: number;
    settleMarginMs: number;
    pollMs: number;
}

export const PLUGIN_PASS_LOG_TIMING: RustPassCursorTiming = {
    flushIntervalMs: FLUSH_INTERVAL_MS,
    settleMarginMs: 100,
    pollMs: 50,
};

/**
 * The plugin logger flushes buffered pass lines on a timer, so a read just after a prompt
 * returns can omit that prompt's lines. Waiting `flushIntervalMs + settleMarginMs` before the
 * read puts every line a completed prompt produced, including a retry's repeat, in the result.
 */
async function settledRustPasses(
    log: RustPassLog,
    timing: RustPassCursorTiming = PLUGIN_PASS_LOG_TIMING,
): Promise<RustPassLine[]> {
    await Bun.sleep(timing.flushIntervalMs + timing.settleMarginMs);
    return log.readRustPasses();
}

/**
 * A count read just after a prompt returns can omit that prompt's buffered lines, and a later
 * step's wait would return on them before its own pass reaches the file. Settling reads the
 * baseline through `settledRustPasses`, so buffered lines from completed steps count toward it.
 */
export class RustPassCursor {
    private seen: number;

    private constructor(
        private readonly log: RustPassLog,
        private readonly timing: RustPassCursorTiming,
        readonly start: number,
    ) {
        this.seen = start;
    }

    /** Call between prompts so the settling delay lets completed prompts' buffered lines flush before the baseline read. */
    static async settle(
        log: RustPassLog,
        timing: RustPassCursorTiming = PLUGIN_PASS_LOG_TIMING,
    ): Promise<RustPassCursor> {
        const settled = await settledRustPasses(log, timing);
        return new RustPassCursor(log, timing, settled.length);
    }

    async next(passes = 1, opts: { timeoutMs?: number } = {}): Promise<RustPassLine[]> {
        const minCount = this.seen + passes;
        const lines = await this.log.waitFor(
            () => {
                const current = this.log.readRustPasses();
                return current.length >= minCount ? current : null;
            },
            {
                timeoutMs: opts.timeoutMs ?? 15_000,
                intervalMs: this.timing.pollMs,
                label: `>= ${minCount} rust passes`,
            },
        );
        this.seen = lines.length;
        return lines;
    }

    /**
     * Returns the lines a completed step produced, read after the flush window. A count wait
     * can return before a reconnect retry's repeat reaches the file, leaving that line to
     * satisfy the next step's wait; the settled read ties every line to the step that made it.
     */
    async settledStep(): Promise<RustPassLine[]> {
        const passes = await settledRustPasses(this.log, this.timing);
        const step = passes.slice(this.seen);
        this.seen = passes.length;
        return step;
    }
}
