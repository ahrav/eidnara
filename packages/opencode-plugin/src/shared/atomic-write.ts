import { chmodSync, renameSync, statSync, writeFileSync } from "node:fs";

/** `writeFileSync` can fail before `renameSync` without modifying `filePath`. */
export function writeFileAtomic(filePath: string, body: string): void {
    const tmpPath = `${filePath}.tmp`;
    writeFileSync(tmpPath, body);
    try {
        const existing = statSync(filePath, { throwIfNoEntry: false });
        if (existing?.isFile()) {
            chmodSync(tmpPath, existing.mode & 0o777);
        }
    } catch {
        /* If statSync fails, retain the temporary file's default mode. */
    }
    renameSync(tmpPath, filePath);
}
