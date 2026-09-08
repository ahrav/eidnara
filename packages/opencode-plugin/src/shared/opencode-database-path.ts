import { existsSync, readdirSync, statSync } from "node:fs";
import { join } from "node:path";
import { getDataDir } from "./data-path";

/**
 * Committed writes land in the `-wal` sidecar until a checkpoint, so the main
 * file's mtime can lag an active database.
 */
function lastActivityMs(dbPath: string): number {
    let latest = statSync(dbPath).mtimeMs;
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

/** An explicit `OPENCODE_DB_PATH` must exist; the discovery ladder ranks each directory's candidates by last activity. */
export function resolveOpenCodeDatabasePath(dataDir: string = getDataDir()): string {
    const explicit = process.env.OPENCODE_DB_PATH;
    if (explicit) {
        if (!existsSync(explicit)) {
            throw new Error(`OPENCODE_DB_PATH is set to ${explicit}, which does not exist`);
        }
        return explicit;
    }

    const opencodeRoot = join(dataDir, "opencode");
    const storageRoot = join(opencodeRoot, "storage");

    // `opencode.db` competes with the channel databases (`opencode-beta.db`, ...) on
    // last activity, so a stale stable database does not shadow the active channel.
    const channelDbCandidates = listDatabaseFiles(opencodeRoot, "opencode");
    if (channelDbCandidates.length > 0) {
        return channelDbCandidates[0];
    }

    const storageDbCandidates = listDatabaseFiles(storageRoot, "");
    if (storageDbCandidates.length > 0) {
        return storageDbCandidates[0];
    }

    throw new Error(
        `Unable to locate OpenCode DB. Checked opencode*.db in ${opencodeRoot} and storage DBs in ${storageRoot}`,
    );
}
