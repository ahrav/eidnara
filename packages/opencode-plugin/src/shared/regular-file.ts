import { closeSync, constants, fstatSync, openSync, readFileSync } from "node:fs";
import { open } from "node:fs/promises";

/**
 * A FIFO without a writer blocks a blocking read-only open; O_NONBLOCK lets the function reject it after fstat.
 * Callers see `ENOENT` for missing paths and an `Error` for non-regular files.
 */
export function readRegularFileSync(filePath: string): string {
    const { O_RDONLY, O_NONBLOCK } = constants;
    const fd = openSync(filePath, O_RDONLY | (O_NONBLOCK ?? 0));
    try {
        if (!fstatSync(fd).isFile()) {
            throw new Error(`not a regular file: ${filePath}`);
        }
        return readFileSync(fd, "utf8");
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
