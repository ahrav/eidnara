import { existsSync, readdirSync, statSync } from "node:fs";
import { join } from "node:path";
import { getDataDir } from "./data-path";

/**
 * Committed writes land in the `-wal` sidecar until a checkpoint, so the main
 * file's mtime can lag an active database.
 */
function lastActivityMs(dbPath: string): number {
    const entry = statSync(dbPath);
    // SQLite's synchronous open would block on a FIFO, so only regular files are candidates.
    if (!entry.isFile()) throw new Error(`not a regular file: ${dbPath}`);
    let latest = entry.mtimeMs;
    try {
        // A checkpoint can delete the WAL before statSync runs; fall back to the main file's mtime.
        latest = Math.max(latest, statSync(`${dbPath}-wal`).mtimeMs);
    } catch (error) {
        if ((error as NodeJS.ErrnoException).code !== "ENOENT") throw error;
    }
    return latest;
}

function listDatabaseFiles(dirPath: string, filePrefix: string): string[] {
    if (!existsSync(dirPath)) {
        return [];
    }

    // A candidate removed between the directory read and its stat drops only itself.
    const files = readdirSync(dirPath)
        .filter((file) => file.endsWith(".db") && file.startsWith(filePrefix))
        .flatMap((file) => {
            const path = join(dirPath, file);
            try {
                return [{ path, activityMs: lastActivityMs(path) }];
            } catch {
                return [];
            }
        });

    return files.sort((left, right) => right.activityMs - left.activityMs).map((file) => file.path);
}

/**
 * Every database the discovery ladder would consider, most likely first: an explicit
 * `OPENCODE_DB_PATH` alone, otherwise `opencode*.db` under the data directory ranked by last
 * activity, then the storage databases. A caller that can test a candidate (open it and run a
 * query) walks this list so a stray `opencode-backup.db` does not hide the live database.
 */
export function resolveOpenCodeDatabaseCandidates(dataDir: string = getDataDir()): string[] {
    const explicit = process.env.OPENCODE_DB_PATH;
    if (explicit) {
        const entry = statSync(explicit, { throwIfNoEntry: false });
        if (entry === undefined) {
            throw new Error(`OPENCODE_DB_PATH is set to ${explicit}, which does not exist`);
        }
        if (!entry.isFile()) {
            throw new Error(`OPENCODE_DB_PATH is set to ${explicit}, which is not a regular file`);
        }
        return [explicit];
    }

    const opencodeRoot = join(dataDir, "opencode");
    const storageRoot = join(opencodeRoot, "storage");

    // `opencode.db` competes with the channel databases (`opencode-beta.db`, ...) on
    // last activity, so a stale stable database does not shadow the active channel.
    const candidates = [
        ...listDatabaseFiles(opencodeRoot, "opencode"),
        ...listDatabaseFiles(storageRoot, ""),
    ];
    if (candidates.length === 0) {
        throw new Error(
            `Unable to locate OpenCode DB. Checked opencode*.db in ${opencodeRoot} and storage DBs in ${storageRoot}`,
        );
    }
    return candidates;
}

/** The first candidate of `resolveOpenCodeDatabaseCandidates`. */
export function resolveOpenCodeDatabasePath(dataDir: string = getDataDir()): string {
    return resolveOpenCodeDatabaseCandidates(dataDir)[0];
}
