import * as fs from "node:fs";
import * as path from "node:path";
import { getEidnaraLogPath } from "./data-path";

const isTestEnv = process.env.NODE_ENV === "test";

let buffer: string[] = [];
let flushTimer: ReturnType<typeof setTimeout> | null = null;
const FLUSH_INTERVAL_MS = 500;
const BUFFER_SIZE_LIMIT = 50;
const MAX_FIELD_CHARS = 2048;

function isControlChar(code: number): boolean {
    return code <= 0x08 || (code >= 0x0b && code <= 0x1f) || code === 0x7f;
}

/**
 * Log text carries provider error bodies and model output, which are untrusted.
 * Newlines and control characters would forge or corrupt entries in the newline-delimited file.
 */
function sanitizeField(value: string): string {
    let flat = "";
    for (const char of value) {
        const code = char.charCodeAt(0);
        if (code === 0x0a || code === 0x0d || code === 0x09) {
            flat += " ";
        } else if (!isControlChar(code)) {
            flat += char;
        }
        if (flat.length >= MAX_FIELD_CHARS) return `${flat}…`;
    }
    return flat;
}

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

function ensureDir(filePath: string): void {
    fs.mkdirSync(path.dirname(filePath), { recursive: true });
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
        ensureDir(logFile);
        fs.appendFileSync(logFile, data);
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
                  ? ` ${sanitizeField(data.message)}${data.stack ? ` | ${sanitizeField(data.stack)}` : ""}`
                  : ` ${sanitizeField(JSON.stringify(data))}`;
        buffer.push(`[${timestamp}] ${sanitizeField(message)}${serialized}\n`);
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
