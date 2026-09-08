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

/** Answers the git worktree root containing `directory`, or the directory's canonical path when no `.git` is found, matching the daemon's canonical project root. commentlint: allow(JUDGE) */
export function resolveProjectRootDirectory(directory: string): string {
    const resolved = path.resolve(directory);
    // Git discovers the repository from the physical path, so a symlinked subtree beneath another checkout belongs to the link target's repository, not the outer one; walking the raw spelling first would find the outer `.git`. The daemon's `ProjectBinding` compares roots after the same `canonical_root` symlink resolution, so a raw spelling would also derive a distinct import identity inside one daemon scope. commentlint: allow(JUDGE)
    let canonical: string;
    try {
        canonical = realpathSync.native(resolved);
    } catch {
        // A path with a missing component has no physical spelling; the raw ancestor chain still finds an existing `.git`.
        return gitRootInAncestorChain(resolved) ?? resolved;
    }
    return gitRootInAncestorChain(canonical) ?? canonical;
}
