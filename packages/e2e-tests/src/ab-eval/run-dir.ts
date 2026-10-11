import {
    closeSync,
    mkdirSync,
    openSync,
    readdirSync,
    renameSync,
    rmSync,
    symlinkSync,
} from "node:fs";
import { join } from "node:path";

/**
 * The directory must be missing or empty, and the claim file is created exclusively, so two runs
 * that name the same directory at the same moment cannot both proceed.
 */
export function claimRunDir(out: string): void {
    mkdirSync(out, { recursive: true });
    const entries = readdirSync(out);
    if (entries.length > 0) {
        throw new Error(
            `${out} already holds ${entries.length} entries from an earlier run; pass a fresh --out`,
        );
    }
    try {
        closeSync(openSync(join(out, ".claimed"), "wx"));
    } catch {
        throw new Error(`${out} was claimed by another run; pass a fresh --out`);
    }
}

export function timestampedRunDir(base: string, now: Date = new Date()): string {
    return join(base, now.toISOString().replace(/[:.]/g, "-"));
}

/** `rename` replaces an existing `latest` symlink atomically. */
export function pointLatest(base: string, dir: string): void {
    const staged = join(base, `.latest-${process.pid}`);
    rmSync(staged, { force: true });
    symlinkSync(dir, staged);
    renameSync(staged, join(base, "latest"));
}
