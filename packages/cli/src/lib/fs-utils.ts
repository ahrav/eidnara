import { closeSync, existsSync, fstatSync, mkdirSync, openSync, readSync } from "node:fs";
import { dirname } from "node:path";

/** Create `filePath`'s parent directory when absent. */
export function ensureParentDir(filePath: string): void {
    if (!existsSync(dirname(filePath))) {
        mkdirSync(dirname(filePath), { recursive: true });
    }
}

/** Exclude leading partial lines so prefix-based redaction cannot miss a secret. */
export function readFileTail(filePath: string, maxBytes: number): string {
    const fd = openSync(filePath, "r");
    try {
        const size = fstatSync(fd).size;
        const offset = Math.max(0, size - maxBytes);
        const buffer = Buffer.alloc(size - offset);
        const read = readSync(fd, buffer, 0, buffer.length, offset);
        const text = buffer.toString("utf-8", 0, read);
        if (offset === 0) return text;
        const firstNewline = text.indexOf("\n");
        return firstNewline === -1 ? "" : text.slice(firstNewline + 1);
    } finally {
        closeSync(fd);
    }
}
