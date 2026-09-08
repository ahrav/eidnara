import { randomBytes } from "node:crypto";
import {
    chmodSync,
    lstatSync,
    mkdirSync,
    readlinkSync,
    realpathSync,
    renameSync,
    rmSync,
    statSync,
    writeFileSync,
} from "node:fs";
import { dirname, resolve } from "node:path";

/**
 * An existing symlink resolves to its target, so `renameSync` replaces the target, not the link.
 * A dangling symlink resolves to its missing target, preserving the link.
 * The staged file is created no more permissively than the existing regular file.
 * A failed write or rename removes the staged sibling before the error propagates.
 * Callers need not create the parent directory.
 */
export function writeFileAtomic(targetPath: string, data: string): void {
    const finalPath = resolveLinkTarget(targetPath);
    mkdirSync(dirname(finalPath), { recursive: true });
    const mode = existingFileMode(finalPath);
    const tmpPath = `${finalPath}.${process.pid}.${randomBytes(6).toString("hex")}.tmp`;
    try {
        writeFileSync(tmpPath, data, { encoding: "utf-8", mode: mode ?? 0o666 });
        // open(2) masks the creation mode with the umask; chmod restores the exact bits.
        if (mode !== undefined) chmodSync(tmpPath, mode);
        renameSync(tmpPath, finalPath);
    } catch (error) {
        rmSync(tmpPath, { force: true });
        throw error;
    }
}

const MAX_LINK_HOPS = 32;

function resolveLinkTarget(path: string): string {
    try {
        return realpathSync.native(path);
    } catch {
        // realpath rejects a dangling link, so the chain is followed by hand to its missing end.
        // A chain that never reaches a non-link is an error: renaming over the unresolved entry would replace the user's link with a file.
        const seen = new Set<string>();
        let current = path;
        for (let hop = 0; hop < MAX_LINK_HOPS; hop++) {
            let link: string;
            try {
                const entry = lstatSync(current, { throwIfNoEntry: false });
                if (!entry?.isSymbolicLink()) return current;
                link = readlinkSync(current);
            } catch {
                return current;
            }
            if (seen.has(current)) {
                throw new Error(
                    `symlink cycle while resolving config target ${path} at ${current}`,
                );
            }
            seen.add(current);
            current = resolve(dirname(current), link);
        }
        throw new Error(
            `symlink chain for config target ${path} exceeds ${MAX_LINK_HOPS} hops at ${current}`,
        );
    }
}

function existingFileMode(path: string): number | undefined {
    try {
        const stat = statSync(path, { throwIfNoEntry: false });
        return stat?.isFile() ? stat.mode & 0o777 : undefined;
    } catch {
        return undefined;
    }
}
