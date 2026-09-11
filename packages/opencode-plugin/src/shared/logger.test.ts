import { afterEach, beforeEach, describe, expect, test } from "bun:test";
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
if (process.env.LOGGER_SCENARIO === "sanitize") {
    const forged = "provider said\\n[2020-01-01T00:00:00.000Z] [eidnara] AUDIT: forged entry";
    logger.log(\`upstream failed: \${forged}\\u001b[31m\\u0007\`);
    logger.log(\`oversized: \${"x".repeat(10_000)}\`, { detail: "y".repeat(10_000) });
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
const originalLogLevel = process.env.EIDNARA_LOG_LEVEL;

afterEach(() => {
    if (originalLogLevel === undefined) delete process.env.EIDNARA_LOG_LEVEL;
    else process.env.EIDNARA_LOG_LEVEL = originalLogLevel;
    for (const root of scenarioRoots.splice(0)) {
        rmSync(root, { recursive: true, force: true });
    }
});

async function runLoggerScenario(
    scenario: "recovery" | "diagnostics" | "sanitize",
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
    unsafeDirectoryWritten: boolean;
    unsafeDirectoryError: string | null;
    foreignOwnerSwallowed: number;
    foreignOwnerError: string | null;
};

const hardeningScenario = `
import { spyOn } from "bun:test";
import * as fs from "node:fs";
import { chmodSync, existsSync, mkdirSync, readFileSync, rmSync, statSync, symlinkSync, writeFileSync } from "node:fs";
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
const mode = (p) => statSync(p).mode & 0o777;
const privateModes = { userRootMode: mode(userRoot), harnessDirMode: mode(harnessDir), fileMode: mode(defaultPath) };

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

const unsafeTarget = path.join(root, "unsafe-directory");
mkdirSync(unsafeTarget);
rmSync(harnessDir, { recursive: true });
symlinkSync(unsafeTarget, harnessDir);
delete process.env.EIDNARA_LOG_PATH;
logger.log("directory hijack");
logger.flushLogger();
const unsafeDirectoryError = logger.getLoggerDiagnostics().lastErrorMessage;
const unsafeDirectoryWritten = existsSync(path.join(unsafeTarget, "eidnara.log"));
rmSync(harnessDir);
mkdirSync(harnessDir, { mode: 0o700 });
const lstat = fs.lstatSync;
const ownerSpy = spyOn(fs, "lstatSync").mockImplementation((p, options) => {
    const stat = lstat(p, options);
    if (p === userRoot) return Object.assign(Object.create(Object.getPrototypeOf(stat)), stat, { uid: process.getuid() + 1 });
    return stat;
});
const beforeForeignOwner = logger.getLoggerDiagnostics().swallowedWriteCount;
let foreignOwnerError;
try {
    logger.log("foreign owner");
    logger.flushLogger();
    foreignOwnerError = logger.getLoggerDiagnostics().lastErrorMessage;
} finally {
    ownerSpy.mockRestore();
}
const foreignOwnerSwallowed = logger.getLoggerDiagnostics().swallowedWriteCount - beforeForeignOwner;
logger.log("restored directory");
logger.flushLogger();
console.log(JSON.stringify({
    defaultPath,
    defaultExists: existsSync(defaultPath),
    ...privateModes,
    victimContent: readFileSync(victim, "utf8"),
    swallowedWriteCount: logger.getLoggerDiagnostics().swallowedWriteCount,
    unsafeDirectoryWritten,
    unsafeDirectoryError,
    foreignOwnerSwallowed,
    foreignOwnerError,
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
logger.log("with function", () => 1);
logger.log("with symbol", Symbol("s"));
logger.log("plain", { ok: true });
if (process.env.LOGGER_SCENARIO === "exit-off") process.env.EIDNARA_LOG_LEVEL = "off";
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

type OverrideDirScenarioResult = {
    sharedDirMode: number;
    cwdMode: number;
    sharedContent: string;
    cwdContent: string;
    swallowedWriteCount: number;
};

// The logger must not change parent-directory modes for absolute or cwd-relative EIDNARA_LOG_PATH values.
const overrideDirScenario = `
import { chmodSync, mkdirSync, readFileSync, statSync } from "node:fs";
import * as path from "node:path";

const root = process.env.LOGGER_SCENARIO_ROOT;
const loggerModuleUrl = process.env.LOGGER_MODULE_URL;
if (!root || !loggerModuleUrl) throw new Error("logger scenario environment is incomplete");

const logger = await import(loggerModuleUrl);

const shared = path.join(root, "shared");
mkdirSync(shared);
chmodSync(shared, 0o775);
process.env.EIDNARA_LOG_PATH = path.join(shared, "eidnara.log");
logger.log("absolute override");
logger.flushLogger();

const project = path.join(root, "project");
mkdirSync(project);
chmodSync(project, 0o755);
process.chdir(project);
process.env.EIDNARA_LOG_PATH = "./eidnara.log";
logger.log("relative override");
logger.flushLogger();

const mode = (p) => statSync(p).mode & 0o777;
console.log(JSON.stringify({
    sharedDirMode: mode(shared),
    cwdMode: mode(project),
    sharedContent: readFileSync(path.join(shared, "eidnara.log"), "utf8"),
    cwdContent: readFileSync(path.join(project, "eidnara.log"), "utf8"),
    swallowedWriteCount: logger.getLoggerDiagnostics().swallowedWriteCount,
}));
`;

async function runOverrideDirScenario(): Promise<OverrideDirScenarioResult> {
    const root = mkdtempSync(path.join(os.tmpdir(), "eidnara-logger-test-"));
    scenarioRoots.push(root);
    const stdout = await spawnScenario(overrideDirScenario, "override-dir", root);
    return JSON.parse(stdout) as OverrideDirScenarioResult;
}

type ExistingFileScenarioResult = {
    fifoReturned: boolean;
    fifoSwallowed: number;
    managedModeAfter: number;
    managedContent: string;
    overrideModeAfter: number;
    overrideContent: string;
};

// Existing files: a FIFO must fail fast instead of blocking the flush, a
// pre-existing managed log is tightened to 0600, and a caller-chosen log keeps its mode.
const existingFileScenario = `
import { chmodSync, mkdirSync, readFileSync, statSync, writeFileSync } from "node:fs";
import * as path from "node:path";

const root = process.env.LOGGER_SCENARIO_ROOT;
const loggerModuleUrl = process.env.LOGGER_MODULE_URL;
if (!root || !loggerModuleUrl) throw new Error("logger scenario environment is incomplete");
delete process.env.EIDNARA_LOG_PATH;

const logger = await import(loggerModuleUrl);
const dataPath = await import(new URL("./data-path.ts", loggerModuleUrl).href);

const fifo = path.join(root, "log.fifo");
Bun.spawnSync({ cmd: ["mkfifo", fifo] });
process.env.EIDNARA_LOG_PATH = fifo;
logger.log("into fifo");
const before = logger.getLoggerDiagnostics().swallowedWriteCount;
logger.flushLogger();
const fifoSwallowed = logger.getLoggerDiagnostics().swallowedWriteCount - before;

delete process.env.EIDNARA_LOG_PATH;
const managed = dataPath.getEidnaraLogPath("opencode");
mkdirSync(path.dirname(managed), { recursive: true, mode: 0o700 });
writeFileSync(managed, "old\\n");
chmodSync(managed, 0o644);
logger.log("managed append");
logger.flushLogger();

const override = path.join(root, "shared", "eidnara.log");
mkdirSync(path.dirname(override));
writeFileSync(override, "old\\n");
chmodSync(override, 0o644);
process.env.EIDNARA_LOG_PATH = override;
logger.log("override append");
logger.flushLogger();

const mode = (p) => statSync(p).mode & 0o777;
console.log(JSON.stringify({
    fifoReturned: true,
    fifoSwallowed,
    managedModeAfter: mode(managed),
    managedContent: readFileSync(managed, "utf8"),
    overrideModeAfter: mode(override),
    overrideContent: readFileSync(override, "utf8"),
}));
`;

async function runExistingFileScenario(): Promise<ExistingFileScenarioResult> {
    const root = mkdtempSync(path.join(os.tmpdir(), "eidnara-logger-test-"));
    scenarioRoots.push(root);
    const stdout = await spawnScenario(existingFileScenario, "existing-file", root);
    return JSON.parse(stdout) as ExistingFileScenarioResult;
}

describe("logger", () => {
    test("off skips caller inspection, sanitization, serialization, timestamps and writes", async () => {
        const root = mkdtempSync(path.join(os.tmpdir(), "eidnara-logger-test-"));
        scenarioRoots.push(root);
        const stdout = await spawnScenario(
            `
            import { spyOn } from "bun:test";
            import * as fs from "node:fs";
            const logger = await import(process.env.LOGGER_MODULE_URL);
            process.env.EIDNARA_LOG_PATH = process.env.LOGGER_SCENARIO_ROOT + "/off.log";
            process.env.EIDNARA_LOG_LEVEL = "off";
            let inspections = 0;
            let dates = 0;
            const poison = {
                get [Symbol.iterator]() { inspections++; return () => "poison"[Symbol.iterator](); },
                get [Symbol.toPrimitive]() { inspections++; return () => "poison"; },
            };
            const data = { get value() { inspections++; return "data"; } };
            const jsonData = { toJSON() { inspections++; return "json"; } };
            const OriginalDate = Date;
            globalThis.Date = new Proxy(Date, {
                construct(target, args) { dates++; return Reflect.construct(target, args); },
            });
            const spies = [
                spyOn(String.prototype, Symbol.iterator),
                spyOn(JSON, "stringify"),
                spyOn(globalThis, "setTimeout"),
                spyOn(fs, "mkdirSync"),
                spyOn(fs, "openSync"),
                spyOn(fs, "writeSync"),
            ];
            try {
                for (let index = 0; index < 50; index++) logger.log("gated", data);
                logger.log("gated json", jsonData);
                logger.log(poison);
                logger.sessionLog(poison, poison, jsonData);
                for (const level of ["debug", "info", "warn", "error"]) {
                    logger.log[level](poison, jsonData);
                    logger.sessionLog[level](poison, poison, jsonData);
                }
                process.env.EIDNARA_LOG_LEVEL = "warn";
                logger.log.debug(poison, jsonData);
                logger.sessionLog.info(poison, poison, data);
                logger.flushLogger();
                const calls = spies.map(spy => spy.mock.calls.length);
                const swallowed = logger.getLoggerDiagnostics().swallowedWriteCount;
                for (const spy of spies) spy.mockRestore();
                console.log(JSON.stringify({ inspections, dates, calls, swallowed }));
            } finally {
                for (const spy of spies) spy.mockRestore();
                globalThis.Date = OriginalDate;
            }
        `,
            "off",
            root,
        );
        expect(JSON.parse(stdout)).toEqual({
            inspections: 0,
            dates: 0,
            calls: [0, 0, 0, 0, 0, 0],
            swallowed: 0,
        });
    });

    test("orders levels, preserves plain call shape and defaults unknown config to debug", async () => {
        const root = mkdtempSync(path.join(os.tmpdir(), "eidnara-logger-test-"));
        scenarioRoots.push(root);
        const stdout = await spawnScenario(
            `
            import { existsSync, readFileSync } from "node:fs";
            const { LogLevel, log, sessionLog, flushLogger } = await import(process.env.LOGGER_MODULE_URL);
            const frozenLevels = Object.isFrozen(LogLevel) && !Reflect.set(LogLevel, "off", 0);
            const independentInfo = log.info !== log && sessionLog.info !== sessionLog;
            const results = [];
            for (const minimum of [undefined, "debug", "info", "warn", "error", "off", "", "DEBUG", "__proto__"]) {
                if (minimum === undefined) delete process.env.EIDNARA_LOG_LEVEL;
                else process.env.EIDNARA_LOG_LEVEL = minimum;
                const file = process.env.LOGGER_SCENARIO_ROOT + "/level-" + results.length;
                process.env.EIDNARA_LOG_PATH = file;
                log("plain", { ok: true });
                sessionLog("session", "plain", { ok: true });
                for (const level of ["debug", "info", "warn", "error"]) {
                    log[level](level);
                    sessionLog[level]("session", level);
                }
                flushLogger();
                results.push(existsSync(file) ? readFileSync(file, "utf8").split("\\n").filter(Boolean)
                    .map(line => line.slice(line.indexOf("] ") + 2)) : []);
            }
            console.log(JSON.stringify({ results, frozenLevels, independentInfo }));
        `,
            "levels",
            root,
        );
        const plain = ['plain {"ok":true}', '[eidnara][session] plain {"ok":true}'];
        const debug = ["debug", "[eidnara][session] debug"];
        const info = ["info", "[eidnara][session] info"];
        const warn = ["warn", "[eidnara][session] warn"];
        const error = ["error", "[eidnara][session] error"];
        const all = [...plain, ...debug, ...info, ...warn, ...error];
        const result = JSON.parse(stdout);
        expect(result.frozenLevels).toBe(true);
        expect(result.independentInfo).toBe(true);
        expect(result.results).toEqual([
            all,
            all,
            [...plain, ...info, ...warn, ...error],
            [...warn, ...error],
            error,
            [],
            all,
            all,
            all,
        ]);
    });

    test("off preserves explicit flush failure accounting and exit flush of admitted entries", async () => {
        process.env.EIDNARA_LOG_LEVEL = "debug";
        const root = mkdtempSync(path.join(os.tmpdir(), "eidnara-logger-test-"));
        scenarioRoots.push(root);
        const result = JSON.parse(
            await spawnScenario(exitScenario, "exit-off", root),
        ) as ExitScenarioResult;
        expect(result.content.split("\n").filter(Boolean)).toHaveLength(6);
        expect(result.idleMsBeforeExit).toBeLessThan(250);
        const stdout = await spawnScenario(
            `
            const logger = await import(process.env.LOGGER_MODULE_URL);
            process.env.EIDNARA_LOG_LEVEL = "warn";
            process.env.EIDNARA_LOG_PATH = process.env.LOGGER_SCENARIO_ROOT;
            logger.log.warn("admitted failure");
            process.env.EIDNARA_LOG_LEVEL = "off";
            logger.log.error("not admitted");
            logger.flushLogger();
            logger.flushLogger();
            console.log(JSON.stringify(logger.getLoggerDiagnostics().swallowedWriteCount));
        `,
            "flush-off",
            root,
        );
        expect(JSON.parse(stdout)).toBe(1);
    });

    test("real transforms and events filter debug lines without changing served bytes or fallback", async () => {
        const root = mkdtempSync(path.join(os.tmpdir(), "eidnara-logger-test-"));
        scenarioRoots.push(root);
        const stdout = await spawnScenario(
            `
            import { existsSync, readFileSync } from "node:fs";
            process.env.EIDNARA_LOG_LEVEL = "off";
            process.env.XDG_DATA_HOME = process.env.LOGGER_SCENARIO_ROOT;
            const { flushLogger, getLoggerDiagnostics } = await import(process.env.LOGGER_MODULE_URL);
            const { createRustModeTransform } = await import("../hooks/context/rust-mode-transform.ts");
            const { createEventHandler } = await import("../hooks/context/event-handler.ts");
            const { setRawMessageProvider } = await import("../hooks/context/read-session-chunk.ts");
            const { BoundedSessionMap } = await import("./bounded-session-map.ts");
            const sessionId = "logging-hook";
            const input = [{ info: { id: "m-1", role: "user", sessionID: sessionId }, parts: [{ type: "text", text: "input" }] }];
            const native = [{ role: "user", parts: [{ type: "text", text: "served\\nbytes" }] }];
            const row = { id: "m-1", timeCreated: 1, contributesOrdinal: true, hasValidInfo: true };
            const unregister = setRawMessageProvider(sessionId, {
                readMessages: () => [row],
                readMessageOrdinalPage: after => after ? [] : [row],
                getStoredMessageCount: () => 1,
            });
            const results = [];
            try {
                for (const minimum of ["debug", "warn", "off"]) {
                    const logFile = process.env.LOGGER_SCENARIO_ROOT + "/hook-" + minimum;
                    process.env.EIDNARA_LOG_PATH = logFile;
                    process.env.EIDNARA_LOG_LEVEL = minimum;
                    const contextUsageMap = new BoundedSessionMap(8);
                    let fail = false;
                    let calls = 0;
                    const transform = createRustModeTransform({
                        client: { session: { get: async () => ({ data: { directory: process.env.LOGGER_SCENARIO_ROOT } }) } },
                        contextUsageMap, clearReasoningAge: 50, cacheTtl: "5m", compactionOff: true,
                        directory: process.env.LOGGER_SCENARIO_ROOT, sessionDirectoryBySession: new Map(),
                        isSubagentSession: () => false, systemPromptHashFor: () => "",
                    }, { moduleClient: { call: async () => {
                        calls++;
                        if (fail) throw new Error("provider\\nfailed\\u0007");
                        return { native_messages: native, decision: "HARD", timings: { handler_total: 5 } };
                    } } });
                    const output = { messages: structuredClone(input) };
                    await transform.run(sessionId, structuredClone(input), output);
                    fail = true;
                    const fallback = { messages: structuredClone(input) };
                    await transform.run(sessionId, structuredClone(input), fallback);
                    const handle = createEventHandler({ contextUsageMap });
                    const event = { event: { type: "message.updated", properties: { info: {
                        role: "assistant", sessionID: sessionId, tokens: { input: 0 },
                    } } } };
                    const eventBefore = JSON.stringify(event);
                    await handle(event);
                    flushLogger();
                    results.push({ served: JSON.stringify(output.messages), fallback: JSON.stringify(fallback.messages),
                        calls, failures: transform.getState(sessionId).failureCount,
                        eventUnchanged: JSON.stringify(event) === eventBefore, usageSize: contextUsageMap.size,
                        content: existsSync(logFile) ? readFileSync(logFile, "utf8") : "",
                        swallowed: getLoggerDiagnostics().swallowedWriteCount });
                }
            } finally { unregister(); }
            console.log(JSON.stringify({ results, input: JSON.stringify(input), native: JSON.stringify(native) }));
        `,
            "hooks",
            root,
        );
        const { results, input, native } = JSON.parse(stdout);
        for (const result of results) {
            expect(result.served).toBe(native);
            expect(result.fallback).toBe(input);
            expect(result.calls).toBe(2);
            expect(result.failures).toBe(1);
            expect(result.eventUnchanged).toBe(true);
            expect(result.usageSize).toBe(0);
            expect(result.swallowed).toBe(0);
        }
        const [debug, warn, off] = results.map((result: { content: string }) => result.content);
        expect(debug.match(/rust pass:/g)).toHaveLength(2);
        expect(debug).toContain("transform stage: stage=rust.");
        expect(debug).toContain("rust module stages:");
        expect(debug.match(/event message.updated:/g)).toHaveLength(2);
        expect(warn.trim().split("\n")).toHaveLength(1);
        expect(warn).toContain("rust transform failed; serving the input unchanged:");
        expect(warn).toContain("provider failed");
        expect(warn).not.toContain("\u0007");
        expect(off).toBe("");
    });
});

describe.each([undefined, "info"])("logger hardening (minimum=%s)", (minimum) => {
    beforeEach(() => {
        if (minimum === undefined) delete process.env.EIDNARA_LOG_LEVEL;
        else process.env.EIDNARA_LOG_LEVEL = minimum;
    });

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

    // Log text often carries provider error bodies and model output, which are untrusted.
    test("neutralizes control characters and bounds entry size in untrusted message text", async () => {
        const result = await runLoggerScenario("sanitize");
        const lines = result.content.split("\n").filter((line) => line.length > 0);

        // "first", the forged entry, and the oversized entry; a newline injection would add a fourth.
        expect(lines).toHaveLength(3);
        for (const line of lines) {
            expect(line).toMatch(/^\[\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}\.\d{3}Z\] /);
        }
        const forgedLine = lines.find((line) => line.includes("upstream failed"));
        expect(forgedLine).toBeDefined();
        expect(forgedLine).toContain("AUDIT: forged entry");
        const controlChars = [...(forgedLine as string)].filter((char) => {
            const code = char.charCodeAt(0);
            return code <= 0x08 || (code >= 0x0b && code <= 0x1f) || code === 0x7f;
        });
        expect(controlChars).toEqual([]);

        const oversized = lines.find((line) => line.includes("oversized"));
        expect(oversized).toBeDefined();
        expect((oversized as string).length).toBeLessThan(5_000);
        const payload = (oversized as string).replace(/^\[[^\]]+\] /, "");
        expect(payload).toBe(
            `oversized: ${"x".repeat(2048 - "oversized: ".length)}… {"detail":"${"y".repeat(2048 - '{"detail":"'.length)}…`,
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
            expect(result.swallowedWriteCount).toBe(3);
            expect(result.unsafeDirectoryWritten).toBe(false);
            expect(result.unsafeDirectoryError).toMatch(/not a plain directory/);
            expect(result.foreignOwnerSwallowed).toBe(1);
            expect(result.foreignOwnerError).toMatch(/owned by another user/);
        },
    );

    test("keeps the message when its data cannot be serialized", async () => {
        const result = await runExitScenario();

        expect(result.content).toContain("with bigint [unserializable data: ");
        expect(result.content).toContain("with cycle [unserializable data: ");
        expect(result.content).toContain("with throwing toJSON [unserializable data: nope]");
        // `JSON.stringify` returns `undefined` for these; the entry must still be recorded.
        expect(result.content).toContain("with function undefined");
        expect(result.content).toContain("with symbol undefined");
        expect(result.content).toContain('plain {"ok":true}');
    });

    test("flushes on exit without holding the process open for the flush interval", async () => {
        const result = await runExitScenario();

        expect(result.content.split("\n").filter(Boolean)).toHaveLength(6);
        // A referenced 500ms timer would keep the process alive for at least 500ms.
        expect(result.idleMsBeforeExit).toBeLessThan(250);
    });

    test.skipIf(process.platform === "win32")(
        "leaves a caller-chosen EIDNARA_LOG_PATH directory's mode alone",
        async () => {
            const result = await runOverrideDirScenario();

            expect(result.sharedDirMode).toBe(0o775);
            expect(result.cwdMode).toBe(0o755);
            expect(result.sharedContent).toContain("absolute override");
            expect(result.cwdContent).toContain("relative override");
            expect(result.swallowedWriteCount).toBe(0);
        },
    );

    test.skipIf(process.platform === "win32")(
        "fails fast on a FIFO and tightens only a managed log that already exists",
        async () => {
            // bun test's per-test timeout bounds the run, so a blocking FIFO open fails this test.
            const result = await runExistingFileScenario();

            expect(result.fifoReturned).toBe(true);
            expect(result.fifoSwallowed).toBe(1);
            expect(result.managedModeAfter).toBe(0o600);
            expect(result.managedContent.startsWith("old\n")).toBe(true);
            expect(result.managedContent).toContain("managed append");
            expect(result.overrideModeAfter).toBe(0o644);
            expect(result.overrideContent).toContain("override append");
        },
    );
});
