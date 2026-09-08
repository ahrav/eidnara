import { delimiter, dirname, extname } from "node:path";

export interface CommandInvocation {
    command: string;
    args: string[];
    /** Overrides for the child environment, merged over `process.env` at spawn time. */
    env?: Record<string, string>;
    /** The pre-quoted `/c` argument must not be quoted again by Node. */
    windowsVerbatimArguments?: true;
}

function isCommandInterpreterScript(binary: string): boolean {
    const extension = extname(binary).toLowerCase();
    return extension === ".cmd" || extension === ".bat";
}

/**
 * Windows treats environment-variable names case-insensitively, so reusing the
 * existing key prevents a duplicate entry. POSIX names are case-sensitive and
 * the child reads exactly `PATH`.
 */
export function pathEnvKey(
    env: NodeJS.ProcessEnv = process.env,
    platform: NodeJS.Platform = process.platform,
): string {
    if (platform !== "win32") return "PATH";
    return Object.keys(env).find((key) => key.toUpperCase() === "PATH") ?? "PATH";
}

/** The child process searches the launcher's directory, then any discovered runtime directories, for the interpreter an `env` shebang names. */
export function childPathWithLauncherDir(
    binary: string,
    parentPath = process.env.PATH,
    runtimeDirs: readonly string[] = [],
): string {
    const launcherDir = dirname(binary);
    const parentDirs = parentPath ? parentPath.split(delimiter) : [];
    const dirs = [
        launcherDir,
        ...runtimeDirs.filter((dir) => dir !== launcherDir && !parentDirs.includes(dir)),
    ];
    return (parentPath ? [...dirs, parentPath] : dirs).join(delimiter);
}

/**
 * cmd.exe removes outer `/c` quotes, so a separately quoted shim path splits
 * at spaces. The single pre-quoted `/c` argument keeps the shim path intact,
 * and `windowsVerbatimArguments` prevents Node from quoting it again.
 *
 * The shim path is read from `binaryEnvName` to prevent percent expansion;
 * `/v:off` prevents delayed expansion of exclamation marks.
 */
export function getCommandInvocation(
    binary: string,
    args: string[],
    binaryEnvName: string,
    runtimeDirs: readonly string[] = [],
): CommandInvocation {
    const env = { [pathEnvKey()]: childPathWithLauncherDir(binary, process.env.PATH, runtimeDirs) };
    if (!isCommandInterpreterScript(binary)) {
        return { command: binary, args, env };
    }

    const command = process.env.ComSpec?.trim() || process.env.COMSPEC?.trim() || "cmd.exe";
    const commandLine = [`%${binaryEnvName}%`, ...args].map((part) => `"${part}"`).join(" ");
    return {
        command,
        args: ["/d", "/s", "/v:off", "/c", `"${commandLine}"`],
        env: { ...env, [binaryEnvName]: binary },
        windowsVerbatimArguments: true,
    };
}

export function invocationSpawnOptions(invocation: CommandInvocation): {
    env?: NodeJS.ProcessEnv;
    windowsVerbatimArguments?: true;
} {
    return {
        ...(invocation.env ? { env: { ...process.env, ...invocation.env } } : {}),
        ...(invocation.windowsVerbatimArguments ? { windowsVerbatimArguments: true } : {}),
    };
}
