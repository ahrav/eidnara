import { afterEach, describe, expect, test } from "bun:test";
import { mkdtempSync, rmSync } from "node:fs";
import * as os from "node:os";
import * as path from "node:path";

type LoggerScenarioResult = {
    exists: boolean;
    content: string;
    healthyDiagnostics: {
        swallowedWriteCount: number;
        lastErrorMessage: string | null;
        lastErrorTime: string | null;
    };
    failedDiagnostics: {
        swallowedWriteCount: number;
        lastErrorMessage: string | null;
        lastErrorTime: string | null;
    };
};

const loggerScenario = `
import { existsSync, readFileSync } from "node:fs";
import * as path from "node:path";

const root = process.env.LOGGER_SCENARIO_ROOT;
const loggerModuleUrl = process.env.LOGGER_MODULE_URL;
if (!root || !loggerModuleUrl) throw new Error("logger scenario environment is incomplete");

const logger = await import(loggerModuleUrl);
const logPath = path.join(root, "nested", "eidnara.log");
process.env.EIDNARA_LOG_PATH = logPath;
logger.log("first");
logger.flushLogger();
const healthyDiagnostics = logger.getLoggerDiagnostics();

const logDirectory = path.dirname(logPath);
if (process.env.LOGGER_SCENARIO === "recovery") {
    const { rmSync } = await import("node:fs");
    rmSync(logDirectory, { recursive: true, force: true });
    logger.log("second");
    logger.flushLogger();
}

// A directory used as the file target makes append fail deterministically on every platform.
const failedPath = path.join(root, "unwritable-log-target");
const { mkdirSync } = await import("node:fs");
mkdirSync(failedPath, { recursive: true });
process.env.EIDNARA_LOG_PATH = failedPath;
logger.log("failed write");
logger.flushLogger();
const failedDiagnostics = logger.getLoggerDiagnostics();

const content = existsSync(logPath) ? readFileSync(logPath, "utf8") : "";
console.log(JSON.stringify({
    exists: existsSync(logPath),
    content,
    healthyDiagnostics,
    failedDiagnostics,
}));
`;

const scenarioRoots: string[] = [];

afterEach(() => {
    for (const root of scenarioRoots.splice(0)) {
        rmSync(root, { recursive: true, force: true });
    }
});

async function runLoggerScenario(
    scenario: "recovery" | "diagnostics",
): Promise<LoggerScenarioResult> {
    const root = mkdtempSync(path.join(os.tmpdir(), "eidnara-logger-test-"));
    scenarioRoots.push(root);
    const stdout = await spawnScenario(loggerScenario, scenario, root);
    return JSON.parse(stdout) as LoggerScenarioResult;
}

async function spawnScenario(script: string, scenario: string, root: string): Promise<string> {
    const child = Bun.spawn({
        cmd: ["bun", "--eval", script],
        cwd: import.meta.dir,
        env: {
            ...process.env,
            NODE_ENV: "production",
            // The default log root derives from os.tmpdir(), so pinning TMPDIR keeps
            // the scenario's default-path writes inside the disposable root.
            TMPDIR: root,
            LOGGER_MODULE_URL: new URL("./logger.ts", import.meta.url).href,
            LOGGER_SCENARIO: scenario,
            LOGGER_SCENARIO_ROOT: root,
        },
        stdout: "pipe",
        stderr: "pipe",
    });
    const [exitCode, stdout, stderr] = await Promise.all([
        child.exited,
        new Response(child.stdout).text(),
        new Response(child.stderr).text(),
    ]);
    expect(exitCode, stderr).toBe(0);
    return stdout.trim();
}

type HardeningScenarioResult = {
    defaultPath: string;
    defaultExists: boolean;
    userRootMode: number;
    harnessDirMode: number;
    fileMode: number;
    victimContent: string;
    swallowedWriteCount: number;
};

const hardeningScenario = `
import { chmodSync, existsSync, mkdirSync, readFileSync, statSync, symlinkSync, writeFileSync } from "node:fs";
import * as os from "node:os";
import * as path from "node:path";

const root = process.env.LOGGER_SCENARIO_ROOT;
const loggerModuleUrl = process.env.LOGGER_MODULE_URL;
if (!root || !loggerModuleUrl) throw new Error("logger scenario environment is incomplete");
delete process.env.EIDNARA_LOG_PATH;

const logger = await import(loggerModuleUrl);
const dataPath = await import(new URL("./data-path.ts", loggerModuleUrl).href);
const defaultPath = dataPath.getEidnaraLogPath("opencode");
const harnessDir = path.dirname(defaultPath);
const userRoot = path.dirname(harnessDir);

// A pre-existing group/world-writable per-user root must be tightened, not trusted.
mkdirSync(userRoot, { recursive: true });
chmodSync(userRoot, 0o777);

logger.log("default");
logger.flushLogger();

// A symlink planted at the log path must not redirect the append into another file.
const victim = path.join(root, "victim.txt");
writeFileSync(victim, "untouched");
const linkDir = path.join(root, "link-dir");
mkdirSync(linkDir, { recursive: true });
const planted = path.join(linkDir, "eidnara.log");
symlinkSync(victim, planted);
process.env.EIDNARA_LOG_PATH = planted;
logger.log("hijack");
logger.flushLogger();

const mode = (p) => statSync(p).mode & 0o777;
console.log(JSON.stringify({
    defaultPath,
    defaultExists: existsSync(defaultPath),
    userRootMode: mode(userRoot),
    harnessDirMode: mode(harnessDir),
    fileMode: existsSync(defaultPath) ? mode(defaultPath) : -1,
    victimContent: readFileSync(victim, "utf8"),
    swallowedWriteCount: logger.getLoggerDiagnostics().swallowedWriteCount,
}));
`;

async function runHardeningScenario(): Promise<HardeningScenarioResult> {
    const root = mkdtempSync(path.join(os.tmpdir(), "eidnara-logger-test-"));
    scenarioRoots.push(root);
    const stdout = await spawnScenario(hardeningScenario, "hardening", root);
    return JSON.parse(stdout) as HardeningScenarioResult;
}

type ExitScenarioResult = {
    idleMsBeforeExit: number;
    content: string;
};

// The scenario never calls flushLogger: the `exit` handler must write the
// buffer, and the unref'd flush timer must not keep the process alive.
const exitScenario = `
import * as path from "node:path";

const root = process.env.LOGGER_SCENARIO_ROOT;
const loggerModuleUrl = process.env.LOGGER_MODULE_URL;
if (!root || !loggerModuleUrl) throw new Error("logger scenario environment is incomplete");

const logPath = path.join(root, "eidnara.log");
process.env.EIDNARA_LOG_PATH = logPath;
const logger = await import(loggerModuleUrl);

const cyclic = {};
cyclic.self = cyclic;
logger.log("with bigint", { count: 1n });
logger.log("with cycle", cyclic);
logger.log("with throwing toJSON", { toJSON() { throw new Error("nope"); } });
logger.log("plain", { ok: true });
const idleSince = Date.now();

// Registered after the logger's own exit handler, so it observes the flushed file.
process.on("exit", () => {
    const { readFileSync } = require("node:fs");
    const idleMsBeforeExit = Date.now() - idleSince;
    const content = readFileSync(logPath, "utf8");
    process.stdout.write(JSON.stringify({ idleMsBeforeExit, content }));
});
`;

async function runExitScenario(): Promise<ExitScenarioResult> {
    const root = mkdtempSync(path.join(os.tmpdir(), "eidnara-logger-test-"));
    scenarioRoots.push(root);
    const stdout = await spawnScenario(exitScenario, "exit", root);
    return JSON.parse(stdout) as ExitScenarioResult;
}

describe("logger", () => {
    test("recreates a log directory removed while the process is running", async () => {
        const result = await runLoggerScenario("recovery");

        expect(result.exists).toBe(true);
        expect(result.content).toContain("second");
    });

    test("reports swallowed writes while healthy writes leave the counter at zero", async () => {
        const result = await runLoggerScenario("diagnostics");

        expect(result.healthyDiagnostics).toEqual({
            swallowedWriteCount: 0,
            lastErrorMessage: null,
            lastErrorTime: null,
        });
        expect(result.failedDiagnostics.swallowedWriteCount).toBe(1);
        expect(result.failedDiagnostics.lastErrorMessage).toBeTruthy();
        expect(result.failedDiagnostics.lastErrorTime).toMatch(
            /^\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}\.\d{3}Z$/,
        );
    });

    test.skipIf(process.platform === "win32")(
        "keeps the default log private to the current user and never follows a planted symlink",
        async () => {
            const result = await runHardeningScenario();

            expect(result.defaultPath).toContain(`eidnara-${process.getuid?.()}`);
            expect(result.defaultExists).toBe(true);
            expect(result.userRootMode).toBe(0o700);
            expect(result.harnessDirMode).toBe(0o700);
            expect(result.fileMode).toBe(0o600);
            expect(result.victimContent).toBe("untouched");
            expect(result.swallowedWriteCount).toBe(1);
        },
    );

    test("keeps the message when its data cannot be serialized", async () => {
        const result = await runExitScenario();

        expect(result.content).toContain("with bigint [unserializable data: ");
        expect(result.content).toContain("with cycle [unserializable data: ");
        expect(result.content).toContain("with throwing toJSON [unserializable data: nope]");
        expect(result.content).toContain('plain {"ok":true}');
    });

    test("flushes on exit without holding the process open for the flush interval", async () => {
        const result = await runExitScenario();

        expect(result.content.split("\n").filter(Boolean)).toHaveLength(4);
        // A referenced 500ms timer would keep the process alive for at least 500ms.
        expect(result.idleMsBeforeExit).toBeLessThan(250);
    });
});
