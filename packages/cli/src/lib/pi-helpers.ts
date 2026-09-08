import { execFileSync, spawnSync } from "node:child_process";
import { readFileSync } from "node:fs";
import { homedir } from "node:os";
import { join, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import {
    type CommandInvocation,
    getCommandInvocation,
    invocationSpawnOptions,
} from "./command-invocation";
import { findOnPath, isExecutableFile, packageManagerBinCandidates } from "./find-on-path";

export interface PiBinaryInfo {
    path: string;
    source: "path" | "home";
}

export const PI_PACKAGE_NAME = "@eidnara/pi";
export const PI_PACKAGE_SOURCE = `npm:${PI_PACKAGE_NAME}`;

/** The `source` string of a Pi `packages[]` entry, or `null` for an unrecognized shape. */
export function piPackageEntrySource(entry: unknown): string | null {
    if (typeof entry === "string") return entry;
    if (
        entry &&
        typeof entry === "object" &&
        typeof (entry as { source?: unknown }).source === "string"
    ) {
        return (entry as { source: string }).source;
    }
    return null;
}

/** Pi treats sources without these prefixes as filesystem paths. */
const NON_LOCAL_SOURCE_PREFIXES = ["npm:", "git:", "github:", "http:", "https:", "ssh:"];

/** Pi expands `~`, accepts `file://` URLs, and resolves relative paths against the scope directory. */
function resolveLocalPackagePath(source: string, baseDir: string): string {
    const trimmed = source.trim();
    if (trimmed === "~") return homedir();
    if (trimmed.startsWith("~/")) return join(homedir(), trimmed.slice(2));
    if (/^file:\/\//.test(trimmed)) return fileURLToPath(trimmed);
    return resolve(baseDir, trimmed);
}

/** Treats versioned npm specs and local checkouts of `@eidnara/pi` as the same package to prevent duplicate plugin loads. */
export function isEidnaraPiPackageEntry(entry: unknown, baseDir: string): boolean {
    const source = piPackageEntrySource(entry);
    if (source === null) return false;
    const trimmed = source.trim();
    if (trimmed.startsWith("npm:")) {
        const spec = trimmed.slice("npm:".length).trim();
        const name = /^(@?[^@]+(?:\/[^@]+)?)(?:@.+)?$/.exec(spec)?.[1];
        return name === PI_PACKAGE_NAME;
    }
    if (NON_LOCAL_SOURCE_PREFIXES.some((prefix) => trimmed.startsWith(prefix))) return false;
    try {
        const manifest = JSON.parse(
            readFileSync(join(resolveLocalPackagePath(trimmed, baseDir), "package.json"), "utf-8"),
        ) as { name?: unknown };
        return manifest.name === PI_PACKAGE_NAME;
    } catch {
        return false;
    }
}

const PI_BINARY_ENV = "EIDNARA_PI_BINARY";

export function getPiCommandInvocation(piPath: string, args: string[]): CommandInvocation {
    return getCommandInvocation(piPath, args, PI_BINARY_ENV);
}

/** The installer's `~/.pi/bin` comes first; the package-manager launchers follow. */
export function getPiFallbackCandidates(
    platform: NodeJS.Platform,
    home: string,
    appData?: string,
): string[] {
    const installerBinary =
        platform === "win32" ? join(home, ".pi", "bin", "pi.cmd") : join(home, ".pi", "bin", "pi");
    return [installerBinary, ...packageManagerBinCandidates("pi", platform, home, appData)];
}

export function detectPiBinary(): PiBinaryInfo | null {
    const fromPath = findOnPath("pi");
    if (fromPath) return { path: fromPath, source: "path" };

    const home = process.env.HOME?.trim() || homedir();
    const candidates = getPiFallbackCandidates(process.platform, home, process.env.APPDATA);
    const candidate = candidates.find((path) => isExecutableFile(path));
    return candidate ? { path: candidate, source: "home" } : null;
}

export function getPiVersion(piPath: string, timeout = 10_000): string | null {
    try {
        const invocation = getPiCommandInvocation(piPath, ["--version"]);
        const result = spawnSync(invocation.command, invocation.args, {
            encoding: "utf-8",
            timeout,
            ...invocationSpawnOptions(invocation),
        });
        // A failing executable can print a dependency's version to stderr;
        // only a clean exit's output is a Pi version.
        if (result.error || result.status !== 0) return null;
        const stdout = result.stdout?.trim();
        if (stdout) return stdout;
        const stderr = result.stderr?.trim();
        if (stderr) return stderr;
        return null;
    } catch {
        return null;
    }
}

export function runPiCommand(piPath: string, args: string[], timeout = 20_000): string | null {
    try {
        const invocation = getPiCommandInvocation(piPath, args);
        return execFileSync(invocation.command, invocation.args, {
            encoding: "utf-8",
            stdio: ["ignore", "pipe", "ignore"],
            timeout,
            ...invocationSpawnOptions(invocation),
        }).trim();
    } catch {
        return null;
    }
}

function stripAnsi(text: string): string {
    return text.replace(new RegExp(`${String.fromCharCode(27)}\\[[0-9;]*m`, "g"), "");
}

const PROVIDER_TOKEN = /^[a-z0-9][a-z0-9._-]*$/i;
// A leading `@` admits scoped model identifiers such as `@modal/qwen/model-v1`.
const MODEL_TOKEN = /^[a-z0-9@][a-z0-9._:/@-]*$/i;
const SIZE_TOKEN = /^(?:\d+(?:\.\d+)?[kmgt]?|-)$/i;
const CAPABILITY_TOKEN = /^(?:yes|no|true|false|-)$/i;

export function parseModelListOutput(output: string): string[] {
    const models = new Set<string>();
    let sawHeader = false;

    for (const rawLine of stripAnsi(output).split(/\r?\n/)) {
        const line = rawLine.trim();
        if (!line) continue;
        const cols = line.split(/\s+/);
        const lower = cols.map((column) => column.toLowerCase());

        if (
            lower[0] === "provider" &&
            lower[1] === "model" &&
            lower.some((column) => column.startsWith("context"))
        ) {
            sawHeader = true;
            continue;
        }
        if (!sawHeader || cols.length < 6) continue;

        const provider = cols[0] ?? "";
        const model = cols[1] ?? "";
        const metadata = cols.slice(-4);
        if (
            PROVIDER_TOKEN.test(provider) &&
            MODEL_TOKEN.test(model) &&
            SIZE_TOKEN.test(metadata[0] ?? "") &&
            SIZE_TOKEN.test(metadata[1] ?? "") &&
            CAPABILITY_TOKEN.test(metadata[2] ?? "") &&
            CAPABILITY_TOKEN.test(metadata[3] ?? "")
        ) {
            models.add(`${provider}/${model}`);
        }
    }
    return [...models];
}

/** The probe order avoids starting a second process when `--list-models` returns models. */
const MODEL_LIST_PROBES: readonly string[][] = [["--list-models"], ["models", "list"]];

export function getAvailableModels(piPath: string): string[] {
    for (const args of MODEL_LIST_PROBES) {
        const output = runPiCommand(piPath, args);
        if (!output) continue;
        const models = parseModelListOutput(output);
        if (models.length > 0) return models;
    }
    return [];
}

/**
 * The lowest Pi release the CLI certifies. It is the floor of the
 * `@earendil-works/pi-coding-agent` peer range that `@eidnara/pi` declares;
 * `pi-helpers.test.ts` fails when the two drift apart.
 */
export const PI_MINIMUM_VERSION = "0.80.2";
