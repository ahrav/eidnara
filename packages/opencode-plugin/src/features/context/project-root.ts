import { existsSync, realpathSync } from "node:fs";
import path from "node:path";

function gitRootInAncestorChain(startDirectory: string): string | null {
    let current = startDirectory;
    while (true) {
        if (existsSync(path.join(current, ".git"))) {
            try {
                return realpathSync.native(current);
            } catch {
                return path.resolve(current);
            }
        }
        const parent = path.dirname(current);
        if (parent === current) {
            return null;
        }
        current = parent;
    }
}

function gitRootDirectory(canonical: string): string | null {
    const direct = gitRootInAncestorChain(canonical);
    if (direct) return direct;
    try {
        const realCanonical = realpathSync.native(canonical);
        return realCanonical === canonical ? null : gitRootInAncestorChain(realCanonical);
    } catch {
        return null;
    }
}

/** Answers the git worktree root containing `directory`, or the directory's canonical path when no `.git` is found, matching the daemon's canonical project root. commentlint: allow(JUDGE) */
export function resolveProjectRootDirectory(directory: string): string {
    const canonical = path.resolve(directory);
    const root = gitRootDirectory(canonical);
    if (root) return root;
    // Matches the daemon's `ProjectBinding`, which compares roots after `canonical_root` symlink resolution; a raw spelling would derive distinct import identities inside one daemon scope. commentlint: allow(JUDGE)
    try {
        return realpathSync.native(canonical);
    } catch {
        return canonical;
    }
}
