#!/usr/bin/env bun

import { spawnSync } from "node:child_process";
import { existsSync, statSync } from "node:fs";
import { join, resolve } from "node:path";
import { probeCapabilities } from "@eidnara/shm-native";

/** The plugin reaches the daemon only through the shared-memory channel, so the channel probe is a prerequisite alongside Cargo. */
export type ChannelProbe = () => { available: boolean; reason?: string };

export interface RustPrerequisiteOptions {
    repoRoot?: string;
    allowBuild?: boolean;
    env?: NodeJS.ProcessEnv;
    /** Defaults to the shm-native capability probe; tests substitute a fixed verdict. */
    channelProbe?: ChannelProbe;
}

export interface RustPrerequisiteResult {
    ok: boolean;
    missing: string[];
    fixtureBin?: string;
}

function isExecutable(path: string): boolean {
    try {
        return statSync(path).isFile() && (statSync(path).mode & 0o111) !== 0;
    } catch {
        return false;
    }
}

function pathCommand(command: string, pathEnv: string | undefined): string | undefined {
    for (const directory of (pathEnv ?? "").split(":").filter(Boolean)) {
        const candidate = join(directory, command);
        if (isExecutable(candidate)) return candidate;
    }
    return undefined;
}

function cargoMetadata(cargo: string, repoRoot: string, env: NodeJS.ProcessEnv): boolean {
    // `--locked` makes a stale or missing `Cargo.lock` a detection failure instead of a lockfile rewrite.
    const result = spawnSync(
        cargo,
        [
            "metadata",
            "--locked",
            "--no-deps",
            "--format-version",
            "1",
            "--manifest-path",
            join(repoRoot, "Cargo.toml"),
        ],
        { env, encoding: "utf8" },
    );
    if (result.error || result.status !== 0 || typeof result.stdout !== "string") return false;
    try {
        const metadata = JSON.parse(result.stdout) as {
            packages?: Array<{
                name?: string;
                targets?: Array<{ name?: string; kind?: string[] }>;
            }>;
        };
        return (
            metadata.packages
                ?.find((pkg) => pkg.name === "daemon")
                ?.targets?.some(
                    (target) =>
                        target.name === "direct_host_fixture" && target.kind?.includes("example"),
                ) === true
        );
    } catch {
        return false;
    }
}

function buildFixture(cargo: string, repoRoot: string, env: NodeJS.ProcessEnv): boolean {
    const result = spawnSync(
        cargo,
        [
            "build",
            "-p",
            "daemon",
            "--example",
            "direct_host_fixture",
            "--features",
            "direct-host-fixture",
            "--locked",
            "--manifest-path",
            join(repoRoot, "Cargo.toml"),
        ],
        { cwd: repoRoot, env, stdio: "inherit" },
    );
    return !result.error && result.status === 0;
}

export function detectRustPrerequisites(
    options: RustPrerequisiteOptions = {},
): RustPrerequisiteResult {
    const repoRoot = resolve(options.repoRoot ?? resolve(import.meta.dir, "../../.."));
    const env = options.env ?? process.env;
    const missing: string[] = [];
    const cargo = pathCommand("cargo", env.PATH);
    const manifest = join(repoRoot, "Cargo.toml");
    const configured = env.EIDNARA_E2E_DIRECT_HOST_FIXTURE_BIN;
    const workspaceFixture = join(repoRoot, "target/debug/examples/direct_host_fixture");
    let fixtureBin = configured && isExecutable(configured) ? configured : undefined;
    // A compiled workspace fixture lets callers that forbid building resolve a usable binary.
    if (!fixtureBin && isExecutable(workspaceFixture)) fixtureBin = workspaceFixture;

    if (!existsSync(manifest)) {
        missing.push(`cargo workspace: missing ${manifest}`);
    } else if (!cargo) {
        missing.push("cargo workspace: cargo is not available on PATH");
    } else if (!cargoMetadata(cargo, repoRoot, env)) {
        missing.push("cargo workspace: direct_host_fixture example is unavailable");
    } else if (options.allowBuild && !fixtureBin) {
        if (buildFixture(cargo, repoRoot, env) && isExecutable(workspaceFixture)) {
            fixtureBin = workspaceFixture;
        } else {
            missing.push("direct host fixture build failed");
        }
    }
    const channel = (options.channelProbe ?? probeCapabilities)();
    if (!channel.available) {
        missing.push(
            `shared-memory channel unavailable on this runtime: ${channel.reason ?? "unknown"}`,
        );
    }

    return {
        ok: missing.length === 0,
        missing,
        ...(fixtureBin ? { fixtureBin } : {}),
    };
}

function parseArgs(args: string[]): { build: boolean; print: boolean } {
    let build = false;
    let print = false;
    for (const arg of args) {
        if (arg === "--build") build = true;
        else if (arg === "--print") print = true;
        else if (arg === "--help" || arg === "-h") {
            console.log("Usage: check-rust-prerequisites.ts [--build] [--print]");
            process.exit(0);
        } else throw new Error(`unknown argument: ${arg}`);
    }
    return { build, print };
}

if (import.meta.main) {
    try {
        const { build, print } = parseArgs(Bun.argv.slice(2));
        const result = detectRustPrerequisites({ allowBuild: build });
        if (!result.ok) {
            for (const reason of result.missing) console.error(`missing prerequisite: ${reason}`);
            process.exit(1);
        }
        if (print) console.log(result.fixtureBin ?? "build-on-demand");
        else console.log("Rust e2e direct-host prerequisites resolved");
    } catch (error) {
        console.error(`Rust prerequisite detector failed: ${String(error)}`);
        process.exit(1);
    }
}
