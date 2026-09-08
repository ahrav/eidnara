#!/usr/bin/env bun

import { resolve } from "node:path";
import { E2E_ROOT, filesForMode, type Mode, validateModeManifest } from "./validate-mode-manifest";

interface CliArgs {
    mode: Mode;
    timeoutMs: number;
    maxConcurrency: number | null;
}

/** Exit status for a malformed command line, distinct from a failing test run's status. */
export const USAGE_EXIT_CODE = 2;

export class UsageError extends Error {}

export function parseArgs(args: string[]): CliArgs {
    let mode: Mode | null = null;
    let timeoutMs = 120_000;
    let maxConcurrency: number | null = null;
    for (let index = 0; index < args.length; index += 1) {
        const arg = args[index];
        if (arg === "--mode") {
            const value = args[++index];
            if (value !== "rust") {
                throw new UsageError(`--mode accepts only rust; got ${JSON.stringify(value)}`);
            }
            mode = value;
        } else if (arg === "--timeout") {
            const value = Number(args[++index]);
            if (!Number.isInteger(value) || value <= 0) {
                throw new UsageError("--timeout requires positive milliseconds");
            }
            timeoutMs = value;
        } else if (arg === "--max-concurrency") {
            const value = Number(args[++index]);
            if (!Number.isInteger(value) || value <= 0) {
                throw new UsageError("--max-concurrency requires a positive integer");
            }
            maxConcurrency = value;
        } else if (arg === "--help" || arg === "-h") {
            console.log(
                "Usage: run-test-selection.ts --mode rust [--timeout <ms>] [--max-concurrency <n>]",
            );
            process.exit(0);
        } else {
            throw new UsageError(`unknown argument: ${arg}`);
        }
    }
    if (mode === null) throw new UsageError("--mode rust is required");
    return { mode, timeoutMs, maxConcurrency };
}

/** The manifest is the only source of the file list, so a test file cannot run unclassified. */
export function selectedTestFiles(mode: Mode): string[] {
    const files = filesForMode(validateModeManifest(), mode);
    if (files.length === 0) throw new Error(`${mode} selection is empty`);
    return files;
}

async function main(): Promise<number> {
    const args = parseArgs(Bun.argv.slice(2));
    const files = selectedTestFiles(args.mode);
    console.log(`Running ${files.length} selected test files`);
    const command = [
        process.execPath,
        "test",
        "--timeout",
        String(args.timeoutMs),
        ...(args.maxConcurrency === null ? [] : [`--max-concurrency=${args.maxConcurrency}`]),
        ...files,
    ];
    const child = Bun.spawn(command, {
        cwd: resolve(E2E_ROOT),
        env: { ...process.env, EIDNARA_E2E_MODE: args.mode },
        stdin: "inherit",
        stdout: "inherit",
        stderr: "inherit",
    });
    return await child.exited;
}

if (import.meta.main) {
    main()
        .then((code) => process.exit(code))
        .catch((error: unknown) => {
            console.error(
                `test selection failed: ${error instanceof Error ? error.message : String(error)}`,
            );
            process.exit(error instanceof UsageError ? USAGE_EXIT_CODE : 1);
        });
}
