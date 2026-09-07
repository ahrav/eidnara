import { writeSync } from "node:fs";

/** `writeSync` can write fewer bytes than requested; the loop resumes at the returned offset and rejects zero-byte writes. */
export function writeAllSync(fd: number, data: string): void {
    const bytes = Buffer.from(data, "utf8");
    let offset = 0;
    while (offset < bytes.length) {
        const written = writeSync(fd, bytes, offset, bytes.length - offset);
        if (written <= 0) {
            throw new Error(`write made no progress at byte ${offset} of ${bytes.length}`);
        }
        offset += written;
    }
}
