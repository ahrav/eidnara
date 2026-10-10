import { closeSync, constants, fstatSync, openSync, readFileSync, readSync } from "node:fs";
import { open } from "node:fs/promises";

/**
 * A FIFO without a writer blocks a blocking read-only open; O_NONBLOCK lets the function reject it after fstat.
 * Callers see `ENOENT` for missing paths and an `Error` for non-regular files.
 */
export function readRegularFileSync(filePath: string, maxBytes?: number): string {
    const { O_RDONLY, O_NONBLOCK } = constants;
    const fd = openSync(filePath, O_RDONLY | (O_NONBLOCK ?? 0));
    try {
        const stat = fstatSync(fd);
        if (!stat.isFile()) {
            throw new Error(`not a regular file: ${filePath}`);
        }
        if (maxBytes === undefined) return readFileSync(fd, "utf8");
        if (stat.size > maxBytes) {
            throw new Error(`file exceeds ${maxBytes} bytes: ${filePath}`);
        }
        const buffer = Buffer.allocUnsafe(stat.size + 1);
        let length = 0;
        while (length < buffer.length) {
            const count = readSync(fd, buffer, length, buffer.length - length, length);
            if (count === 0) break;
            length += count;
        }
        if (length > stat.size) {
            throw new Error(`file grew while it was read: ${filePath}`);
        }
        return buffer.toString("utf8", 0, length);
    } finally {
        closeSync(fd);
    }
}

export async function readRegularFile(filePath: string): Promise<string> {
    const { O_RDONLY, O_NONBLOCK } = constants;
    const handle = await open(filePath, O_RDONLY | (O_NONBLOCK ?? 0));
    try {
        if (!(await handle.stat()).isFile()) {
            throw new Error(`not a regular file: ${filePath}`);
        }
        return await handle.readFile("utf8");
    } finally {
        await handle.close();
    }
}
