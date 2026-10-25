export type NonUnitReason =
    | "non_text_part"
    | "ignored_text"
    | "unsettled_text"
    | "unsettled_tool"
    | "non_text_item"
    | "skipped_item"
    | "non_message_entry"
    | "uncaptured_role";

export interface ExpectedTool {
    readonly name: string;
    readonly callId: string;
    /** The OpenCode message holding the tool part, or the Pi entry's `parentId`; `null` when the entry has none. */
    readonly parentMessageId: string | null;
    readonly isError: boolean;
    readonly exit: number | null;
    readonly truncated: boolean | null;
}

export interface ExpectedUnit {
    readonly index: number;
    /** The OpenCode part id, or the Pi entry id. */
    readonly partId: string;
    /** The block index capture publishes the unit under: the part or item position for text, the output block within a tool result. */
    readonly block: number;
    readonly byteLen: number;
    readonly tool: ExpectedTool | null;
}

export interface NonUnit {
    /** The part or item position; -1 when the reason covers the whole record. */
    readonly index: number;
    readonly partId: string;
    readonly reason: NonUnitReason;
    readonly kind: string;
}

export interface ExpectedRecord {
    readonly id: string;
    readonly role: string;
    readonly units: readonly ExpectedUnit[];
    readonly nonUnits: readonly NonUnit[];
}
