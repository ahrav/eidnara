import { closeSync, fstatSync, openSync, readSync } from "node:fs";

export const DEFAULT_LOG_TAIL_BYTES = 4 * 1024 * 1024;

export const TRUNCATED_RECORD_MARKER = "[truncated record] ";

/** Limits the read to the final `maxBytes` to bound memory use on an unrotated log. */
export function readLogTailLines(path: string, maxBytes = DEFAULT_LOG_TAIL_BYTES): string[] {
    const fd = openSync(path, "r");
    try {
        const size = fstatSync(fd).size;
        const start = Math.max(0, size - maxBytes);
        // One extra byte before the window tells whether the window opens on a record boundary.
        const readStart = start > 0 ? start - 1 : 0;
        const length = size - readStart;
        const buffer = Buffer.allocUnsafe(length);
        let offset = 0;
        while (offset < length) {
            const read = readSync(fd, buffer, offset, length - offset, readStart + offset);
            if (read === 0) break;
            offset += read;
        }
        const bytes = buffer.subarray(0, offset);
        if (start === 0) return bytes.toString("utf-8").split(/\r?\n/);

        const opensOnBoundary = bytes[0] === 0x0a;
        const lines = bytes.subarray(1).toString("utf-8").split(/\r?\n/);
        if (opensOnBoundary) return lines;

        // The first split entry began mid-record or mid-UTF-8 sequence.
        const partial = lines.shift() ?? "";
        // A record larger than the window has no complete line; return its read portion with `TRUNCATED_RECORD_MARKER`.
        if (!lines.some((line) => line !== "")) {
            lines.unshift(`${TRUNCATED_RECORD_MARKER}${partial.replace(/^\uFFFD+/, "")}`);
        }
        return lines;
    } finally {
        closeSync(fd);
    }
}
