import { types } from "node:util";
import type { ResolvedTransformMode as TransformMode } from "../config/transform-mode";
import { readOwnDataProperty } from "../hooks/context/transform-capture";
import { log } from "../shared/logger";

type MessageWithParts = {
    info: import("@opencode-ai/sdk").Message;
    parts: import("@opencode-ai/sdk").Part[];
};

type MessagesTransformOutput = { messages: MessageWithParts[] };

function returnableMessageArray(value: unknown): boolean {
    return (
        !types.isProxy(value) &&
        Array.isArray(value) &&
        Object.getPrototypeOf(value) === Array.prototype &&
        Object.getPrototypeOf(Array.prototype) === Object.prototype &&
        Object.getPrototypeOf(Object.prototype) === null &&
        !("then" in value)
    );
}

/**
 * The hook publishes its result by replacing entries of `output.messages` in one synchronous
 * all-or-none step, never by editing a message's `info` or `parts` in place.
 */
type EidnaraTransformHooks = {
    "experimental.chat.messages.transform"?: (
        input: Record<string, never>,
        output: MessagesTransformOutput,
    ) => Promise<void>;
} | null;

/**
 * `ts` mode returns messages unchanged because this plugin has no TypeScript transform.
 * The hook owns publication. The wrapper never restores captured contents after an error.
 * Unsupported containers stay on the host output object but are not returned: promise resolution
 * reads `then`, which could invoke a proxy trap or a getter.
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
        const messages = readOwnDataProperty(output, "messages");
        if (!returnableMessageArray(messages)) {
            log("[eidnara] transform declined: unsupported wrapper array or then property");
            return;
        }
        const eidnara = args.getEidnara ? args.getEidnara() : args.eidnara;
        try {
            await eidnara?.["experimental.chat.messages.transform"]?.(input, output);
        } catch (error) {
            const code = (error as { code?: string } | null)?.code;
            const name = (error as { name?: string } | null)?.name;
            const message = error instanceof Error ? error.message : String(error);
            log(
                `[eidnara] transform FAILED code=${code ?? "none"} name=${name ?? "none"}: ${message}. Keeping current host messages for this pass.`,
                error,
            );
        }
        const result = readOwnDataProperty(output, "messages") as MessageWithParts[];
        if (!returnableMessageArray(result)) {
            log(
                "[eidnara] transform return declined SourceRejected: unsupported array or then property",
            );
            return;
        }
        return result;
    };
}
