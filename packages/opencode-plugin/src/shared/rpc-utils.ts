import { execFileSync } from "node:child_process";
import { createHash } from "node:crypto";
import { readFileSync } from "node:fs";
import { join } from "node:path";

export type ProcessKind = "OpenCode server" | "OpenCode instance (TUI/CLI)" | "Pi" | "process";

export interface RpcPortFileRecord {
    port: number;
    pid: number;
    started_at: number;
    /** Optional producer-provided kind; older records omit it. */
    kind?: string;
    /** Older discovery records use `harness`. */
    harness?: string;
    /**
     * Per-process bearer token. The server requires it on all non-health RPC
     * calls so a random local process or browser-origin script that merely
     * discovers/guesses the port cannot drive side-effecting endpoints
     * (recomp/upgrade/dismiss). Optional in the type for forward/backward
     * compatibility with port files written by older builds (treated as "no
     * auth required" only when the server itself didn't set one).
     */
    token?: string;
    /** When present, `instance_id` distinguishes port-file names for same-PID instances. */
    instance_id?: string;
}

/**
 * Stable hash for a project directory — scopes RPC port files per-project
 * so different project directories use separate RPC port-file directories unless their 64-bit hashes collide.
 */
function projectHash(directory: string): string {
    // Windows accepts either separator, so `C:\repo\sub`, `C:/repo/sub`, and `C:\repo\sub\` scope to one directory there; on POSIX a backslash is an ordinary filename character.
    const slashed = rpcIdentityPlatform === "win32" ? directory.replaceAll("\\", "/") : directory;
    const normalized = slashed.replace(/\/+$/, "");
    return createHash("sha256").update(normalized).digest("hex").slice(0, 16);
}

/* */
export function rpcPortDir(storageDir: string, directory: string): string {
    return join(storageDir, "rpc", projectHash(directory));
}

/** A separator in `instanceId` can introduce path traversal outside `rpcPortDir`. */
const RPC_INSTANCE_ID_PATTERN = /^[A-Za-z0-9_-]{1,64}$/;

function isValidInstanceId(instanceId: string): boolean {
    return RPC_INSTANCE_ID_PATTERN.test(instanceId);
}

/** Throws when `instanceId` would not stay inside `rpcPortDir`. */
export function rpcPortFilePath(
    storageDir: string,
    directory: string,
    pid = process.pid,
    instanceId?: string,
): string {
    if (instanceId && !isValidInstanceId(instanceId)) {
        throw new Error(`Eidnara: invalid RPC instance id ${JSON.stringify(instanceId)}`);
    }
    const suffix = instanceId ? `-${instanceId}` : "";
    return join(rpcPortDir(storageDir, directory), `port-${pid}${suffix}.json`);
}

/* */
export function legacyRpcPortFilePath(storageDir: string, directory: string): string {
    return join(rpcPortDir(storageDir, directory), "port");
}

export type PidLiveness = "alive" | "dead" | "inconclusive";

/**
 * Check whether the platform confirms a PID is live without treating a denied
 * probe as confirmation. Windows uses tasklist because MSYS2/Cygwin ps does not
 * support the options used by the Unix probe. Sandboxes commonly reject
 * `kill(pid, 0)` with EPERM even when the PID does not exist outside their view.
 */
export function isPidAlive(pid: number): PidLiveness {
    if (!Number.isInteger(pid) || pid <= 0) return "dead";
    if (rpcIdentityPlatform === "win32") return readWindowsProcess(pid).state;
    try {
        rpcIdentityProcessKill(pid, 0);
        return "alive";
    } catch (error) {
        return (error as NodeJS.ErrnoException).code === "ESRCH" ? "dead" : "inconclusive";
    }
}

const RPC_IDENTITY_SKEW_TOLERANCE_MS = 120_000;
/** `/proc/<pid>/stat` field 22 uses the fixed userspace `USER_HZ` value of 100, not kernel `CONFIG_HZ`. */
const LINUX_CLOCK_TICKS_PER_SECOND = 100;
const PS_PROBE_TIMEOUT_MS = 1_000;
/** PowerShell start-up dominates the Windows process-list probe, so it gets a longer budget. */
const WINDOWS_PROCESS_LIST_TIMEOUT_MS = 10_000;

let rpcIdentityReadFileSync: typeof readFileSync = readFileSync;
let rpcIdentityExecFileSync: typeof execFileSync = execFileSync;
let rpcIdentityProcessKill: typeof process.kill = process.kill;
let rpcProcessListExecFileSync: typeof execFileSync = execFileSync;
let rpcIdentityPlatform: NodeJS.Platform = process.platform;
let rpcIdentityNowMs: () => number = () => Date.now();

function parseLinuxProcessStartTime(statContent: string, uptimeContent: string): number | null {
    const closingCommandName = statContent.lastIndexOf(")");
    if (closingCommandName < 0) return null;

    // The fields after the command name begin at field 3 (`state`), so field 22
    // (`starttime`) is index 19 in this suffix. The command name can contain ')',
    // hence the last closing parenthesis rather than the first one is significant.
    const statFields = statContent
        .slice(closingCommandName + 1)
        .trim()
        .split(/\s+/);
    const startTimeTicks = Number(statFields[19]);
    const uptimeSeconds = Number(uptimeContent.trim().split(/\s+/)[0]);
    if (
        !Number.isFinite(startTimeTicks) ||
        startTimeTicks < 0 ||
        !Number.isFinite(uptimeSeconds) ||
        uptimeSeconds < 0
    ) {
        return null;
    }

    const processStartTime =
        rpcIdentityNowMs() -
        uptimeSeconds * 1_000 +
        (startTimeTicks / LINUX_CLOCK_TICKS_PER_SECOND) * 1_000;
    return Number.isFinite(processStartTime) ? processStartTime : null;
}

function readLinuxProcessStartTime(pid: number): number | null {
    try {
        const statContent = String(rpcIdentityReadFileSync(`/proc/${pid}/stat`, "utf8"));
        const uptimeContent = String(rpcIdentityReadFileSync("/proc/uptime", "utf8"));
        return parseLinuxProcessStartTime(statContent, uptimeContent);
    } catch {
        return null;
    }
}

function readPsProcessStartTime(pid: number): number | null {
    try {
        // `lstart` is `strftime("%c")`, so a non-C `LC_TIME` yields month names `Date.parse` rejects.
        const output = rpcIdentityExecFileSync("ps", ["-p", String(pid), "-o", "lstart="], {
            encoding: "utf8",
            timeout: PS_PROBE_TIMEOUT_MS,
            stdio: ["ignore", "pipe", "pipe"],
            env: { ...process.env, LC_ALL: "C" },
        });
        const processStartTime = Date.parse(String(output).trim());
        return Number.isFinite(processStartTime) ? processStartTime : null;
    } catch {
        return null;
    }
}

interface ProcessListEntry {
    pid: number;
    command: string;
}

/** An unterminated quoted field invalidates the entire output rather than returning partial records. */
function parseCsvRecords(text: string): string[][] | null {
    const records: string[][] = [];
    let fields: string[] = [];
    let field = "";
    let quoted = false;
    let recordStarted = false;
    for (let index = 0; index < text.length; index += 1) {
        const character = text[index];
        if (quoted) {
            if (character !== '"') {
                field += character;
            } else if (text[index + 1] === '"') {
                field += '"';
                index += 1;
            } else {
                quoted = false;
            }
            continue;
        }
        if (character === '"') {
            quoted = true;
            recordStarted = true;
        } else if (character === ",") {
            fields.push(field);
            field = "";
            recordStarted = true;
        } else if (character === "\n" || character === "\r") {
            if (character === "\r" && text[index + 1] === "\n") index += 1;
            if (recordStarted) {
                fields.push(field);
                records.push(fields);
            }
            fields = [];
            field = "";
            recordStarted = false;
        } else {
            field += character;
            recordStarted = true;
        }
    }
    if (quoted) return null;
    if (recordStarted) {
        fields.push(field);
        records.push(fields);
    }
    return records;
}

/** Returns `null` without the header row, so output that is not a process list cannot indicate no processes. */
function parseTasklistOutput(output: string): ProcessListEntry[] | null {
    const records = parseCsvRecords(output);
    if (records === null) return null;
    const entries: ProcessListEntry[] = [];
    let sawHeader = false;
    for (const fields of records) {
        if (fields[1]?.trim().toLowerCase() === "pid") {
            sawHeader = true;
            continue;
        }
        const pid = Number(fields[1]);
        if (!Number.isInteger(pid) || pid <= 0 || !fields[0]) continue;
        entries.push({ pid, command: fields[0] });
    }
    return sawHeader ? entries : null;
}

/** A `/FI` filter with no match prints localized non-CSV output, so the full list is requested and filtered here. */
function readWindowsProcess(pid: number): { state: PidLiveness; command?: string } {
    try {
        const output = rpcIdentityExecFileSync("tasklist", ["/FO", "CSV"], {
            encoding: "utf8",
            timeout: PS_PROBE_TIMEOUT_MS,
            stdio: ["ignore", "pipe", "pipe"],
        });
        const entries = parseTasklistOutput(String(output));
        if (entries === null) return { state: "inconclusive" };
        const process = entries.find((entry) => entry.pid === pid);
        return process ? { state: "alive", command: process.command } : { state: "dead" };
    } catch {
        return { state: "inconclusive" };
    }
}

function readLinuxProcessCommand(pid: number): string | null {
    try {
        return String(rpcIdentityReadFileSync(`/proc/${pid}/cmdline`, "utf8"));
    } catch {
        return null;
    }
}

function readPsProcessCommand(pid: number): string | null {
    try {
        const output = rpcIdentityExecFileSync("ps", ["-p", String(pid), "-o", "command="], {
            encoding: "utf8",
            timeout: PS_PROBE_TIMEOUT_MS,
            stdio: ["ignore", "pipe", "pipe"],
        });
        return String(output);
    } catch {
        return null;
    }
}

/* */
export function readProcessCommand(pid: number): string | null {
    if (!Number.isInteger(pid) || pid <= 0) return null;
    return rpcIdentityPlatform === "linux"
        ? readLinuxProcessCommand(pid)
        : rpcIdentityPlatform === "win32"
          ? (readWindowsProcess(pid).command ?? null)
          : readPsProcessCommand(pid);
}

function executableName(token: string | undefined): string {
    return (token ?? "").split("/").at(-1) ?? "";
}

/**
 * `/proc/<pid>/cmdline` separates arguments with NUL, so an argument keeps its
 * spaces. `ps` and CIM output separate with whitespace, and CIM quotes paths
 * such as `"C:\Program Files\nodejs\node.exe"`, so quotes group one argument.
 */
function commandTokens(command: string): string[] {
    const normalized = command.toLowerCase().replaceAll("\\", "/");
    if (normalized.includes("\u0000")) return normalized.split("\u0000").filter(Boolean);
    const tokens: string[] = [];
    let current = "";
    let inToken = false;
    let quote: string | null = null;
    for (const character of normalized) {
        if (quote !== null) {
            if (character === quote) quote = null;
            else current += character;
        } else if (character === '"' || character === "'") {
            quote = character;
            inToken = true;
        } else if (/\s/.test(character)) {
            if (inToken) tokens.push(current);
            current = "";
            inToken = false;
        } else {
            current += character;
            inToken = true;
        }
    }
    if (inToken) tokens.push(current);
    return tokens;
}

const PI_EXECUTABLE_NAMES = ["pi", "omp", "oh-my-pi"];
const PI_SCRIPT_NAMES = ["pi", "pi.js", "pi.mjs", "pi.cjs"];
const SCRIPT_INTERPRETER_NAMES = ["node", "bun", "deno"];
/** These runtimes can host OpenCode or Pi scripts, so their names do not identify either. */
const HOSTED_RUNTIME_NAMES = [...SCRIPT_INTERPRETER_NAMES, "electron"];
/** Each of these runs the operand after its own options as a new program. */
const COMMAND_WRAPPER_NAMES = [
    "sh",
    "bash",
    "zsh",
    "dash",
    "exec",
    "env",
    "nice",
    "nohup",
    "timeout",
    "sudo",
    "doas",
    "stdbuf",
    "npx",
    "bunx",
    "pnpx",
];

function baseExecutable(token: string | undefined): string {
    return executableName(token).replace(/\.(?:exe|cmd)$/, "");
}

function isOption(token: string): boolean {
    return token.startsWith("-");
}

/** `timeout 3600`, `nice 10`, and `env KEY=VALUE` consume one operand before the wrapped program. */
function isWrapperOperand(token: string): boolean {
    return /^\d+(?:\.\d+)?[smhd]?$/.test(token) || /^[a-z_][a-z0-9_]*=/.test(token);
}

/** A bare word after an interpreter option is that option's value, so only a path or a file name can be a script. */
function looksLikeScriptPath(token: string): boolean {
    return token.includes("/") || /\.[a-z0-9]+$/.test(token);
}

interface CommandProgram {
    index: number;
    viaInterpreter: boolean;
}

/**
 * A program sits at `argv[0]`, after a wrapper's options, or as a script
 * operand of an interpreter. `node app.js --model pi` names no `pi` program
 * because `pi` is neither of those.
 */
function commandPrograms(tokens: readonly string[]): CommandProgram[] {
    const programs: CommandProgram[] = [];
    let index = 0;
    while (index < tokens.length) {
        const name = baseExecutable(tokens[index]);
        programs.push({ index, viaInterpreter: false });
        if (SCRIPT_INTERPRETER_NAMES.includes(name)) {
            for (let position = index + 1; position < tokens.length; position += 1) {
                const token = tokens[position];
                if (!isOption(token) && looksLikeScriptPath(token)) {
                    programs.push({ index: position, viaInterpreter: true });
                }
            }
            return programs;
        }
        if (!COMMAND_WRAPPER_NAMES.includes(name)) return programs;
        index += 1;
        while (
            index < tokens.length &&
            (isOption(tokens[index]) || isWrapperOperand(tokens[index]))
        ) {
            index += 1;
        }
    }
    return programs;
}

function commandHasOpenCodeExecutable(tokens: readonly string[]): number {
    return (
        commandPrograms(tokens).find(({ index }) => baseExecutable(tokens[index]) === "opencode")
            ?.index ?? -1
    );
}

/** `inspectLivePiProcesses` and `classifyProcessKind` share this predicate so the database-holder guard and the process label cannot disagree. */
function commandHasPiExecutable(tokens: readonly string[]): boolean {
    return commandPrograms(tokens).some(({ index, viaInterpreter }) => {
        const token = tokens[index];
        const name = baseExecutable(token);
        return viaInterpreter
            ? PI_SCRIPT_NAMES.includes(name) || token.includes("pi-coding-agent")
            : PI_EXECUTABLE_NAMES.includes(name);
    });
}

function commandRunsHostedRuntime(tokens: readonly string[]): boolean {
    return commandPrograms(tokens).some(
        ({ index, viaInterpreter }) =>
            !viaInterpreter && HOSTED_RUNTIME_NAMES.includes(baseExecutable(tokens[index])),
    );
}

/** Classify a process command without changing the liveness decision. */
export function classifyProcessKind(command: string | null | undefined): ProcessKind {
    if (!command) return "process";
    const tokens = commandTokens(command);
    const openCodeIndex = commandHasOpenCodeExecutable(tokens);
    if (openCodeIndex >= 0) {
        const args = tokens.slice(openCodeIndex + 1);
        if (
            args.some(
                (token) => token === "serve" || token === "--serve" || token.startsWith("--serve="),
            )
        ) {
            return "OpenCode server";
        }
        return "OpenCode instance (TUI/CLI)";
    }
    return commandHasPiExecutable(tokens) ? "Pi" : "process";
}

/**
 * Verify that a live PID still belongs to the process that wrote a port record.
 *
 * A PID can be reused after its original process exits.
 * Windows lacks a start-time probe. Records without `started_at` fall back to the command check.
 * A failed filesystem or process probe is inconclusive, not proof that this port record still belongs to OpenCode.
 */
export type PidIdentityPlausibility = "plausible" | "implausible" | "inconclusive";

/** A bare JavaScript runtime is inconclusive, not plausible: a reused PID running an unrelated script must not pass the cold database-open guard as OpenCode. */
function commandIdentityPlausibility(command: string): PidIdentityPlausibility {
    const tokens = commandTokens(command);
    if (commandHasOpenCodeExecutable(tokens) >= 0) return "plausible";
    return commandRunsHostedRuntime(tokens) ? "inconclusive" : "implausible";
}

/** `undefined` means the platform has no start-time probe; `null` means the probe failed. */
function readProcessStartTime(pid: number): number | null | undefined {
    if (rpcIdentityPlatform === "linux") return readLinuxProcessStartTime(pid);
    if (rpcIdentityPlatform === "win32") return undefined;
    return readPsProcessStartTime(pid);
}

export function isPidIdentityPlausible(record: RpcPortFileRecord): PidIdentityPlausibility {
    if (!Number.isInteger(record.pid) || record.pid <= 0) return "implausible";

    if (Number.isFinite(record.started_at) && record.started_at > 0) {
        const processStartTime = readProcessStartTime(record.pid);
        if (processStartTime === null) return "inconclusive";
        if (processStartTime !== undefined) {
            if (processStartTime <= record.started_at) return "plausible";
            if (processStartTime > record.started_at + RPC_IDENTITY_SKEW_TOLERANCE_MS) {
                return "implausible";
            }
            // Inside the tolerance window a start time cannot separate probe skew from a recycled PID, so the command decides.
        }
    }

    const command = readProcessCommand(record.pid);
    if (command === null) return "inconclusive";
    return commandIdentityPlausibility(command);
}

export function __setRpcIdentityTestHooks(hooks: {
    readFileSync?: typeof readFileSync;
    execFileSync?: typeof execFileSync;
    processKill?: typeof process.kill;
    processListExecFileSync?: typeof execFileSync;
    platform?: NodeJS.Platform;
    nowMs?: () => number;
}): void {
    rpcIdentityReadFileSync = hooks.readFileSync ?? readFileSync;
    rpcIdentityExecFileSync = hooks.execFileSync ?? execFileSync;
    rpcIdentityProcessKill = hooks.processKill ?? process.kill;
    rpcProcessListExecFileSync = hooks.processListExecFileSync ?? execFileSync;
    rpcIdentityPlatform = hooks.platform ?? process.platform;
    rpcIdentityNowMs = hooks.nowMs ?? (() => Date.now());
}

export function __resetRpcIdentityTestHooks(): void {
    rpcIdentityReadFileSync = readFileSync;
    rpcIdentityExecFileSync = execFileSync;
    rpcIdentityProcessKill = process.kill;
    rpcProcessListExecFileSync = execFileSync;
    rpcIdentityPlatform = process.platform;
    rpcIdentityNowMs = () => Date.now();
}

/** Result of checking whether Pi/OMP processes may currently hold the shared database. */
export interface PiProcessDiscovery {
    state: "known" | "unreadable";
    processIds: number[];
    error?: string;
}

function knownPiProcesses(pids: Iterable<number>): PiProcessDiscovery {
    return { state: "known", processIds: [...pids].sort((left, right) => left - right) };
}

function unreadablePiProcesses(error: string): PiProcessDiscovery {
    return { state: "unreadable", processIds: [], error };
}

const WINDOWS_PROCESS_LIST_ARGS = [
    "-NoProfile",
    "-NonInteractive",
    "-Command",
    "Get-CimInstance Win32_Process | Select-Object ProcessId,Name,CommandLine | ConvertTo-Csv -NoTypeInformation",
];

interface WindowsProcessEntry {
    pid: number;
    name: string;
    commandLine: string;
}

/** Returns `null` when the CSV header is missing, so empty or diagnostic output cannot indicate no processes. */
function parseWindowsProcessList(output: string): WindowsProcessEntry[] | null {
    const records = parseCsvRecords(output);
    if (records === null) return null;
    const entries: WindowsProcessEntry[] = [];
    let sawHeader = false;
    for (const fields of records) {
        if (fields[0]?.trim().toLowerCase() === "processid") {
            sawHeader = true;
            continue;
        }
        const pid = Number(fields[0]);
        if (!Number.isInteger(pid) || pid <= 0) continue;
        entries.push({ pid, name: fields[1] ?? "", commandLine: fields[2] ?? "" });
    }
    return sawHeader ? entries : null;
}

/**
 * `tasklist` reports only the image name, under which an npm-installed Pi is
 * `node.exe`, so the Windows probe reads command lines through CIM. CIM
 * withholds the command line of a process the caller cannot open; a bare
 * runtime image without one could still be Pi, so the probe fails closed.
 */
function inspectWindowsPiProcesses(): PiProcessDiscovery {
    const output = String(
        rpcProcessListExecFileSync("powershell.exe", WINDOWS_PROCESS_LIST_ARGS, {
            encoding: "utf8",
            timeout: WINDOWS_PROCESS_LIST_TIMEOUT_MS,
            stdio: ["ignore", "pipe", "pipe"],
        }),
    );
    const entries = parseWindowsProcessList(output);
    if (entries === null) return unreadablePiProcesses("PowerShell process list unavailable");
    const pids = new Set<number>();
    const unclassified: string[] = [];
    for (const entry of entries) {
        if (entry.pid === process.pid) continue;
        if (entry.commandLine) {
            if (commandHasPiExecutable(commandTokens(entry.commandLine))) pids.add(entry.pid);
            continue;
        }
        const imageTokens = commandTokens(entry.name);
        if (commandHasPiExecutable(imageTokens)) {
            pids.add(entry.pid);
        } else if (
            imageTokens.some((token) => SCRIPT_INTERPRETER_NAMES.includes(baseExecutable(token)))
        ) {
            unclassified.push(`${entry.name} (pid ${entry.pid})`);
        }
    }
    if (unclassified.length > 0) {
        return unreadablePiProcesses(`command line unavailable for ${unclassified.join(", ")}`);
    }
    return knownPiProcesses(pids);
}

/**
 * A failed process-list probe leaves database ownership inconclusive.
 * Failed probes return `unreadable` rather than indicating that no harness is running.
 * Destructive maintenance can fail closed when `state` is `"unreadable"`.
 * Tests replace the probe through `__setRpcIdentityTestHooks`; no environment variable skips it.
 */
export function inspectLivePiProcesses(): PiProcessDiscovery {
    try {
        if (rpcIdentityPlatform === "win32") return inspectWindowsPiProcesses();
        const output = String(
            rpcProcessListExecFileSync("ps", ["-axo", "pid=,command="], {
                encoding: "utf8",
                timeout: PS_PROBE_TIMEOUT_MS,
                stdio: ["ignore", "pipe", "pipe"],
            }),
        );
        const pids = new Set<number>();
        for (const line of output.split(/\r?\n/)) {
            const match = /^\s*(\d+)\s+(.+)$/.exec(line);
            if (!match) continue;
            const pid = Number(match[1]);
            if (!Number.isInteger(pid) || pid <= 0 || pid === process.pid) continue;
            if (commandHasPiExecutable(commandTokens(match[2]))) pids.add(pid);
        }
        return knownPiProcesses(pids);
    } catch (error) {
        return unreadablePiProcesses(error instanceof Error ? error.message : String(error));
    }
}

/* */
export function discoverLivePiProcessIds(): number[] {
    return inspectLivePiProcesses().processIds;
}

export function parseRpcPortFile(content: string, fallbackPid = 0): RpcPortFileRecord | null {
    const trimmed = content.trim();
    if (!trimmed) return null;

    if (trimmed.startsWith("{")) {
        try {
            const parsed = JSON.parse(trimmed) as Partial<RpcPortFileRecord>;
            const port = Number(parsed.port);
            const pid = Number(parsed.pid);
            const startedAt = Number(parsed.started_at);
            if (!isValidPort(port) || !Number.isInteger(pid) || pid <= 0) return null;
            return {
                port,
                pid,
                started_at: Number.isFinite(startedAt) ? startedAt : 0,
                kind: typeof parsed.kind === "string" ? parsed.kind : undefined,
                harness: typeof parsed.harness === "string" ? parsed.harness : undefined,
                token: typeof parsed.token === "string" ? parsed.token : undefined,
                instance_id:
                    typeof parsed.instance_id === "string" && isValidInstanceId(parsed.instance_id)
                        ? parsed.instance_id
                        : undefined,
            };
        } catch {
            return null;
        }
    }

    // `Number.parseInt` would accept `43123garbage`, so a torn or corrupted record must fail the whole-string check.
    if (!/^\d{1,5}$/.test(trimmed)) return null;
    const port = Number(trimmed);
    if (!isValidPort(port)) return null;
    return { port, pid: fallbackPid, started_at: 0 };
}

function isValidPort(port: number): boolean {
    return Number.isInteger(port) && port > 0 && port <= 65535;
}
