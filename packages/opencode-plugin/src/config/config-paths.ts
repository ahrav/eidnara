import { homedir } from "node:os";
import { isAbsolute, join } from "node:path";

const CONFIG_FILE_BASENAME = "eidnara";

/** The environment's home takes precedence over the account database, so a harness or test can point every home-relative path at a scratch directory. commentlint: allow(JUDGE) */
export function homeDir(): string {
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

export function resolveEidnaraUserConfigPath(): string {
    return `${eidnaraUserConfigBasePath()}.jsonc`;
}

export function resolveEidnaraProjectConfigPath(directory: string): string {
    return `${eidnaraProjectConfigBasePath(directory)}.jsonc`;
}
