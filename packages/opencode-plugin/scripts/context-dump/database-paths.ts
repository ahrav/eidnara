import { existsSync, readdirSync, statSync } from "node:fs";
import { join } from "node:path";
import { getDataDir, getOpenCodeStorageDir } from "../../src/shared/data-path";

/**
 * Committed writes land in the `-wal` sidecar until a checkpoint, so the main
 * file's mtime can lag an active database.
 */
function lastActivityMs(dbPath: string): number {
    let latest = statSync(dbPath).mtimeMs;
    const wal = `${dbPath}-wal`;
    if (existsSync(wal)) latest = Math.max(latest, statSync(wal).mtimeMs);
    return latest;
}

function listDatabaseFiles(dirPath: string, filePrefix: string): string[] {
    if (!existsSync(dirPath)) {
        return [];
    }

    const files = readdirSync(dirPath)
        .filter((file) => file.endsWith(".db") && file.startsWith(filePrefix))
        .map((file) => join(dirPath, file));

    return files.sort((left, right) => lastActivityMs(right) - lastActivityMs(left));
}

export function resolveOpenCodeDatabasePath(): string {
    const explicit = process.env.OPENCODE_DB_PATH;
    if (explicit) {
        if (!existsSync(explicit)) {
            throw new Error(`OPENCODE_DB_PATH is set to ${explicit}, which does not exist`);
        }
        return explicit;
    }

    const dataDir = getDataDir();
    const opencodeRoot = join(dataDir, "opencode");

    // `opencode.db` competes with the channel databases (`opencode-beta.db`, ...) on
    // last activity, so a stale stable database does not shadow the active channel.
    const channelDbCandidates = listDatabaseFiles(opencodeRoot, "opencode");
    if (channelDbCandidates.length > 0) {
        return channelDbCandidates[0];
    }

    const storageDbCandidates = listDatabaseFiles(getOpenCodeStorageDir(), "");
    if (storageDbCandidates.length > 0) {
        return storageDbCandidates[0];
    }

    throw new Error(
        `Unable to locate OpenCode DB. Checked opencode*.db in ${opencodeRoot} and storage DBs in ${getOpenCodeStorageDir()}`,
    );
}
