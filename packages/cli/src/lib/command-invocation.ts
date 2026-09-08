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

/** Windows treats environment-variable names case-insensitively; reusing the existing PATH key prevents a duplicate entry. */
function pathEnvKey(): string {
    return Object.keys(process.env).find((key) => key.toUpperCase() === "PATH") ?? "PATH";
}

/** The child process searches the launcher's directory for sibling runtime executables. */
export function childPathWithLauncherDir(binary: string, parentPath = process.env.PATH): string {
    const launcherDir = dirname(binary);
    return parentPath ? `${launcherDir}${delimiter}${parentPath}` : launcherDir;
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
): CommandInvocation {
    const env = { [pathEnvKey()]: childPathWithLauncherDir(binary) };
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
