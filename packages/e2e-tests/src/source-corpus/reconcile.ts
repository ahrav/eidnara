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
    /** One key per unmatched `messages` or `raw_tool_spans` descriptor in `sessionId`. */
    readonly phantom: readonly string[];
}

export function unitKey(record: ExpectedRecord, unit: ExpectedUnit): string {
    return unit.tool
        ? `raw_tool_spans:${unit.tool.parentMessageId}:${unit.tool.callId}:${unit.block}`
        : `messages:${record.id}:${unit.block}`;
}

function descriptorKey(descriptor: DescriptorIdentity): string | null {
    const { identity } = descriptor;
    if (descriptor.class === "messages")
        return `messages:${identity.message_id}:${identity.block_index}`;
    if (descriptor.class === "raw_tool_spans") {
        return `raw_tool_spans:${identity.parent_message_id}:${identity.tool_call_id}:${identity.block_index}`;
    }
    return null;
}

/**
 * Partitions every expected unit of `sessionId` into admitted (a live descriptor names it) or one
 * predicted loss condition: `unit_too_large` above the cap, `secret_bearing` for a unit `secret`
 * names. Each descriptor admits at most one unit. Anything else is unclassified, and each
 * descriptor left over, such as a second revision of one tool result, is phantom.
 */
export function reconcile(
    records: readonly ExpectedRecord[],
    descriptors: readonly DescriptorIdentity[],
    sessionId: string,
    secret: (record: ExpectedRecord, unit: ExpectedUnit) => boolean,
): Reconciliation {
    const live = new Map<string, number>();
    for (const descriptor of descriptors) {
        if (descriptor.identity.session_id !== sessionId) continue;
        const key = descriptorKey(descriptor);
        if (key !== null) live.set(key, (live.get(key) ?? 0) + 1);
    }
    const losses: Record<PredictedLoss, number> = { unit_too_large: 0, secret_bearing: 0 };
    const unclassified: string[] = [];
    let expected = 0;
    let admitted = 0;
    for (const record of records) {
        for (const unit of record.units) {
            expected += 1;
            const key = unitKey(record, unit);
            const remaining = live.get(key) ?? 0;
            if (remaining > 0) {
                live.set(key, remaining - 1);
                admitted += 1;
            } else if (unit.byteLen > UNIT_CAP_BYTES) losses.unit_too_large += 1;
            else if (secret(record, unit)) losses.secret_bearing += 1;
            else unclassified.push(key);
        }
    }
    const phantom = [...live].flatMap(([key, count]) => Array<string>(count).fill(key));
    return { expected, admitted, losses, unclassified, phantom };
}
