import { existsSync, mkdirSync, writeFileSync } from "node:fs";
import { dirname } from "node:path";

/** Create `filePath`'s parent directory when absent. */
export function ensureParentDir(filePath: string): void {
    if (!existsSync(dirname(filePath))) {
        mkdirSync(dirname(filePath), { recursive: true });
    }
}

const MAX_NEW_FILE_NAME_ATTEMPTS = 100;

/**
 * Write `data` to `<stem>.md`, or to the first free `<stem>-<n>.md`, and return the path.
 *
 * Creation uses `wx`, so existing paths, including symlinks, remain unchanged and a second
 * report written in the same second keeps the first intact.
 */
export function writeNewFile(stem: string, data: string): string {
    for (let attempt = 1; attempt <= MAX_NEW_FILE_NAME_ATTEMPTS; attempt++) {
        const path = attempt === 1 ? `${stem}.md` : `${stem}-${attempt}.md`;
        try {
            writeFileSync(path, data, { flag: "wx" });
            return path;
        } catch (error) {
            if ((error as NodeJS.ErrnoException).code !== "EEXIST") throw error;
        }
    }
    throw new Error(
        `Could not find a free file name after ${MAX_NEW_FILE_NAME_ATTEMPTS} tries at ${stem}`,
    );
}
