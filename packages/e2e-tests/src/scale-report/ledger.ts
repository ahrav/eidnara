/**
 * Builds a pass row's stage ledger from what one plugin pass and its final transform response
 * measured. The plugin stages plus the daemon total sum to the full-hook `response_us`; the
 * daemon stages sum to the daemon total. `other_us` is every moment of the hook outside the
 * named stages and the transport round trip, adapter work and note delivery included;
 * `transport_and_encode_us` is the residual after every other plugin stage and the daemon
 * total, the round trip less the daemon's own span.
 */

import { NO_LEDGER, type PassRow } from "./rows";

/** The plugin's pass ledger, as `observePassLedgers` delivers it. */
export interface ObservedPassLedger {
    prefixGuardMs: number;
    prepareMs: number;
    wireBuildMs: number;
    serializeMs: number;
    transportMs: number;
    applyMs: number;
    daemonLedger: Record<string, unknown> | undefined;
}

type Ledger = Pick<PassRow, keyof typeof NO_LEDGER>;

const DAEMON_FIELDS = [
    "admit_us",
    "lane_gate_and_decode_us",
    "transform_core_us",
    "native_attach_us",
    "residual_us",
    "total_us",
    "window_messages",
    "window_blocks",
] as const;

const us = (ms: number): number => Math.max(0, Math.round(ms * 1_000));

function count(value: unknown): number | undefined {
    return typeof value === "number" && Number.isSafeInteger(value) && value >= 0
        ? value
        : undefined;
}

/** The daemon ledger the response carried, or `undefined` when it is absent or malformed. */
function daemonLedger(
    value: Record<string, unknown> | undefined,
): (Record<(typeof DAEMON_FIELDS)[number], number> & { derivation_us: number | null }) | undefined {
    if (!value) return undefined;
    const fields = Object.fromEntries(DAEMON_FIELDS.map((field) => [field, count(value[field])]));
    if (DAEMON_FIELDS.some((field) => fields[field] === undefined)) return undefined;
    // A native profile's pass derives its CK; a CK profile's carries `null`.
    const deriveCk =
        value.derivation_us === null || value.derivation_us === undefined
            ? null
            : count(value.derivation_us);
    if (deriveCk === undefined) return undefined;
    return {
        ...(fields as Record<(typeof DAEMON_FIELDS)[number], number>),
        derivation_us: deriveCk,
    };
}

/**
 * The row's ledger fields for a pass whose full hook took `responseUs`. A pass without a
 * daemon ledger has every stage unavailable, since its plugin stages cannot reconcile; a pass
 * whose residual would be negative keeps only its daemon ledger.
 */
export function rowLedger(responseUs: number, observed: ObservedPassLedger | undefined): Ledger {
    const daemon = daemonLedger(observed?.daemonLedger);
    if (!observed || !daemon) return { ...NO_LEDGER };
    const daemonFields: Ledger = {
        ...NO_LEDGER,
        admit_us: daemon.admit_us,
        lane_gate_and_decode_us: daemon.lane_gate_and_decode_us,
        derivation_us: daemon.derivation_us,
        transform_core_us: daemon.transform_core_us,
        native_attach_us: daemon.native_attach_us,
        daemon_residual_us: daemon.residual_us,
        daemon_total_us: daemon.total_us,
        window_messages: daemon.window_messages,
        window_blocks: daemon.window_blocks,
    };
    const named = {
        prefix_guard_us: us(observed.prefixGuardMs),
        prepare_us: us(observed.prepareMs),
        wire_build_us: us(observed.wireBuildMs),
        serialize_us: us(observed.serializeMs),
        apply_us: us(observed.applyMs),
    };
    const namedSum = Object.values(named).reduce((a, b) => a + b, 0);
    const otherUs = Math.max(0, responseUs - namedSum - us(observed.transportMs));
    const transportAndEncodeUs = responseUs - namedSum - otherUs - daemon.total_us;
    if (transportAndEncodeUs < 0) return daemonFields;
    return {
        ...daemonFields,
        ...named,
        transport_and_encode_us: transportAndEncodeUs,
        other_us: otherUs,
    };
}
