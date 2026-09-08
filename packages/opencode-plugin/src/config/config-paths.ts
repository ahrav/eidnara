import { homedir } from "node:os";
import { isAbsolute, join } from "node:path";
import { detectConfigFile } from "../shared/jsonc-parser";

const CONFIG_FILE_BASENAME = "eidnara";

function homeDir(): string {
    if (process.platform === "win32") {
        return process.env.USERPROFILE || process.env.HOME || homedir();
    }
    return process.env.HOME || homedir();
}

function configHome(): string {
    const xdg = process.env.XDG_CONFIG_HOME;
    if (xdg && isAbsolute(xdg)) return xdg;
    return join(homeDir(), ".config");
}

/** `~/.config/eidnara/eidnara` (no extension, for `detectConfigFile`). */
export function eidnaraUserConfigBasePath(): string {
    return join(configHome(), "eidnara", CONFIG_FILE_BASENAME);
}

/** `<root>/.eidnara/eidnara` (no extension, for `detectConfigFile`). */
export function eidnaraProjectConfigBasePath(directory: string): string {
    return join(directory, ".eidnara", CONFIG_FILE_BASENAME);
}

/** Returns an existing `.jsonc` path, then `.json`, or the `.jsonc` path for a new file. */
export function resolveEidnaraUserConfigPath(): string {
    return detectConfigFile(eidnaraUserConfigBasePath()).path;
}

export function resolveEidnaraProjectConfigPath(directory: string): string {
    return detectConfigFile(eidnaraProjectConfigBasePath(directory)).path;
}
