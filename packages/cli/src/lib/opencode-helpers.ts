import { execFileSync, execSync } from "node:child_process";
import {
    type CommandInvocation,
    getCommandInvocation,
    invocationSpawnOptions,
} from "./command-invocation";
import type { OpenCodeInstallation } from "./opencode-detect";
import { standaloneVersion } from "./semver";

const OPENCODE_BINARY_ENV = "EIDNARA_OPENCODE_BINARY";

export function getOpenCodeCommandInvocation(binary: string, args: string[]): CommandInvocation {
    return getCommandInvocation(binary, args, OPENCODE_BINARY_ENV);
}

function runOpenCode(
    args: string[],
    binary: string | null | undefined,
    timeoutMs: number,
): string | null {
    try {
        const options = { stdio: "pipe" as const, timeout: timeoutMs };
        if (binary) {
            const invocation = getOpenCodeCommandInvocation(binary, args);
            return execFileSync(invocation.command, invocation.args, {
                ...options,
                ...invocationSpawnOptions(invocation),
            })
                .toString()
                .trim();
        }
        return execSync(`opencode ${args.join(" ")}`, options)
            .toString()
            .trim();
    } catch {
        return null;
    }
}

/**
 * A 2,000 ms timeout prevents broken shims from blocking version probes indefinitely.
 */
export const OPENCODE_VERSION_PROBE_TIMEOUT_MS = 2_000;

/**
 * Model discovery loads provider catalogs, so it gets the same 20,000 ms bound as the Pi probe.
 */
export const OPENCODE_MODELS_PROBE_TIMEOUT_MS = 20_000;

export function getOpenCodeVersion(binary?: string | null): string | null {
    const output = runOpenCode(["--version"], binary, OPENCODE_VERSION_PROBE_TIMEOUT_MS);
    return output ? output : null;
}

export interface OpenCodeInstallationReport extends OpenCodeInstallation {
    version: string;
    active: boolean;
}

/** The report preserves detection order and probes only CLI installations. */
export function describeOpenCodeInstallations(
    installations: OpenCodeInstallation[],
): OpenCodeInstallationReport[] {
    return installations.map((installation, index) => ({
        ...installation,
        // Only the parsed semver enters the report; a wrapper's stdout can carry warnings naming paths or credentials.
        version:
            installation.kind === "cli"
                ? (standaloneVersion(getOpenCodeVersion(installation.path)) ?? "unknown")
                : "unknown",
        active: index === 0,
    }));
}

export function getAvailableModels(
    binary?: string | null,
    timeoutMs = OPENCODE_MODELS_PROBE_TIMEOUT_MS,
): string[] {
    const output = runOpenCode(["models"], binary, timeoutMs);
    if (output === null) return [];
    return output
        .split("\n")
        .map((l) => l.trim())
        .filter(Boolean);
}
