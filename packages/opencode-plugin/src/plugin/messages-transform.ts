import type { ResolvedTransformMode as TransformMode } from "../config/transform-mode";
import { log } from "../shared/logger";

type MessageWithParts = {
    info: import("@opencode-ai/sdk").Message;
    parts: import("@opencode-ai/sdk").Part[];
};

type MessagesTransformOutput = { messages: MessageWithParts[] };

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
    /** The session's resolved transform mode; `ts` passes the input through unchanged. commentlint: allow(JUDGE) */
    transformMode: TransformMode;
}): (input: Record<string, never>, output: MessagesTransformOutput) => Promise<MessageWithParts[]> {
    if (args.transformMode === "ts") {
        console.warn(
            "[eidnara] transform_mode ts: messages pass through unchanged (the TypeScript transform is not part of this plugin; set transform_mode to rust to use the daemon)",
        );
        return async (_input, output): Promise<MessageWithParts[]> => output.messages;
    }

    return async (input, output): Promise<MessageWithParts[]> => {
        const eidnara = args.getEidnara ? args.getEidnara() : args.eidnara;
        // The hook edits `output.messages` in place, so a throw mid-edit would otherwise leak a partial history to the model.
        const snapshot = output.messages.slice();
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
        return output.messages;
    };
}
