type MessageTime = { created?: number };

type MessageInfo = {
    role?: string;
    time?: MessageTime;
};

type MessagePart = {
    type?: string;
    text?: string;
};

type SessionMessage = {
    info?: MessageInfo;
    parts?: unknown;
};

import { ownKeys, readField } from "./guarded-read";
import { isRecord } from "./record-type-guard";

/** A message whose accessor or proxy trap throws is unusable and yields `null`. */
function asSessionMessage(value: unknown): SessionMessage | null {
    try {
        return readSessionMessage(value);
    } catch {
        return null;
    }
}

function readSessionMessage(value: unknown): SessionMessage | null {
    if (!isRecord(value)) return null;
    const info = value.info;
    const parts = value.parts;
    if (!isRecord(info)) return { info: undefined, parts };
    const time = info.time;
    // A single read keeps validation and storage on the same value when `created` is a getter.
    const created = isRecord(time) ? time.created : undefined;
    return {
        info: {
            role: typeof info.role === "string" ? info.role : undefined,
            time: isRecord(time)
                ? {
                      created:
                          typeof created === "number" && Number.isFinite(created)
                              ? created
                              : undefined,
                  }
                : undefined,
        },
        parts,
    };
}

/** Absent or non-finite timestamps sort below every real one, including `0`. */
function getCreatedTime(message: SessionMessage): number {
    return message.info?.time?.created ?? Number.NEGATIVE_INFINITY;
}

function getTextParts(message: SessionMessage): MessagePart[] {
    if (!Array.isArray(message.parts)) return [];
    return message.parts
        .filter((part): part is Record<string, unknown> => isRecord(part))
        .map((part) => ({
            type: typeof part.type === "string" ? part.type : undefined,
            text: typeof part.text === "string" ? part.text : undefined,
        }))
        .filter((part) => part.type === "text" && Boolean(part.text));
}

export function extractLatestAssistantText(messages: unknown): string | null {
    if (!Array.isArray(messages) || messages.length === 0) return null;

    // `>=` lets a later array position win a timestamp tie. Elements are read through `readField`
    // so a trapping index skips that entry instead of ending the scan.
    let latest: SessionMessage | undefined;
    let latestCreated = Number.NEGATIVE_INFINITY;
    for (let index = 0; index < messages.length; index += 1) {
        const message = asSessionMessage(readField(messages, index));
        if (message?.info?.role !== "assistant") continue;
        const created = getCreatedTime(message);
        if (created >= latestCreated) {
            latest = message;
            latestCreated = created;
        }
    }
    if (!latest) return null;

    // A latest message whose parts trap on read has no readable text.
    try {
        return (
            getTextParts(latest)
                .map((part) => part.text)
                .join("\n") || null
        );
    } catch {
        return null;
    }
}

/** A payload that cannot be enumerated at all is reported as not capped rather than propagating. */
export function hasLengthCappedOutput(value: unknown): boolean {
    try {
        return walkForLengthCap(value, new WeakSet());
    } catch {
        return false;
    }
}

/**
 * Marker and member reads go through `readField` and `ownKeys`, so a trapping property is skipped
 * and a capping marker on a readable sibling is still found.
 */
function walkForLengthCap(value: unknown, seen: WeakSet<object>): boolean {
    // `seen` prevents recursive traversal from looping on cyclic or revisiting shared object references.
    if (typeof value === "object" && value !== null) {
        if (seen.has(value)) return false;
        seen.add(value);
    }
    if (Array.isArray(value)) {
        for (let index = 0; index < value.length; index += 1) {
            if (walkForLengthCap(readField(value, index), seen)) return true;
        }
        return false;
    }
    if (!isRecord(value)) return false;

    if (readField(value, "length_capped") === true || readField(value, "lengthCapped") === true) {
        return true;
    }
    if (
        isCappingFinishReason(readField(value, "finish_reason")) ||
        isCappingFinishReason(readField(value, "finishReason"))
    ) {
        return true;
    }

    for (const key of ownKeys(value)) {
        if (walkForLengthCap(readField(value, key), seen)) return true;
    }
    return false;
}

function isCappingFinishReason(value: unknown): boolean {
    if (typeof value !== "string") return false;
    const normalized = value.toLowerCase();
    return (
        normalized === "length" || normalized === "max_tokens" || normalized === "max_output_tokens"
    );
}
