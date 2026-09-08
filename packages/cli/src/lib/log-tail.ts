import { closeSync, fstatSync, openSync, readSync } from "node:fs";

export const DEFAULT_LOG_TAIL_BYTES = 4 * 1024 * 1024;

/** Limits the read to the final `maxBytes` to bound memory use on an unrotated log. */
export function readLogTailLines(path: string, maxBytes = DEFAULT_LOG_TAIL_BYTES): string[] {
    const fd = openSync(path, "r");
    try {
        const size = fstatSync(fd).size;
        const start = Math.max(0, size - maxBytes);
        const length = size - start;
        const buffer = Buffer.allocUnsafe(length);
        let offset = 0;
        while (offset < length) {
            const read = readSync(fd, buffer, offset, length - offset, start + offset);
            if (read === 0) break;
            offset += read;
        }
        const text = buffer.subarray(0, offset).toString("utf-8");
        const lines = text.split(/\r?\n/);
        if (start > 0) {
            // The first split entry may begin mid-record or mid-UTF-8 sequence.
            const partial = lines.shift() ?? "";
            // A record larger than the window has no complete line; return its read portion with `TRUNCATED_RECORD_MARKER`.
            if (!lines.some((line) => line !== "")) {
                lines.unshift(`${TRUNCATED_RECORD_MARKER}${partial.replace(/^\uFFFD+/, "")}`);
            }
        }
        return lines;
    } finally {
        closeSync(fd);
    }
}

export const TRUNCATED_RECORD_MARKER = "[truncated record] ";
