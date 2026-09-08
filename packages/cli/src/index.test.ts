import { afterAll, describe, expect, test } from "bun:test";
import { spawnSync } from "node:child_process";
import { mkdtempSync, readFileSync, rmSync, symlinkSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { type CliDispatchDependencies, dispatchCli, usageText } from "./dispatch";
import { PromptCancelledError } from "./lib/prompts";

const builtCliRoot = mkdtempSync(join(tmpdir(), "eidnara-cli-built-"));
// A per-run home prevents concurrent shared-host runs from racing on a fixed path and keeps the CLI out of the real home directory.
const entrypointHome = mkdtempSync(join(tmpdir(), "eidnara-cli-entrypoint-"));
// An absolute data root stops lifecycle resolution at XDG_DATA_HOME, avoiding the NODE_ENV=test fallback's one-time stderr warning.
const entrypointDataRoot = join(entrypointHome, ".local", "share");

afterAll(() => {
    rmSync(builtCliRoot, { recursive: true, force: true });
    rmSync(entrypointHome, { recursive: true, force: true });
});

function dependencies() {
    const daemonArgs: string[][] = [];
    const stdout: string[] = [];
    const stderr: string[] = [];
    const deps: CliDispatchDependencies = {
        runDaemon: async (args) => {
            daemonArgs.push(args);
            return 0;
        },
        stdout: (line) => stdout.push(line),
        stderr: (line) => stderr.push(line),
    };
    return {
        deps,
        daemonArgs,
        stdout,
        stderr,
    };
}

describe("import-safe CLI dispatch", () => {
    test("help lists all daemon actions and --json", async () => {
        const h = dependencies();

        const exit = await dispatchCli(["--help"], h.deps);

        expect(exit).toBe(0);
        expect(h.stdout).toEqual([usageText()]);
        for (const action of ["start", "stop", "restart", "status", "doctor"]) {
            expect(h.stdout[0]).toContain(`daemon ${action}`);
        }
        expect(h.stdout[0]).toContain("--json");
    });

    test.each([
        "start",
        "stop",
        "restart",
        "status",
        "doctor",
    ])("daemon %s dispatches to the daemon command", async (action) => {
        const h = dependencies();

        const exit = await dispatchCli(["daemon", action, "--json"], h.deps);

        expect(exit).toBe(0);
        expect(h.daemonArgs).toEqual([[action, "--json"]]);
    });

    test("--version prints the package version", async () => {
        const h = dependencies();
        const pkg = JSON.parse(
            readFileSync(join(import.meta.dir, "..", "package.json"), "utf8"),
        ) as {
            version: string;
        };

        const exit = await dispatchCli(["--version"], h.deps);

        expect(exit).toBe(0);
        expect(pkg.version).toBe("0.1.0");
        expect(h.stdout).toEqual([pkg.version]);
    });

    test("help lists no storage or migration subcommands", () => {
        const text = usageText();
        for (const gone of [
            "--clear",
            "repair-db",
            "reset-db",
            "merge-identity",
            "drain-authority",
            "migrate",
            "npx",
        ]) {
            expect(text).not.toContain(gone);
        }
    });

    test("a cancelled prompt exits 0 rather than escaping as an error", async () => {
        const h = dependencies();
        // `return await` lets `dispatchCli` catch `PromptCancelledError` rejections and return 0.
        h.deps.runDaemon = async () => {
            throw new PromptCancelledError("Cancelled.");
        };

        const exit = await dispatchCli(["daemon", "status"], h.deps);

        expect(exit).toBe(0);
        expect(h.stderr).toEqual([]);
    });

    test.each([
        [["setup", "--help"]],
        [["doctor", "-h"]],
        [["doctor", "--harness", "pi", "--help"]],
    ])("%p prints the usage instead of running the command", async (argv) => {
        const h = dependencies();

        const exit = await dispatchCli(argv, h.deps);

        expect(exit).toBe(0);
        expect(h.stdout).toEqual([usageText()]);
        expect(h.stderr).toEqual([]);
    });

    test.each([
        [["doctor", "repair-db"], "Unknown doctor argument: repair-db"],
        [["doctor", "--clear"], "Unknown doctor argument: --clear"],
        [["doctor", "--harness", "pi", "migrate"], "Unknown doctor argument: migrate"],
        [
            ["doctor", "--harness", "pi", "--harness", "repair-db"],
            "Unknown doctor arguments: --harness repair-db",
        ],
        [["setup", "migrate", "--clear"], "Unknown setup arguments: migrate --clear"],
    ])("%p is rejected as an argument error", async (argv, message) => {
        const h = dependencies();

        const exit = await dispatchCli(argv, h.deps);

        expect(exit).toBe(1);
        expect(h.stderr).toEqual([message]);
        expect(h.stdout).toEqual([usageText()]);
    });

    test("the --harness value is not reported as an unknown argument", async () => {
        const h = dependencies();

        // An invalid value reaches the harness resolver, which rejects it by name.
        await expect(dispatchCli(["doctor", "--harness", "opencdoe"], h.deps)).rejects.toThrow(
            "Invalid --harness value: opencdoe",
        );
        expect(h.stderr).toEqual([]);
    });

    test("importing the executable module does not run or exit", async () => {
        const cliRoot = join(import.meta.dir, "..");
        const child = Bun.spawn({
            cmd: [
                process.execPath,
                "-e",
                'await import("./src/index.ts"); console.log("IMPORT_SAFE")',
            ],
            cwd: cliRoot,
            stdout: "pipe",
            stderr: "pipe",
        });

        const [exit, stdout, stderr] = await Promise.all([
            child.exited,
            new Response(child.stdout).text(),
            new Response(child.stderr).text(),
        ]);

        expect(exit).toBe(0);
        expect(stdout.trim()).toBe("IMPORT_SAFE");
        expect(stderr).toBe("");
    });

    test("an unresolvable entry path reports the failure instead of exiting 0 silently", async () => {
        const cliRoot = join(import.meta.dir, "..");
        const missingEntry = join(builtCliRoot, "vanished", "context");
        const child = Bun.spawn({
            cmd: [
                process.execPath,
                "-e",
                // A bin path that cannot be realpath-resolved must not be treated as a module import.
                `process.argv[1] = ${JSON.stringify(missingEntry)}; await import("./src/index.ts")`,
            ],
            cwd: cliRoot,
            stdout: "pipe",
            stderr: "pipe",
        });

        const [exit, stdout, stderr] = await Promise.all([
            child.exited,
            new Response(child.stdout).text(),
            new Response(child.stderr).text(),
        ]);

        expect(exit).toBe(1);
        expect(stdout).toBe("");
        expect(stderr).toContain("cannot resolve the invoked path");
        expect(stderr).toContain(missingEntry);
    });

    test("Node dispatches a built CLI invoked through an npm-style bin symlink", async () => {
        const cliRoot = join(import.meta.dir, "..");
        // Node resolves the bundle's `@eidnara/shm-native` import through the workspace package's `import` export, which its `build:js` produces.
        const nativeJs = spawnSync("bun", ["run", "--cwd", "../shm-native", "build:js"], {
            cwd: cliRoot,
            encoding: "utf8",
        });
        expect(nativeJs.status).toBe(0);
        const built = spawnSync("bun", ["run", "build"], {
            cwd: cliRoot,
            encoding: "utf8",
        });
        expect(built.status).toBe(0);
        const entry = join(cliRoot, "dist", "index.js");
        expect(readFileSync(entry, "utf8").startsWith("#!/usr/bin/env node")).toBe(true);
        const bin = join(builtCliRoot, "eidnara");
        symlinkSync(entry, bin);

        const version = Bun.spawnSync({
            cmd: ["node", bin, "--version"],
            stdout: "pipe",
            stderr: "pipe",
        });
        expect(version.exitCode).toBe(0);
        expect(version.stdout.toString().trim()).toBe("0.1.0");
        expect(version.stderr.toString()).toBe("");

        const child = Bun.spawn({
            cmd: ["node", bin, "--help"],
            stdout: "pipe",
            stderr: "pipe",
        });
        const [exit, stdout, stderr] = await Promise.all([
            child.exited,
            new Response(child.stdout).text(),
            new Response(child.stderr).text(),
        ]);

        expect(exit).toBe(0);
        expect(stdout).toContain("daemon start");
        expect(stderr).toBe("");
    });

    test.each([
        "start",
        "stop",
        "restart",
        "status",
        "doctor",
    ])("subprocess entrypoint dispatches daemon %s as one JSON result", async (action) => {
        const cliRoot = join(import.meta.dir, "..");
        const child = Bun.spawn({
            cmd: [process.execPath, "src/index.ts", "daemon", action, "--json"],
            cwd: cliRoot,
            env: {
                ...process.env,
                XDG_DATA_HOME: entrypointDataRoot,
                EIDNARA_TEST_DATA_DIR: entrypointDataRoot,
                HOME: entrypointHome,
            },
            stdout: "pipe",
            stderr: "pipe",
        });

        const [exit, stdout, stderr] = await Promise.all([
            child.exited,
            new Response(child.stdout).text(),
            new Response(child.stderr).text(),
        ]);

        expect(exit).toBe(1);
        expect(stderr).toBe("");
        const lines = stdout.trim().split("\n");
        expect(lines).toHaveLength(1);
        expect(JSON.parse(lines[0] ?? "")).toMatchObject({
            schema: "eidnara.daemon/v1",
            command: action,
            ok: false,
        });
    });
});
