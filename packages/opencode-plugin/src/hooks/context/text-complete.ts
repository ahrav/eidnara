import { stripPersistedAssistantText } from "./tag-content-primitives";

/** Strips persisted Eidnara tag notation from a completed assistant text part. */
export function createTextCompleteHandler() {
    return async (
        _input: { sessionID: string; messageID: string; partID: string },
        output: { text: string },
    ): Promise<void> => {
        output.text = stripPersistedAssistantText(output.text);
    };
}
