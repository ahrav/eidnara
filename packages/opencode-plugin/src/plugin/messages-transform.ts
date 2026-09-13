import type { ResolvedTransformMode as TransformMode } from "../config/transform-mode";
import {
    assertReferenceableMessages,
    readOwnDataProperty,
    rootArrayRejection,
    SourceRejected,
} from "../hooks/context/transform-capture";
import { log } from "../shared/logger";

type MessageWithParts = {
    info: import("@opencode-ai/sdk").Message;
    parts: import("@opencode-ai/sdk").Part[];
};

type MessagesTransformOutput = { messages: MessageWithParts[] };

function logSourceDecline(error: SourceRejected, stage: string): void {
    log[error.logLevel](`[eidnara] transform declined ${error.name}: ${error.message} (${stage})`);
}

/**
 * The hook publishes its result by replacing entries of `output.messages`, never by editing a
 * message's `info` or `parts` in place: the handler's rollback restores array membership only.
 */
type EidnaraTransformHooks = {
    "experimental.chat.messages.transform"?: (
        input: Record<string, never>,
        output: MessagesTransformOutput,
    ) => Promise<void>;
} | null;

/**
 * `ts` mode returns messages unchanged because this plugin has no TypeScript transform.
 * If the hook throws, the handler restores the pre-hook message array.
 */
export function createMessagesTransformHandler(args: {
    eidnara: EidnaraTransformHooks;
    /** `getEidnara` lets a later hook instance replace `eidnara` without rebuilding the handler. */
    getEidnara?: () => EidnaraTransformHooks;
    /** The session's resolved transform mode; `ts` passes the input through unchanged. */
    transformMode: TransformMode;
}): (
    input: Record<string, never>,
    output: MessagesTransformOutput,
) => Promise<MessageWithParts[] | undefined> {
    if (args.transformMode === "ts") {
        console.warn(
            "[eidnara] transform_mode ts: messages pass through unchanged (the TypeScript transform is not part of this plugin; set transform_mode to rust to use the daemon)",
        );
        return async (_input, output): Promise<MessageWithParts[]> => output.messages;
    }

    return async (input, output): Promise<MessageWithParts[] | undefined> => {
        const messages = readOwnDataProperty(output, "messages") as MessageWithParts[];
        try {
            assertReferenceableMessages(messages);
        } catch (error) {
            if (!(error instanceof SourceRejected)) throw error;
            logSourceDecline(error, "entry");
            return;
        }
        // A throw after the hook has replaced some entries would otherwise send that partial history to the model.
        const snapshot = messages.slice();
        const eidnara = args.getEidnara ? args.getEidnara() : args.eidnara;
        try {
            await eidnara?.["experimental.chat.messages.transform"]?.(input, output);
        } catch (error) {
            output.messages = snapshot;
            const code = (error as { code?: string } | null)?.code;
            const name = (error as { name?: string } | null)?.name;
            const message = error instanceof Error ? error.message : String(error);
            log(
                `[eidnara] transform FAILED code=${code ?? "none"} name=${name ?? "none"}: ${message}. Continuing with unmodified messages for this pass.`,
                error,
            );
        }
        // Only the root is rechecked here: nested data is the hook's published output, and this check
        // guards promise assimilation of the return value, not publication.
        const result = readOwnDataProperty(output, "messages") as MessageWithParts[];
        const rejection = rootArrayRejection(result);
        if (rejection !== undefined) {
            logSourceDecline(rejection, "return");
            return;
        }
        return result;
    };
}
