import { existsSync, mkdirSync, readFileSync, unlinkSync } from "node:fs";
import { dirname } from "node:path";
import { writeFileAtomic } from "./atomic-write";

interface FileSnapshot {
    path: string;
    /** `null` records that the file did not exist, so restore removes it. */
    content: string | null;
}

/** Paths are deduplicated; a path that cannot be read is recorded as absent. */
export function snapshotFiles(paths: Iterable<string>): FileSnapshot[] {
    const seen = new Set<string>();
    const snapshots: FileSnapshot[] = [];
    for (const path of paths) {
        if (seen.has(path)) continue;
        seen.add(path);
        let content: string | null = null;
        try {
            content = existsSync(path) ? readFileSync(path, "utf-8") : null;
        } catch {
            content = null;
        }
        snapshots.push({ path, content });
    }
    return snapshots;
}

/**
 * Restores snapshots in reverse order. Snapshots with `content === null` remove existing files.
 * Restoration continues after failures, then throws an error listing paths that need manual repair.
 */
export function restoreFiles(snapshots: FileSnapshot[]): void {
    const failed: string[] = [];
    for (const snapshot of [...snapshots].reverse()) {
        try {
            if (snapshot.content === null) {
                if (existsSync(snapshot.path)) unlinkSync(snapshot.path);
            } else {
                mkdirSync(dirname(snapshot.path), { recursive: true });
                writeFileAtomic(snapshot.path, snapshot.content);
            }
        } catch {
            failed.push(snapshot.path);
        }
    }
    if (failed.length > 0) {
        throw new Error(`Could not restore: ${failed.join(", ")}`);
    }
}
