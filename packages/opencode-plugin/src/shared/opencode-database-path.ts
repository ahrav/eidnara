import { existsSync, readdirSync, statSync } from "node:fs";
import { join } from "node:path";
import { getDataDir } from "./data-path";

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
                return [{ path, mtimeMs: statSync(path).mtimeMs }];
            } catch {
                return [];
            }
        });

    return files.sort((left, right) => right.mtimeMs - left.mtimeMs).map((file) => file.path);
}

/** `OPENCODE_DB_PATH` takes precedence over the default database, channel-specific databases, and storage databases. */
export function resolveOpenCodeDatabasePath(dataDir: string = getDataDir()): string {
    const explicit = process.env.OPENCODE_DB_PATH;
    if (explicit && existsSync(explicit)) {
        return explicit;
    }

    const opencodeRoot = join(dataDir, "opencode");
    const defaultDb = join(opencodeRoot, "opencode.db");
    if (existsSync(defaultDb)) {
        return defaultDb;
    }

    const channelDbCandidates = listDatabaseFiles(opencodeRoot, "opencode");
    if (channelDbCandidates.length > 0) {
        return channelDbCandidates[0];
    }

    const storageDir = join(opencodeRoot, "storage");
    const storageDbCandidates = listDatabaseFiles(storageDir, "");
    if (storageDbCandidates.length > 0) {
        return storageDbCandidates[0];
    }

    throw new Error(
        `Unable to locate OpenCode DB. Checked ${defaultDb}, channel DBs in ${opencodeRoot}, and storage DBs in ${storageDir}`,
    );
}
