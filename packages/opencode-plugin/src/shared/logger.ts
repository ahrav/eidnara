import * as fs from "node:fs";
import * as os from "node:os";
import * as path from "node:path";
import { getEidnaraLogPath } from "./data-path";

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

/** For paths outside `os.tmpdir()`, the caller selects the ancestors, so only `dir` is checked. */
function ownedDirChain(dir: string): string[] {
    const tmp = path.resolve(os.tmpdir());
    const relative = path.relative(tmp, dir);
    if (relative === "" || relative.startsWith("..") || path.isAbsolute(relative)) return [dir];
    const chain: string[] = [];
    let current = tmp;
    for (const segment of relative.split(path.sep)) {
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

function ensurePrivateDir(dir: string): void {
    fs.mkdirSync(dir, { recursive: true, mode: PRIVATE_DIR_MODE });
    for (const owned of ownedDirChain(dir)) {
        assertPrivateDir(owned);
    }
}

/**
 * `O_NOFOLLOW` makes a symlink at the log path fail the open instead of
 * redirecting the append; the requested create mode grants access only to the owner.
 */
function appendPrivate(logFile: string, data: string): void {
    const { O_WRONLY, O_APPEND, O_CREAT, O_NOFOLLOW } = fs.constants;
    const flags = O_WRONLY | O_APPEND | O_CREAT | (O_NOFOLLOW ?? 0);
    const fd = fs.openSync(logFile, flags, PRIVATE_FILE_MODE);
    try {
        fs.writeSync(fd, data);
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
        ensurePrivateDir(path.dirname(logFile));
        appendPrivate(logFile, data);
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
}

export function log(message: string, data?: unknown): void {
    if (isTestEnv) return;
    try {
        const timestamp = new Date().toISOString();
        const serialized =
            data === undefined
                ? ""
                : data instanceof Error
                  ? ` ${data.message}${data.stack ? `\n${data.stack}` : ""}`
                  : ` ${JSON.stringify(data)}`;
        buffer.push(`[${timestamp}] ${message}${serialized}\n`);
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
