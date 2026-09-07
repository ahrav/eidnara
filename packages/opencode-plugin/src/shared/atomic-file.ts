import { closeSync, constants, fchmodSync, openSync, renameSync, rmSync, statSync } from "node:fs";
import { writeAllSync } from "./write-all";

/**
 * Staging plus rename keeps a write failure from leaving `filePath` empty or partial.
 * `O_EXCL | O_NOFOLLOW` makes a planted entry at the predictable staging name fail the open instead of being truncated through.
 * An existing file's mode is applied with `fchmod` because the create mode is filtered through the umask.
 */
export function writeFileAtomicSync(filePath: string, data: string): void {
    const existing = statSync(filePath, { throwIfNoEntry: false });
    const existingMode = existing?.isFile() ? existing.mode & 0o777 : null;
    const { O_WRONLY, O_CREAT, O_EXCL, O_NOFOLLOW } = constants;
    const tmpPath = `${filePath}.${process.pid}.tmp`;
    const fd = openSync(tmpPath, O_WRONLY | O_CREAT | O_EXCL | (O_NOFOLLOW ?? 0), 0o666);
    try {
        try {
            if (existingMode !== null) fchmodSync(fd, existingMode);
            writeAllSync(fd, data);
        } finally {
            closeSync(fd);
        }
        renameSync(tmpPath, filePath);
    } catch (error) {
        rmSync(tmpPath, { force: true });
        throw error;
    }
}
