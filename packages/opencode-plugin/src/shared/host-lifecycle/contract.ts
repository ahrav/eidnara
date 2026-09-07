/**
 *
 * Unknown fields, out-of-union values, unsorted check lists, and exit/result disagreements throw `ContractViolation`.
 * Invalid input throws `ContractViolation` instead of being cast.
 */

import { lstatSync } from "node:fs";
import * as path from "node:path";
import hostRelease from "../../../../../release/host-release.json";
import type {
    CheckId,
    CheckStatus,
    DaemonCommand,
    DaemonState,
    FailingReason,
    NonFailingReason,
    Remediation,
} from "./contract-vocabulary";
import { coordinationDirPath, runtimeDirPath } from "./paths";

export type {
    CheckId,
    CheckStatus,
    DaemonCommand,
    DaemonState,
    FailingReason,
    HarnessUnavailableReason,
    KernelReadinessState,
    NonFailingReason,
    Remediation,
    StorageReadinessState,
    SynapseReadinessState,
    TransportReadinessState,
} from "./contract-vocabulary";
export type DaemonReason = FailingReason | NonFailingReason;

export const DAEMON_RESULT_SCHEMA = hostRelease.cli.result_schema;

const COMMANDS = new Set<string>(hostRelease.cli.commands);
const STATES = new Set<string>(hostRelease.cli.states);
const CHECK_IDS = new Set<string>(hostRelease.cli.check_ids);
const CHECK_STATUSES = new Set<string>(hostRelease.cli.check_statuses);
const REMEDIATIONS = new Set<string>(hostRelease.cli.remediations);
const FAILING_REASONS = new Map<string, { precedence: number; remediation: string | null }>(
    hostRelease.cli.reasons.failing_by_precedence.map((entry, index) => [
        entry.id,
        { precedence: index + 1, remediation: entry.remediation ?? null },
    ]),
);
const NON_FAILING_REASONS = new Set<string>(hostRelease.cli.reasons.non_failing);
const WARN_REMEDIATIONS = new Map<string, string>(
    Object.entries(hostRelease.cli.reasons.warn_remediations),
);
const READINESS_STATES: Record<string, ReadonlySet<string>> = {
    transport: new Set(hostRelease.cli.readiness_states.transport),
    storage: new Set(hostRelease.cli.readiness_states.storage),
    synapse: new Set(hostRelease.cli.readiness_states.synapse),
    kernel: new Set(hostRelease.cli.readiness_states.kernel),
};

export function isDaemonReason(value: string): value is DaemonReason {
    return FAILING_REASONS.has(value) || NON_FAILING_REASONS.has(value);
}

/** Failing reasons have 1-based precedence; lower values win. Non-failing reasons have null precedence. */
export function reasonPrecedence(reason: DaemonReason): number | null {
    return FAILING_REASONS.get(reason)?.precedence ?? null;
}

/**
 * For `harness_unavailable`, this function returns null because remediation depends on the subreason.
 * Warn-class non-failing reasons resolve through `warn_remediations`; every other non-failing reason has none.
 */
export function remediationForReason(reason: DaemonReason): Remediation | null {
    const entry = FAILING_REASONS.get(reason);
    if (!entry) return (WARN_REMEDIATIONS.get(reason) as Remediation | undefined) ?? null;
    return (entry.remediation as Remediation | null) ?? null;
}

/** `harness_unavailable` permits only `null` or `restart_with_supported_harness` remediation. */
function remediationFitsReason(reason: DaemonReason, remediation: string | null): boolean {
    if (reason === "harness_unavailable") {
        return remediation === null || remediation === "restart_with_supported_harness";
    }
    return remediation === remediationForReason(reason);
}

/**
 * `no_data_dir` denotes an unresolved data root. The binary's `envelope_failure_result` pairs the other two with the probed state, which is `unavailable` when the probe finds no data root. commentlint: allow(JUDGE)
 */
const UNAVAILABLE_REASONS: ReadonlySet<string> = new Set([
    "no_data_dir",
    "harness_unavailable",
    "internal_error",
]);

const HARNESS_REASONS = new Map<string, string | null>(
    hostRelease.harness_unavailable.reasons_by_precedence.map((entry) => [
        entry.id,
        entry.remediation ?? null,
    ]),
);

/**
 * Unknown harness subreasons throw a violation instead of using a guessed remediation.
 */
export function harnessRemediationFor(subreason: string): Remediation | null {
    if (!HARNESS_REASONS.has(subreason)) {
        throw new ContractViolation(`unknown harness_unavailable_reason: ${bounded(subreason)}`);
    }
    return (HARNESS_REASONS.get(subreason) as Remediation | null) ?? null;
}

export interface ReadinessRecord {
    state: string;
    reason: DaemonReason;
}

export interface DaemonReadiness {
    transport?: ReadinessRecord;
    storage?: ReadinessRecord;
    synapse?: ReadinessRecord;
    kernel?: ReadinessRecord;
}

export interface DaemonCheck {
    id: CheckId;
    status: CheckStatus;
    reason: DaemonReason;
    remediation: Remediation | null;
}

export interface RestartEffects {
    stop_committed: boolean;
    start_committed: boolean;
}

export interface DaemonVersions {
    release: string | null;
    proof: string | null;
    daemon: string | null;
    context: string | null;
    synapse: string | null;
    broca: string | null;
}

/* */
export interface DaemonResultV1 {
    schema: string;
    command: DaemonCommand;
    ok: boolean;
    state: DaemonState;
    reason: DaemonReason;
    remediation: Remediation | null;
    effects: RestartEffects | null;
    readiness: DaemonReadiness | null;
    checks: DaemonCheck[];
    versions: DaemonVersions;
}

/* */
export class ContractViolation extends Error {
    constructor(message: string) {
        super(message);
        this.name = "ContractViolation";
    }
}

const MAX_DETAIL_LEN = 80;

function bounded(value: string): string {
    return value.length > MAX_DETAIL_LEN ? `${value.slice(0, MAX_DETAIL_LEN)}…` : value;
}

function fail(detail: string): never {
    throw new ContractViolation(`daemon result rejected: ${detail}`);
}

function requireExactKeys(record: Record<string, unknown>, expected: string[], what: string): void {
    const keys = Object.keys(record).sort();
    const sorted = [...expected].sort();
    if (keys.length !== sorted.length || keys.some((key, i) => key !== sorted[i])) {
        fail(`${what} has an unexpected key set`);
    }
}

function requireObject(value: unknown, what: string): Record<string, unknown> {
    if (typeof value !== "object" || value === null || Array.isArray(value)) {
        fail(`${what} is not an object`);
    }
    return value as Record<string, unknown>;
}

function nullableString(value: unknown, what: string): string | null {
    if (value === null || value === undefined) return null;
    if (typeof value !== "string" || value.length === 0 || value.length > 256) {
        fail(`${what} is not a bounded nonempty string or null`);
    }
    return value;
}

function parseReadinessRecord(value: unknown, component: string): ReadinessRecord {
    const record = requireObject(value, `readiness.${component}`);
    requireExactKeys(record, ["state", "reason"], `readiness.${component}`);
    const state = record.state;
    const states = READINESS_STATES[component];
    if (typeof state !== "string" || !states || !states.has(state)) {
        fail(`readiness.${component}.state is outside its closed set`);
    }
    const reason = record.reason;
    if (typeof reason !== "string" || !isDaemonReason(reason)) {
        fail(`readiness.${component}.reason is outside the closed reason union`);
    }
    // Component states admit explicit reason sets instead of a blanket failing/non-failing split.
    // `ready` accepts only non-failing reasons.
    // `unsupported` may pair with `synapse_unsupported` without failing.
    // `starting` accepts only failing reasons.
    // `non-ready` does not imply a failing reason because `unsupported` can be non-failing.
    const allowed = {
        transport: {
            ready: ["healthy"],
            starting: ["starting", "lifecycle_busy"],
            unavailable: ["startup_timeout", "publication_missing", "authentication_failed"],
        },
        storage: {
            ready: ["healthy"],
            starting: ["storage_starting", "starting"],
            unavailable: ["storage_unavailable"],
        },
        synapse: {
            ready: ["healthy"],
            starting: ["synapse_starting", "starting"],
            degraded: ["synapse_degraded"],
            unsupported: ["synapse_unsupported"],
        },
        kernel: {
            ready: ["healthy", "kernel_lagging", "kernel_capacity_warn", "no_required_consumer"],
            starting: ["kernel_starting", "starting"],
            unavailable: ["kernel_unavailable"],
        },
    } as const;
    const componentAllowed = allowed[component as keyof typeof allowed] as
        | Record<string, readonly string[]>
        | undefined;
    if (!componentAllowed?.[state]?.includes(reason)) {
        if (state === "ready" && !NON_FAILING_REASONS.has(reason)) {
            fail(`readiness.${component} is ready with a failing reason`);
        }
        fail(`readiness.${component} state contradicts its reason`);
    }
    return { state, reason };
}

/**
 * The parser validates the native binary's stdout as one v1 result.
 * The input must contain one JSON object; `JSON.parse` rejects trailing non-whitespace input.
 * The result must have the exact v1 key set, and each value must belong to its closed union.
 * `readiness` and `shared_memory` are optional keys.
 */
export function parseDaemonResult(stdoutText: string): DaemonResultV1 {
    const trimmed = stdoutText.trim();
    if (trimmed.length === 0) fail("empty output");
    let parsed: unknown;
    try {
        parsed = JSON.parse(trimmed);
    } catch {
        fail("output is not a single JSON value");
    }
    const record = requireObject(parsed, "result");
    const resultKeys = [
        "schema",
        "command",
        "ok",
        "state",
        "reason",
        "remediation",
        "effects",
        "checks",
        "versions",
    ];
    if ("readiness" in record) resultKeys.push("readiness");
    if ("shared_memory" in record) resultKeys.push("shared_memory");
    requireExactKeys(record, resultKeys, "result");
    if (record.schema !== DAEMON_RESULT_SCHEMA) fail("schema is not eidnara.daemon/v1");
    if (record.shared_memory !== undefined && record.shared_memory !== null) {
        fail("shared_memory diagnostics are not supported by this release");
    }
    const command = record.command;
    // The binary accepts `probe` in argv but returns `status`.
    // A result containing `probe` is nonconforming because the binary returns `status`.
    if (typeof command !== "string" || !COMMANDS.has(command)) {
        fail("command is outside the closed union");
    }
    if (typeof record.ok !== "boolean") fail("ok is not a boolean");
    const state = record.state;
    if (typeof state !== "string" || !STATES.has(state)) {
        fail("state is outside the closed union");
    }
    const reason = record.reason;
    if (typeof reason !== "string" || !isDaemonReason(reason)) {
        fail("reason is outside the closed union");
    }
    // `ok` equals whether the reason is non-failing.
    if (record.ok !== NON_FAILING_REASONS.has(reason)) {
        fail("ok disagrees with reason class");
    }
    if (state === "unavailable" && !UNAVAILABLE_REASONS.has(reason)) {
        fail("unavailable is legal only with an unresolved-data-root reason");
    }
    const remediation = record.remediation;
    if (
        remediation !== null &&
        (typeof remediation !== "string" || !REMEDIATIONS.has(remediation))
    ) {
        fail("remediation is outside the closed union");
    }
    const expectedOk = NON_FAILING_REASONS.has(reason);
    if (record.ok !== expectedOk) {
        fail("ok contradicts the selected reason");
    }
    if (!remediationFitsReason(reason, remediation as string | null)) {
        fail("remediation does not match its reason");
    }
    // `shutdown_timeout` carries the state the stop phase last observed; the binary reports `running` when the shutdown request's commit is uncertain. commentlint: allow(JUDGE)
    const fixedReasonStates: Partial<Record<DaemonReason, readonly DaemonState[]>> = {
        healthy: ["running"],
        started: ["running"],
        already_running: ["running"],
        stopped: ["stopped"],
        already_stopped: ["stopped"],
        not_running: ["stopped"],
        no_data_dir: ["unavailable"],
        starting: ["starting"],
        stopping: ["stopping"],
        wedged: ["wedged"],
        shutdown_timeout: ["stopping", "running"],
    };
    // Non-failing top-level verdicts require a fixed daemon state; component-only
    // reasons such as `kernel_lagging` are rejected here.
    if (NON_FAILING_REASONS.has(reason) && fixedReasonStates[reason] === undefined) {
        fail("a component-only reason is not a top-level verdict");
    }
    const expectedStates = fixedReasonStates[reason];
    if (expectedStates !== undefined && !expectedStates.includes(state as DaemonState)) {
        fail("state contradicts the selected reason");
    }
    // A success verdict names the effect its command produced, so one command cannot borrow another's.
    const successReasons: Record<string, readonly string[]> = {
        start: ["started", "already_running"],
        restart: ["started"],
        stop: ["stopped", "already_stopped"],
        status: ["healthy"],
        doctor: ["healthy"],
    };
    if (record.ok && !successReasons[command]?.includes(reason)) {
        fail("a successful result carries a verdict its command cannot produce");
    }
    let effects: RestartEffects | null = null;
    if (record.effects !== null) {
        if (command !== "restart") fail("effects are restart-only");
        const rawEffects = requireObject(record.effects, "effects");
        requireExactKeys(rawEffects, ["stop_committed", "start_committed"], "effects");
        if (
            typeof rawEffects.stop_committed !== "boolean" ||
            typeof rawEffects.start_committed !== "boolean"
        ) {
            fail("effects fields are not booleans");
        }
        effects = {
            stop_committed: rawEffects.stop_committed,
            start_committed: rawEffects.start_committed,
        };
        //
        if (record.ok && !effects.start_committed) {
            fail("a successful restart must report a committed start");
        }
        if (record.ok && (state !== "running" || reason !== "started")) {
            fail("a successful restart contradicts its start effect");
        }
    } else if (command === "restart" && record.ok) {
        //
        fail("a successful restart must carry its effects");
    }
    let readiness: DaemonReadiness | null = null;
    if (record.readiness !== null && record.readiness !== undefined) {
        const rawReadiness = requireObject(record.readiness, "readiness");
        readiness = {};
        for (const [component, value] of Object.entries(rawReadiness)) {
            const normalized = component === "shared_memory" ? "transport" : component;
            if (
                normalized !== "transport" &&
                normalized !== "storage" &&
                normalized !== "synapse" &&
                normalized !== "kernel"
            ) {
                fail("readiness carries an unknown component");
            }
            if (readiness[normalized] !== undefined) {
                fail("readiness carries duplicate transport components");
            }
            readiness[normalized] = parseReadinessRecord(value, normalized);
        }
    }
    if (!Array.isArray(record.checks) || record.checks.length > CHECK_IDS.size) {
        fail("checks is not a bounded array");
    }
    const checks: DaemonCheck[] = record.checks.map((raw) => {
        const check = requireObject(raw, "check");
        requireExactKeys(check, ["id", "status", "reason", "remediation"], "check");
        const id = check.id;
        if (typeof id !== "string" || !CHECK_IDS.has(id)) {
            fail("check id is outside the closed union");
        }
        const status = check.status;
        if (typeof status !== "string" || !CHECK_STATUSES.has(status)) {
            fail("check status is outside the closed union");
        }
        const checkReason = check.reason;
        if (typeof checkReason !== "string" || !isDaemonReason(checkReason)) {
            fail("check reason is outside the closed union");
        }
        //
        if (status === "pass" && !NON_FAILING_REASONS.has(checkReason)) {
            fail("a passing check carries a failing reason");
        }
        if (status === "fail" && NON_FAILING_REASONS.has(checkReason)) {
            fail("a failing check carries a non-failing reason");
        }
        const checkRemediation = check.remediation;
        if (
            checkRemediation !== null &&
            (typeof checkRemediation !== "string" || !REMEDIATIONS.has(checkRemediation))
        ) {
            fail("check remediation is outside the closed union");
        }
        if (status === "pass" && !NON_FAILING_REASONS.has(checkReason)) {
            fail("a passing check carries a failing reason");
        }
        if (status === "fail" && NON_FAILING_REASONS.has(checkReason)) {
            fail("a failing check carries a non-failing reason");
        }
        if (!remediationFitsReason(checkReason, checkRemediation as string | null)) {
            fail("check remediation contradicts its reason");
        }
        return {
            id: id as CheckId,
            status: status as CheckStatus,
            reason: checkReason as DaemonReason,
            remediation: checkRemediation as Remediation | null,
        };
    });
    for (let i = 1; i < checks.length; i++) {
        const prev = checks[i - 1] as DaemonCheck;
        const current = checks[i] as DaemonCheck;
        if (prev.id >= current.id) fail("checks are not lexicographically sorted unique ids");
    }
    if (record.ok && checks.some((check) => check.status === "fail")) {
        fail("successful result contains a failed check");
    }
    const rawVersions = requireObject(record.versions, "versions");
    requireExactKeys(
        rawVersions,
        ["release", "proof", "daemon", "context", "synapse", "broca"],
        "versions",
    );
    const versions: DaemonVersions = {
        release: nullableString(rawVersions.release, "versions.release"),
        proof: nullableString(rawVersions.proof, "versions.proof"),
        daemon: nullableString(rawVersions.daemon, "versions.daemon"),
        context: nullableString(rawVersions.context, "versions.context"),
        synapse: nullableString(rawVersions.synapse, "versions.synapse"),
        broca: nullableString(rawVersions.broca, "versions.broca"),
    };
    if (versions.proof !== null && versions.proof !== "current") {
        fail("versions.proof is outside its closed literal");
    }
    // Only an authenticated, successful start or restart vouches for the running code; status and stop never authenticate.
    const provesCurrent = record.ok && (command === "start" || command === "restart");
    if (versions.proof === "current" && !provesCurrent) {
        fail("versions.proof claims current from a result that cannot authenticate");
    }
    return {
        schema: DAEMON_RESULT_SCHEMA,
        command: command as DaemonCommand,
        ok: record.ok,
        state: state as DaemonState,
        reason: reason as DaemonReason,
        remediation: (remediation as Remediation | null) ?? null,
        effects,
        readiness,
        checks,
        versions,
    };
}

/**
 */
export function exitAgreesWithResult(exitCode: number, result: DaemonResultV1): boolean {
    if (exitCode === 0) return result.ok;
    if (exitCode === 1) return !result.ok;
    return false;
}

// ---------------------------------------------------------------------------
// ---------------------------------------------------------------------------

export type PreNativeRootsClassification =
    | { kind: "absent" }
    | { kind: "residual" }
    | { kind: "hazard"; hazard: "symlink" | "special" | "access_error" | "race" };

type ProbeOutcome = "absent" | "directory" | "symlink" | "special" | "access_error";

function probeEntry(entryPath: string): ProbeOutcome {
    const absolute = path.resolve(entryPath);
    const { root } = path.parse(absolute);
    const components = path.relative(root, absolute).split(path.sep).filter(Boolean);
    let current = root;
    for (let index = -1; index < components.length; index++) {
        if (index >= 0) current = path.join(current, components[index] as string);
        try {
            const stat = lstatSync(current);
            if (stat.isSymbolicLink()) return "symlink";
            if (!stat.isDirectory()) return "special";
        } catch (error) {
            const code = (error as NodeJS.ErrnoException).code;
            if (code === "ENOENT") return "absent";
            if (code === "ENOTDIR") return "special";
            return "access_error";
        }
    }
    return "directory";
}

/**
 *
 */
export function classifyPreNativeRoots(dataRoot: string): PreNativeRootsClassification {
    const entries = [coordinationDirPath(dataRoot), runtimeDirPath(dataRoot)];
    const first = entries.map(probeEntry);
    const second = entries.map(probeEntry);
    for (let i = 0; i < entries.length; i++) {
        if (first[i] !== second[i]) return { kind: "hazard", hazard: "race" };
    }
    for (const outcome of second) {
        if (outcome === "symlink") return { kind: "hazard", hazard: "symlink" };
        if (outcome === "special") return { kind: "hazard", hazard: "special" };
        if (outcome === "access_error") return { kind: "hazard", hazard: "access_error" };
    }
    if (second.every((outcome) => outcome === "absent")) return { kind: "absent" };
    return { kind: "residual" };
}

/* */
export function preNativeState(classification: PreNativeRootsClassification): DaemonState {
    return classification.kind === "absent" ? "stopped" : "wedged";
}

/**
 */
export function probeFallbackVerdict(classification: PreNativeRootsClassification): {
    state: DaemonState;
    reason: DaemonReason;
} {
    if (classification.kind === "absent") return { state: "stopped", reason: "not_running" };
    return { state: "wedged", reason: "native_probe_unavailable" };
}
