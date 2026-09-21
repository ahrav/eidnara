import { statSync } from "node:fs";
import { join } from "node:path";

export interface DaemonExample {
    /** The Cargo example target name, also the binary's file name under `target/debug/examples`. */
    readonly example: string;
    /** The `required-features` entry that gates the example in `crates/daemon/Cargo.toml`. */
    readonly feature: string;
    /** Environment variable for a prebuilt executable; invalid values produce `kind: "invalid"`. */
    readonly prebuiltEnv: string;
}

export const DIRECT_HOST_FIXTURE: DaemonExample = {
    example: "direct_host_fixture",
    feature: "direct-host-fixture",
    prebuiltEnv: "EIDNARA_E2E_DIRECT_HOST_FIXTURE_BIN",
};

export const EVAL_RUNNER: DaemonExample = {
    example: "eval_runner",
    feature: "eval-runner",
    prebuiltEnv: "EIDNARA_E2E_EVAL_RUNNER_BIN",
};

export const DAEMON_EXAMPLES: readonly DaemonExample[] = [DIRECT_HOST_FIXTURE, EVAL_RUNNER];

export function workspaceExampleBinary(repoRoot: string, example: DaemonExample): string {
    return join(repoRoot, "target/debug/examples", example.example);
}

export function isExecutableFile(path: string): boolean {
    try {
        const stat = statSync(path);
        return stat.isFile() && (stat.mode & 0o111) !== 0;
    } catch {
        return false;
    }
}

export type PrebuiltOverride =
    | { kind: "unset" }
    | { kind: "file"; path: string }
    | { kind: "invalid"; path: string };

export function prebuiltOverride(env: NodeJS.ProcessEnv, example: DaemonExample): PrebuiltOverride {
    const configured = env[example.prebuiltEnv];
    if (configured === undefined || configured === "") return { kind: "unset" };
    return isExecutableFile(configured)
        ? { kind: "file", path: configured }
        : { kind: "invalid", path: configured };
}

export function invalidOverrideMessage(example: DaemonExample, path: string): string {
    return `${example.prebuiltEnv}=${path} is not an executable file`;
}

/** Arguments for `cargo` that build one example; `manifestPath` pins the workspace when `cwd` does not. */
export function cargoBuildExampleArgs(example: DaemonExample, manifestPath?: string): string[] {
    const args = [
        "build",
        "-p",
        "daemon",
        "--example",
        example.example,
        "--features",
        example.feature,
        "--locked",
    ];
    return manifestPath ? [...args, "--manifest-path", manifestPath] : args;
}

interface CargoMetadata {
    packages?: Array<{
        name?: string;
        targets?: Array<{ name?: string; kind?: string[] }>;
    }>;
}

/** The examples from the table that `cargo metadata --no-deps` output does not list under `daemon`. */
export function missingExampleTargets(
    metadataJson: string,
    examples: readonly DaemonExample[] = DAEMON_EXAMPLES,
): DaemonExample[] {
    let metadata: CargoMetadata;
    try {
        metadata = JSON.parse(metadataJson) as CargoMetadata;
    } catch {
        return [...examples];
    }
    const targets = metadata.packages?.find((pkg) => pkg.name === "daemon")?.targets ?? [];
    return examples.filter(
        (example) =>
            !targets.some(
                (target) => target.name === example.example && target.kind?.includes("example"),
            ),
    );
}
