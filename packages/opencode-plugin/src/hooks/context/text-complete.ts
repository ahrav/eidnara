import { sessionLog } from "../../shared/logger";
import { HOST_SDK_READ_TIMEOUT_MS, withTimeout } from "../../shared/with-timeout";
import { stripPersistedAssistantText } from "./tag-content-primitives";

type CompletedText = { sessionID: string; messageID: string; partID: string };

/** The final-text hook is awaited by OpenCode, unlike idle event subscribers. */
export function createTextCompleteHandler(
    checkpoint?: (input: CompletedText, text: string) => Promise<void>,
    notifyPending?: () => Promise<unknown>,
) {
    return async (input: CompletedText, output: { text: string }): Promise<void> => {
        output.text = stripPersistedAssistantText(output.text);
        try {
            if (output.text.trim()) await checkpoint?.(input, output.text);
        } catch (error) {
            sessionLog.warn(
                input.sessionID,
                "memory capture final-text checkpoint pending:",
                error,
            );
            try {
                if (notifyPending)
                    await withTimeout(
                        notifyPending(),
                        HOST_SDK_READ_TIMEOUT_MS,
                        "capture notification timed out",
                    );
            } catch {
                // A notification failure must not discard the completed response.
            }
        }
    };
}
