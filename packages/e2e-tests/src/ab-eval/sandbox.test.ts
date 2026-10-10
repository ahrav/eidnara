import { afterAll, beforeAll, describe, expect, it } from "bun:test";
import { type ChildProcess, spawn, spawnSync } from "node:child_process";
import { existsSync, mkdirSync, mkdtempSync, readdirSync, rmSync, writeFileSync } from "node:fs";
import { homedir } from "node:os";
import { join } from "node:path";
import { stopOwnedTree } from "./procs";
import {
    assertRunRootMaskable,
    ensureSandboxScript,
    maskedPaths,
    sandboxAvailable,
} from "./sandbox";

describe("sandbox masking plan", () => {
    it("masks a run root outside /tmp and the home directory", () => {
        expect(maskedPaths("/dev/shm/ab-run")).toContain("/dev/shm/ab-run");
    });

    it("folds a run root under /tmp into the /tmp mask", () => {
        expect(maskedPaths("/tmp/ab-eval/runs/x").filter((p) => p.startsWith("/tmp"))).toEqual([
            "/tmp",
        ]);
    });

    it("refuses a run root inside a path every arm keeps visible", () => {
        expect(() => assertRunRootMaskable("/repo/out/run", ["/repo", "/node"])).toThrow(
            /inside \/repo/,
        );
        expect(() => assertRunRootMaskable("/dev/shm/run", ["/repo", "/node"])).not.toThrow();
    });
});

describe.skipIf(!sandboxAvailable())("sandbox isolation", () => {
    const sentinel = `ab-sandbox-sentinel-${process.pid}`;
    let runRoot = "";
    let armRoot = "";
    let outsider: ChildProcess | null = null;

    beforeAll(() => {
        runRoot = mkdtempSync("/dev/shm/ab-sandbox-test-");
        armRoot = join(runRoot, "arms/a");
        mkdirSync(armRoot, { recursive: true });
        writeFileSync(join(armRoot, "arm.txt"), "own arm\n");
        writeFileSync(join(runRoot, "world.json"), '{"answer":"secret"}\n');
        outsider = spawn("/bin/sleep", ["60"], {
            env: { ...process.env, AB_SANDBOX_SENTINEL: sentinel },
            stdio: "ignore",
        });
    });

    afterAll(() => {
        outsider?.kill("SIGKILL");
        rmSync(runRoot, { recursive: true, force: true });
    });

    function inSandbox(script: string): Record<string, string> {
        const run = spawnSync(
            ensureSandboxScript(runRoot),
            [armRoot, "--", "/bin/sh", "-c", script],
            { encoding: "utf8", timeout: 30_000 },
        );
        expect(run.stderr).toBe("");
        expect(run.status).toBe(0);
        return Object.fromEntries(
            run.stdout
                .trim()
                .split("\n")
                .map((line) => line.split("=", 2) as [string, string]),
        );
    }

    it("shows the arm its own directory and nothing else of the run", () => {
        const seen = inSandbox(`
echo uid=$(id -u)
echo arm=$(cat ${armRoot}/arm.txt)
[ -e ${runRoot}/world.json ] && echo world=visible || echo world=hidden
echo tmp=$(ls -A /tmp | wc -l)
[ -e ${homedir()}/.bashrc ] && echo home=visible || echo home=hidden
`);
        expect(seen).toEqual({
            uid: String(process.getuid?.()),
            arm: "own arm",
            world: "hidden",
            tmp: "0",
            home: "hidden",
        });
    });

    it("hides processes outside the arm, so their environment and root stay unreadable", () => {
        const pid = outsider?.pid as number;
        const needle = join(armRoot, "needle.txt");
        writeFileSync(needle, `${sentinel}\n`);
        const seen = inSandbox(`
[ -e /proc/${pid} ] && echo outsider=visible || echo outsider=hidden
if grep -qsFf ${needle} /proc/[0-9]*/environ; then echo sentinel=visible; else echo sentinel=hidden; fi
`);
        expect(seen).toEqual({ outsider: "hidden", sentinel: "hidden" });
    });

    it("keeps the agent from regaining root through sudo", () => {
        const seen = inSandbox(`
if sudo -n true 2>/dev/null; then echo sudo=root; else echo sudo=denied; fi
`);
        expect(seen).toEqual({ sudo: "denied" });
    });

    it("removes its staging directory once the arm's mounts are in place", () => {
        const staging = () =>
            readdirSync("/dev/shm")
                .filter((name) => name.startsWith("abst."))
                .sort();
        const before = staging();
        inSandbox("echo ok=yes");
        expect(staging()).toEqual(before);
    });

    it("stops the sandboxed harness when asked", async () => {
        const child = spawn(ensureSandboxScript(runRoot), [armRoot, "--", "/bin/sleep", "300"], {
            stdio: "ignore",
        });
        const exited = new Promise<void>((resolve) => child.once("exit", () => resolve()));
        await Bun.sleep(500);
        const started = Date.now();
        await stopOwnedTree(child.pid as number, 5_000);
        await exited;
        expect(Date.now() - started).toBeLessThan(5_000);
        expect(existsSync(`/proc/${child.pid}`)).toBe(false);
    });
});
