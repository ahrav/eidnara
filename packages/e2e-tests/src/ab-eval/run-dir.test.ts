import { afterEach, describe, expect, it } from "bun:test";
import { existsSync, mkdirSync, mkdtempSync, readlinkSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { claimRunDir, pointLatest, timestampedRunDir } from "./run-dir";

const roots: string[] = [];

afterEach(() => {
    for (const root of roots.splice(0)) rmSync(root, { recursive: true, force: true });
});

function scratch(): string {
    const root = mkdtempSync(join(tmpdir(), "ab-run-dir-"));
    roots.push(root);
    return root;
}

describe("run directory", () => {
    it("creates a missing run directory", () => {
        const out = join(scratch(), "runs/new");
        claimRunDir(out);
        expect(existsSync(out)).toBe(true);
    });

    it("accepts an existing empty directory", () => {
        const out = join(scratch(), "empty");
        mkdirSync(out);
        expect(() => claimRunDir(out)).not.toThrow();
    });

    it("refuses a directory that holds an earlier run", () => {
        const out = join(scratch(), "used");
        mkdirSync(join(out, "results/pi-on"), { recursive: true });
        writeFileSync(join(out, "results/pi-on/turns.jsonl"), "{}\n");
        expect(() => claimRunDir(out)).toThrow(/already holds/);
    });

    it("repoints latest at the newest run", () => {
        const base = scratch();
        const first = timestampedRunDir(base, new Date("2026-10-10T20:00:00.000Z"));
        const second = timestampedRunDir(base, new Date("2026-10-10T21:00:00.000Z"));
        expect(first).not.toBe(second);
        for (const dir of [first, second]) claimRunDir(dir);
        pointLatest(base, first);
        expect(readlinkSync(join(base, "latest"))).toBe(first);
        pointLatest(base, second);
        expect(readlinkSync(join(base, "latest"))).toBe(second);
    });
});
