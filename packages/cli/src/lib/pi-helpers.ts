import { execFileSync, spawnSync } from "node:child_process";
import { homedir } from "node:os";
import { join } from "node:path";
import {
    type CommandInvocation,
    getCommandInvocation,
    invocationSpawnOptions,
} from "./command-invocation";
import { findOnPath, isExecutableFile } from "./find-on-path";

export interface PiBinaryInfo {
    path: string;
    source: "path" | "home";
}

export const PI_PACKAGE_SOURCE = "npm:@eidnara/pi";

const PI_BINARY_ENV = "EIDNARA_PI_BINARY";

export function getPiCommandInvocation(piPath: string, args: string[]): CommandInvocation {
    return getCommandInvocation(piPath, args, PI_BINARY_ENV);
}

export function detectPiBinary(): PiBinaryInfo | null {
    const fromPath = findOnPath("pi");
    if (fromPath) return { path: fromPath, source: "path" };

    const home = process.env.HOME?.trim() || homedir();
    const homeCandidate =
        process.platform === "win32"
            ? join(home, ".pi", "bin", "pi.cmd")
            : join(home, ".pi", "bin", "pi");
    if (isExecutableFile(homeCandidate)) return { path: homeCandidate, source: "home" };

    return null;
}

export function getPiVersion(piPath: string, timeout = 10_000): string | null {
    try {
        const invocation = getPiCommandInvocation(piPath, ["--version"]);
        const result = spawnSync(invocation.command, invocation.args, {
            encoding: "utf-8",
            timeout,
            ...invocationSpawnOptions(invocation),
        });
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
const MODEL_TOKEN = /^[a-z0-9][a-z0-9._:/-]*$/i;
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
