import * as fs from "node:fs";
import * as path from "node:path";
import { getEidnaraLogPath, getEidnaraTempRoot } from "./data-path";
import { writeAllSync } from "./write-all";

const isTestEnv = process.env.NODE_ENV === "test";

let buffer: string[] = [];
let flushTimer: ReturnType<typeof setTimeout> | null = null;
const FLUSH_INTERVAL_MS = 500;
const BUFFER_SIZE_LIMIT = 50;

export interface LoggerDiagnostics {
    swallowedWriteCount: number;
    lastErrorMessage: string | null;
    lastErrorTime: string | null;
}

let swallowedWriteCount = 0;
let lastErrorMessage: string | null = null;
let lastErrorTime: string | null = null;

function recordSwallowedWrite(error: unknown): void {
    try {
        swallowedWriteCount++;
        lastErrorMessage = error instanceof Error ? error.message : String(error);
        lastErrorTime = new Date().toISOString();
    } catch {
        // Diagnostics must not make the logger throw either.
    }
}

const PRIVATE_DIR_MODE = 0o700;
const PRIVATE_FILE_MODE = 0o600;
const GROUP_OTHER_BITS = 0o077;

/**
 * Only directories inside the per-user root have their modes enforced; a
 * caller-chosen `EIDNARA_LOG_PATH` directory keeps whatever mode its owner set.
 */
function managedDirChain(dir: string): string[] | null {
    const root = path.resolve(getEidnaraTempRoot());
    const relative = path.relative(root, path.resolve(dir));
    if (relative.startsWith("..") || path.isAbsolute(relative)) return null;
    const chain = [root];
    let current = root;
    for (const segment of relative.split(path.sep)) {
        if (segment === "") continue;
        current = path.join(current, segment);
        chain.push(current);
    }
    return chain;
}

/**
 * Reject directories owned by another user because their owner can replace
 * entries with symlinks. commentlint: allow(JUDGE)
 */
function assertPrivateDir(dir: string): void {
    const stat = fs.lstatSync(dir);
    if (stat.isSymbolicLink() || !stat.isDirectory()) {
        throw new Error(`log directory is not a plain directory: ${dir}`);
    }
    // Ownership and mode bits are POSIX concepts; process.getuid is absent on Windows.
    const uid = process.getuid?.();
    if (uid === undefined) return;
    if (stat.uid !== uid) {
        throw new Error(`log directory is owned by another user: ${dir}`);
    }
    if ((stat.mode & GROUP_OTHER_BITS) !== 0) {
        fs.chmodSync(dir, PRIVATE_DIR_MODE);
    }
}

function ensureLogDir(dir: string): boolean {
    const chain = managedDirChain(dir);
    if (chain === null) {
        fs.mkdirSync(dir, { recursive: true });
        return false;
    }
    fs.mkdirSync(dir, { recursive: true, mode: PRIVATE_DIR_MODE });
    for (const owned of chain) {
        assertPrivateDir(owned);
    }
    return true;
}

/**
 * `O_NOFOLLOW` makes a symlink at the log path fail the open instead of
 * redirecting the append; the requested create mode grants access only to the owner.
 * `O_NONBLOCK` turns a FIFO with no reader into `ENXIO` instead of a hang, and
 * the descriptor is rejected unless it names a regular file. commentlint: allow(JUDGE)
 */
function appendPrivate(logFile: string, data: string, managed: boolean): void {
    const { O_WRONLY, O_APPEND, O_CREAT, O_NOFOLLOW, O_NONBLOCK } = fs.constants;
    const flags = O_WRONLY | O_APPEND | O_CREAT | (O_NOFOLLOW ?? 0) | (O_NONBLOCK ?? 0);
    const fd = fs.openSync(logFile, flags, PRIVATE_FILE_MODE);
    try {
        const stat = fs.fstatSync(fd);
        if (!stat.isFile()) {
            throw new Error(`log path is not a regular file: ${logFile}`);
        }
        // The create mode does not apply to existing files, so managed logs are tightened here.
        if (managed && (stat.mode & GROUP_OTHER_BITS) !== 0) {
            fs.fchmodSync(fd, PRIVATE_FILE_MODE);
        }
        writeAllSync(fd, data);
    } finally {
        fs.closeSync(fd);
    }
}

function flush(): void {
    if (flushTimer) {
        clearTimeout(flushTimer);
        flushTimer = null;
    }
    if (buffer.length === 0) return;
    const data = buffer.join("");
    buffer = [];
    try {
        const logFile = getEidnaraLogPath();
        const managed = ensureLogDir(path.dirname(logFile));
        appendPrivate(logFile, data, managed);
    } catch (error) {
        recordSwallowedWrite(error);
    }
}

function scheduleFlush(): void {
    if (flushTimer) return;
    flushTimer = setTimeout(() => {
        flushTimer = null;
        flush();
    }, FLUSH_INTERVAL_MS);
    // An active timer keeps the process alive until `FLUSH_INTERVAL_MS` elapses;
    // the `exit` handler below flushes whatever is buffered.
    flushTimer.unref?.();
}

/**
 * `JSON.stringify` throws on a bigint, a cycle, or a throwing `toJSON`.
 * Those cases yield a marker so `message` is still recorded.
 */
function serializeData(data: unknown): string {
    if (data === undefined) return "";
    if (data instanceof Error) return ` ${data.message}${data.stack ? `\n${data.stack}` : ""}`;
    try {
        return ` ${JSON.stringify(data)}`;
    } catch (error) {
        return ` [unserializable data: ${error instanceof Error ? error.message : String(error)}]`;
    }
}

export function log(message: string, data?: unknown): void {
    if (isTestEnv) return;
    try {
        const timestamp = new Date().toISOString();
        buffer.push(`[${timestamp}] ${message}${serializeData(data)}\n`);
        if (buffer.length >= BUFFER_SIZE_LIMIT) {
            flush();
        } else {
            scheduleFlush();
        }
    } catch {
        // Logging must never throw.
    }
}

export function sessionLog(sessionId: string, message: string, data?: unknown): void {
    log(`[eidnara][${sessionId}] ${message}`, data);
}

export function getLoggerDiagnostics(): LoggerDiagnostics {
    return {
        swallowedWriteCount,
        lastErrorMessage,
        lastErrorTime,
    };
}

/* */
export function flushLogger(): void {
    flush();
}

// Flush remaining buffer on process exit
if (!isTestEnv) {
    process.on("exit", flush);
}
