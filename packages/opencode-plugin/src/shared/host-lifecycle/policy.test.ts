import { describe, expect, test } from "bun:test";
import { chmodSync, mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import * as os from "node:os";
import * as path from "node:path";
import hostRelease from "../../../../../release/host-release.json";
import type { CatalogEntry } from "../host-client";
import type { PlatformReaders } from "./bootstrap";
import { parseDaemonResult } from "./contract";
import { buildManagedCredentialEnvelope } from "./managed-policy";
import type { NativeStartupEnvelope } from "./native-launcher";
import {
    aggregateForTarget,
    HostLifecyclePolicy,
    OUTER_AGGREGATE_MS,
    STORAGE_HARD_BUDGET_MS,
    type WaiterDetachedError,
} from "./policy";

function tempDir(prefix: string): string {
    return mkdtempSync(path.join(os.tmpdir(), prefix));
}

/** A handshake-authenticated peer at a given daemon version. */
function authenticatedPeerAt(daemonVer: string) {
    return { daemonVer, daemonId: new Uint8Array([7]), proof: "current" as const };
}

let counter = 0;

function startResultJson(command: string): string {
    const mutating = command === "start" || command === "restart";
    return JSON.stringify({
        schema: "eidnara.daemon/v1",
        command,
        ok: true,
        state: "running",
        reason: mutating ? "started" : "healthy",
        remediation: null,
        // A successful restart must carry its commit evidence, so a fixture that
        // reported null here would be rejected by the parser and land as
        // `internal_error` — passing any test that only checks `command`.
        effects: command === "restart" ? { stop_committed: true, start_committed: true } : null,
        readiness: { transport: { state: "ready", reason: "healthy" } },
        checks: [],
        versions: {
            release: "0.38.0",
            // Only a successful start or restart authenticates the running code.
            proof: mutating ? "current" : null,
            daemon: "eidnara-host/0.1.0",
            context: null,
            synapse: null,
            broca: null,
        },
    });
}

function missingPayloadResultJson(): string {
    return JSON.stringify({
        schema: "eidnara.daemon/v1",
        command: "start",
        ok: false,
        state: "stopped",
        reason: "native_payload_missing",
        remediation: "install_native_payload",
        effects: null,
        readiness: null,
        checks: [],
        versions: {
            release: "0.38.0",
            proof: null,
            daemon: null,
            context: null,
            synapse: null,
            broca: null,
        },
    });
}

function restartResultJson(): string {
    return JSON.stringify({
        ...JSON.parse(startResultJson("restart")),
        reason: "started",
        effects: { stop_committed: true, start_committed: true },
    });
}

function harnessUnavailableResultJson(): string {
    const base = JSON.parse(startResultJson("start"));
    return JSON.stringify({
        ...base,
        ok: false,
        state: "running",
        reason: "harness_unavailable",
        readiness: null,
        // A failed start authenticates nothing.
        versions: { ...base.versions, proof: null },
    });
}

/** A fake eidnara-host recording each invocation and emitting one result. */
function fakeBinary(
    dir: string,
    options: { sleepSeconds?: number } = {},
): {
    binary: string;
    invocationLog: string;
} {
    counter += 1;
    const invocationLog = path.join(dir, `invocations-${counter}.log`);
    writeFileSync(invocationLog, "");
    const binary = path.join(dir, `fake-eidnara-host-${counter}.sh`);
    const sleep = options.sleepSeconds ? `sleep ${options.sleepSeconds}\n` : "";
    writeFileSync(
        binary,
        `#!/bin/sh\necho "$1" >> ${invocationLog}\n${sleep}case "$1" in\n` +
            // The real binary accepts the `probe` argv but answers `status`,
            // the contracted name for the read-only observation. A fixture that
            // echoed `probe` back would only prove the client agrees with
            // itself.
            `  probe) echo '${startResultJson("status").replace("started", "healthy")}';;\n` +
            // Restart is its own case rather than a sed of the start payload:
            // it is the one command whose success must carry effects, and the
            // rewrite could not add them.
            `  restart) echo '${startResultJson("restart")}';;\n` +
            `  *) echo '${startResultJson("start")}' | sed "s/\\"command\\":\\"start\\"/\\"command\\":\\"$1\\"/";;\n` +
            "esac\nexit 0\n",
    );
    chmodSync(binary, 0o700);
    return { binary, invocationLog };
}

function invocations(logPath: string): string[] {
    return readFileSync(logPath, "utf8").split("\n").filter(Boolean);
}

function policyFor(
    options: ConstructorParameters<typeof HostLifecyclePolicy>[0],
): HostLifecyclePolicy {
    return new HostLifecyclePolicy({
        platformReaders: {
            platform: "linux",
            arch: "x64",
            kernelRelease: () => "6.1.0",
            glibcVersion: () => "2.34",
            procSelfFdUsable: () => true,
        },
        compatibilityProbe: async () => compatibleObservation(),
        ...options,
    });
}

function catalogEntry(moduleId: string, moduleVersion = "0.1.0"): CatalogEntry {
    return {
        module_id: moduleId,
        module_version: moduleVersion,
        roles: [],
        control_ops: [],
    };
}

const compatibleCatalog = [catalogEntry("context"), catalogEntry("synapse"), catalogEntry("broca")];

function compatibleObservation() {
    return {
        authenticatedPeer: authenticatedPeerAt(hostRelease.versions.daemon),
        catalog: compatibleCatalog,
        epochs: { ...hostRelease.epochs },
    };
}

/** A Linux host whose kernel sits below the contract floor. */
function unsupportedPlatformReaders(): PlatformReaders {
    return {
        platform: "linux",
        arch: "x64",
        kernelRelease: () => "4.17.0",
        glibcVersion: () => "2.34",
        procSelfFdUsable: () => true,
    };
}

describe("pre-native outcomes", () => {
    test("no absolute root: unavailable/no_data_dir, restart effects false,false", async () => {
        const policy = policyFor({ env: { HOME: "relative-home" } });
        for (const op of ["start", "stop", "status", "doctor"] as const) {
            const result = await policy[op]();
            expect(result.state).toBe("unavailable");
            expect(result.reason).toBe("no_data_dir");
            expect(result.remediation).toBe("set_data_directory");
            expect(result.ok).toBe(false);
            expect(result.effects).toBeNull();
        }
        const restart = await policy.restart();
        expect(restart.state).toBe("unavailable");
        expect(restart.effects).toEqual({ stop_committed: false, start_committed: false });
    });

    test("a rejected filesystem is unsupported_filesystem with the classifier state", async () => {
        const root = tempDir("eidnara-policy-fs-");
        try {
            const policy = policyFor({
                env: { XDG_DATA_HOME: root },
                admissionIo: {
                    platform: "linux",
                    readMounts: () => `remote:/x / nfs4 rw 0 0\n`,
                },
            });
            const result = await policy.status();
            expect(result.reason).toBe("unsupported_filesystem");
            expect(result.remediation).toBe("set_data_directory");
            expect(result.state).toBe("stopped");
        } finally {
            rmSync(root, { recursive: true, force: true });
        }
    });

    test("unsupported platforms fail every command before native invocation", async () => {
        const root = tempDir("eidnara-policy-platform-");
        const { binary, invocationLog } = fakeBinary(root);
        try {
            for (const platformReaders of [
                {
                    platform: "linux" as const,
                    arch: "x64",
                    kernelRelease: () => "4.17.0",
                    glibcVersion: () => "2.34",
                    procSelfFdUsable: () => true,
                },
                {
                    platform: "darwin" as const,
                    arch: "arm64",
                    kernelRelease: () => "23.0.0",
                    glibcVersion: () => null,
                    procSelfFdUsable: () => false,
                },
                {
                    platform: "linux" as const,
                    arch: "arm64",
                    kernelRelease: () => "6.8.0",
                    glibcVersion: () => "2.39",
                    procSelfFdUsable: () => true,
                },
            ]) {
                const policy = policyFor({
                    env: { XDG_DATA_HOME: root },
                    launchTarget: { kind: "test-binary", path: binary },
                    platformReaders,
                });
                for (const operation of ["start", "status", "doctor"] as const) {
                    const result = await policy[operation]();
                    expect(result.reason).toBe("unsupported_platform");
                    expect(result.remediation).toBe("use_supported_platform");
                }
            }
            expect(invocations(invocationLog)).toEqual([]);
        } finally {
            rmSync(root, { recursive: true, force: true });
        }
    });

    test("filesystem rejection keeps precedence over unsupported platform", async () => {
        const root = tempDir("eidnara-policy-platform-fs-precedence-");
        try {
            const policy = policyFor({
                env: { XDG_DATA_HOME: root },
                admissionIo: {
                    platform: "linux",
                    readMounts: () => "remote:/x / nfs4 rw 0 0\n",
                },
                platformReaders: unsupportedPlatformReaders(),
            });
            for (const operation of ["status", "doctor"] as const) {
                expect((await policy[operation]()).reason).toBe("unsupported_filesystem");
            }
        } finally {
            rmSync(root, { recursive: true, force: true });
        }
    });

    test("an unsupported platform gates status and doctor too", async () => {
        const root = tempDir("eidnara-policy-platform-observational-");
        const { binary, invocationLog } = fakeBinary(root);
        try {
            const gated = policyFor({
                env: { XDG_DATA_HOME: root },
                launchTarget: { kind: "test-binary", path: binary },
                platformReaders: unsupportedPlatformReaders(),
            });
            for (const op of ["status", "doctor"] as const) {
                const result = await gated[op]();
                expect(result.reason).toBe("unsupported_platform");
                expect(result.remediation).toBe("use_supported_platform");
                expect(result.ok).toBe(false);
            }
            expect(invocations(invocationLog)).toEqual([]);
            // Without a launch target the platform rejection still outranks the
            // no-probe classifier: an unrunnable host has no daemon state.
            const noTarget = policyFor({
                env: { XDG_DATA_HOME: root },
                platformReaders: unsupportedPlatformReaders(),
            });
            const status = await noTarget.status();
            expect(status.reason).toBe("unsupported_platform");
        } finally {
            rmSync(root, { recursive: true, force: true });
        }
    });
});

describe("observational commands without a trusted bootstrap (U3 scenario 21)", () => {
    test("wholly absent roots are stopped/not_running with no mutation", async () => {
        const root = tempDir("eidnara-policy-absent-");
        try {
            const policy = policyFor({ env: { XDG_DATA_HOME: root } });
            const status = await policy.status();
            expect(status.state).toBe("stopped");
            expect(status.reason).toBe("not_running");
            const doctor = await policy.doctor();
            expect(doctor.state).toBe("stopped");
        } finally {
            rmSync(root, { recursive: true, force: true });
        }
    });

    test("residual roots are wedged/native_probe_unavailable and untouched", async () => {
        const root = tempDir("eidnara-policy-residual-");
        try {
            const coordination = path.join(root, ".eidnara-coordination");
            mkdirSync(coordination);
            writeFileSync(path.join(coordination, "transaction.lock"), "");
            const before = readFileSync(path.join(coordination, "transaction.lock"));
            const policy = policyFor({ env: { XDG_DATA_HOME: root } });
            const status = await policy.status();
            expect(status.state).toBe("wedged");
            expect(status.reason).toBe("native_probe_unavailable");
            expect(status.remediation).toBe("run_daemon_restart");
            expect(readFileSync(path.join(coordination, "transaction.lock"))).toEqual(before);
        } finally {
            rmSync(root, { recursive: true, force: true });
        }
    });

    test("mutating commands without any launch target fail with the package reason", async () => {
        const root = tempDir("eidnara-policy-nopayload-");
        try {
            const policy = policyFor({ env: { XDG_DATA_HOME: root } });
            const result = await policy.start();
            expect(result.reason).toBe("native_payload_missing");
            expect(result.remediation).toBe("install_native_payload");
        } finally {
            rmSync(root, { recursive: true, force: true });
        }
    });
});

describe("native invocation mapping", () => {
    test("managed credential envelopes include only bounded Broca credential names", () => {
        expect(
            buildManagedCredentialEnvelope({
                OPENAI_API_KEY: "secret",
                PATH: "/poisoned",
                EMPTY: "",
            }),
        ).toEqual({
            schema: 1,
            credentials: { OPENAI_API_KEY: "secret" },
        });
    });

    test("native current validation precedes one deferred certified package lookup", async () => {
        const root = tempDir("eidnara-policy-fallback-");
        const invocationLog = path.join(root, "fallback-invocations.log");
        const binary = path.join(root, "fallback-eidnara-host.sh");
        writeFileSync(
            binary,
            `#!/bin/sh\necho "$*" >> ${invocationLog}\n` +
                `if [ "$2" != "--payload-dir" ]; then echo '${missingPayloadResultJson()}'; exit 1; fi\n` +
                `echo '${startResultJson("start")}'\nexit 0\n`,
        );
        chmodSync(binary, 0o700);
        let lookups = 0;
        try {
            const policy = policyFor({
                env: { XDG_DATA_HOME: root },
                launchTarget: { kind: "test-binary", path: binary },
                payloadDirFallback: () => {
                    lookups += 1;
                    return "/qualified/package";
                },
            });

            const result = await policy.start();

            expect(result.reason).toBe("started");
            expect(lookups).toBe(1);
            expect(readFileSync(invocationLog, "utf8").trim().split("\n")).toEqual([
                "start",
                "start --payload-dir /qualified/package",
            ]);
        } finally {
            rmSync(root, { recursive: true, force: true });
        }
    });

    test("the fallback retry spends what the first launch left, not a fresh aggregate", async () => {
        // The aggregate is one request-to-transport bound for the command. A
        // first launch that answers `native_payload_missing`, plus the package
        // lookup that follows it, both spend from it — so handing the retry the
        // full aggregate again would let one `start` run twice the budget its
        // platform was qualified for.
        const root = tempDir("eidnara-policy-fallback-budget-");
        const invocationLog = path.join(root, "budget-invocations.log");
        const binary = path.join(root, "budget-eidnara-host.sh");
        // The retry's two seconds fit a fresh aggregate but not the residual.
        writeFileSync(
            binary,
            `#!/bin/sh\necho "$*" >> ${invocationLog}\n` +
                `if [ "$2" != "--payload-dir" ]; then sleep 1; echo '${missingPayloadResultJson()}'; exit 1; fi\n` +
                `sleep 2\necho '${startResultJson("start")}'\nexit 0\n`,
        );
        chmodSync(binary, 0o700);
        const LOOKUP_MS = 500;
        const AGGREGATE_MS = 3_000;
        try {
            const policy = policyFor({
                env: { XDG_DATA_HOME: root },
                launchTarget: { kind: "test-binary", path: binary },
                outerAggregateMs: AGGREGATE_MS,
                payloadDirFallback: () => {
                    // Stands in for resolving and hashing the certified package,
                    // which is synchronous on the real path.
                    const until = Date.now() + LOOKUP_MS;
                    while (Date.now() < until) {
                        /* burn budget before the retry */
                    }
                    return "/qualified/package";
                },
            });

            const started = Date.now();
            const result = await policy.start();
            const elapsed = Date.now() - started;

            // Both invocations happened, so the retry really did run.
            expect(readFileSync(invocationLog, "utf8").trim().split("\n")).toEqual([
                "start",
                "start --payload-dir /qualified/package",
            ]);
            // A fresh aggregate would let the retry return `started` after about 3.5 seconds.
            expect(result.reason).toBe("startup_timeout");
            expect(elapsed).toBeGreaterThanOrEqual(AGGREGATE_MS - 100);
            expect(elapsed).toBeLessThan(AGGREGATE_MS + 500);
        } finally {
            rmSync(root, { recursive: true, force: true });
        }
    }, 30_000);

    test("status and doctor route through the native probe of the trusted target", async () => {
        const root = tempDir("eidnara-policy-native-");
        const { binary, invocationLog } = fakeBinary(root);
        try {
            const policy = policyFor({
                env: { XDG_DATA_HOME: root },
                launchTarget: { kind: "test-binary", path: binary },
            });
            const status = await policy.status();
            expect(status.command).toBe("status");
            expect(status.state).toBe("running");
            const doctor = await policy.doctor();
            expect(doctor.command).toBe("doctor");
            expect(invocations(invocationLog)).toEqual(["probe", "probe"]);
        } finally {
            rmSync(root, { recursive: true, force: true });
        }
    });

    test("status and doctor derive health from authenticated component readiness", async () => {
        const root = tempDir("eidnara-policy-readiness-");
        const { binary } = fakeBinary(root);
        try {
            const policy = policyFor({
                env: { XDG_DATA_HOME: root },
                launchTarget: { kind: "test-binary", path: binary },
                readinessProbe: async () => ({
                    ...compatibleObservation(),
                    readiness: {
                        transport: { state: "ready", reason: "healthy" },
                        storage: { state: "unavailable", reason: "storage_unavailable" },
                        synapse: { state: "degraded", reason: "synapse_degraded" },
                    },
                }),
            });

            for (const result of [await policy.status(), await policy.doctor()]) {
                expect(result.ok).toBe(false);
                expect(result.reason).toBe("storage_unavailable");
                expect(result.readiness?.storage?.state).toBe("unavailable");
                expect(result.readiness?.synapse?.state).toBe("degraded");
                expect(result.versions.daemon).toBe("eidnara-host/0.1.0");
                expect(result.checks.map((check) => [check.id, check.status])).toEqual([
                    ["compatibility.daemon", "pass"],
                    ["compatibility.epochs", "pass"],
                    ["compatibility.modules", "pass"],
                    ["readiness.storage", "fail"],
                    ["readiness.synapse", "fail"],
                    ["readiness.transport", "pass"],
                ]);
            }
        } finally {
            rmSync(root, { recursive: true, force: true });
        }
    });

    test("status and doctor classify kernel readiness as pass, warn, or fail", async () => {
        const cases = [
            {
                kernel: { state: "ready", reason: "healthy" },
                status: "pass",
                ok: true,
                remediation: null,
            },
            {
                kernel: { state: "ready", reason: "kernel_lagging" },
                status: "warn",
                ok: true,
                remediation: "inspect_kernel_projector",
            },
            {
                kernel: { state: "ready", reason: "kernel_capacity_warn" },
                status: "warn",
                ok: true,
                remediation: "inspect_storage",
            },
            {
                kernel: { state: "ready", reason: "no_required_consumer" },
                status: "warn",
                ok: true,
                remediation: null,
            },
            {
                kernel: { state: "starting", reason: "kernel_starting" },
                status: "fail",
                ok: false,
                remediation: "wait_and_retry",
            },
            {
                kernel: { state: "unavailable", reason: "kernel_unavailable" },
                status: "fail",
                ok: false,
                remediation: "inspect_storage",
            },
        ] as const;
        for (const { kernel, status, ok, remediation } of cases) {
            const root = tempDir(`eidnara-policy-kernel-${kernel.reason}-`);
            const { binary } = fakeBinary(root);
            try {
                const policy = policyFor({
                    env: { XDG_DATA_HOME: root },
                    launchTarget: { kind: "test-binary", path: binary },
                    readinessProbe: async () => ({
                        ...compatibleObservation(),
                        readiness: {
                            transport: { state: "ready", reason: "healthy" },
                            storage: { state: "ready", reason: "healthy" },
                            synapse: { state: "ready", reason: "healthy" },
                            kernel,
                        },
                    }),
                });
                for (const result of [await policy.status(), await policy.doctor()]) {
                    expect(result.ok).toBe(ok);
                    // A warn-class kernel reason never becomes the result reason.
                    expect(result.reason).toBe(ok ? "healthy" : kernel.reason);
                    expect(result.readiness?.kernel).toEqual(kernel);
                    expect(result.checks.find((check) => check.id === "readiness.kernel")).toEqual({
                        id: "readiness.kernel",
                        status,
                        reason: kernel.reason,
                        remediation,
                    });
                    // The rendered result must round-trip through the strict parser.
                    expect(() => parseDaemonResult(JSON.stringify(result))).not.toThrow();
                }
            } finally {
                rmSync(root, { recursive: true, force: true });
            }
        }
    });

    test("a component still starting carries the state its promoted reason requires", async () => {
        const cases = [
            { component: "transport", record: { state: "starting", reason: "starting" } },
            { component: "storage", record: { state: "starting", reason: "starting" } },
            { component: "synapse", record: { state: "starting", reason: "starting" } },
            { component: "kernel", record: { state: "starting", reason: "starting" } },
        ] as const;
        for (const { component, record } of cases) {
            const root = tempDir(`eidnara-policy-${component}-starting-`);
            const { binary } = fakeBinary(root);
            try {
                const policy = policyFor({
                    env: { XDG_DATA_HOME: root },
                    launchTarget: { kind: "test-binary", path: binary },
                    readinessProbe: async () => ({
                        ...compatibleObservation(),
                        readiness: {
                            transport: { state: "ready", reason: "healthy" },
                            storage: { state: "ready", reason: "healthy" },
                            synapse: { state: "ready", reason: "healthy" },
                            kernel: { state: "ready", reason: "healthy" },
                            [component]: record,
                        },
                    }),
                });
                for (const result of [await policy.status(), await policy.doctor()]) {
                    expect(result.ok).toBe(false);
                    expect(result.reason).toBe("starting");
                    expect(result.state).toBe("starting");
                    expect(result.remediation).toBe("wait_and_retry");
                    expect(() => parseDaemonResult(JSON.stringify(result))).not.toThrow();
                }
            } finally {
                rmSync(root, { recursive: true, force: true });
            }
        }
    });

    test("status and doctor surface authenticated daemon, module, and epoch mismatches", async () => {
        const cases = [
            {
                reason: "incompatible_daemon",
                observation: {
                    ...compatibleObservation(),
                    authenticatedPeer: authenticatedPeerAt("eidnara-host/0.2.0"),
                },
                failedCheck: "compatibility.daemon",
            },
            {
                reason: "incompatible_module",
                observation: {
                    ...compatibleObservation(),
                    catalog: compatibleCatalog.map((entry) =>
                        entry.module_id === "context" ? catalogEntry("context", "0.2.0") : entry,
                    ),
                },
                failedCheck: "compatibility.modules",
            },
            {
                reason: "incompatible_epochs",
                observation: {
                    ...compatibleObservation(),
                    epochs: {
                        ...hostRelease.epochs,
                        state_sync: hostRelease.epochs.state_sync + 1,
                    },
                },
                failedCheck: "compatibility.epochs",
            },
        ] as const;

        for (const { reason, observation, failedCheck } of cases) {
            const root = tempDir(`eidnara-policy-${reason}-`);
            const { binary, invocationLog } = fakeBinary(root);
            try {
                const policy = policyFor({
                    env: { XDG_DATA_HOME: root },
                    launchTarget: { kind: "test-binary", path: binary },
                    readinessProbe: async () => ({
                        ...observation,
                        readiness: {
                            transport: { state: "ready", reason: "healthy" },
                            storage: { state: "ready", reason: "healthy" },
                            synapse: { state: "ready", reason: "healthy" },
                        },
                    }),
                });

                for (const result of [await policy.status(), await policy.doctor()]) {
                    expect(result.ok).toBe(false);
                    expect(result.state).toBe("running");
                    expect(result.reason).toBe(reason);
                    expect(result.remediation).toBe("align_versions");
                    expect(result.checks.find((check) => check.id === failedCheck)).toMatchObject({
                        status: "fail",
                        reason,
                    });
                }
                expect(invocations(invocationLog)).toEqual(["probe", "probe"]);
            } finally {
                rmSync(root, { recursive: true, force: true });
            }
        }
    });

    test("a short-circuited probe reports no check for an unobserved component", async () => {
        const root = tempDir("eidnara-policy-unobserved-");
        const { binary } = fakeBinary(root);
        try {
            const policy = policyFor({
                env: { XDG_DATA_HOME: root },
                launchTarget: { kind: "test-binary", path: binary },
                // The daemon stage failed, so host.status never ran: only the
                // handshake-proven transport state is reported.
                readinessProbe: async () => ({
                    ...compatibleObservation(),
                    authenticatedPeer: authenticatedPeerAt("eidnara-host/0.2.0"),
                    catalog: [],
                    epochs: {},
                    evaluatedThrough: "daemon" as const,
                    readiness: { transport: { state: "ready", reason: "healthy" } },
                }),
            });

            for (const result of [await policy.status(), await policy.doctor()]) {
                expect(result.reason).toBe("incompatible_daemon");
                const ids = result.checks.map((check) => check.id);
                expect(ids).toContain("compatibility.daemon");
                // Never observed, so they must not be asserted as failures that
                // would point remediation away from the version mismatch.
                expect(ids).not.toContain("readiness.storage");
                expect(ids).not.toContain("readiness.synapse");
                // Stages the probe never reached emit no verdict either.
                expect(ids).not.toContain("compatibility.modules");
                expect(ids).not.toContain("compatibility.epochs");
            }
        } finally {
            rmSync(root, { recursive: true, force: true });
        }
    });

    test("a failing readiness probe keeps the observation it already proved", async () => {
        // The native probe child already answered and was validated, so a
        // readiness failure on top of it must not be reported as an internal
        // error for a daemon this call verifiably observed. Same rule the
        // storage probe follows: degrade, never erase a successful observation.
        const root = tempDir("eidnara-policy-readiness-failure-");
        const { binary } = fakeBinary(root);
        try {
            const policy = policyFor({
                env: { XDG_DATA_HOME: root },
                launchTarget: { kind: "test-binary", path: binary },
                readinessProbe: async () => {
                    throw new Error("route collapsed mid-probe");
                },
            });
            for (const result of [await policy.status(), await policy.doctor()]) {
                expect(result.ok).toBe(true);
                expect(result.state).toBe("running");
                expect(result.reason).not.toBe("internal_error");
                // The native result's own readiness survives untouched, and no
                // probe-derived component is invented on top of it.
                expect(result.readiness?.storage).toBeUndefined();
                expect(result.readiness?.synapse).toBeUndefined();
                expect(result.checks.some((check) => check.id.startsWith("readiness."))).toBe(
                    false,
                );
            }
        } finally {
            rmSync(root, { recursive: true, force: true });
        }
    });

    test("the readiness probe is bounded by what the probe child left of the aggregate", async () => {
        // The aggregate is a request-to-transport bound shared with the child, so
        // the probe gets the residual rather than a fresh full budget. The floor
        // is also load-bearing: it must never hand out a token 1ms budget, which
        // would start a probe that can only fail.
        const root = tempDir("eidnara-policy-readiness-residual-");
        const CHILD_MS = 1_000;
        const AGGREGATE_MS = 20_000;
        const { binary } = fakeBinary(root, { sleepSeconds: CHILD_MS / 1_000 });
        const budgets: number[] = [];
        try {
            const policy = policyFor({
                env: { XDG_DATA_HOME: root },
                launchTarget: { kind: "test-binary", path: binary },
                outerAggregateMs: AGGREGATE_MS,
                readinessProbe: async (budgetMs) => {
                    budgets.push(budgetMs);
                    return {
                        ...compatibleObservation(),
                        readiness: {
                            transport: { state: "ready", reason: "healthy" },
                        },
                    };
                },
            });
            const result = await policy.status();
            expect(result.ok).toBe(true);
            expect(budgets).toHaveLength(1);
            // The child's second of runtime came out of the aggregate.
            expect(budgets[0]).toBeLessThan(AGGREGATE_MS - CHILD_MS + 1);
            // And a real budget was handed over, not the old 1ms floor.
            expect(budgets[0]).toBeGreaterThan(1);
        } finally {
            rmSync(root, { recursive: true, force: true });
        }
    }, 20_000);

    test("an authenticated daemon outside the supported range is never healthy", async () => {
        // Readiness answers whether the components are serving, not whether this
        // client may talk to this daemon at all. Without the compatibility gate a
        // running daemon on an unsupported version reported `healthy` and stamped
        // that version with `proof: "current"`.
        const root = tempDir("eidnara-policy-incompatible-daemon-");
        const { binary } = fakeBinary(root);
        try {
            const policy = policyFor({
                env: { XDG_DATA_HOME: root },
                launchTarget: { kind: "test-binary", path: binary },
                readinessProbe: async () => ({
                    ...compatibleObservation(),
                    // Every component is ready, so only the version can fail it.
                    authenticatedPeer: authenticatedPeerAt("eidnara-host/9.9.9"),
                    readiness: {
                        transport: { state: "ready", reason: "healthy" },
                        storage: { state: "ready", reason: "healthy" },
                    },
                }),
            });
            for (const result of [await policy.status(), await policy.doctor()]) {
                expect(result.ok).toBe(false);
                expect(result.reason).toBe("incompatible_daemon");
                expect(result.remediation).toBe("align_versions");
                expect(
                    result.checks.find((check) => check.id === "compatibility.daemon")?.status,
                ).toBe("fail");
            }
        } finally {
            rmSync(root, { recursive: true, force: true });
        }
    });

    test("the reported readiness reason follows contract precedence, not check-id order", async () => {
        const root = tempDir("eidnara-policy-readiness-precedence-");
        const { binary } = fakeBinary(root);
        try {
            const policy = policyFor({
                env: { XDG_DATA_HOME: root },
                launchTarget: { kind: "test-binary", path: binary },
                readinessProbe: async () => ({
                    ...compatibleObservation(),
                    readiness: {
                        // `readiness.storage` sorts before `readiness.transport`,
                        // but `authentication_failed` outranks `storage_unavailable`
                        // in the release contract's failing-reason precedence.
                        transport: { state: "unavailable", reason: "authentication_failed" },
                        storage: { state: "unavailable", reason: "storage_unavailable" },
                        synapse: { state: "degraded", reason: "synapse_degraded" },
                    },
                }),
            });

            for (const result of [await policy.status(), await policy.doctor()]) {
                expect(result.ok).toBe(false);
                expect(result.reason).toBe("authentication_failed");
                expect(result.remediation).toBe("inspect_daemon_process");
                // The check list itself stays sorted by id: the v1 result
                // requires lexicographically sorted unique check ids.
                expect(result.checks.map((check) => check.id)).toEqual([
                    "compatibility.daemon",
                    "compatibility.epochs",
                    "compatibility.modules",
                    "readiness.storage",
                    "readiness.synapse",
                    "readiness.transport",
                ]);
            }
        } finally {
            rmSync(root, { recursive: true, force: true });
        }
    });

    test("a readiness probe that ignores its budget cannot hold status past the aggregate", async () => {
        const root = tempDir("eidnara-policy-readiness-budget-");
        const { binary } = fakeBinary(root);
        try {
            const budgets: number[] = [];
            const policy = policyFor({
                env: { XDG_DATA_HOME: root },
                launchTarget: { kind: "test-binary", path: binary },
                outerAggregateMs: 750,
                readinessProbe: (budgetMs) => {
                    budgets.push(budgetMs);
                    return new Promise<never>(() => {});
                },
            });
            const unprobed = policyFor({
                env: { XDG_DATA_HOME: root },
                launchTarget: { kind: "test-binary", path: binary },
                outerAggregateMs: 750,
            });
            for (const command of ["status", "doctor"] as const) {
                const startedAt = performance.now();
                const result = await policy[command]();
                const elapsed = performance.now() - startedAt;
                // The aggregate deadline drops only readiness; native status remains healthy.
                expect(elapsed).toBeLessThan(750 + 500);
                expect(result).toEqual(await unprobed[command]());
                expect(result.ok).toBe(true);
                expect(result.reason).toBe("healthy");
            }
            expect(budgets).toHaveLength(2);
            for (const budget of budgets) expect(budget).toBeLessThanOrEqual(750);
        } finally {
            rmSync(root, { recursive: true, force: true });
        }
    }, 20_000);

    test("a readiness probe whose synchronous prefix outlasts the aggregate is dropped", async () => {
        const root = tempDir("eidnara-policy-readiness-sync-prefix-");
        const { binary } = fakeBinary(root);
        try {
            const policy = policyFor({
                env: { XDG_DATA_HOME: root },
                launchTarget: { kind: "test-binary", path: binary },
                outerAggregateMs: 750,
                readinessProbe: (budgetMs) => {
                    const until = performance.now() + budgetMs + 200;
                    while (performance.now() < until) {
                        // spin
                    }
                    return Promise.resolve({
                        ...compatibleObservation(),
                        readiness: {
                            transport: { state: "ready", reason: "healthy" },
                            storage: { state: "unavailable", reason: "storage_unavailable" },
                        },
                    });
                },
            });
            for (const command of ["status", "doctor"] as const) {
                const result = await policy[command]();
                // The stale observation is refused; the native probe's verdict stands alone.
                expect(result.ok).toBe(true);
                expect(result.reason).toBe("healthy");
                expect(
                    result.checks.find((check) => check.id === "readiness.storage"),
                ).toBeUndefined();
            }
        } finally {
            rmSync(root, { recursive: true, force: true });
        }
    }, 20_000);

    test("a compatibility verdict beyond the reported stage still fails status and doctor", async () => {
        const root = tempDir("eidnara-policy-verdict-beyond-stage-");
        const { binary } = fakeBinary(root);
        try {
            const policy = policyFor({
                env: { XDG_DATA_HOME: root },
                launchTarget: { kind: "test-binary", path: binary },
                readinessProbe: async () => ({
                    ...compatibleObservation(),
                    // The daemon stage passed and the probe stopped there, yet the
                    // snapshot carries mismatched epochs.
                    epochs: { ...compatibleObservation().epochs, tagger: 99 },
                    evaluatedThrough: "daemon" as const,
                    readiness: { transport: { state: "ready", reason: "healthy" } },
                }),
            });
            for (const result of [await policy.status(), await policy.doctor()]) {
                expect(result.ok).toBe(false);
                expect(result.reason).toBe("incompatible_epochs");
                expect(result.remediation).toBe("align_versions");
                // The unreached stage is still not asserted as a check.
                expect(
                    result.checks.find((check) => check.id === "compatibility.epochs"),
                ).toBeUndefined();
                expect(() => parseDaemonResult(JSON.stringify(result))).not.toThrow();
            }
        } finally {
            rmSync(root, { recursive: true, force: true });
        }
    });

    test("restart resolves the certified payload before one native transaction", async () => {
        const root = tempDir("eidnara-policy-restart-payload-");
        const invocationLog = path.join(root, "restart-invocations.log");
        const binary = path.join(root, "restart-eidnara-host.sh");
        writeFileSync(
            binary,
            `#!/bin/sh\necho "$*" >> ${invocationLog}\necho '${restartResultJson()}'\nexit 0\n`,
        );
        chmodSync(binary, 0o700);
        let lookups = 0;
        try {
            const policy = policyFor({
                env: { XDG_DATA_HOME: root },
                launchTarget: { kind: "test-binary", path: binary },
                payloadDirFallback: () => {
                    lookups += 1;
                    return "/qualified/package";
                },
            });
            const result = await policy.restart();
            expect(result.effects).toEqual({
                stop_committed: true,
                start_committed: true,
            });
            expect(lookups).toBe(1);
            expect(readFileSync(invocationLog, "utf8").trim()).toBe(
                "restart --payload-dir /qualified/package",
            );
        } finally {
            rmSync(root, { recursive: true, force: true });
        }
    });

    test("a joiner with a non-finite deadline is rejected without cancelling the start", async () => {
        // Gating only the create path left joiners reaching raceWaiter, where a
        // non-finite budget survives the residual subtraction and setTimeout
        // coerces it to 1ms. The discriminating case is a shared start that
        // settles inside one microtask drain: the joiner then ADOPTS the result
        // on an invalid budget instead of being rejected, so identical input
        // resolves or rejects purely on whether another demand was in flight.
        // launchTarget: null gives exactly that — start() returns a local result
        // without spawning.
        const root = tempDir("eidnara-policy-joiner-deadline-");
        try {
            const policy = policyFor({ env: { XDG_DATA_HOME: root }, launchTarget: null });
            // Created in the same tick, so the joiners below observe it in flight.
            const creator = policy.demandStart({
                origin: "managed-default",
                capability: "context",
                deadlineMs: 20_000,
            });
            expect(policy.inflightStartCount).toBe(1);

            const joinerOutcomes: string[] = [];
            for (const deadlineMs of [Number.NaN, Number.POSITIVE_INFINITY, 0, -5]) {
                try {
                    const outcome = await policy.demandStart({
                        origin: "managed-default",
                        capability: "context",
                        deadlineMs,
                    });
                    joinerOutcomes.push(`adopted:${outcome.result.reason}`);
                } catch (error) {
                    joinerOutcomes.push(`detached:${(error as WaiterDetachedError).cause_kind}`);
                }
            }
            // Every joiner is rejected; none adopts a result it had no budget for.
            expect(joinerOutcomes).toEqual([
                "detached:deadline",
                "detached:deadline",
                "detached:deadline",
                "detached:deadline",
            ]);

            // Rejecting joiners is not cancelling: the creator's start still
            // resolves, which is the detach-only guarantee this design requires.
            const outcome = await creator;
            expect(outcome.result.reason).toBe("native_payload_missing");
        } finally {
            rmSync(root, { recursive: true, force: true });
        }
    }, 30_000);

    test("an already-inactive first demand never spawns the daemon", async () => {
        // `start()` is async, but its synchronous prefix runs all the way through
        // runNativeLifecycle to spawn() before the first await. Detaching only
        // inside raceWaiter would therefore leave a mutating child running for a
        // caller that was already gone, with no live waiter to own it.
        const root = tempDir("eidnara-policy-inactive-demand-");
        try {
            const { binary, invocationLog } = fakeBinary(root);
            const policy = policyFor({
                env: { XDG_DATA_HOME: root },
                launchTarget: { kind: "test-binary", path: binary },
            });
            const aborted = AbortSignal.abort();
            let abortKind: string | null = null;
            try {
                await policy.demandStart({
                    origin: "managed-default",
                    capability: "context",
                    signal: aborted,
                });
            } catch (error) {
                abortKind = (error as WaiterDetachedError).cause_kind;
            }
            expect(abortKind).toBe("aborted");

            let deadlineKind: string | null = null;
            try {
                await policy.demandStart({
                    origin: "managed-default",
                    capability: "context",
                    deadlineMs: 0,
                });
            } catch (error) {
                deadlineKind = (error as WaiterDetachedError).cause_kind;
            }
            expect(deadlineKind).toBe("deadline");

            // Non-finite budgets are inactive too, not generous: NaN stays NaN
            // through the residual subtraction, and setTimeout coerces both NaN
            // and Infinity to a 1ms delay, so the waiter would detach at once
            // and leave the start unowned.
            for (const deadlineMs of [Number.NaN, Number.POSITIVE_INFINITY]) {
                let kind: string | null = null;
                try {
                    await policy.demandStart({
                        origin: "managed-default",
                        capability: "context",
                        deadlineMs,
                    });
                } catch (error) {
                    kind = (error as WaiterDetachedError).cause_kind;
                }
                expect(kind).toBe("deadline");
            }

            // The load-bearing assertion: no native invocation happened at all.
            expect(invocations(invocationLog)).toEqual([]);
            expect(policy.inflightStartCount).toBe(0);
        } finally {
            rmSync(root, { recursive: true, force: true });
        }
    }, 20_000);

    test("preflight time is deducted from the native aggregate", async () => {
        // The aggregate is a request-to-transport bound, so a slow synchronous
        // preflight must shrink the child's share of it rather than being
        // followed by a fresh full budget. Exhausting it entirely is this
        // command's timeout, and nothing was spawned, so a restart reports no
        // committed effects rather than unknown ones.
        const root = tempDir("eidnara-policy-preflight-budget-");
        try {
            const { binary, invocationLog } = fakeBinary(root);
            const SYNC_MS = 300;
            const slowGate: PlatformReaders = {
                platform: "linux",
                arch: "x64",
                kernelRelease: () => "6.1.0",
                glibcVersion: () => {
                    const until = Date.now() + SYNC_MS;
                    while (Date.now() < until) {}
                    return "2.34";
                },
                procSelfFdUsable: () => true,
            };
            const policy = policyFor({
                env: { XDG_DATA_HOME: root },
                launchTarget: { kind: "test-binary", path: binary },
                platformReaders: slowGate,
                // Smaller than the preflight cost, so the residual is exhausted.
                outerAggregateMs: SYNC_MS / 3,
            });
            const result = await policy.start();
            expect(result.reason).toBe("startup_timeout");
            expect(result.ok).toBe(false);
            // No child was spawned: the budget was gone before the launch.
            expect(invocations(invocationLog)).toEqual([]);

            // A restart on the same exhausted path reports nothing committed,
            // not unknown effects.
            const restartPolicy = policyFor({
                env: { XDG_DATA_HOME: root },
                launchTarget: { kind: "test-binary", path: binary },
                platformReaders: slowGate,
                outerAggregateMs: SYNC_MS / 3,
            });
            const restart = await restartPolicy.restart();
            expect(restart.reason).toBe("startup_timeout");
            expect(restart.effects).toEqual({ stop_committed: false, start_committed: false });
        } finally {
            rmSync(root, { recursive: true, force: true });
        }
    }, 20_000);

    test("each qualified target gets the aggregate it was qualified for", () => {
        expect(aggregateForTarget("linux-x64-gnu")).toBe(OUTER_AGGREGATE_MS);
    });

    test("an explicit aggregate still overrides the platform default", async () => {
        // The override is what every other test in this file relies on, so it
        // must keep winning over the platform-derived value.
        const root = tempDir("eidnara-policy-aggregate-override-");
        try {
            const { binary } = fakeBinary(root, { sleepSeconds: 30 });
            const policy = policyFor({
                env: { XDG_DATA_HOME: root },
                launchTarget: { kind: "test-binary", path: binary },
                outerAggregateMs: 400,
            });
            const started = Date.now();
            const result = await policy.start();
            // 400ms beat both platform defaults, so the child was killed at the
            // injected deadline rather than at 15s or 60s.
            expect(Date.now() - started).toBeLessThan(10_000);
            expect(result.reason).toBe("startup_timeout");
        } finally {
            rmSync(root, { recursive: true, force: true });
        }
    }, 20_000);

    test("a demand runs the platform readers once, inside the start's preflight", async () => {
        const root = tempDir("eidnara-policy-readers-once-");
        const { binary } = fakeBinary(root);
        let kernelReads = 0;
        try {
            const policy = new HostLifecyclePolicy({
                env: { XDG_DATA_HOME: root },
                launchTarget: { kind: "test-binary", path: binary },
                platformReaders: {
                    platform: "linux",
                    arch: "x64",
                    kernelRelease: () => {
                        kernelReads += 1;
                        return "6.1.0";
                    },
                    glibcVersion: () => "2.34",
                    procSelfFdUsable: () => true,
                },
                compatibilityProbe: async () => compatibleObservation(),
                storageProbe: async () => "ready",
            });
            const outcome = await policy.demandStart({
                origin: "managed-default",
                capability: "context",
            });
            expect(outcome.result.reason).toBe("started");
            expect(kernelReads).toBe(1);
        } finally {
            rmSync(root, { recursive: true, force: true });
        }
    }, 20_000);

    test("restart is one native transaction, never TS stop+start", async () => {
        const root = tempDir("eidnara-policy-restart-");
        const { binary, invocationLog } = fakeBinary(root);
        try {
            const policy = policyFor({
                env: { XDG_DATA_HOME: root },
                launchTarget: { kind: "test-binary", path: binary },
            });
            const result = await policy.restart();
            expect(result.command).toBe("restart");
            // Asserting `command` alone would pass even when the native payload
            // was rejected, because `launchFailure` also stamps the caller's
            // command onto its `internal_error` result. The outcome fields are
            // what prove the native restart was actually accepted.
            expect(result.ok).toBe(true);
            expect(result.reason).toBe("started");
            expect(result.effects).toEqual({ stop_committed: true, start_committed: true });
            expect(invocations(invocationLog)).toEqual(["restart"]);
        } finally {
            rmSync(root, { recursive: true, force: true });
        }
    });

    test("restart sends the default credential envelope", async () => {
        const root = tempDir("eidnara-policy-restart-envelope-");
        const envelopeLog = path.join(root, "restart-envelope.json");
        const binary = path.join(root, "restart-envelope-eidnara-host.sh");
        writeFileSync(
            binary,
            `#!/bin/sh\ncat > ${envelopeLog}\necho '${restartResultJson()}'\nexit 0\n`,
        );
        chmodSync(binary, 0o700);
        try {
            const policy = policyFor({
                env: { XDG_DATA_HOME: root },
                launchTarget: { kind: "test-binary", path: binary },
                defaultStartupEnvelope: {
                    schema: 1,
                    credentials: { OPENAI_API_KEY: "preserved-secret" },
                },
            });
            const result = await policy.restart();
            expect(result.ok).toBe(true);
            expect(JSON.parse(readFileSync(envelopeLog, "utf8"))).toEqual({
                schema: 1,
                credentials: { OPENAI_API_KEY: "preserved-secret" },
            });
        } finally {
            rmSync(root, { recursive: true, force: true });
        }
    });
});

describe("demand-start coalescing and detachment (U3 scenarios 15-16)", () => {
    test("compatibility mismatch blocks readiness without stop, restart, or a second start", async () => {
        const cases = [
            {
                reason: "incompatible_daemon",
                observation: {
                    ...compatibleObservation(),
                    authenticatedPeer: authenticatedPeerAt("eidnara-host/0.2.0"),
                },
            },
            {
                reason: "incompatible_module",
                observation: {
                    ...compatibleObservation(),
                    catalog: compatibleCatalog.map((entry) =>
                        entry.module_id === "synapse" ? catalogEntry("synapse", "0.2.0") : entry,
                    ),
                },
            },
            {
                reason: "incompatible_epochs",
                observation: {
                    ...compatibleObservation(),
                    epochs: {
                        ...hostRelease.epochs,
                        memory_render: hostRelease.epochs.memory_render - 1,
                    },
                },
            },
        ] as const;

        for (const { reason, observation } of cases) {
            const root = tempDir(`eidnara-policy-demand-${reason}-`);
            const { binary, invocationLog } = fakeBinary(root);
            let storageProbes = 0;
            try {
                const policy = policyFor({
                    env: { XDG_DATA_HOME: root },
                    launchTarget: { kind: "test-binary", path: binary },
                    compatibilityProbe: async () => observation,
                    storageProbe: async () => {
                        storageProbes += 1;
                        return "ready";
                    },
                });
                const outcome = await policy.demandStart({
                    origin: "managed-default",
                    capability: "context",
                });

                expect(outcome.result.ok).toBe(false);
                expect(outcome.result.state).toBe("running");
                expect(outcome.result.reason).toBe(reason);
                expect(outcome.result.remediation).toBe("align_versions");
                expect(outcome.storage).toBeNull();
                expect(storageProbes).toBe(0);
                expect(invocations(invocationLog)).toEqual(["start"]);
            } finally {
                rmSync(root, { recursive: true, force: true });
            }
        }
    });

    test("concurrent demands share one compatibility probe", async () => {
        const root = tempDir("eidnara-policy-probe-coalesce-");
        const { binary } = fakeBinary(root);
        let probes = 0;
        try {
            const policy = policyFor({
                env: { XDG_DATA_HOME: root },
                launchTarget: { kind: "test-binary", path: binary },
                compatibilityProbe: async () => {
                    probes += 1;
                    await new Promise((resolve) => setTimeout(resolve, 5));
                    return compatibleObservation();
                },
            });

            // The snapshot describes the daemon incarnation, not the requesting
            // capability, so distinct capabilities still share one probe.
            const outcomes = await Promise.all([
                policy.demandStart({ origin: "managed-default", capability: "context" }),
                policy.demandStart({ origin: "managed-default", capability: "synapse" }),
            ]);

            for (const outcome of outcomes) {
                expect(outcome.result.ok).toBe(true);
                expect(outcome.authenticatedDaemonId).toEqual(new Uint8Array([7]));
            }
            expect(probes).toBe(1);
        } finally {
            rmSync(root, { recursive: true, force: true });
        }
    });

    test("the shared compatibility probe budget does not come from the creating waiter", async () => {
        const root = tempDir("eidnara-policy-probe-budget-");
        const { binary } = fakeBinary(root);
        const budgets: number[] = [];
        try {
            const policy = policyFor({
                env: { XDG_DATA_HOME: root },
                launchTarget: { kind: "test-binary", path: binary },
                outerAggregateMs: 30_000,
                compatibilityProbe: async (budgetMs) => {
                    budgets.push(budgetMs);
                    return compatibleObservation();
                },
            });
            const outcome = await policy.demandStart({
                origin: "managed-default",
                capability: "context",
                deadlineMs: 5_000,
            });

            // Every waiter joins whichever probe exists, so a nearly expired
            // caller must not mint one too short for the long-lived waiters that
            // join it — they would read the truncated failure as an unproven
            // compatibility claim while still holding ample time. The caller's own
            // deadline still bounds its wait through `raceDetached`.
            expect(budgets).toEqual([30_000]);
            expect(outcome.result.ok).toBe(true);
        } finally {
            rmSync(root, { recursive: true, force: true });
        }
    });

    test("the demand's compatibility wait is bounded by what the start left of the aggregate", async () => {
        const root = tempDir("eidnara-policy-compat-residual-");
        const { binary } = fakeBinary(root, { sleepSeconds: 1 });
        let storageProbes = 0;
        try {
            const policy = policyFor({
                env: { XDG_DATA_HOME: root },
                launchTarget: { kind: "test-binary", path: binary },
                outerAggregateMs: 2_000,
                compatibilityProbe: () => new Promise<never>(() => {}),
                storageProbe: async () => {
                    storageProbes += 1;
                    return "ready";
                },
            });
            const startedAt = performance.now();
            const outcome = await policy.demandStart({
                origin: "managed-default",
                capability: "context",
            });
            const elapsed = performance.now() - startedAt;
            expect(elapsed).toBeLessThan(2_000 + 1_000);
            expect(outcome.result.ok).toBe(false);
            expect(outcome.result.reason).toBe("native_probe_unavailable");
            expect(storageProbes).toBe(0);
        } finally {
            rmSync(root, { recursive: true, force: true });
        }
    }, 20_000);

    test("a deadline beyond the timer limit waits for the result instead of detaching at once", async () => {
        const root = tempDir("eidnara-policy-huge-deadline-");
        const { binary } = fakeBinary(root, { sleepSeconds: 1 });
        try {
            const policy = policyFor({
                env: { XDG_DATA_HOME: root },
                launchTarget: { kind: "test-binary", path: binary },
                storageProbe: async () => "ready",
            });
            const outcome = await policy.demandStart({
                origin: "managed-default",
                capability: "context",
                // Above the 32-bit signed millisecond limit `setTimeout` honors.
                deadlineMs: 2_147_483_648 * 4,
            });
            expect(outcome.result.reason).toBe("started");
            expect(outcome.storage).toBe("ready");
        } finally {
            rmSync(root, { recursive: true, force: true });
        }
    }, 20_000);

    test("a compatibility probe that ignores its budget is bounded by the policy aggregate", async () => {
        const root = tempDir("eidnara-policy-probe-hang-");
        const { binary, invocationLog } = fakeBinary(root);
        let storageProbes = 0;
        let compatibilityProbes = 0;
        try {
            const policy = policyFor({
                env: { XDG_DATA_HOME: root },
                launchTarget: { kind: "test-binary", path: binary },
                outerAggregateMs: 750,
                compatibilityProbe: () => {
                    compatibilityProbes += 1;
                    return new Promise<never>(() => {});
                },
                storageProbe: async () => {
                    storageProbes += 1;
                    return "ready";
                },
            });
            const startedAt = performance.now();
            // No caller deadline: the policy's own aggregate must end the wait,
            // and expiring it is an unproven claim rather than a detachment.
            const outcome = await policy.demandStart({
                origin: "managed-default",
                capability: "context",
            });
            expect(performance.now() - startedAt).toBeLessThan(750 + 1_000);
            expect(outcome.result.ok).toBe(false);
            expect(outcome.result.reason).toBe("native_probe_unavailable");
            expect(outcome.authenticatedDaemonId).toBeUndefined();
            expect(outcome.storage).toBeNull();
            expect(storageProbes).toBe(0);
            expect(invocations(invocationLog)).toEqual(["start"]);

            // The expired probe was evicted, so the next demand starts a fresh one.
            await policy.demandStart({ origin: "managed-default", capability: "context" });
            expect(compatibilityProbes).toBe(2);
        } finally {
            rmSync(root, { recursive: true, force: true });
        }
    }, 20_000);

    test("a compatibility probe outlived by a restart is rejected and not joined", async () => {
        const root = tempDir("eidnara-policy-probe-generation-");
        const { binary, invocationLog } = fakeBinary(root);
        const releases: Array<() => void> = [];
        let storageProbes = 0;
        try {
            const policy = policyFor({
                env: { XDG_DATA_HOME: root },
                launchTarget: { kind: "test-binary", path: binary },
                compatibilityProbe: () =>
                    new Promise((resolve) => {
                        releases.push(() => resolve(compatibleObservation()));
                    }),
                storageProbe: async () => {
                    storageProbes += 1;
                    return "ready";
                },
            });
            const first = policy.demandStart({ origin: "managed-default", capability: "context" });
            while (releases.length === 0) await new Promise((r) => setTimeout(r, 5));

            // Restart replaces the daemon while its compatibility probe is in flight.
            const restarted = await policy.restart();
            expect(restarted.reason).toBe("started");

            // The later demand starts a probe against the new incarnation.
            const second = policy.demandStart({ origin: "managed-default", capability: "context" });
            while (releases.length < 2) await new Promise((r) => setTimeout(r, 5));
            expect(releases).toHaveLength(2);

            for (const release of releases) release();
            const [a, b] = await Promise.all([first, second]);

            expect(a.result.ok).toBe(false);
            expect(a.result.reason).toBe("native_probe_unavailable");
            expect(a.authenticatedDaemonId).toBeUndefined();
            expect(b.result.ok).toBe(true);
            expect(b.storage).toBe("ready");
            // Only the demand certified against the live incarnation reached storage.
            expect(storageProbes).toBe(1);
            expect(invocations(invocationLog)).toEqual(["start", "restart", "start"]);
        } finally {
            rmSync(root, { recursive: true, force: true });
        }
    }, 20_000);

    test("a start that finds the daemon already running does not invalidate a pending probe", async () => {
        const root = tempDir("eidnara-policy-probe-already-running-");
        const invocationLog = path.join(root, "already-running-invocations.log");
        const binary = path.join(root, "already-running-eidnara-host.sh");
        const alreadyRunning = JSON.stringify({
            ...JSON.parse(startResultJson("start")),
            reason: "already_running",
        });
        writeFileSync(
            binary,
            `#!/bin/sh\necho "$1" >> ${invocationLog}\n` +
                `if [ "$(wc -l < ${invocationLog})" -gt 1 ]; then echo '${alreadyRunning}'; else echo '${startResultJson("start")}'; fi\nexit 0\n`,
        );
        chmodSync(binary, 0o700);
        let compatibilityProbes = 0;
        try {
            const policy = policyFor({
                env: { XDG_DATA_HOME: root },
                launchTarget: { kind: "test-binary", path: binary },
                compatibilityProbe: async () => {
                    compatibilityProbes += 1;
                    await new Promise((resolve) => setTimeout(resolve, 1_000));
                    return compatibleObservation();
                },
            });
            const first = policy.demandStart({ origin: "managed-default", capability: "context" });
            // The second demand runs after the first start settles but before
            // its compatibility probe completes, so its start answers `already_running`.
            await new Promise((resolve) => setTimeout(resolve, 400));
            const second = policy.demandStart({ origin: "managed-default", capability: "context" });
            const [a, b] = await Promise.all([first, second]);
            expect(invocations(invocationLog)).toEqual(["start", "start"]);
            expect(a.result.ok).toBe(true);
            expect(b.result.ok).toBe(true);
            expect(compatibilityProbes).toBe(1);
        } finally {
            rmSync(root, { recursive: true, force: true });
        }
    }, 20_000);

    test("a mutation whose child never ran does not invalidate a pending probe", async () => {
        const root = tempDir("eidnara-policy-probe-no-child-");
        const { binary, invocationLog } = fakeBinary(root);
        let release: () => void = () => {};
        try {
            const policy = policyFor({
                env: { XDG_DATA_HOME: root },
                launchTarget: { kind: "test-binary", path: binary },
                // A relative payload directory is rejected before any spawn.
                payloadDirFallback: () => "relative/package",
                compatibilityProbe: () =>
                    new Promise((resolve) => {
                        release = () => resolve(compatibleObservation());
                    }),
            });
            const demand = policy.demandStart({ origin: "managed-default", capability: "context" });
            await new Promise((resolve) => setTimeout(resolve, 300));
            const restarted = await policy.restart();
            expect(restarted.reason).toBe("internal_error");
            expect(restarted.effects).toEqual({ stop_committed: false, start_committed: false });

            release();
            const outcome = await demand;
            // The daemon was left exactly as the start found it, so the probe stands.
            expect(outcome.result.ok).toBe(true);
            expect(outcome.result.reason).toBe("started");
            expect(invocations(invocationLog)).toEqual(["start"]);
        } finally {
            rmSync(root, { recursive: true, force: true });
        }
    }, 20_000);

    test("a restart that committed no effects does not invalidate a pending probe", async () => {
        const root = tempDir("eidnara-policy-probe-restart-no-effects-");
        const invocationLog = path.join(root, "no-effects-invocations.log");
        const binary = path.join(root, "no-effects-eidnara-host.sh");
        // `restart` fails before touching the daemon and says so in its effects.
        const failedRestart = JSON.stringify({
            ...JSON.parse(startResultJson("restart")),
            ok: false,
            reason: "authentication_failed",
            remediation: "inspect_daemon_process",
            effects: { stop_committed: false, start_committed: false },
            versions: { ...JSON.parse(startResultJson("restart")).versions, proof: null },
        });
        writeFileSync(
            binary,
            `#!/bin/sh\necho "$1" >> ${invocationLog}\n` +
                `if [ "$1" = "restart" ]; then echo '${failedRestart}'; exit 1; fi\n` +
                `echo '${startResultJson("start")}'\nexit 0\n`,
        );
        chmodSync(binary, 0o700);
        let release: () => void = () => {};
        try {
            const policy = policyFor({
                env: { XDG_DATA_HOME: root },
                launchTarget: { kind: "test-binary", path: binary },
                compatibilityProbe: () =>
                    new Promise((resolve) => {
                        release = () => resolve(compatibleObservation());
                    }),
            });
            const demand = policy.demandStart({ origin: "managed-default", capability: "context" });
            await new Promise((resolve) => setTimeout(resolve, 300));
            const restarted = await policy.restart();
            expect(restarted.reason).toBe("authentication_failed");
            expect(restarted.effects).toEqual({ stop_committed: false, start_committed: false });

            release();
            const outcome = await demand;
            expect(outcome.result.ok).toBe(true);
            expect(outcome.result.reason).toBe("started");
            expect(invocations(invocationLog)).toEqual(["start", "restart"]);
        } finally {
            rmSync(root, { recursive: true, force: true });
        }
    }, 20_000);

    test("a fallback lookup that exhausts the budget after a found payload is a timeout", async () => {
        const root = tempDir("eidnara-policy-fallback-exhausts-");
        const invocationLog = path.join(root, "exhaust-invocations.log");
        const binary = path.join(root, "exhaust-eidnara-host.sh");
        writeFileSync(
            binary,
            `#!/bin/sh\necho "$*" >> ${invocationLog}\necho '${missingPayloadResultJson()}'\nexit 1\n`,
        );
        chmodSync(binary, 0o700);
        try {
            const policy = policyFor({
                env: { XDG_DATA_HOME: root },
                launchTarget: { kind: "test-binary", path: binary },
                outerAggregateMs: 500,
                payloadDirFallback: () => {
                    const until = Date.now() + 700;
                    while (Date.now() < until) {
                        // spin
                    }
                    return "/qualified/package";
                },
            });
            const result = await policy.start();
            // The found payload disproves `native_payload_missing`, but no retry budget remains.
            expect(result.reason).toBe("startup_timeout");
            expect(result.effects).toBeNull();
            expect(invocations(invocationLog)).toEqual(["start"]);
        } finally {
            rmSync(root, { recursive: true, force: true });
        }
    }, 20_000);

    test("a deadline spent before spawn reports known effects, not unknown ones", async () => {
        const root = tempDir("eidnara-policy-pre-spawn-timeout-");
        const { binary, invocationLog } = fakeBinary(root);
        try {
            const policy = policyFor({
                env: { XDG_DATA_HOME: root },
                launchTarget: { kind: "test-binary", path: binary },
                outerAggregateMs: 300,
            });
            // Serializing this envelope exceeds the aggregate deadline before any child exists.
            const slowEnvelope = {
                toJSON() {
                    const until = performance.now() + 400;
                    while (performance.now() < until) {
                        // spin
                    }
                    return { schema: 1 };
                },
            } as unknown as NativeStartupEnvelope;
            const restarted = await policy.restart(slowEnvelope);
            expect(restarted.reason).toBe("startup_timeout");
            // No process existed, so both effects are known false rather than null.
            expect(restarted.effects).toEqual({ stop_committed: false, start_committed: false });
            expect(invocations(invocationLog)).toEqual([]);
        } finally {
            rmSync(root, { recursive: true, force: true });
        }
    }, 20_000);

    test("a failed compatibility probe becomes a typed closed result, not a raw rejection", async () => {
        const root = tempDir("eidnara-policy-probe-failure-");
        const { binary, invocationLog } = fakeBinary(root);
        let storageProbes = 0;
        try {
            const policy = policyFor({
                env: { XDG_DATA_HOME: root },
                launchTarget: { kind: "test-binary", path: binary },
                compatibilityProbe: async () => {
                    throw new Error("authenticated peer changed during compatibility probe");
                },
                storageProbe: async () => {
                    storageProbes += 1;
                    return "ready";
                },
            });
            const outcome = await policy.demandStart({
                origin: "managed-default",
                capability: "context",
            });

            // An unproven compatibility claim authorizes no application traffic,
            // and callers act on the reason rather than an unclassified throw.
            expect(outcome.result.ok).toBe(false);
            expect(outcome.result.reason).toBe("native_probe_unavailable");
            expect(outcome.result.remediation).toBe("run_daemon_restart");
            expect(outcome.authenticatedDaemonId).toBeUndefined();
            expect(outcome.storage).toBeNull();
            expect(storageProbes).toBe(0);
            expect(invocations(invocationLog)).toEqual(["start"]);
        } finally {
            rmSync(root, { recursive: true, force: true });
        }
    });

    test("managed demand without a compatibility probe fails closed", async () => {
        const root = tempDir("eidnara-policy-no-compat-probe-");
        const { binary, invocationLog } = fakeBinary(root);
        let storageProbes = 0;
        try {
            const policy = policyFor({
                env: { XDG_DATA_HOME: root },
                launchTarget: { kind: "test-binary", path: binary },
                compatibilityProbe: undefined,
                storageProbe: async () => {
                    storageProbes += 1;
                    return "ready";
                },
            });
            const outcome = await policy.demandStart({
                origin: "managed-default",
                capability: "context",
            });

            // The native start succeeded, but nothing certified the incarnation.
            // A certified daemon ID is required before probing storage;
            // otherwise `undefined` disables client fencing.
            expect(outcome.result.ok).toBe(false);
            expect(outcome.result.reason).toBe("native_probe_unavailable");
            expect(outcome.result.remediation).toBe("run_daemon_restart");
            // The native start's `proof: "current"` is withdrawn with its success.
            expect(outcome.result.versions.proof).toBeNull();
            expect(() => parseDaemonResult(JSON.stringify(outcome.result))).not.toThrow();
            expect(outcome.authenticatedDaemonId).toBeUndefined();
            expect(outcome.storage).toBeNull();
            expect(storageProbes).toBe(0);
            expect(invocations(invocationLog)).toEqual(["start"]);
        } finally {
            rmSync(root, { recursive: true, force: true });
        }
    });

    test("storage readiness read on another incarnation is refused, not reported", async () => {
        const root = tempDir("eidnara-policy-storage-rotation-");
        const { binary, invocationLog } = fakeBinary(root);
        const expectations: Array<Uint8Array | undefined> = [];
        try {
            const policy = policyFor({
                env: { XDG_DATA_HOME: root },
                launchTarget: { kind: "test-binary", path: binary },
                compatibilityProbe: async () => compatibleObservation(),
                storageProbe: async (_budgetMs, expectedDaemonId) => {
                    expectations.push(expectedDaemonId);
                    throw new Error(
                        "storage probe observed a different daemon than compatibility certified",
                    );
                },
            });
            const outcome = await policy.demandStart({
                origin: "managed-default",
                capability: "context",
            });

            // The probe is told which incarnation was certified. If readiness
            // cannot be observed there, compatibility remains proven but storage
            // stays unavailable and application traffic remains blocked.
            expect(expectations).toEqual([new Uint8Array([7])]);
            expect(outcome.result.ok).toBe(true);
            expect(outcome.storage).toBe("unavailable");
            expect(outcome.authenticatedDaemonId).toEqual(new Uint8Array([7]));
            expect(invocations(invocationLog)).toEqual(["start"]);
        } finally {
            rmSync(root, { recursive: true, force: true });
        }
    });

    test("managed harness mismatch never preempts the running daemon", async () => {
        const root = tempDir("eidnara-policy-converge-");
        const invocationLog = path.join(root, "converge-invocations.log");
        const binary = path.join(root, "converge-eidnara-host.sh");
        writeFileSync(
            binary,
            `#!/bin/sh\necho "$1" >> ${invocationLog}\n` +
                `if [ "$1" = "start" ]; then echo '${harnessUnavailableResultJson()}'; exit 1; fi\n` +
                `echo '${restartResultJson()}'\nexit 0\n`,
        );
        chmodSync(binary, 0o700);
        try {
            const policy = policyFor({
                env: { XDG_DATA_HOME: root },
                launchTarget: { kind: "test-binary", path: binary },
            });
            const outcome = await policy.demandStart({
                origin: "managed-default",
                capability: "context",
                startupEnvelope: {
                    schema: 1,
                    credentials: { OPENAI_API_KEY: "shared-secret" },
                },
            });
            expect(outcome.result.command).toBe("start");
            expect(outcome.result.reason).toBe("harness_unavailable");
            expect(outcome.result.effects).toBeNull();
            expect(invocations(invocationLog)).toEqual(["start"]);
        } finally {
            rmSync(root, { recursive: true, force: true });
        }
    });

    test("non-managed origins are refused before any work", async () => {
        const policy = policyFor({ env: { HOME: "/nonexistent-home" } });
        for (const origin of ["explicit", "injected"] as const) {
            let rejected = false;
            try {
                await policy.demandStart({ origin, capability: "context" });
            } catch {
                rejected = true;
            }
            expect(rejected).toBe(true);
        }
    });

    test("concurrent managed demands share one native start", async () => {
        const root = tempDir("eidnara-policy-coalesce-");
        const { binary, invocationLog } = fakeBinary(root, { sleepSeconds: 1 });
        try {
            const policy = policyFor({
                env: { XDG_DATA_HOME: root },
                launchTarget: { kind: "test-binary", path: binary },
                storageProbe: async () => "ready",
            });
            const [a, b, c] = await Promise.all([
                policy.demandStart({ origin: "managed-default", capability: "context" }),
                policy.demandStart({ origin: "managed-default", capability: "context" }),
                policy.demandStart({ origin: "managed-default", capability: "context" }),
            ]);
            expect(a.result.reason).toBe("started");
            expect(b.result.reason).toBe("started");
            expect(c.result.reason).toBe("started");
            expect(a.storage).toBe("ready");
            expect(invocations(invocationLog)).toEqual(["start"]);
        } finally {
            rmSync(root, { recursive: true, force: true });
        }
    }, 20_000);

    test("demands coalesce only when their startup envelopes are the same request", async () => {
        const root = tempDir("eidnara-policy-coalesce-envelope-");
        const { binary, invocationLog } = fakeBinary(root, { sleepSeconds: 1 });
        try {
            const policy = policyFor({
                env: { XDG_DATA_HOME: root },
                launchTarget: { kind: "test-binary", path: binary },
                storageProbe: async () => "ready",
            });
            const [a, b, c, d] = await Promise.all([
                policy.demandStart({
                    origin: "managed-default",
                    capability: "context",
                    startupEnvelope: { schema: 1, credentials: { A: "1", B: "2" } },
                }),
                // Same envelope, different key order: one request, so it joins.
                policy.demandStart({
                    origin: "managed-default",
                    capability: "context",
                    startupEnvelope: { schema: 1, credentials: { B: "2", A: "1" } },
                }),
                // An optional field set to `undefined` is omitted on the wire, so requests with and without the field join.
                policy.demandStart({
                    origin: "managed-default",
                    capability: "context",
                    startupEnvelope: { schema: 1, credentials: { A: "1", B: "2" }, pi: undefined },
                }),
                // Different credentials require a separate native `start` request.
                policy.demandStart({
                    origin: "managed-default",
                    capability: "context",
                    startupEnvelope: { schema: 1, credentials: { A: "1", B: "changed" } },
                }),
            ]);
            expect(a.result.reason).toBe("started");
            expect(b.result.reason).toBe("started");
            expect(c.result.reason).toBe("started");
            expect(d.result.reason).toBe("started");
            expect(invocations(invocationLog)).toEqual(["start", "start"]);
        } finally {
            rmSync(root, { recursive: true, force: true });
        }
    }, 20_000);

    test("an omitted envelope coalesces with an explicit copy of the default", async () => {
        const root = tempDir("eidnara-policy-coalesce-default-envelope-");
        const { binary, invocationLog } = fakeBinary(root, { sleepSeconds: 1 });
        try {
            const policy = policyFor({
                env: { XDG_DATA_HOME: root },
                launchTarget: { kind: "test-binary", path: binary },
                defaultStartupEnvelope: { schema: 1, credentials: { KEY: "default" } },
                storageProbe: async () => "ready",
            });
            const [a, b] = await Promise.all([
                policy.demandStart({ origin: "managed-default", capability: "context" }),
                policy.demandStart({
                    origin: "managed-default",
                    capability: "context",
                    startupEnvelope: { schema: 1, credentials: { KEY: "default" } },
                }),
            ]);
            expect(a.result.reason).toBe("started");
            expect(b.result.reason).toBe("started");
            expect(invocations(invocationLog)).toEqual(["start"]);
        } finally {
            rmSync(root, { recursive: true, force: true });
        }
    }, 20_000);

    test("demands for different capabilities share the one host of their data root", async () => {
        const root = tempDir("eidnara-policy-coalesce-capability-");
        const { binary, invocationLog } = fakeBinary(root, { sleepSeconds: 1 });
        try {
            const policy = policyFor({
                env: { XDG_DATA_HOME: root },
                launchTarget: { kind: "test-binary", path: binary },
                storageProbe: async () => "ready",
            });
            const [magic, synapse] = await Promise.all([
                policy.demandStart({ origin: "managed-default", capability: "context" }),
                policy.demandStart({ origin: "managed-default", capability: "synapse" }),
            ]);
            expect(magic.result.reason).toBe("started");
            expect(synapse.result.reason).toBe("started");
            // One daemon serves every capability, so a capability-keyed second
            // start would race the first for the transaction lock.
            expect(invocations(invocationLog)).toEqual(["start"]);
        } finally {
            rmSync(root, { recursive: true, force: true });
        }
    }, 20_000);

    test("a detaching waiter neither cancels the start nor blocks later waiters", async () => {
        const root = tempDir("eidnara-policy-detach-");
        const { binary, invocationLog } = fakeBinary(root, { sleepSeconds: 1 });
        try {
            const policy = policyFor({
                env: { XDG_DATA_HOME: root },
                launchTarget: { kind: "test-binary", path: binary },
            });
            const controller = new AbortController();
            const cancelled = policy.demandStart({
                origin: "managed-default",
                capability: "context",
                signal: controller.signal,
            });
            const deadline = policy.demandStart({
                origin: "managed-default",
                capability: "context",
                deadlineMs: 50,
            });
            const survivor = policy.demandStart({
                origin: "managed-default",
                capability: "context",
            });
            controller.abort();
            let cancelledKind: string | null = null;
            try {
                await cancelled;
            } catch (error) {
                cancelledKind = (error as WaiterDetachedError).cause_kind;
            }
            let deadlineKind: string | null = null;
            try {
                await deadline;
            } catch (error) {
                deadlineKind = (error as WaiterDetachedError).cause_kind;
            }
            expect(cancelledKind).toBe("aborted");
            expect(deadlineKind).toBe("deadline");
            const outcome = await survivor;
            expect(outcome.result.reason).toBe("started");
            expect(invocations(invocationLog)).toEqual(["start"]);
        } finally {
            rmSync(root, { recursive: true, force: true });
        }
    }, 20_000);

    test("the caller deadline also bounds storage readiness", async () => {
        // The waiter has already cleared its timer and detached its abort
        // listener by the time the start resolves, so the storage probe needs a
        // bound of its own or it would keep `demandStart` pending forever with
        // nothing watching. Expiry remains caller detachment even though the
        // shared start itself succeeded.
        const root = tempDir("eidnara-policy-storage-deadline-");
        const { binary } = fakeBinary(root);
        const budgets: number[] = [];
        const CALLER_BUDGET_MS = 1_000;
        try {
            const policy = policyFor({
                env: { XDG_DATA_HOME: root },
                launchTarget: { kind: "test-binary", path: binary },
                storageProbe: async (budgetMs) => {
                    budgets.push(budgetMs);
                    // Outlasts both the caller budget and STORAGE_HARD_BUDGET_MS.
                    await new Promise((resolve) => setTimeout(resolve, 30_000));
                    return "ready";
                },
            });
            await expect(
                policy.demandStart({
                    origin: "managed-default",
                    capability: "context",
                    deadlineMs: CALLER_BUDGET_MS,
                }),
            ).rejects.toMatchObject({
                name: "WaiterDetachedError",
                cause_kind: "deadline",
            });
            // The caller's residual budget bounded the probe, not the 5s hard cap.
            expect(budgets).toHaveLength(1);
            expect(budgets[0]).toBeLessThanOrEqual(CALLER_BUDGET_MS);
        } finally {
            rmSync(root, { recursive: true, force: true });
        }
    }, 20_000);

    test("caller cancellation during storage readiness stays detached", async () => {
        const root = tempDir("eidnara-policy-storage-abort-");
        const { binary } = fakeBinary(root);
        const controller = new AbortController();
        let probeSignal: AbortSignal | undefined;
        try {
            const policy = policyFor({
                env: { XDG_DATA_HOME: root },
                launchTarget: { kind: "test-binary", path: binary },
                storageProbe: (_budgetMs, _expectedDaemonId, signal) => {
                    probeSignal = signal;
                    queueMicrotask(() => controller.abort());
                    return new Promise<never>(() => {});
                },
            });

            await expect(
                policy.demandStart({
                    origin: "managed-default",
                    capability: "context",
                    signal: controller.signal,
                }),
            ).rejects.toMatchObject({
                name: "WaiterDetachedError",
                cause_kind: "aborted",
            });
            expect(probeSignal).toBe(controller.signal);
            expect(probeSignal?.aborted).toBe(true);
        } finally {
            rmSync(root, { recursive: true, force: true });
        }
    }, 20_000);

    test("the waiter budget is spent from the call, not from after preflight", async () => {
        // `start()` is async, but its synchronous prefix — root resolution,
        // filesystem admission, and the platform gate — runs inside the
        // `this.start()` call, before the promise reaches the waiter. If the
        // waiter armed the full `deadlineMs` at that point, a caller whose
        // budget was already spent would still be handed a successful result.
        const root = tempDir("eidnara-policy-residual-");
        try {
            const { binary } = fakeBinary(root, {});
            const SYNC_MS = 400;
            const slowGate: PlatformReaders = {
                platform: "linux",
                arch: "x64",
                kernelRelease: () => "6.1.0",
                glibcVersion: () => {
                    const until = Date.now() + SYNC_MS;
                    while (Date.now() < until) {
                        /* burn the caller's budget before the waiter attaches */
                    }
                    return "2.34";
                },
                procSelfFdUsable: () => true,
            };
            const policy = policyFor({
                env: { XDG_DATA_HOME: root },
                launchTarget: { kind: "test-binary", path: binary },
                platformReaders: slowGate,
            });
            let kind: string | null = null;
            let accepted: string | null = null;
            try {
                const outcome = await policy.demandStart({
                    origin: "managed-default",
                    capability: "context",
                    deadlineMs: SYNC_MS / 4,
                });
                accepted = outcome.result.reason;
            } catch (error) {
                kind = (error as WaiterDetachedError).cause_kind;
            }
            // The budget was already gone when the waiter attached, so it must
            // detach rather than accept the start that lands right after.
            expect(accepted).toBeNull();
            expect(kind).toBe("deadline");
        } finally {
            rmSync(root, { recursive: true, force: true });
        }
    }, 20_000);

    test("an envelope whose serialization spends the deadline never spawns a start", async () => {
        const root = tempDir("eidnara-policy-slow-envelope-key-");
        const { binary, invocationLog } = fakeBinary(root);
        try {
            const policy = policyFor({
                env: { XDG_DATA_HOME: root },
                launchTarget: { kind: "test-binary", path: binary },
            });
            // The coalescing key serializes the envelope before any start exists.
            const slowEnvelope = {
                toJSON() {
                    const until = performance.now() + 300;
                    while (performance.now() < until) {
                        // spin
                    }
                    return { schema: 1 };
                },
            } as unknown as NativeStartupEnvelope;
            await expect(
                policy.demandStart({
                    origin: "managed-default",
                    capability: "context",
                    deadlineMs: 150,
                    startupEnvelope: slowEnvelope,
                }),
            ).rejects.toMatchObject({ name: "WaiterDetachedError", cause_kind: "deadline" });
            // A caller that had already expired must not have launched a child.
            expect(invocations(invocationLog)).toEqual([]);
            expect(policy.inflightStartCount).toBe(0);
        } finally {
            rmSync(root, { recursive: true, force: true });
        }
    }, 20_000);

    test("an envelope whose serialization spends the aggregate never spawns a start", async () => {
        const root = tempDir("eidnara-policy-slow-envelope-aggregate-");
        const { binary, invocationLog } = fakeBinary(root);
        try {
            const policy = policyFor({
                env: { XDG_DATA_HOME: root },
                launchTarget: { kind: "test-binary", path: binary },
                outerAggregateMs: 200,
            });
            // The coalescing key serializes first, so it consumes the aggregate before launcher serialization.
            let serialized = false;
            const slowEnvelope = {
                toJSON() {
                    if (!serialized) {
                        serialized = true;
                        const until = performance.now() + 350;
                        while (performance.now() < until) {
                            // spin
                        }
                    }
                    return { schema: 1 };
                },
            } as unknown as NativeStartupEnvelope;
            // No caller deadline: the policy aggregate is the only bound.
            const outcome = await policy.demandStart({
                origin: "managed-default",
                capability: "context",
                startupEnvelope: slowEnvelope,
            });
            expect(outcome.result.reason).toBe("startup_timeout");
            expect(outcome.result.ok).toBe(false);
            expect(invocations(invocationLog)).toEqual([]);
            expect(policy.inflightStartCount).toBe(0);
        } finally {
            rmSync(root, { recursive: true, force: true });
        }
    }, 20_000);

    test("the shared start is bounded by what the demand's aggregate has left", async () => {
        const root = tempDir("eidnara-policy-start-within-aggregate-");
        const { binary, invocationLog } = fakeBinary(root, { sleepSeconds: 1 });
        try {
            const policy = policyFor({
                env: { XDG_DATA_HOME: root },
                launchTarget: { kind: "test-binary", path: binary },
                outerAggregateMs: 600,
            });
            // Key serialization consumes 300 ms of the demand's 600 ms aggregate; the start receives a fresh aggregate.
            let serialized = false;
            const slowEnvelope = {
                toJSON() {
                    if (!serialized) {
                        serialized = true;
                        const until = performance.now() + 300;
                        while (performance.now() < until) {
                            // spin
                        }
                    }
                    return { schema: 1 };
                },
            } as unknown as NativeStartupEnvelope;
            const startedAt = performance.now();
            const outcome = await policy.demandStart({
                origin: "managed-default",
                capability: "context",
                startupEnvelope: slowEnvelope,
            });
            // The demand answers at its own aggregate, not after the start's.
            expect(performance.now() - startedAt).toBeLessThan(600 + 250);
            expect(outcome.result.reason).toBe("startup_timeout");
            // The start launched despite the demand timeout.
            expect(invocations(invocationLog)).toEqual(["start"]);
        } finally {
            rmSync(root, { recursive: true, force: true });
        }
    }, 20_000);

    test("a stateful envelope serializer cannot coalesce demands that would launch differently", async () => {
        const root = tempDir("eidnara-policy-stateful-envelope-");
        const stdinLog = path.join(root, "stdin.log");
        const binary = path.join(root, "stateful-eidnara-host.sh");
        writeFileSync(
            binary,
            `#!/bin/sh\ncat >> ${stdinLog}\necho >> ${stdinLog}\nsleep 1\necho '${startResultJson("start")}'\nexit 0\n`,
        );
        chmodSync(binary, 0o700);
        try {
            const policy = policyFor({
                env: { XDG_DATA_HOME: root },
                launchTarget: { kind: "test-binary", path: binary },
                storageProbe: async () => "ready",
            });
            // `shifty` returns one credential set on its first serialization and another on later serializations.
            const shifty = (first: string, later: string) => {
                let calls = 0;
                return {
                    toJSON() {
                        calls += 1;
                        return { schema: 1, credentials: { KEY: calls === 1 ? first : later } };
                    },
                } as unknown as NativeStartupEnvelope;
            };
            const [a, b] = await Promise.all([
                policy.demandStart({
                    origin: "managed-default",
                    capability: "context",
                    startupEnvelope: shifty("same", "alpha"),
                }),
                policy.demandStart({
                    origin: "managed-default",
                    capability: "context",
                    startupEnvelope: shifty("same", "beta"),
                }),
            ]);
            expect(a.result.reason).toBe("started");
            expect(b.result.reason).toBe("started");
            // The two demands use their first serialization as the coalescing key, and the child receives that snapshot rather than a second serialization.
            const received = readFileSync(stdinLog, "utf8").trim().split("\n");
            expect(received).toEqual([JSON.stringify({ schema: 1, credentials: { KEY: "same" } })]);
        } finally {
            rmSync(root, { recursive: true, force: true });
        }
    }, 20_000);

    test("an envelope serializer that aborts the caller never spawns a start", async () => {
        const root = tempDir("eidnara-policy-envelope-aborts-");
        const { binary, invocationLog } = fakeBinary(root);
        const controller = new AbortController();
        try {
            const policy = policyFor({
                env: { XDG_DATA_HOME: root },
                launchTarget: { kind: "test-binary", path: binary },
            });
            const abortingEnvelope = {
                toJSON() {
                    controller.abort();
                    return { schema: 1 };
                },
            } as unknown as NativeStartupEnvelope;
            await expect(
                policy.demandStart({
                    origin: "managed-default",
                    capability: "context",
                    signal: controller.signal,
                    startupEnvelope: abortingEnvelope,
                }),
            ).rejects.toMatchObject({ name: "WaiterDetachedError", cause_kind: "aborted" });
            expect(invocations(invocationLog)).toEqual([]);
            expect(policy.inflightStartCount).toBe(0);
        } finally {
            rmSync(root, { recursive: true, force: true });
        }
    }, 20_000);

    test("an already-expired deadline detaches instead of taking a settled result", async () => {
        // This root resolution fails synchronously, so the shared start is
        // already settled when the waiter attaches and only the guard can stop
        // its microtask from beating the timer.
        const policy = policyFor({ env: { HOME: "relative-home" } });
        for (const deadlineMs of [0, -50]) {
            let kind: string | null = null;
            try {
                await policy.demandStart({
                    origin: "managed-default",
                    capability: "context",
                    deadlineMs,
                });
            } catch (error) {
                kind = (error as WaiterDetachedError).cause_kind;
            }
            expect(kind).toBe("deadline");
        }
    });

    test("settled shared promises are evicted: a later demand starts again", async () => {
        const root = tempDir("eidnara-policy-evict-");
        const { binary, invocationLog } = fakeBinary(root);
        try {
            const policy = policyFor({
                env: { XDG_DATA_HOME: root },
                launchTarget: { kind: "test-binary", path: binary },
            });
            await policy.demandStart({ origin: "managed-default", capability: "context" });
            expect(policy.inflightStartCount).toBe(0);
            await policy.demandStart({ origin: "managed-default", capability: "context" });
            expect(invocations(invocationLog)).toEqual(["start", "start"]);
        } finally {
            rmSync(root, { recursive: true, force: true });
        }
    }, 20_000);

    test("storage readiness rides the outcome for context capability only", async () => {
        const root = tempDir("eidnara-policy-storage-");
        const { binary } = fakeBinary(root);
        try {
            const probes: number[] = [];
            const policy = policyFor({
                env: { XDG_DATA_HOME: root },
                launchTarget: { kind: "test-binary", path: binary },
                storageProbe: async (budgetMs) => {
                    probes.push(budgetMs);
                    return "starting";
                },
            });
            const magic = await policy.demandStart({
                origin: "managed-default",
                capability: "context",
            });
            expect(magic.storage).toBe("starting");
            expect(probes).toEqual([5_000]);
            const synapse = await policy.demandStart({
                origin: "managed-default",
                capability: "synapse",
            });
            expect(synapse.storage).toBeNull();
            expect(probes).toEqual([5_000]);
        } finally {
            rmSync(root, { recursive: true, force: true });
        }
    }, 20_000);

    test("an unwired storage probe reports unavailable, never ready", async () => {
        const root = tempDir("eidnara-policy-storage-default-");
        const { binary } = fakeBinary(root);
        try {
            // No storageProbe: the gate must not authorize a body on a daemon
            // whose storage state was never examined.
            const policy = policyFor({
                env: { XDG_DATA_HOME: root },
                launchTarget: { kind: "test-binary", path: binary },
            });
            const outcome = await policy.demandStart({
                origin: "managed-default",
                capability: "context",
            });
            expect(outcome.result.ok).toBe(true);
            expect(outcome.storage).toBe("unavailable");
        } finally {
            rmSync(root, { recursive: true, force: true });
        }
    }, 20_000);

    test("a hanging storage probe detaches at the caller deadline", async () => {
        const root = tempDir("eidnara-policy-storage-hang-");
        const { binary } = fakeBinary(root);
        try {
            const policy = policyFor({
                env: { XDG_DATA_HOME: root },
                launchTarget: { kind: "test-binary", path: binary },
                // Never settles; the policy's own bound must end the wait.
                storageProbe: () => new Promise<never>(() => {}),
            });
            await expect(
                policy.demandStart({
                    origin: "managed-default",
                    capability: "context",
                    deadlineMs: 250,
                }),
            ).rejects.toMatchObject({
                name: "WaiterDetachedError",
                cause_kind: "deadline",
            });
        } finally {
            rmSync(root, { recursive: true, force: true });
        }
    }, 20_000);

    test("a hanging storage probe under the policy's own cap degrades to unavailable", async () => {
        const root = tempDir("eidnara-policy-storage-hard-cap-");
        const { binary } = fakeBinary(root);
        try {
            const policy = policyFor({
                env: { XDG_DATA_HOME: root },
                launchTarget: { kind: "test-binary", path: binary },
                storageProbe: () => new Promise<never>(() => {}),
            });
            // No caller deadline: the storage cap expiring makes storage
            // unavailable rather than detaching the caller.
            const startedAt = performance.now();
            const outcome = await policy.demandStart({
                origin: "managed-default",
                capability: "context",
            });
            expect(performance.now() - startedAt).toBeGreaterThanOrEqual(
                STORAGE_HARD_BUDGET_MS - 50,
            );
            expect(outcome.result.reason).toBe("started");
            expect(outcome.storage).toBe("unavailable");
            expect(outcome.authenticatedDaemonId).toEqual(new Uint8Array([7]));
        } finally {
            rmSync(root, { recursive: true, force: true });
        }
    }, 20_000);

    test("a rejecting storage probe still returns the successful start result", async () => {
        const root = tempDir("eidnara-policy-storage-reject-");
        const { binary } = fakeBinary(root);
        try {
            const policy = policyFor({
                env: { XDG_DATA_HOME: root },
                launchTarget: { kind: "test-binary", path: binary },
                storageProbe: async () => {
                    throw new Error("probe exploded");
                },
            });
            const outcome = await policy.demandStart({
                origin: "managed-default",
                capability: "context",
            });
            expect(outcome.result.reason).toBe("started");
            expect(outcome.storage).toBe("unavailable");
        } finally {
            rmSync(root, { recursive: true, force: true });
        }
    }, 20_000);

    test("a synchronously throwing storage probe degrades to unavailable", async () => {
        const root = tempDir("eidnara-policy-storage-throw-sync-");
        const { binary } = fakeBinary(root);
        try {
            const policy = policyFor({
                env: { XDG_DATA_HOME: root },
                launchTarget: { kind: "test-binary", path: binary },
                // Throws before it ever returns a promise, so no rejection
                // handler on the probe can catch it.
                storageProbe: () => {
                    throw new Error("probe exploded synchronously");
                },
            });
            const outcome = await policy.demandStart({
                origin: "managed-default",
                capability: "context",
            });
            expect(outcome.result.reason).toBe("started");
            expect(outcome.storage).toBe("unavailable");
        } finally {
            rmSync(root, { recursive: true, force: true });
        }
    }, 20_000);

    test("a storage probe whose synchronous prefix outlasts the caller deadline detaches", async () => {
        const root = tempDir("eidnara-policy-storage-sync-prefix-");
        const { binary } = fakeBinary(root);
        try {
            const policy = policyFor({
                env: { XDG_DATA_HOME: root },
                launchTarget: { kind: "test-binary", path: binary },
                storageProbe: () => {
                    const until = performance.now() + 1_200;
                    while (performance.now() < until) {
                        // spin
                    }
                    return Promise.resolve("ready" as const);
                },
            });
            await expect(
                policy.demandStart({
                    origin: "managed-default",
                    capability: "context",
                    deadlineMs: 1_000,
                }),
            ).rejects.toMatchObject({
                name: "WaiterDetachedError",
                cause_kind: "deadline",
            });
        } finally {
            rmSync(root, { recursive: true, force: true });
        }
    }, 20_000);

    test("a storage probe that answers after the caller deadline still detaches", async () => {
        const root = tempDir("eidnara-policy-storage-at-deadline-");
        const { binary } = fakeBinary(root);
        try {
            const policy = policyFor({
                env: { XDG_DATA_HOME: root },
                launchTarget: { kind: "test-binary", path: binary },
                // The probe blocks the event loop past its deadline so its resolution microtask runs before the policy deadline timer.
                storageProbe: (budgetMs) =>
                    new Promise((resolve) => {
                        const budgetEndsAt = performance.now() + budgetMs;
                        setTimeout(() => {
                            while (performance.now() < budgetEndsAt + 30) {
                                // spin
                            }
                            resolve("starting");
                        }, budgetMs - 20);
                    }),
            });
            await expect(
                policy.demandStart({
                    origin: "managed-default",
                    capability: "context",
                    deadlineMs: 800,
                }),
            ).rejects.toMatchObject({
                name: "WaiterDetachedError",
                cause_kind: "deadline",
            });
        } finally {
            rmSync(root, { recursive: true, force: true });
        }
    }, 20_000);
});

describe("native result labeling and indeterminate effects", () => {
    test("a child answering a different command is internal_error, not relabeled", async () => {
        const root = tempDir("eidnara-policy-mislabel-");
        try {
            // Always answers `stop`, whatever it was asked. `stop` is inside the
            // contract's command union, so the payload parses and the
            // disagreement is caught by the label check rather than by the
            // schema.
            const binary = path.join(root, "mislabeling-host.sh");
            writeFileSync(
                binary,
                `#!/bin/sh\necho '${startResultJson("stop").replace("started", "stopped")}'\nexit 0\n`,
            );
            chmodSync(binary, 0o700);
            const policy = policyFor({
                env: { XDG_DATA_HOME: root },
                launchTarget: { kind: "test-binary", path: binary },
            });
            const result = await policy.start();
            expect(result.command).toBe("start");
            expect(result.ok).toBe(false);
            expect(result.reason).toBe("internal_error");
            // Never a `restart` payload wearing a `start` label.
            expect(result.effects).toBeNull();
        } finally {
            rmSync(root, { recursive: true, force: true });
        }
    }, 20_000);

    test("a restart killed at the deadline reports unknown effects, not false,false", async () => {
        const root = tempDir("eidnara-policy-restart-timeout-");
        try {
            const binary = path.join(root, "hanging-host.sh");
            writeFileSync(binary, "#!/bin/sh\nsleep 30\n");
            chmodSync(binary, 0o700);
            const policy = policyFor({
                env: { XDG_DATA_HOME: root },
                launchTarget: { kind: "test-binary", path: binary },
                outerAggregateMs: 250,
            });
            const result = await policy.restart();
            expect(result.command).toBe("restart");
            expect(result.reason).toBe("startup_timeout");
            // The native transaction was SIGKILLed mid-flight: the stop may
            // already have committed, so its effects are unknown.
            expect(result.effects).toBeNull();
        } finally {
            rmSync(root, { recursive: true, force: true });
        }
    }, 20_000);

    test("a child killed at the deadline reports the reason its command earned", async () => {
        const root = tempDir("eidnara-policy-timeout-reasons-");
        try {
            const binary = path.join(root, "hanging-host.sh");
            writeFileSync(binary, "#!/bin/sh\nsleep 30\n");
            chmodSync(binary, 0o700);
            const policy = policyFor({
                env: { XDG_DATA_HOME: root },
                launchTarget: { kind: "test-binary", path: binary },
                outerAggregateMs: 250,
            });
            const expected = {
                start: { reason: "startup_timeout", state: "stopped" },
                restart: { reason: "startup_timeout", state: "stopped" },
                // `shutdown_timeout` requires `stopping`; `stopped` fails parser validation.
                stop: { reason: "shutdown_timeout", state: "stopping" },
                // A read-only probe ran out of time; no startup was attempted.
                status: { reason: "native_probe_unavailable", state: "stopped" },
                doctor: { reason: "native_probe_unavailable", state: "stopped" },
            } as const;
            for (const op of ["start", "restart", "stop", "status", "doctor"] as const) {
                const result = await policy[op]();
                expect(result.command).toBe(op);
                expect(result.reason).toBe(expected[op].reason);
                expect(result.state).toBe(expected[op].state);
                expect(() => parseDaemonResult(JSON.stringify(result))).not.toThrow();
            }
        } finally {
            rmSync(root, { recursive: true, force: true });
        }
    }, 20_000);
});
