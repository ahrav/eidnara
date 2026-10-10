import { describe, expect, it } from "bun:test";
import { spawnSync } from "node:child_process";
import { join } from "node:path";
import { settleWithin } from "./deadline";

const DEADLINE_MODULE = join(import.meta.dir, "deadline.ts");

describe("prompt deadlines", () => {
    it("lets the process exit once the work settles before its deadline", () => {
        const script = `import { settleWithin } from ${JSON.stringify(DEADLINE_MODULE)};
await settleWithin(Promise.resolve(1), 30_000, { abort: async () => undefined, drainMs: 30_000 });`;
        const started = Date.now();
        const run = spawnSync(process.execPath, ["-e", script], { timeout: 10_000 });
        expect(run.status).toBe(0);
        expect(Date.now() - started).toBeLessThan(5_000);
    });

    it("returns the work's value when it settles first", async () => {
        let aborted = 0;
        const outcome = await settleWithin(Promise.resolve("answer"), 1_000, {
            abort: async () => {
                aborted++;
            },
            drainMs: 1_000,
        });
        expect(outcome).toEqual({ timedOut: false, value: "answer" });
        expect(aborted).toBe(0);
    });

    it("aborts timed-out work and waits for it to stop before reporting the timeout", async () => {
        let finish: (value: string) => void = () => undefined;
        let settled = false;
        const work = new Promise<string>((resolve) => {
            finish = resolve;
        }).then((value) => {
            settled = true;
            return value;
        });
        let aborted = 0;
        const outcome = await settleWithin(work, 20, {
            abort: async () => {
                aborted++;
                setTimeout(() => finish("aborted"), 20);
            },
            drainMs: 1_000,
        });
        expect(outcome).toEqual({ timedOut: true });
        expect(aborted).toBe(1);
        expect(settled).toBe(true);
    });

    it("fails when timed-out work keeps running after the abort", async () => {
        const never = new Promise<string>(() => undefined);
        await expect(
            settleWithin(never, 20, { abort: async () => undefined, drainMs: 30 }),
        ).rejects.toThrow(/still running/);
    });
});
