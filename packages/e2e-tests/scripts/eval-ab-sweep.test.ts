import { describe, expect, it } from "bun:test";
import { spawnSync } from "node:child_process";
import { mkdirSync, mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";

const SWEEP = join(import.meta.dir, "eval-ab-sweep.ts");

/** A run directory with one arm; `turns` is the arm's turns.jsonl text, `done` whether the arm finished. */
function runDir(turns: string, options: { done?: boolean; expected?: number } = {}): string {
    const root = mkdtempSync(join(tmpdir(), "ab-eval-sweep-"));
    const arm = join(root, "results/pi-off");
    mkdirSync(arm, { recursive: true });
    writeFileSync(join(root, "options.json"), JSON.stringify({ arms: ["pi-off"] }));
    writeFileSync(
        join(root, "world.json"),
        JSON.stringify({
            sessions: [{ turns: Array.from({ length: options.expected ?? 1 }, () => ({})) }],
        }),
    );
    writeFileSync(join(arm, "turns.jsonl"), turns);
    if (options.done !== false) writeFileSync(join(arm, "done.json"), "{}");
    return root;
}

const sweep = (root: string) =>
    spawnSync(process.execPath, [SWEEP, root], { encoding: "utf8", timeout: 60_000 });

const probe = JSON.stringify({
    kind: "probe",
    factKind: "repo_fact",
    scope: "cross_session",
    grade: "correct",
    ms: 10,
    error: null,
    harnessRss: 1,
    hostRss: null,
});

describe("eval-ab-sweep", () => {
    it("scores repository-fact recall in its own column", () => {
        const root = runDir(`${probe}\n`);
        try {
            const result = sweep(root);
            expect(result.status).toBe(0);
            const [header, , row] = result.stdout.trim().split("\n");
            expect(header).toContain("| repo |");
            const repo = (header as string)
                .split("|")
                .map((c) => c.trim())
                .indexOf("repo");
            expect((row as string).split("|").map((c) => c.trim())[repo]).toBe("1/1");
        } finally {
            rmSync(root, { recursive: true, force: true });
        }
    });

    it("fails on an arm that did not finish", () => {
        const root = runDir(`${probe}\n`, { done: false });
        try {
            const result = sweep(root);
            expect(result.status).not.toBe(0);
            expect(result.stderr).toContain("pi-off");
        } finally {
            rmSync(root, { recursive: true, force: true });
        }
    });

    it("fails on a malformed record", () => {
        const root = runDir(`${probe}\n{"kind":"pro\n`, { expected: 2 });
        try {
            const result = sweep(root);
            expect(result.status).not.toBe(0);
            expect(result.stderr).toContain("turns.jsonl:2");
        } finally {
            rmSync(root, { recursive: true, force: true });
        }
    });
});
