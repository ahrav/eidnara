/**
 * Writes scale report pass rows as JSON lines that `eval_core::parse_pass_row` reads
 * losslessly: every field present, keys in the Rust struct's order, closed vocabularies, and
 * only safe non-negative integers.
 */

import { closeSync, openSync, writeSync } from "node:fs";

export type ScaleTier = "10k" | "s3_100k" | "s4_1m";
export type ScaleHarness = "opencode" | "pi";
export type BoundaryState = "cold" | "warming" | "steady" | "after_restart";
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
}

const TIERS: readonly ScaleTier[] = ["10k", "s3_100k", "s4_1m"];
const HARNESSES: readonly ScaleHarness[] = ["opencode", "pi"];
const BOUNDARY_STATES: readonly BoundaryState[] = ["cold", "warming", "steady", "after_restart"];
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
