import { describe, expect, test } from "bun:test";
import { mkdirSync, mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { pathToFileURL } from "node:url";
import kernelHealthBlocks from "../../../../../crates/daemon/tests/fixtures/kernel-health-blocks.json";
import hostRelease from "../../../../../release/host-release.json";
import type { AuthenticatedPeer, CatalogEntry } from "../host-client";
import { evaluateCompatibility } from "./compatibility";
import {
    type CompatibilityProbeResult,
    createManagedLifecyclePolicy,
    kernelReadiness,
    type ManagedCompatibilityClient,
    managedProbes,
    readCompatibilitySnapshot,
    synapseReadiness,
} from "./managed-policy";
import { STORAGE_HARD_BUDGET_MS } from "./policy";

function entry(moduleId: string, moduleVersion = "0.1.0"): CatalogEntry {
    return {
        module_id: moduleId,
        module_version: moduleVersion,
        roles: [],
        control_ops: [],
    };
}

const catalog = [entry("context"), entry("synapse"), entry("broca")];

function peer(daemonVer = hostRelease.versions.daemon, daemonId = 7): AuthenticatedPeer {
    return {
        daemonVer,
        daemonId: new Uint8Array([daemonId]),
        proof: "current",
    };
}

/**
 */
function wireEpochs(overrides: Record<string, unknown> = {}) {
    return {
        epochs: {
            memory_render_epoch: hostRelease.epochs.memory_render,
            compartment_render_epoch: hostRelease.epochs.compartment_render,
            profile_epoch: hostRelease.epochs.profile_claude_code_anthropic,
            tagger_epoch: hostRelease.epochs.tagger,
            state_sync_epoch: hostRelease.epochs.state_sync,
            ...overrides,
        },
    };
}

function client(options: {
    authenticated?: AuthenticatedPeer;
    catalog?: CatalogEntry[];
    status?: unknown;
    calls: string[];
}): ManagedCompatibilityClient {
    return {
        authenticated: options.authenticated ?? peer(),
        catalogList: async () => {
            options.calls.push("catalog.list");
            return options.catalog ?? catalog;
        },
        hostStatus: async () => {
            options.calls.push("host.status");
            return {
                health: "ok",
                metrics: {
                    components: {
                        context: {
                            metrics: options.status ?? wireEpochs(),
                        },
                    },
                },
            };
        },
    };
}

function verdict(snapshot: Awaited<ReturnType<typeof readCompatibilitySnapshot>>) {
    return evaluateCompatibility({
        authenticatedPeer: snapshot.authenticatedPeer,
        catalog: snapshot.catalog,
        epochs: snapshot.epochs,
    });
}

describe("managed authenticated compatibility probe", () => {
    test("daemon mismatch sends no catalog or host status request", async () => {
        const calls: string[] = [];
        const snapshot = await readCompatibilitySnapshot(
            client({ authenticated: peer("eidnara-host/0.2.0"), calls }),
            performance.now() + 1_000,
        );

        expect(verdict(snapshot)).toMatchObject({
            ok: false,
            reason: "incompatible_daemon",
        });
        expect(snapshot.evaluatedThrough).toBe("daemon");
        expect(calls).toEqual([]);
    });

    test("module mismatch stops before the host status request", async () => {
        const calls: string[] = [];
        const snapshot = await readCompatibilitySnapshot(
            client({
                catalog: catalog.map((candidate) =>
                    candidate.module_id === "broca" ? entry("broca", "0.2.0") : candidate,
                ),
                calls,
            }),
            performance.now() + 1_000,
        );

        expect(verdict(snapshot)).toMatchObject({
            ok: false,
            reason: "incompatible_module",
        });
        expect(snapshot.evaluatedThrough).toBe("modules");
        expect(calls).toEqual(["catalog.list"]);
    });

    test("a fully compatible daemon reaches the epoch stage and passes", async () => {
        const calls: string[] = [];
        const snapshot = await readCompatibilitySnapshot(
            client({ calls }),
            performance.now() + 1_000,
        );

        expect(verdict(snapshot).ok).toBe(true);
        expect(snapshot.evaluatedThrough).toBe("epochs");
        expect(snapshot.epochs).toEqual({ ...hostRelease.epochs });
        expect(calls).toEqual(["catalog.list", "host.status"]);
    });

    test("epoch mismatch uses one bounded host status request", async () => {
        const calls: string[] = [];
        const snapshot = await readCompatibilitySnapshot(
            client({
                status: wireEpochs({
                    state_sync_epoch: hostRelease.epochs.state_sync + 1,
                }),
                calls,
            }),
            performance.now() + 1_000,
        );

        expect(verdict(snapshot)).toMatchObject({
            ok: false,
            reason: "incompatible_epochs",
        });
        expect(snapshot.evaluatedThrough).toBe("epochs");
        expect(calls).toEqual(["catalog.list", "host.status"]);
    });

    test("daemon rotation rejects a mixed compatibility snapshot", async () => {
        const calls: string[] = [];
        let authenticated = peer();
        const rotating: ManagedCompatibilityClient = {
            get authenticated() {
                return authenticated;
            },
            catalogList: async () => {
                calls.push("catalog.list");
                authenticated = peer(hostRelease.versions.daemon, 8);
                return catalog;
            },
            hostStatus: async () => ({ health: "ok", metrics: {} }),
        };

        await expect(
            readCompatibilitySnapshot(rotating, performance.now() + 1_000),
        ).rejects.toThrow("authenticated peer changed");
        expect(calls).toEqual(["catalog.list"]);
    });

    test("detachment while catalog is pending sends no host status request", async () => {
        const calls: string[] = [];
        const controller = new AbortController();
        const detaching = client({ calls });
        detaching.catalogList = async () => {
            calls.push("catalog.list");
            controller.abort(new Error("detached"));
            return catalog;
        };

        await expect(
            readCompatibilitySnapshot(detaching, performance.now() + 1_000, controller.signal),
        ).rejects.toThrow("detached");
        expect(calls).toEqual(["catalog.list"]);
    });

    test("an expired probe deadline sends no host status request", async () => {
        const calls: string[] = [];
        const expired = client({ calls });
        expired.catalogList = async () => {
            calls.push("catalog.list");
            await new Promise((resolve) => setTimeout(resolve, 5));
            return catalog;
        };

        await expect(readCompatibilitySnapshot(expired, performance.now() + 1)).rejects.toThrow(
            "deadline expired",
        );
        expect(calls).toEqual(["catalog.list"]);
    });

    test("catalog collection spends only the time left until the probe deadline", async () => {
        const calls: string[] = [];
        const timeouts: Array<number | undefined> = [];
        const bounded = client({ calls });
        bounded.catalogList = async (options) => {
            calls.push("catalog.list");
            timeouts.push(options?.timeoutMs);
            return catalog;
        };

        await readCompatibilitySnapshot(bounded, performance.now() + 40);

        expect(timeouts).toHaveLength(1);
        expect(timeouts[0]).toBeGreaterThan(0);
        expect(timeouts[0]).toBeLessThanOrEqual(40);
    });

    test("an already-expired deadline sends no catalog request", async () => {
        const calls: string[] = [];
        await expect(
            readCompatibilitySnapshot(client({ calls }), performance.now() - 1),
        ).rejects.toThrow("deadline expired");
        expect(calls).toEqual([]);
    });
});

describe("managed observational platform gate", () => {
    test("policy construction preserves each unsupported platform verdict", async () => {
        const root = mkdtempSync(join(tmpdir(), "eidnara-managed-platform-"));
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
                const policy = createManagedLifecyclePolicy({
                    mode: "observational",
                    declaringModuleUrl: import.meta.url,
                    parentPackageName: "@eidnara/cli",
                    env: { XDG_DATA_HOME: root },
                    platformReaders,
                });
                for (const result of [await policy.status(), await policy.doctor()]) {
                    expect(result.reason).toBe("unsupported_platform");
                    expect(result.remediation).toBe("use_supported_platform");
                }
            }
        } finally {
            rmSync(root, { recursive: true, force: true });
        }
    });
});

describe("managed payload discovery", () => {
    const supportedLinux = {
        platform: "linux" as const,
        arch: "x64",
        kernelRelease: () => "6.8.0",
        glibcVersion: () => "2.39",
        procSelfFdUsable: () => true,
    };
    const admissionIo = {
        platform: "linux" as const,
        readMounts: () => "/dev/root / ext4 rw 0 0\n",
    };
    // Compiled Bun binaries load declaring modules from an embedded filesystem, so `orphanModuleUrl` has no ancestor `package.json`.
    const orphanModuleUrl = "file:///nonexistent-compiled-root/bin/main.js";

    // Only mutating commands surface the bootstrap failure; observation answers from the pre-native classifier instead.
    test("an explicit external root is examined without the declaring parent walk", async () => {
        const root = mkdtempSync(join(tmpdir(), "eidnara-managed-data-"));
        const external = mkdtempSync(join(tmpdir(), "eidnara-managed-external-"));
        try {
            // The payload directory exists but carries no manifest, so resolution succeeds and verification is the first stage to fail.
            mkdirSync(
                join(external, "node_modules", ...hostRelease.packages.payloads[0].split("/")),
                {
                    recursive: true,
                },
            );
            const policy = createManagedLifecyclePolicy({
                mode: "mutating",
                declaringModuleUrl: orphanModuleUrl,
                parentPackageName: "@eidnara/cli",
                explicitExternalRoot: external,
                env: { XDG_DATA_HOME: root },
                platformReaders: supportedLinux,
                admissionIo,
            });
            expect((await policy.start()).reason).toBe("native_payload_invalid");
        } finally {
            rmSync(root, { recursive: true, force: true });
            rmSync(external, { recursive: true, force: true });
        }
    });

    test("without an external root the declaring parent walk still gates the layout", async () => {
        const root = mkdtempSync(join(tmpdir(), "eidnara-managed-data-"));
        try {
            const policy = createManagedLifecyclePolicy({
                mode: "mutating",
                declaringModuleUrl: orphanModuleUrl,
                parentPackageName: "@eidnara/cli",
                env: { XDG_DATA_HOME: root },
                platformReaders: supportedLinux,
                admissionIo,
            });
            expect((await policy.start()).reason).toBe("unsupported_install_layout");
        } finally {
            rmSync(root, { recursive: true, force: true });
        }
    });

    test("a malformed descriptor between the module and its package stops the walk", async () => {
        const root = mkdtempSync(join(tmpdir(), "eidnara-managed-data-"));
        const install = mkdtempSync(join(tmpdir(), "eidnara-managed-install-"));
        try {
            // The farther ancestor carries the requested name; the nearer descriptor is present but unparseable, so climbing past it would certify the farther install.
            writeFileSync(join(install, "package.json"), JSON.stringify({ name: "@eidnara/cli" }));
            const nested = join(install, "nested");
            mkdirSync(join(nested, "dist"), { recursive: true });
            writeFileSync(join(nested, "package.json"), "{ not json");
            const policy = createManagedLifecyclePolicy({
                mode: "mutating",
                declaringModuleUrl: pathToFileURL(join(nested, "dist", "main.js")).href,
                parentPackageName: "@eidnara/cli",
                env: { XDG_DATA_HOME: root },
                platformReaders: supportedLinux,
                admissionIo,
            });
            expect((await policy.start()).reason).toBe("unsupported_install_layout");
        } finally {
            rmSync(root, { recursive: true, force: true });
            rmSync(install, { recursive: true, force: true });
        }
    });

    test("an unrelated readable descriptor still lets the walk reach the declaring package", async () => {
        const root = mkdtempSync(join(tmpdir(), "eidnara-managed-data-"));
        const install = mkdtempSync(join(tmpdir(), "eidnara-managed-install-"));
        try {
            writeFileSync(join(install, "package.json"), JSON.stringify({ name: "@eidnara/cli" }));
            const nested = join(install, "dist");
            mkdirSync(nested, { recursive: true });
            writeFileSync(join(nested, "package.json"), JSON.stringify({ type: "module" }));
            const policy = createManagedLifecyclePolicy({
                mode: "mutating",
                declaringModuleUrl: pathToFileURL(join(nested, "main.js")).href,
                parentPackageName: "@eidnara/cli",
                env: { XDG_DATA_HOME: root },
                platformReaders: supportedLinux,
                admissionIo,
            });
            // The walk found the declaring package; the failure is the payload lookup beneath it.
            expect((await policy.start()).reason).toBe("native_payload_missing");
        } finally {
            rmSync(root, { recursive: true, force: true });
            rmSync(install, { recursive: true, force: true });
        }
    });

    test("commands and probes read the environment the policy was built from", async () => {
        const root = mkdtempSync(join(tmpdir(), "eidnara-managed-data-"));
        try {
            const env: Record<string, string | undefined> = { XDG_DATA_HOME: root };
            const policy = createManagedLifecyclePolicy({
                mode: "mutating",
                declaringModuleUrl: orphanModuleUrl,
                parentPackageName: "@eidnara/cli",
                env,
                platformReaders: supportedLinux,
                admissionIo,
            });
            delete env.XDG_DATA_HOME;
            // A policy reading the live object would now resolve no data root at all.
            expect((await policy.start()).reason).toBe("unsupported_install_layout");
        } finally {
            rmSync(root, { recursive: true, force: true });
        }
    });
});

describe("managed probes", () => {
    const daemon = (id: number) => new Uint8Array([id]);
    const result = (daemonId: Uint8Array, storage: string | null): CompatibilityProbeResult => ({
        snapshot: {
            authenticatedPeer: {
                daemonVer: hostRelease.versions.daemon,
                daemonId,
                proof: "current",
            },
            catalog,
            epochs: {},
        },
        status:
            storage === null
                ? null
                : {
                      health: "ok",
                      metrics: {
                          components: { context: { metrics: { storage_state: storage } } },
                      },
                  },
    });

    test("every waiter of one compatibility probe reads its terminal storage observation", async () => {
        let polls = 0;
        const probes = managedProbes({
            compatibility: async () => result(daemon(7), "ready"),
            storage: async () => {
                polls += 1;
                return "unavailable";
            },
        });
        await probes.compatibilityProbe(1_000);
        const states = await Promise.all([
            probes.storageProbe(100, daemon(7)),
            probes.storageProbe(100, daemon(7)),
            probes.storageProbe(100, daemon(7)),
        ]);
        expect(states).toEqual(["ready", "ready", "ready"]);
        expect(polls).toBe(0);
    });

    test("a superseded compatibility probe that settles late cannot overwrite the newer record", async () => {
        let polls = 0;
        const releases: Array<(value: CompatibilityProbeResult) => void> = [];
        const probes = managedProbes({
            compatibility: () =>
                new Promise((resolve) => {
                    releases.push(resolve);
                }),
            storage: async () => {
                polls += 1;
                return "unavailable";
            },
        });
        const stale = probes.compatibilityProbe(1_000);
        const fresh = probes.compatibilityProbe(1_000);
        // The replacement answers first with `starting`; the abandoned probe then
        // settles with the older `ready` it saw on the same daemon.
        releases[1]?.(result(daemon(7), "starting"));
        await fresh;
        releases[0]?.(result(daemon(7), "ready"));
        await stale;

        // The storage probe polls instead of returning `ready` from the stale observation.
        expect(await probes.storageProbe(100, daemon(7))).toBe("unavailable");
        expect(polls).toBe(1);
    });

    test("a starting observation polls once for concurrent waiters", async () => {
        let polls = 0;
        const budgets: number[] = [];
        let release: (state: "ready") => void = () => {};
        const probes = managedProbes({
            compatibility: async () => result(daemon(7), "starting"),
            storage: (budgetMs) => {
                polls += 1;
                budgets.push(budgetMs);
                return new Promise((resolve) => {
                    release = resolve;
                });
            },
        });
        await probes.compatibilityProbe(1_000);
        const waiting = Promise.all([
            probes.storageProbe(100, daemon(7)),
            probes.storageProbe(100, daemon(7)),
        ]);
        expect(polls).toBe(1);
        // The shared poll runs on the hard budget, not the first waiter's.
        expect(budgets).toEqual([STORAGE_HARD_BUDGET_MS]);
        release("ready");
        expect(await waiting).toEqual(["ready", "ready"]);
        // The settled poll is evicted, so the next storage probe opens a fresh one.
        const fresh = probes.storageProbe(100, daemon(7));
        expect(polls).toBe(2);
        release("ready");
        expect(await fresh).toBe("ready");
    });

    test("a waiter that outlives its budget answers starting while longer waiters keep polling", async () => {
        let release: (state: "ready") => void = () => {};
        let aborted = false;
        const probes = managedProbes({
            compatibility: async () => result(daemon(7), "starting"),
            storage: (_budget, _expected, signal) => {
                signal?.addEventListener("abort", () => {
                    aborted = true;
                });
                return new Promise((resolve) => {
                    release = resolve;
                });
            },
        });
        await probes.compatibilityProbe(1_000);
        const patient = probes.storageProbe(1_000, daemon(7));
        expect(await probes.storageProbe(5, daemon(7))).toBe("starting");
        // The impatient waiter left, but the patient one still holds the poll open.
        expect(aborted).toBe(false);
        release("ready");
        expect(await patient).toBe("ready");
    });

    test("the shared poll is aborted and evicted when its last waiter leaves", async () => {
        let polls = 0;
        const signals: AbortSignal[] = [];
        const probes = managedProbes({
            compatibility: async () => result(daemon(7), "starting"),
            storage: (_budget, _expected, signal) => {
                polls += 1;
                if (signal !== undefined) signals.push(signal);
                return new Promise(() => {});
            },
        });
        await probes.compatibilityProbe(1_000);
        expect(await probes.storageProbe(5, daemon(7))).toBe("starting");
        expect(signals[0]?.aborted).toBe(true);
        // A later waiter must not join the abandoned poll.
        const next = probes.storageProbe(5, daemon(7));
        expect(polls).toBe(2);
        expect(await next).toBe("starting");
    });

    test("an aborted waiter leaves the shared poll at once", async () => {
        const signals: AbortSignal[] = [];
        const probes = managedProbes({
            compatibility: async () => result(daemon(7), "starting"),
            storage: (_budget, _expected, signal) => {
                if (signal !== undefined) signals.push(signal);
                return new Promise(() => {});
            },
        });
        await probes.compatibilityProbe(1_000);
        const patient = new AbortController();
        const canceled = new AbortController();
        const kept = probes.storageProbe(60_000, daemon(7), patient.signal);
        const dropped = probes.storageProbe(60_000, daemon(7), canceled.signal);

        canceled.abort();
        expect(await dropped).toBe("starting");
        // The other waiter still holds the poll open.
        expect(signals[0]?.aborted).toBe(false);

        patient.abort();
        expect(await kept).toBe("starting");
        // The last waiter left, so the poll is released without waiting out its budget.
        expect(signals[0]?.aborted).toBe(true);
    });

    test("an observation from another daemon is never reused", async () => {
        const polled: Array<Uint8Array | undefined> = [];
        const probes = managedProbes({
            compatibility: async () => result(daemon(7), "ready"),
            storage: async (_budget, expected) => {
                polled.push(expected);
                return "starting";
            },
        });
        await probes.compatibilityProbe(1_000);
        expect(await probes.storageProbe(100, daemon(8))).toBe("starting");
        expect(polled).toEqual([daemon(8)]);
    });

    test("a short-circuited compatibility probe leaves no observation to reuse", async () => {
        let polls = 0;
        const probes = managedProbes({
            compatibility: async () => result(daemon(7), null),
            storage: async () => {
                polls += 1;
                return "unavailable";
            },
        });
        await probes.compatibilityProbe(1_000);
        expect(await probes.storageProbe(100, daemon(7))).toBe("unavailable");
        expect(polls).toBe(1);
    });

    test("polls for different daemons do not coalesce", async () => {
        const polled: Array<Uint8Array | undefined> = [];
        const probes = managedProbes({
            compatibility: async () => result(daemon(7), "starting"),
            storage: async (_budget, expected) => {
                polled.push(expected);
                return "starting";
            },
        });
        await probes.compatibilityProbe(1_000);
        await Promise.all([
            probes.storageProbe(100, daemon(7)),
            probes.storageProbe(100, daemon(8)),
        ]);
        expect(polled).toEqual([daemon(7), daemon(8)]);
    });
});

describe("kernel readiness from host.status metrics", () => {
    const readyBlock = {
        kernel_state: "ready",
        sampled_at_ms: 1_700_000_000_000,
        core_file_bytes: 4096,
        core_file_warn: false,
        artifact_usage_bytes: 0,
        artifact_cap_bytes: 1_048_576,
        artifact_warn: false,
        outbox_position_lag: 0,
        oldest_unconsumed_age_ms: 0,
        retained_outbox_rows: 0,
        required_consumer_count: 1,
        lag_threshold_tripped: false,
    };
    const metricsWith = (kernel: unknown) => ({
        components: {
            context: {
                metrics: {
                    storage_state: "ready",
                    ...(kernel === undefined ? {} : { kernel }),
                },
            },
        },
    });

    test("a missing block is unknown, never absent or healthy", () => {
        expect(kernelReadiness(metricsWith(undefined))).toEqual({
            state: "unavailable",
            reason: "kernel_unavailable",
        });
        expect(kernelReadiness({})).toEqual({
            state: "unavailable",
            reason: "kernel_unavailable",
        });
    });

    test("a block without a valid kernel_state is unavailable", () => {
        expect(kernelReadiness(metricsWith({}))).toEqual({
            state: "unavailable",
            reason: "kernel_unavailable",
        });
        expect(kernelReadiness(metricsWith({ kernel_state: "degraded" }))).toEqual({
            state: "unavailable",
            reason: "kernel_unavailable",
        });
        expect(kernelReadiness(metricsWith("ready"))).toEqual({
            state: "unavailable",
            reason: "kernel_unavailable",
        });
    });

    test("starting and unavailable states pass through with their reasons", () => {
        expect(kernelReadiness(metricsWith({ kernel_state: "starting" }))).toEqual({
            state: "starting",
            reason: "kernel_starting",
        });
        expect(
            kernelReadiness(
                metricsWith({
                    kernel_state: "unavailable",
                    unavailable_reason: "store_unsupported",
                }),
            ),
        ).toEqual({ state: "unavailable", reason: "kernel_unavailable" });
    });

    test("a ready block with lag past threshold warns as kernel_lagging", () => {
        expect(
            kernelReadiness(
                metricsWith({
                    ...readyBlock,
                    outbox_position_lag: 5000,
                    lag_threshold_tripped: true,
                }),
            ),
        ).toEqual({ state: "ready", reason: "kernel_lagging" });
    });

    test("lag outranks an empty required-consumer set", () => {
        expect(
            kernelReadiness(
                metricsWith({
                    ...readyBlock,
                    required_consumer_count: 0,
                    lag_threshold_tripped: true,
                }),
            ),
        ).toEqual({ state: "ready", reason: "kernel_lagging" });
    });

    test("either capacity flag warns as kernel_capacity_warn", () => {
        expect(kernelReadiness(metricsWith({ ...readyBlock, core_file_warn: true }))).toEqual({
            state: "ready",
            reason: "kernel_capacity_warn",
        });
        expect(kernelReadiness(metricsWith({ ...readyBlock, artifact_warn: true }))).toEqual({
            state: "ready",
            reason: "kernel_capacity_warn",
        });
    });

    test("warn reasons rank lagging over capacity over no required consumer", () => {
        expect(
            kernelReadiness(
                metricsWith({
                    ...readyBlock,
                    artifact_warn: true,
                    required_consumer_count: 0,
                    lag_threshold_tripped: true,
                }),
            ),
        ).toEqual({ state: "ready", reason: "kernel_lagging" });
        expect(
            kernelReadiness(
                metricsWith({ ...readyBlock, artifact_warn: true, required_consumer_count: 0 }),
            ),
        ).toEqual({ state: "ready", reason: "kernel_capacity_warn" });
    });

    test("a ready block with no required consumer warns as no_required_consumer", () => {
        expect(kernelReadiness(metricsWith({ ...readyBlock, required_consumer_count: 0 }))).toEqual(
            { state: "ready", reason: "no_required_consumer" },
        );
    });

    test("a ready block within threshold and with a consumer is healthy", () => {
        expect(kernelReadiness(metricsWith(readyBlock))).toEqual({
            state: "ready",
            reason: "healthy",
        });
        // The sanitizer may drop invalid numeric fields; a bare ready state is
        // still healthy because neither warn signal is asserted.
        expect(kernelReadiness(metricsWith({ kernel_state: "ready" }))).toEqual({
            state: "ready",
            reason: "healthy",
        });
    });

    test("blocks a live daemon emits classify as the shared fixture names them", () => {
        const blocks: Record<string, unknown> = kernelHealthBlocks;
        for (const [block, reason] of [
            ["healthy", "healthy"],
            ["kernel_lagging", "kernel_lagging"],
            ["no_required_consumer", "no_required_consumer"],
            ["kernel_capacity_warn_core_file", "kernel_capacity_warn"],
            ["kernel_capacity_warn_artifact", "kernel_capacity_warn"],
        ]) {
            expect(kernelReadiness(metricsWith(blocks[block]))).toEqual({
                state: "ready",
                reason,
            });
        }
        expect(kernelReadiness(metricsWith(blocks.kernel_unavailable))).toEqual({
            state: "unavailable",
            reason: "kernel_unavailable",
        });
    });
});

describe("synapse readiness from host.status metrics", () => {
    const withSynapse = (synapse: unknown) => ({
        components: {
            context: { status: "ok", metrics: { storage_state: "ready" } },
            ...(synapse === undefined ? {} : { synapse }),
        },
    });

    test("an absent component is an unproven lane, not an unsupported one", () => {
        // The fixed profile reports `unsupported` as an explicit literal, so
        // omission is never read as proof of that state.
        expect(synapseReadiness(withSynapse(undefined))).toEqual({
            state: "degraded",
            reason: "synapse_degraded",
        });
        expect(synapseReadiness({})).toEqual({
            state: "degraded",
            reason: "synapse_degraded",
        });
    });

    test("wire states pass through with their reasons", () => {
        for (const [state, expected] of [
            ["ready", { state: "ready", reason: "healthy" }],
            ["starting", { state: "starting", reason: "synapse_starting" }],
            ["degraded", { state: "degraded", reason: "synapse_degraded" }],
            ["unsupported", { state: "unsupported", reason: "synapse_unsupported" }],
        ] as const) {
            expect(
                synapseReadiness(withSynapse({ status: "ok", metrics: { synapse_state: state } })),
            ).toEqual(expected);
        }
    });

    test("a named component without a wire state is a failure, not an absent lane", () => {
        expect(synapseReadiness(withSynapse({ status: "failing", metrics: {} }))).toEqual({
            state: "degraded",
            reason: "synapse_degraded",
        });
        expect(synapseReadiness(withSynapse({ status: "ok", metrics: null }))).toEqual({
            state: "degraded",
            reason: "synapse_degraded",
        });
        expect(
            synapseReadiness(
                withSynapse({ status: "degraded", metrics: { synapse_state: "unexpected" } }),
            ),
        ).toEqual({ state: "degraded", reason: "synapse_degraded" });
    });
});
