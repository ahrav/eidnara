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
    return {
        info: isRecord(info)
            ? {
                  role: typeof info.role === "string" ? info.role : undefined,
                  time: isRecord(info.time)
                      ? {
                            created: Number.isFinite(info.time.created)
                                ? (info.time.created as number)
                                : undefined,
                        }
                      : undefined,
              }
            : undefined,
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

    // `>=` lets a later array position win a timestamp tie.
    let latest: SessionMessage | undefined;
    let latestCreated = Number.NEGATIVE_INFINITY;
    for (const raw of messages) {
        const message = asSessionMessage(raw);
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

/** A payload whose accessor or proxy trap throws is reported as not capped rather than propagating. */
export function hasLengthCappedOutput(value: unknown): boolean {
    try {
        return walkForLengthCap(value, new WeakSet());
    } catch {
        return false;
    }
}

function walkForLengthCap(value: unknown, seen: WeakSet<object>): boolean {
    // `seen` prevents recursive traversal from looping on cyclic or revisiting shared object references.
    if (typeof value === "object" && value !== null) {
        if (seen.has(value)) return false;
        seen.add(value);
    }
    if (Array.isArray(value)) return value.some((item) => walkForLengthCap(item, seen));
    if (!isRecord(value)) return false;

    if (value.length_capped === true || value.lengthCapped === true) return true;
    if (isCappingFinishReason(value.finish_reason) || isCappingFinishReason(value.finishReason)) {
        return true;
    }

    return Object.values(value).some((item) => walkForLengthCap(item, seen));
}

function isCappingFinishReason(value: unknown): boolean {
    if (typeof value !== "string") return false;
    const normalized = value.toLowerCase();
    return (
        normalized === "length" || normalized === "max_tokens" || normalized === "max_output_tokens"
    );
}
