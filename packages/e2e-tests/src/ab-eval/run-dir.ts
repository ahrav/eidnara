import { mkdirSync, readdirSync, renameSync, rmSync, symlinkSync } from "node:fs";
import { join } from "node:path";

export function claimRunDir(out: string): void {
    mkdirSync(out, { recursive: true });
    const entries = readdirSync(out);
    if (entries.length > 0) {
        throw new Error(
            `${out} already holds ${entries.length} entries from an earlier run; pass a fresh --out`,
        );
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
