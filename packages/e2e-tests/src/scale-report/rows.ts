/**
 * Writes scale report pass rows as JSON lines that `eval_core::parse_pass_row` reads
 * losslessly: every field present, keys in the Rust struct's order, closed vocabularies, and
 * only safe non-negative integers.
 *
 * `response_us` is the full plugin hook. The stage fields are an additive ledger in whole
 * microseconds, `null` when unavailable: the plugin stages plus `daemon_total_us` sum to
 * `response_us`, and the daemon stages sum to `daemon_total_us`. `window_messages` and
 * `window_blocks` count the CK window the daemon served.
 */

import { closeSync, openSync, writeSync } from "node:fs";

export type ScaleTier = "10k" | "s3_100k" | "s4_1m";
export type ScaleHarness = "opencode" | "pi";
export type BoundaryState = "cold" | "warming" | "steady" | "replay" | "after_restart";
export type PassOutcome = "completed" | "censored" | "refused";
export type RefusalReason = "declined" | "daemon_error" | "transport_error";

export interface PassRow {
    harness: ScaleHarness;
    tier: ScaleTier;
    session: string;
    turn: number;
    boundary_state: BoundaryState;
    outcome: PassOutcome;
    refusal: RefusalReason | null;
    response_us: number;
    service_us: number | null;
    rss_bytes: number;
    ipc_bytes: number;
    prefix_guard_us: number | null;
    prepare_us: number | null;
    wire_build_us: number | null;
    serialize_us: number | null;
    transport_and_encode_us: number | null;
    apply_us: number | null;
    other_us: number | null;
    admit_us: number | null;
    lane_gate_and_decode_us: number | null;
    derive_ck_us: number | null;
    transform_core_us: number | null;
    native_attach_us: number | null;
    daemon_residual_us: number | null;
    daemon_total_us: number | null;
    window_messages: number | null;
    window_blocks: number | null;
}

/** The plugin stage fields, in ledger order. */
export const PLUGIN_STAGES = [
    "prefix_guard_us",
    "prepare_us",
    "wire_build_us",
    "serialize_us",
    "transport_and_encode_us",
    "apply_us",
    "other_us",
] as const;
/** The daemon stage fields, in ledger order; `derive_ck_us` stays `null` until the daemon derives CK. */
export const DAEMON_STAGES = [
    "admit_us",
    "lane_gate_and_decode_us",
    "derive_ck_us",
    "transform_core_us",
    "native_attach_us",
    "daemon_residual_us",
] as const;
type LedgerField =
    | (typeof PLUGIN_STAGES)[number]
    | (typeof DAEMON_STAGES)[number]
    | "daemon_total_us"
    | "window_messages"
    | "window_blocks";
const LEDGER_FIELDS: readonly LedgerField[] = [
    ...PLUGIN_STAGES,
    ...DAEMON_STAGES,
    "daemon_total_us",
    "window_messages",
    "window_blocks",
];

/** A row with every ledger field unavailable. */
export const NO_LEDGER: Pick<PassRow, LedgerField> = Object.fromEntries(
    LEDGER_FIELDS.map((field) => [field, null]),
) as Pick<PassRow, LedgerField>;

const TIERS: readonly ScaleTier[] = ["10k", "s3_100k", "s4_1m"];
const HARNESSES: readonly ScaleHarness[] = ["opencode", "pi"];
const BOUNDARY_STATES: readonly BoundaryState[] = [
    "cold",
    "warming",
    "steady",
    "replay",
    "after_restart",
];
const OUTCOMES: readonly PassOutcome[] = ["completed", "censored", "refused"];
const REFUSALS: readonly RefusalReason[] = ["declined", "daemon_error", "transport_error"];

export const TIER_MESSAGES: Readonly<Record<ScaleTier, number>> = {
    "10k": 10_000,
    s3_100k: 100_000,
    s4_1m: 1_000_000,
};

function count(name: string, value: number): number {
    if (!Number.isSafeInteger(value) || value < 0)
        throw new Error(`${name} must be a safe non-negative integer, got ${value}`);
    return value;
}

function optionalCount(name: string, value: number | null): number | null {
    return value === null ? null : count(name, value);
}

/** The checks `eval_core`'s report build applies, so a row the report would refuse never reaches the file. */
function checkLedger(row: PassRow): void {
    const plugin = PLUGIN_STAGES.map((field) => row[field]);
    const daemon = DAEMON_STAGES.filter((field) => field !== "derive_ck_us").map(
        (field) => row[field],
    );
    const present = (values: (number | null)[]) => values.every((value) => value !== null);
    const absent = (values: (number | null)[]) => values.every((value) => value === null);
    if (!present(plugin) && !absent(plugin))
        throw new Error("plugin stages are all present or all unavailable");
    if (!present(daemon) && !absent([...daemon, row.derive_ck_us]))
        throw new Error("daemon stages are all present or all unavailable");
    if (present(daemon) !== (row.daemon_total_us !== null))
        throw new Error("daemon_total_us accompanies exactly the daemon stages");
    const sum = (values: (number | null)[]) => values.reduce<number>((a, b) => a + (b ?? 0), 0);
    if (row.daemon_total_us !== null && sum([...daemon, row.derive_ck_us]) !== row.daemon_total_us)
        throw new Error("daemon stages must sum to daemon_total_us");
    if (present(plugin)) {
        if (row.daemon_total_us === null)
            throw new Error("plugin stages need daemon_total_us to reconcile response_us");
        if (sum(plugin) + row.daemon_total_us !== row.response_us)
            throw new Error("plugin stages plus daemon_total_us must sum to response_us");
    }
}

function member<T extends string>(name: string, value: T, allowed: readonly T[]): T {
    if (!allowed.includes(value))
        throw new Error(`${name} ${JSON.stringify(value)} is not one of ${allowed.join(", ")}`);
    return value;
}

/** One row as one JSON line, keys in the order the Rust parser serializes them. */
export function serializePassRow(row: PassRow): string {
    if (row.session.trim().length === 0) throw new Error("session must not be blank");
    if ((row.outcome === "refused") !== (row.refusal !== null))
        throw new Error("a refusal reason accompanies exactly the refused outcome");
    const ledger = Object.fromEntries(
        LEDGER_FIELDS.map((field) => [field, optionalCount(field, row[field])]),
    );
    checkLedger(row);
    return JSON.stringify({
        harness: member("harness", row.harness, HARNESSES),
        tier: member("tier", row.tier, TIERS),
        session: row.session,
        turn: count("turn", row.turn),
        boundary_state: member("boundary_state", row.boundary_state, BOUNDARY_STATES),
        outcome: member("outcome", row.outcome, OUTCOMES),
        refusal: row.refusal === null ? null : member("refusal", row.refusal, REFUSALS),
        response_us: count("response_us", row.response_us),
        service_us: row.service_us === null ? null : count("service_us", row.service_us),
        rss_bytes: count("rss_bytes", row.rss_bytes),
        ipc_bytes: count("ipc_bytes", row.ipc_bytes),
        ...ledger,
    });
}

/** Appends rows to a JSON-lines file, one synchronous write per row. */
export class PassRowWriter {
    private readonly fd: number;
    private closed = false;

    constructor(path: string) {
        this.fd = openSync(path, "a", 0o600);
    }

    write(row: PassRow): void {
        if (this.closed) throw new Error("pass row writer is closed");
        writeSync(this.fd, `${serializePassRow(row)}\n`);
    }

    close(): void {
        if (this.closed) return;
        this.closed = true;
        closeSync(this.fd);
    }
}
