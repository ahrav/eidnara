import { spawnSync } from "node:child_process";
import { existsSync } from "node:fs";
import { dirname, extname, join } from "node:path";
import { readRegularFileSync } from "@eidnara/opencode/shared/regular-file";
import {
    type CommandInvocation,
    getCommandInvocation,
    invocationSpawnOptions,
} from "./command-invocation";
import {
    findBunRuntime,
    findOnPath,
    isExecutableFile,
    packageManagerBinCandidates,
} from "./find-on-path";
import { absoluteHomeDir, getOmpPackageDir } from "./paths";
import { standaloneVersion } from "./semver";
export interface OmpBinaryInfo {
    path: string;
    source: "path" | "home" | "package";
}

export interface OmpCommandResult {
    ok: boolean;
    stdout: string;
    stderr: string;
}

export interface OmpPluginInfo {
    name: string;
    version: string;
    enabled: boolean;
    path?: string;
}

export const OMP_PLUGIN_PACKAGE = "@eidnara/pi";

const OMP_BINARY_ENV = "EIDNARA_OMP_BINARY";
const BUN_BINARY_ENV = "EIDNARA_BUN_BINARY";

/**
 * OMP publishes its CLI as a Bun script (`#!/usr/bin/env bun`), not a native executable.
 * Windows does not execute shebangs.
 * A bare `dist/cli.js` path requires Bun to run it.
 */
function detectOmpPackageCli(): string | null {
    const packageDir = getOmpPackageDir();
    if (!packageDir) return null;
    if (!findBunRuntime()) return null;
    try {
        const manifest = JSON.parse(readRegularFileSync(join(packageDir, "package.json"))) as {
            name?: unknown;
        };
        if (manifest.name !== "@oh-my-pi/pi-coding-agent") return null;
        const cli = join(packageDir, "dist", "cli.js");
        return existsSync(cli) ? cli : null;
    } catch {
        return null;
    }
}
export function getOmpCommandInvocation(ompPath: string, args: string[]): CommandInvocation {
    const bun = findBunRuntime();
    // A Bun found as an npm `.cmd` shim needs cmd.exe like any other shim.
    if (bun && extname(ompPath).toLowerCase() === ".js") {
        return getCommandInvocation(bun, [ompPath, ...args], BUN_BINARY_ENV);
    }
    // An extensionless launcher starts with `#!/usr/bin/env bun`, which resolves Bun through the child PATH.
    const runtimeDirs = bun && extname(ompPath) === "" ? [dirname(bun)] : [];
    return getCommandInvocation(ompPath, args, OMP_BINARY_ENV, runtimeDirs);
}

export function getOmpFallbackCandidates(
    platform: NodeJS.Platform,
    home: string | undefined,
    appData?: string,
): string[] {
    return packageManagerBinCandidates("omp", platform, home, appData);
}

export function detectOmpBinary(): OmpBinaryInfo | null {
    const fromPath = findOnPath("omp");
    if (fromPath) return { path: fromPath, source: "path" };

    const fromPackage = detectOmpPackageCli();
    if (fromPackage) return { path: fromPackage, source: "package" };

    const candidates = getOmpFallbackCandidates(
        process.platform,
        absoluteHomeDir(),
        process.env.APPDATA,
    );
    const candidate = candidates.find((path) => isExecutableFile(path));
    return candidate ? { path: candidate, source: "home" } : null;
}

export function runOmpCommand(ompPath: string, args: string[], timeout = 30_000): OmpCommandResult {
    try {
        const invocation = getOmpCommandInvocation(ompPath, args);
        const result = spawnSync(invocation.command, invocation.args, {
            encoding: "utf-8",
            timeout,
            maxBuffer: 10 * 1024 * 1024,
            stdio: ["ignore", "pipe", "pipe"],
            ...invocationSpawnOptions(invocation),
        });
        return {
            ok: result.status === 0 && !result.error,
            stdout: result.stdout?.trim() ?? "",
            stderr: result.stderr?.trim() || result.error?.message || "",
        };
    } catch (error) {
        return {
            ok: false,
            stdout: "",
            stderr: error instanceof Error ? error.message : String(error),
        };
    }
}

export function getOmpVersion(ompPath: string): string | null {
    const result = runOmpCommand(ompPath, ["--version"], 10_000);
    if (!result.ok) return null;
    // OMP prints `omp/X.Y.Z`; only a line that is nothing but that counts, so a
    // wrapper warning quoting another tool's version cannot pass as OMP's.
    return standaloneVersion(result.stdout || result.stderr, "omp/");
}

export function parseOmpModelsOutput(output: string): string[] {
    try {
        const parsed = JSON.parse(output) as { models?: unknown };
        if (!Array.isArray(parsed.models)) return [];
        const models = new Set<string>();
        for (const entry of parsed.models) {
            if (!entry || typeof entry !== "object") continue;
            const value = entry as Record<string, unknown>;
            if (typeof value.selector === "string" && value.selector.length > 0) {
                models.add(value.selector);
                continue;
            }
            if (
                typeof value.provider === "string" &&
                value.provider.length > 0 &&
                typeof value.id === "string" &&
                value.id.length > 0
            ) {
                models.add(`${value.provider}/${value.id}`);
            }
        }
        return [...models];
    } catch {
        return [];
    }
}

export function getOmpAvailableModels(ompPath: string): string[] {
    const result = runOmpCommand(ompPath, ["models", "--json"], 30_000);
    return result.ok ? parseOmpModelsOutput(result.stdout) : [];
}

export function listOmpPlugins(ompPath: string): OmpPluginInfo[] | null {
    const result = runOmpCommand(ompPath, ["plugin", "list", "--json"], 30_000);
    if (!result.ok) return null;
    try {
        const parsed: unknown = JSON.parse(result.stdout);
        // A payload without an `npm` array is treated as unknown, not as an
        // empty plugin list, so callers fail closed instead of assuming absence.
        if (parsed === null || typeof parsed !== "object") return null;
        const npm = (parsed as { npm?: unknown }).npm;
        if (!Array.isArray(npm)) return null;
        const plugins: OmpPluginInfo[] = [];
        for (const entry of npm) {
            // A row that does not fit the schema makes the whole probe unknown;
            // dropping malformed entries could report present plugins as absent.
            if (!entry || typeof entry !== "object") return null;
            const value = entry as Record<string, unknown>;
            if (typeof value.name !== "string" || typeof value.version !== "string") return null;
            if (value.enabled !== undefined && typeof value.enabled !== "boolean") return null;
            plugins.push({
                name: value.name,
                version: value.version,
                enabled: value.enabled !== false,
                ...(typeof value.path === "string" ? { path: value.path } : {}),
            });
        }
        return plugins;
    } catch {
        return null;
    }
}

export function getOmpSetting(ompPath: string, key: "compaction.enabled"): boolean | null;
export function getOmpSetting(ompPath: string, key: "memory.backend"): string | null;
export function getOmpSetting(
    ompPath: string,
    key: "compaction.enabled" | "memory.backend",
): boolean | string | null {
    const result = runOmpCommand(ompPath, ["config", "get", key, "--json"], 10_000);
    if (!result.ok) return null;
    try {
        const parsed = JSON.parse(result.stdout) as { value?: unknown };
        const expected = key === "compaction.enabled" ? "boolean" : "string";
        return typeof parsed.value === expected ? (parsed.value as boolean | string) : null;
    } catch {
        return null;
    }
}
