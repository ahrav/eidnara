import { afterEach, describe, expect, it } from "bun:test";
import { type ChildProcess, spawn } from "node:child_process";
import { readFileSync } from "node:fs";
import { descendants, running, stallFor, stopOwnedTree, treeStats } from "./procs";

const spawned: ChildProcess[] = [];

afterEach(() => {
    for (const child of spawned.splice(0)) {
        if (child.pid) {
            for (const pid of descendants(child.pid)) {
                try {
                    process.kill(pid, "SIGKILL");
                } catch {}
            }
        }
        child.kill("SIGKILL");
    }
});

async function waitFor<T>(probe: () => T | undefined, timeoutMs = 5_000): Promise<T> {
    const deadline = Date.now() + timeoutMs;
    for (;;) {
        const value = probe();
        if (value !== undefined) return value;
        if (Date.now() > deadline) throw new Error("condition not reached");
        await Bun.sleep(20);
    }
}

describe("process sampling", () => {
    it("finds children and grandchildren and sums their usage", async () => {
        const child = spawn("/bin/sh", ["-c", "sh -c 'sleep 30; true' & sleep 30 & wait"], {
            stdio: "ignore",
        });
        spawned.push(child);
        const pid = child.pid as number;
        const tree = await waitFor(() => {
            const found = descendants(pid);
            return found.length >= 3 ? found : undefined;
        });
        expect(tree.length).toBe(3);
        const total = treeStats(pid);
        expect(total).not.toBeNull();
        expect(total?.rss ?? 0).toBeGreaterThan(0);
    });

    it("reports no stats for a missing process", () => {
        expect(treeStats(undefined)).toBeNull();
        expect(treeStats(2 ** 22 + 1)).toBeNull();
    });
});

describe("stopping an owned tree", () => {
    it("stops the children of an owned root, not only the root", async () => {
        const child = spawn("/bin/sh", ["-c", "sleep 30 & wait"], { stdio: "ignore" });
        spawned.push(child);
        const pid = child.pid as number;
        const [grandchild] = await waitFor(() => {
            const found = descendants(pid);
            return found.length >= 1 ? found : undefined;
        });
        await stopOwnedTree(pid, 2_000);
        // The orphaned grandchild may still be a zombie until init reaps it.
        expect(running(grandchild as number)).toBe(false);
    });
});

describe("stalling a process", () => {
    it("tolerates a stalled process that exits before the resume", async () => {
        const child = spawn("/bin/sleep", ["30"], { stdio: "ignore" });
        spawned.push(child);
        const pid = child.pid as number;
        await stallFor(pid, 100);
        child.kill("SIGKILL");
        await Bun.sleep(300);
        expect(running(pid)).toBe(false);
    });

    it("returns once the process has stopped", async () => {
        const child = spawn("/bin/sleep", ["30"], { stdio: "ignore" });
        spawned.push(child);
        const pid = child.pid as number;
        await stallFor(pid, 5_000);
        const stat = readFileSync(`/proc/${pid}/stat`, "utf8");
        expect(stat.slice(stat.lastIndexOf(")") + 2).split(" ")[0]).toBe("T");
        process.kill(pid, "SIGCONT");
    });
});
