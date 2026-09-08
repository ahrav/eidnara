import { extname } from "node:path";

export interface CommandInvocation {
    command: string;
    args: string[];
    /** Command-interpreter shims use this environment to pass their script path. */
    env?: Record<string, string>;
    /** The pre-quoted `/c` argument must not be quoted again by Node. */
    windowsVerbatimArguments?: true;
}

function isCommandInterpreterScript(binary: string): boolean {
    const extension = extname(binary).toLowerCase();
    return extension === ".cmd" || extension === ".bat";
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
    if (!isCommandInterpreterScript(binary)) {
        return { command: binary, args };
    }

    const command = process.env.ComSpec?.trim() || process.env.COMSPEC?.trim() || "cmd.exe";
    const commandLine = [`%${binaryEnvName}%`, ...args].map((part) => `"${part}"`).join(" ");
    return {
        command,
        args: ["/d", "/s", "/v:off", "/c", `"${commandLine}"`],
        env: { [binaryEnvName]: binary },
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
