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
        // Drop the first split entry when `start > 0`: it may begin mid-record or mid-UTF-8 sequence.
        if (start > 0) lines.shift();
        return lines;
    } finally {
        closeSync(fd);
    }
}
