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

function asSessionMessage(value: unknown): SessionMessage | null {
    if (!isRecord(value)) return null;
    const info = value.info;
    const parts = value.parts;
    return {
        info: isRecord(info)
            ? {
                  role: typeof info.role === "string" ? info.role : undefined,
                  time: isRecord(info.time)
                      ? {
                            created:
                                typeof info.time.created === "number"
                                    ? info.time.created
                                    : undefined,
                        }
                      : undefined,
              }
            : undefined,
        parts,
    };
}

function getCreatedTime(message: SessionMessage): number {
    return message.info?.time?.created ?? 0;
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

    return (
        getTextParts(latest)
            .map((part) => part.text)
            .join("\n") || null
    );
}

export function hasLengthCappedOutput(
    value: unknown,
    seen: WeakSet<object> = new WeakSet(),
): boolean {
    // `seen` prevents recursive traversal from looping on cyclic or revisiting shared object references.
    if (typeof value === "object" && value !== null) {
        if (seen.has(value)) return false;
        seen.add(value);
    }
    if (Array.isArray(value)) return value.some((item) => hasLengthCappedOutput(item, seen));
    if (!isRecord(value)) return false;

    if (value.length_capped === true || value.lengthCapped === true) return true;
    const finishReason = value.finish_reason ?? value.finishReason;
    if (typeof finishReason === "string") {
        const normalized = finishReason.toLowerCase();
        if (
            normalized === "length" ||
            normalized === "max_tokens" ||
            normalized === "max_output_tokens"
        ) {
            return true;
        }
    }

    return Object.values(value).some((item) => hasLengthCappedOutput(item, seen));
}
