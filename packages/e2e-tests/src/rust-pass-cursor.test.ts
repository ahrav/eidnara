import { describe, expect, it } from "bun:test";
import { parseRustPassLine, type RustPassLine } from "./rust-harness";
import { RustPassCursor, type RustPassLog } from "./rust-pass-cursor";

function bufferedPassLog(): RustPassLog & {
    write(decision: string, inputCount?: number): void;
    flushNow(): void;
    flushAfter(ms: number): void;
} {
    const visible: RustPassLine[] = [];
    let buffered: RustPassLine[] = [];
    const flush = () => {
        visible.push(...buffered);
        buffered = [];
    };
    return {
        readRustPasses: () => [...visible],
        async waitFor(predicate, opts) {
            const deadline = Date.now() + (opts.timeoutMs ?? 1_000);
            while (Date.now() < deadline) {
                const value = predicate();
                if (value) return value;
                await Bun.sleep(opts.intervalMs ?? 1);
            }
            throw new Error(`waitFor timed out (${opts.label})`);
        },
        write(decision, inputCount = 1) {
            const pass = parseRustPassLine(
                `[eidnara] rust pass: decision=${decision} in=${inputCount}`,
            );
            if (!pass) throw new Error("pass line no longer parses");
            buffered.push(pass);
        },
        flushNow: flush,
        flushAfter(ms) {
            setTimeout(flush, ms);
        },
    };
}

const TIMING = { flushIntervalMs: 40, settleMarginMs: 10, pollMs: 2 };

describe("RustPassCursor", () => {
    it("waits for a step's own pass when the passes before its baseline were still buffered", async () => {
        const log = bufferedPassLog();
        log.write("DEFER");
        log.write("DEFER");
        log.flushAfter(20);
        const cursor = await RustPassCursor.settle(log, TIMING);

        await Bun.sleep(25);
        log.write("HARD");
        log.flushAfter(20);
        const passes = await cursor.next();
        expect(passes.slice(cursor.start).map((pass) => pass.decision)).toEqual(["HARD"]);
        expect(cursor.start).toBe(2);
    });

    it("absorbs an extra pass already in the file, so the next step still waits for its own", async () => {
        const log = bufferedPassLog();
        const cursor = await RustPassCursor.settle(log, TIMING);
        log.write("DEFER");
        log.write("DEFER");
        log.flushNow();
        expect((await cursor.next()).length).toBe(2);

        log.write("HARD");
        log.flushAfter(20);
        const passes = await cursor.next();
        expect(passes.at(-1)?.decision).toBe("HARD");
    });

    it("fails loudly when a step logs no pass", async () => {
        const log = bufferedPassLog();
        const cursor = await RustPassCursor.settle(log, TIMING);
        await expect(cursor.next(1, { timeoutMs: 50 })).rejects.toThrow(/>= 1 rust passes/);
    });

    it("a step's read keeps its retry's buffered repeat out of the next step's wait", async () => {
        const log = bufferedPassLog();
        const cursor = await RustPassCursor.settle(log, TIMING);
        log.write("HARD", 1);
        log.flushNow();
        // The reconnect retry repeats prompt 1's pass; its line is still buffered when the prompt returns.
        log.write("HARD", 1);
        log.flushAfter(20);
        const first = await cursor.settledStep();
        expect(first.map((pass) => pass.inputCount)).toEqual([1, 1]);

        log.write("HARD", 2);
        log.flushAfter(20);
        const second = await cursor.settledStep();
        expect(second.map((pass) => pass.inputCount)).toEqual([2]);
    });
});
