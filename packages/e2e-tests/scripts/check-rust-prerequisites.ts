#!/usr/bin/env bun

import { spawnSync } from "node:child_process";
import { existsSync } from "node:fs";
import { join, resolve } from "node:path";
import { probeCapabilities } from "@eidnara/shm-native";
import {
    cargoBuildExampleArgs,
    DAEMON_EXAMPLES,
    type DaemonExample,
    invalidOverrideMessage,
    isExecutableFile,
    missingExampleTargets,
    prebuiltOverride,
    workspaceExampleBinary,
} from "../src/rust-runner/daemon-examples";

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
    /** Resolved example binaries keyed by their `prebuiltEnv` name, ready to export into a child's environment. */
    binaries: Record<string, string>;
}

function pathCommand(command: string, pathEnv: string | undefined): string | undefined {
    for (const directory of (pathEnv ?? "").split(":").filter(Boolean)) {
        const candidate = join(directory, command);
        if (isExecutableFile(candidate)) return candidate;
    }
    return undefined;
}

/** `null` if Cargo metadata fails; otherwise examples absent from its metadata. */
function unavailableExamples(
    cargo: string,
    repoRoot: string,
    env: NodeJS.ProcessEnv,
): DaemonExample[] | null {
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
    if (result.error || result.status !== 0 || typeof result.stdout !== "string") return null;
    return missingExampleTargets(result.stdout);
}

function buildExample(
    cargo: string,
    repoRoot: string,
    env: NodeJS.ProcessEnv,
    example: DaemonExample,
): boolean {
    const result = spawnSync(cargo, cargoBuildExampleArgs(example, join(repoRoot, "Cargo.toml")), {
        cwd: repoRoot,
        env,
        stdio: "inherit",
    });
    return !result.error && result.status === 0;
}

export function detectRustPrerequisites(
    options: RustPrerequisiteOptions = {},
): RustPrerequisiteResult {
    const repoRoot = resolve(options.repoRoot ?? resolve(import.meta.dir, "../../.."));
    const env = options.env ?? process.env;
    const missing: string[] = [];
    const binaries: Record<string, string> = {};
    const cargo = pathCommand("cargo", env.PATH);
    const manifest = join(repoRoot, "Cargo.toml");

    const unresolved: DaemonExample[] = [];
    for (const example of DAEMON_EXAMPLES) {
        const override = prebuiltOverride(env, example);
        if (override.kind === "invalid") {
            missing.push(invalidOverrideMessage(example, override.path));
            continue;
        }
        // A compiled workspace binary lets callers that forbid building resolve a usable one.
        const workspace = workspaceExampleBinary(repoRoot, example);
        const path =
            override.kind === "file"
                ? override.path
                : isExecutableFile(workspace)
                  ? workspace
                  : undefined;
        if (path) binaries[example.prebuiltEnv] = path;
        else unresolved.push(example);
    }

    if (!existsSync(manifest)) {
        missing.push(`cargo workspace: missing ${manifest}`);
    } else if (!cargo) {
        missing.push("cargo workspace: cargo is not available on PATH");
    } else {
        const unavailable = unavailableExamples(cargo, repoRoot, env);
        if (unavailable === null) {
            missing.push("cargo workspace: metadata does not resolve");
        } else if (unavailable.length > 0) {
            for (const example of unavailable) {
                missing.push(`cargo workspace: ${example.example} example is unavailable`);
            }
        } else if (options.allowBuild) {
            for (const example of unresolved) {
                const workspace = workspaceExampleBinary(repoRoot, example);
                if (buildExample(cargo, repoRoot, env, example) && isExecutableFile(workspace)) {
                    binaries[example.prebuiltEnv] = workspace;
                } else {
                    missing.push(`${example.example} example build failed`);
                }
            }
        }
    }
    const channel = (options.channelProbe ?? probeCapabilities)();
    if (!channel.available) {
        missing.push(
            `shared-memory channel unavailable on this runtime: ${channel.reason ?? "unknown"}`,
        );
    }

    return { ok: missing.length === 0, missing, binaries };
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
        if (print) {
            for (const example of DAEMON_EXAMPLES) {
                const path = result.binaries[example.prebuiltEnv] ?? "build-on-demand";
                console.log(`${example.prebuiltEnv}=${path}`);
            }
        } else console.log("Rust e2e direct-host prerequisites resolved");
    } catch (error) {
        console.error(`Rust prerequisite detector failed: ${String(error)}`);
        process.exit(1);
    }
}
