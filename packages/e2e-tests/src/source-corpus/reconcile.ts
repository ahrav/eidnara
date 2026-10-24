import type { ExpectedRecord, ExpectedUnit } from "./expected.ts";

/** The per-unit byte cap; a larger unit is refused whole as `unit_too_large`. */
export const UNIT_CAP_BYTES = 64 * 1024;

/** One live descriptor as a reconciliation reads it: its class and identity fields. */
export interface DescriptorIdentity {
    readonly class: string;
    readonly identity: Readonly<Record<string, string>>;
}

/** Loss conditions the live reconciliation predicts from unit bytes alone. */
export type PredictedLoss = "unit_too_large" | "secret_bearing";

export interface Reconciliation {
    readonly expected: number;
    readonly admitted: number;
    readonly losses: Readonly<Record<PredictedLoss, number>>;
    /** Expected units neither admitted nor accounted to a loss condition, by key. */
    readonly unclassified: readonly string[];
    /** Live descriptors of the session that no expected unit names, by key. */
    readonly phantom: readonly string[];
}

/** `messages` units are keyed by message and block, tool units by call and block. */
export function unitKey(record: ExpectedRecord, unit: ExpectedUnit): string {
    return unit.tool
        ? `raw_tool_spans:${unit.tool.callId}:${unit.block}`
        : `messages:${record.id}:${unit.block}`;
}

function descriptorKey(descriptor: DescriptorIdentity): string | null {
    const { identity } = descriptor;
    if (descriptor.class === "messages")
        return `messages:${identity.message_id}:${identity.block_index}`;
    if (descriptor.class === "raw_tool_spans") {
        return `raw_tool_spans:${identity.tool_call_id}:${identity.block_index}`;
    }
    return null;
}

/**
 * Partitions every expected unit of `sessionId` into admitted (a live descriptor names it) or one
 * predicted loss condition: `unit_too_large` above the cap, `secret_bearing` for a unit `secret`
 * names. Anything else is unclassified, and a live descriptor no expected unit names is phantom.
 */
export function reconcile(
    records: readonly ExpectedRecord[],
    descriptors: readonly DescriptorIdentity[],
    sessionId: string,
    secret: (record: ExpectedRecord, unit: ExpectedUnit) => boolean,
): Reconciliation {
    const live = new Set(
        descriptors
            .filter((descriptor) => descriptor.identity.session_id === sessionId)
            .map(descriptorKey)
            .filter((key): key is string => key !== null),
    );
    const losses: Record<PredictedLoss, number> = { unit_too_large: 0, secret_bearing: 0 };
    const unclassified: string[] = [];
    const expectedKeys = new Set<string>();
    let expected = 0;
    let admitted = 0;
    for (const record of records) {
        for (const unit of record.units) {
            expected += 1;
            const key = unitKey(record, unit);
            expectedKeys.add(key);
            if (live.has(key)) admitted += 1;
            else if (unit.byteLen > UNIT_CAP_BYTES) losses.unit_too_large += 1;
            else if (secret(record, unit)) losses.secret_bearing += 1;
            else unclassified.push(key);
        }
    }
    const phantom = [...live].filter((key) => !expectedKeys.has(key));
    return { expected, admitted, losses, unclassified, phantom };
}
