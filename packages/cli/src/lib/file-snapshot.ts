import { existsSync, lstatSync, mkdirSync, readFileSync, unlinkSync } from "node:fs";
import { dirname } from "node:path";
import { resolveLinkTarget, writeFileAtomic } from "./atomic-write";

interface FileSnapshot {
    /** The path the caller named, which may be a symlink. */
    path: string;
    /** The regular file a write through `path` lands on; equal to `path` when it is not a link. */
    target: string;
    /** `null` instructs `restoreFiles` to remove the target. */
    content: string | null;
}

/** Paths are deduplicated; a path that cannot be read is recorded as absent. */
export function snapshotFiles(paths: Iterable<string>): FileSnapshot[] {
    const seen = new Set<string>();
    const snapshots: FileSnapshot[] = [];
    for (const path of paths) {
        if (seen.has(path)) continue;
        seen.add(path);
        let target = path;
        let content: string | null = null;
        try {
            // Resolving the link preserves a dangling link: restore removes its target, not the link.
            if (lstatSync(path, { throwIfNoEntry: false })?.isSymbolicLink()) {
                target = resolveLinkTarget(path);
            }
            content = existsSync(target) ? readFileSync(target, "utf-8") : null;
        } catch {
            content = null;
        }
        snapshots.push({ path, target, content });
    }
    return snapshots;
}

/**
 * Restores snapshots in reverse order. Snapshots with `content === null` remove existing targets.
 * Restoration continues after failures, then throws an error listing paths that need manual repair.
 */
export function restoreFiles(snapshots: FileSnapshot[]): void {
    const failed: string[] = [];
    for (const snapshot of [...snapshots].reverse()) {
        try {
            if (snapshot.content === null) {
                if (existsSync(snapshot.target)) unlinkSync(snapshot.target);
            } else {
                mkdirSync(dirname(snapshot.target), { recursive: true });
                writeFileAtomic(snapshot.target, snapshot.content);
            }
        } catch {
            failed.push(snapshot.path);
        }
    }
    if (failed.length > 0) {
        throw new Error(`Could not restore: ${failed.join(", ")}`);
    }
}
