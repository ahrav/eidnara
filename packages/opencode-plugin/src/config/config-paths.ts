import { isAbsolute, join } from "node:path";

const CONFIG_FILE_BASENAME = "eidnara";

// Only an absolute home prevents user-tier paths from resolving relative to the process CWD.
function absoluteOrUndefined(value: string | undefined): string | undefined {
    return value && isAbsolute(value) ? value : undefined;
}

/** Only an absolute home from the environment yields a user tier; an unset or empty home does not fall back to the passwd entry. */
function homeDir(): string | undefined {
    const candidates =
        process.platform === "win32"
            ? [process.env.USERPROFILE, process.env.HOME]
            : [process.env.HOME];
    for (const candidate of candidates) {
        const absolute = absoluteOrUndefined(candidate);
        if (absolute) return absolute;
    }
    return undefined;
}

function configHome(): string | undefined {
    const xdg = absoluteOrUndefined(process.env.XDG_CONFIG_HOME);
    if (xdg) return xdg;
    const home = homeDir();
    return home === undefined ? undefined : join(home, ".config");
}

export function eidnaraUserConfigBasePath(): string | undefined {
    const home = configHome();
    return home === undefined ? undefined : join(home, "eidnara", CONFIG_FILE_BASENAME);
}

export function eidnaraProjectConfigBasePath(directory: string): string {
    return join(directory, ".eidnara", CONFIG_FILE_BASENAME);
}

export function resolveEidnaraUserConfigPath(): string | undefined {
    const base = eidnaraUserConfigBasePath();
    return base === undefined ? undefined : `${base}.jsonc`;
}

export function resolveEidnaraProjectConfigPath(directory: string): string {
    return `${eidnaraProjectConfigBasePath(directory)}.jsonc`;
}
