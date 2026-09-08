import { afterEach, describe, expect, it } from "bun:test";
import { execFileSync } from "node:child_process";
import { mkdirSync, mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { OmpAdapter, readOmpRuntimeEnabled } from "./omp";

const original = {
    HOME: process.env.HOME,
    PATH: process.env.PATH,
    PI_CODING_AGENT_DIR: process.env.PI_CODING_AGENT_DIR,
    XDG_DATA_HOME: process.env.XDG_DATA_HOME,
};
const roots: string[] = [];

afterEach(() => {
    for (const [key, value] of Object.entries(original)) {
        if (value === undefined) delete process.env[key];
        else process.env[key] = value;
    }
    for (const root of roots.splice(0)) rmSync(root, { recursive: true, force: true });
});

// The fixture is an extensionless `#!/bin/sh` script, which `findOnPath` does not accept on Windows.
describe.if(process.platform !== "win32")("OmpAdapter", () => {
    it("detects an enabled Eidnara plugin from omp plugin list", () => {
        const root = mkdtempSync(join(tmpdir(), "eidnara-omp-adapter-"));
        roots.push(root);
        const bin = join(root, "bin");
        mkdirSync(bin, { recursive: true });
        const omp = join(bin, "omp");
        writeFileSync(
            omp,
            `#!/bin/sh
if [ "$1 $2 $3" = "plugin list --json" ]; then
  printf '%s' '{"npm":[{"name":"@eidnara/pi","version":"0.33.0","enabled":true}],"marketplace":[]}'
fi
`,
            { mode: 0o755 },
        );
        process.env.PATH = bin;
        process.env.HOME = root;
        delete process.env.XDG_DATA_HOME;

        const adapter = new OmpAdapter();
        expect(adapter.isInstalled()).toBe(true);
        expect(adapter.hasPluginEntry()).toBe(true);
    }, 30_000);
});

describe.if(process.platform !== "win32")("readOmpRuntimeEnabled", () => {
    it("reads the enable flag from a regular lock file", () => {
        const root = mkdtempSync(join(tmpdir(), "eidnara-omp-lock-"));
        roots.push(root);
        const lock = join(root, "omp-plugins.lock.json");
        writeFileSync(lock, JSON.stringify({ plugins: { "@eidnara/pi": { enabled: false } } }));
        expect(readOmpRuntimeEnabled(lock)).toBe(false);
    });

    it("returns undefined without blocking when the lock path is a FIFO", () => {
        const root = mkdtempSync(join(tmpdir(), "eidnara-omp-lock-"));
        roots.push(root);
        const lock = join(root, "omp-plugins.lock.json");
        execFileSync("mkfifo", [lock]);
        const started = performance.now();
        expect(readOmpRuntimeEnabled(lock)).toBeUndefined();
        expect(performance.now() - started).toBeLessThan(5_000);
    });
});
