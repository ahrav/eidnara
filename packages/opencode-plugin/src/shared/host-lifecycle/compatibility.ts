/**
 * These functions return mismatches without stopping or replacing live daemons.
 */

import type { AuthenticatedPeer, CatalogEntry } from "../host-client";
import type { DaemonReason } from "./contract";

/** `versions.daemon` reads `<prefix>X.Y.Z`; the prefix is everything through the slash. */
const DAEMON_VERSION_PREFIX: string = hostRelease.versions.daemon.slice(
    0,
    hostRelease.versions.daemon.indexOf("/") + 1,
);

import hostRelease from "../../../../../release/host-release.json";
import type { EpochName, ModuleKey } from "./contract-vocabulary";

export type CompatibilityVerdict =
    | { ok: true }
    | {
          ok: false;
          reason: Extract<
              DaemonReason,
              "incompatible_daemon" | "incompatible_module" | "incompatible_epochs"
          >;
          /** detail identifies the mismatch. */
          detail: string;
      };

type SemverTriple = [number, number, number];

/**
 * `\d+` admits leading zeroes that `Number.parseInt` silently normalizes.
 * Reject leading zeroes so compatible versions remain canonical `X.Y.Z` values.
 */
const CANONICAL_SEMVER = /^(0|[1-9]\d*)\.(0|[1-9]\d*)\.(0|[1-9]\d*)$/;
export function parseSemverTriple(value: string): SemverTriple | null {
    const match = CANONICAL_SEMVER.exec(value);
    if (!match) return null;
    const triple: SemverTriple = [
        Number.parseInt(match[1] as string, 10),
        Number.parseInt(match[2] as string, 10),
        Number.parseInt(match[3] as string, 10),
    ];
    if (triple.some((part) => !Number.isSafeInteger(part))) return null;
    return triple;
}

function compareTriples(a: SemverTriple, b: SemverTriple): number {
    for (let i = 0; i < 3; i++) {
        const delta = (a[i] as number) - (b[i] as number);
        if (delta !== 0) return delta;
    }
    return 0;
}

function inHalfOpenRange(
    version: SemverTriple,
    range: { min_inclusive: string; max_exclusive: string },
): boolean {
    const min = parseSemverTriple(range.min_inclusive);
    const max = parseSemverTriple(range.max_exclusive);
    if (!min || !max) return false;
    return compareTriples(version, min) >= 0 && compareTriples(version, max) < 0;
}

/**
 *
 */
export function evaluateDaemonCompatibility(peer: AuthenticatedPeer): CompatibilityVerdict {
    const authenticatedDaemonVer = peer.daemonVer;
    const raw = authenticatedDaemonVer.startsWith(DAEMON_VERSION_PREFIX)
        ? authenticatedDaemonVer.slice(DAEMON_VERSION_PREFIX.length)
        : null;
    const triple = raw === null ? null : parseSemverTriple(raw);
    if (triple === null) {
        return {
            ok: false,
            reason: "incompatible_daemon",
            detail: `daemon version is not a canonical ${DAEMON_VERSION_PREFIX}X.Y.Z value`,
        };
    }
    if (!inHalfOpenRange(triple, hostRelease.versions.supported_daemon_range)) {
        return {
            ok: false,
            reason: "incompatible_daemon",
            detail: "daemon version is outside the supported range",
        };
    }
    return { ok: true };
}

const FIXED_MODULES: ReadonlyArray<{
    catalogId: string;
    contractKey: ModuleKey;
}> = [
    { catalogId: "context", contractKey: "context" },
    { catalogId: "synapse", contractKey: "synapse" },
    { catalogId: "broca", contractKey: "broca" },
];

/**
 */
export function evaluateModuleCompatibility(catalog: CatalogEntry[]): CompatibilityVerdict {
    const byId = new Map(catalog.map((entry) => [entry.module_id, entry]));
    for (const { catalogId, contractKey } of FIXED_MODULES) {
        const entry = byId.get(catalogId);
        if (!entry) {
            return {
                ok: false,
                reason: "incompatible_module",
                detail: `module ${catalogId} is absent from the catalog`,
            };
        }
        const triple = parseSemverTriple(entry.module_version);
        if (!triple) {
            return {
                ok: false,
                reason: "incompatible_module",
                detail: `module ${catalogId} version is not canonical semver`,
            };
        }
        if (!inHalfOpenRange(triple, hostRelease.versions.modules[contractKey].range)) {
            return {
                ok: false,
                reason: "incompatible_module",
                detail: `module ${catalogId} version is outside the supported range`,
            };
        }
    }
    return { ok: true };
}

export type { EpochName } from "./contract-vocabulary";

export type ObservedEpochs = Partial<Record<EpochName, unknown>>;

function asRecord(value: unknown): Record<string, unknown> | null {
    return value !== null && typeof value === "object" && !Array.isArray(value)
        ? (value as Record<string, unknown>)
        : null;
}

/** Maps `host.status` `metrics.epochs` wire names to release-contract epoch names. */
const WIRE_EPOCH_NAMES: Readonly<Record<string, EpochName>> = {
    memory_render_epoch: "memory_render",
    compartment_render_epoch: "compartment_render",
    profile_epoch: "profile_claude_code_anthropic",
    tagger_epoch: "tagger",
    state_sync_epoch: "state_sync",
};

/**
 * The epoch evaluator performs numeric validation without coercing values.
 * Unknown wire epoch names invalidate the whole observed set.
 */
export function observedEpochsFromContextMetrics(metrics: unknown): ObservedEpochs {
    const epochs = asRecord(asRecord(metrics)?.epochs);
    if (epochs === null) return {};
    const observed: ObservedEpochs = {};
    for (const [wireName, value] of Object.entries(epochs)) {
        const name = WIRE_EPOCH_NAMES[wireName];
        if (name === undefined) return {};
        observed[name] = value;
    }
    return observed;
}

/**
 * Treat `observed` as unknown because decoded JSON has no TypeScript type guarantee.
 * An extra observed key names an epoch that this release cannot interpret and returns a mismatch.
 * Non-numeric, stale, and future epoch values return a mismatch naming the failing epoch.
 */
export function evaluateEpochCompatibility(observed: ObservedEpochs): CompatibilityVerdict {
    const expectedNames = new Set(Object.keys(hostRelease.epochs));
    const observedNames = Object.keys(observed);
    if (
        observedNames.length !== expectedNames.size ||
        observedNames.some((name) => !expectedNames.has(name))
    ) {
        return {
            ok: false,
            reason: "incompatible_epochs",
            detail: "epoch set does not match the release contract",
        };
    }
    for (const [name, expected] of Object.entries(hostRelease.epochs)) {
        const value = observed[name as EpochName];
        if (typeof value !== "number" || !Number.isSafeInteger(value)) {
            return {
                ok: false,
                reason: "incompatible_epochs",
                detail: `epoch ${name} is missing or nonnumeric`,
            };
        }
        if (value !== expected) {
            return {
                ok: false,
                reason: "incompatible_epochs",
                detail: `epoch ${name} does not match the release contract`,
            };
        }
    }
    return { ok: true };
}

export interface CompatibilityInput {
    /**
     * The handshake-authenticated peer. Connection-file daemon versions are
     * untrusted text and never stand in for it.
     */
    authenticatedPeer: AuthenticatedPeer;
    catalog: CatalogEntry[];
    epochs: ObservedEpochs;
}

/**
 * All compatibility consumers derive stage order, check IDs, and evaluators from this list.
 * `evaluateCompatibility`, managed-probe `evaluatedThrough` labels, and policy checks derive from this list.
 * Deriving all consumers from this list prevents stage-order drift between probes and reported checks.
 * checks disagreeing.
 */
export const COMPATIBILITY_STAGES = [
    {
        stage: "daemon",
        checkId: "compatibility.daemon",
        evaluate: (input: CompatibilityInput): CompatibilityVerdict =>
            evaluateDaemonCompatibility(input.authenticatedPeer),
    },
    {
        stage: "modules",
        checkId: "compatibility.modules",
        evaluate: (input: CompatibilityInput): CompatibilityVerdict =>
            evaluateModuleCompatibility(input.catalog),
    },
    {
        stage: "epochs",
        checkId: "compatibility.epochs",
        evaluate: (input: CompatibilityInput): CompatibilityVerdict =>
            evaluateEpochCompatibility(input.epochs),
    },
] as const;

export type CompatibilityStage = (typeof COMPATIBILITY_STAGES)[number]["stage"];

/* */
export function compatibilityStageIndex(stage: CompatibilityStage): number {
    return COMPATIBILITY_STAGES.findIndex((entry) => entry.stage === stage);
}

/**
 * Demand, status, and doctor evaluate daemon range, modules, then epochs.
 * Compatibility evaluation never stops, replaces, or restarts a daemon.
 */
export function evaluateCompatibility(input: CompatibilityInput): CompatibilityVerdict {
    for (const stage of COMPATIBILITY_STAGES) {
        const verdict = stage.evaluate(input);
        if (!verdict.ok) return verdict;
    }
    return { ok: true };
}
