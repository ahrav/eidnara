#!/usr/bin/env bun

import { resolve } from "node:path";

const repoRoot = resolve(import.meta.dir, "../../..");

const SHORT_SOAK = "short_soak_keeps_fd_mapping_thread_and_rss_envelopes_bounded";
const LONG_SOAK = "long_soak_keeps_fd_mapping_thread_and_rss_envelopes_bounded";
const DEFAULT_HOURS = 5;

export type SoakInvocation = {
    command: string[];
    environment: Record<string, string>;
};

export type SoakResult = {
    exitCode: number | null;
    signalCode?: string | number | null;
};

export const USAGE = "Usage: run-shm-soak.ts [--smoke | --hours <n>]";

export function soakInvocation(args: string[]): SoakInvocation {
    let smoke = false;
    let hours: number | null = null;
    for (let index = 0; index < args.length; index += 1) {
        const arg = args[index];
        if (arg === "--smoke") {
            smoke = true;
        } else if (arg === "--hours") {
            index += 1;
            const value = index < args.length ? Number(args[index]) : Number.NaN;
            if (!Number.isFinite(value) || value <= 0) {
                throw new Error("--hours must be a positive number");
            }
            hours = value;
        } else {
            throw new Error(`unknown argument: ${arg}\n${USAGE}`);
        }
    }
    if (smoke && hours !== null) {
        throw new Error(`--smoke and --hours are mutually exclusive\n${USAGE}`);
    }
    hours ??= DEFAULT_HOURS;
    // `Math.round` maps values below 0.5 seconds to zero, which the test
    // binary rejects.
    if (!smoke && Math.round(hours * 3600) < 1) {
        throw new Error("--hours must resolve to at least one second");
    }

    // `--locked` keeps both soak variants on the committed dependency graph instead of resolving a new one.
    return {
        command: smoke
            ? ["cargo", "test", "--locked", "-p", "host-runtime", "--test", "shm_soak", SHORT_SOAK]
            : [
                  "cargo",
                  "test",
                  "--locked",
                  "--release",
                  "-p",
                  "host-runtime",
                  "--test",
                  "shm_soak",
                  LONG_SOAK,
                  "--",
                  "--ignored",
                  "--exact",
              ],
        environment: smoke ? {} : { EIDNARA_SHM_SOAK_SECONDS: String(Math.round(hours * 3600)) },
    };
}

/**
 * A signal-terminated child reports a null exit code. `process.exit(null)`
 * exits 0, so signal-terminated children return 1.
 */
export function exitStatus(result: SoakResult): number {
    if (typeof result.exitCode === "number") {
        return result.exitCode;
    }
    return 1;
}

if (import.meta.main) {
    const args = Bun.argv.slice(2);
    if (args.includes("--help") || args.includes("-h")) {
        console.log(USAGE);
        process.exit(0);
    }
    let invocation: SoakInvocation;
    try {
        invocation = soakInvocation(args);
    } catch (error) {
        console.error(error instanceof Error ? error.message : String(error));
        process.exit(2);
    }
    const result = Bun.spawnSync({
        cmd: invocation.command,
        cwd: repoRoot,
        env: { ...process.env, ...invocation.environment },
        stdout: "inherit",
        stderr: "inherit",
    });
    if (typeof result.exitCode !== "number") {
        console.error(`soak terminated by signal ${String(result.signalCode)}`);
    }
    process.exit(exitStatus(result));
}
