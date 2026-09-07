import { createHash } from "node:crypto";
import { type ToolDefinition, tool } from "@opencode-ai/plugin";
import type { RustToolBackends } from "../../plugin/rust-tool-backends";
import { getErrorMessage } from "../../shared/error-message";
import { unwrapImitatedReducedArgs } from "../unwrap-imitated-reduced-args";
import { CTX_REDUCE_DESCRIPTION } from "./constants";
import type { CtxReduceArgs } from "./types";

export { CTX_REDUCE_LIGHT_DESCRIPTION } from "../light-descriptions";

export interface CtxReduceToolDeps {
    rustToolBackends: RustToolBackends;
}

function formatRawDropForAck(rawDrop: string): string {
    return rawDrop
        .trim()
        .split(",")
        .map((token) => {
            const trimmed = token.trim();
            return /^\d+$/.test(trimmed) ? `§${trimmed}§` : trimmed;
        })
        .join(", ");
}

const ctxReduceArgsShape = {
    drop: tool.schema
        .string()
        .optional()
        .describe("Tag IDs to drop entirely. Ranges: '3-5', '1,2,9'"),
};
// Accepts fields outside ctxReduceArgsShape.
const ctxReduceArgsSchema = tool.schema.object(ctxReduceArgsShape).passthrough();

function createCtxReduceTool(deps: CtxReduceToolDeps): ToolDefinition {
    let fallbackCommandSequence = 0;

    const commandIdForInvocation = (sessionId: string, toolContext: unknown): string => {
        const context =
            toolContext !== null && typeof toolContext === "object"
                ? (toolContext as Record<string, unknown>)
                : {};
        const callId =
            (typeof context.callID === "string" && context.callID.trim()) ||
            (typeof context.callId === "string" && context.callId.trim());
        if (callId) {
            const stableId = `oc-${sessionId}-${callId}`;
            if (Buffer.byteLength(stableId) <= 128) return stableId;
            return `oc-${createHash("sha256").update(stableId).digest("hex")}`;
        }
        fallbackCommandSequence += 1;
        const monotonicId = `oc-${sessionId}-${fallbackCommandSequence}`;
        return Buffer.byteLength(monotonicId) <= 128
            ? monotonicId
            : `oc-${createHash("sha256").update(monotonicId).digest("hex")}`;
    };

    return tool({
        description: CTX_REDUCE_DESCRIPTION,
        args: ctxReduceArgsShape,
        async execute(rawArgs: CtxReduceArgs, toolContext) {
            const parsedArgs = ctxReduceArgsSchema.safeParse(rawArgs);
            let args = (parsedArgs.success ? parsedArgs.data : rawArgs) as CtxReduceArgs;
            args = unwrapImitatedReducedArgs(args, ["drop"], { drop: "string" });
            const sessionId = toolContext.sessionID;

            if (!args.drop) {
                return "Error: 'drop' must be provided.";
            }

            const rustReduce = deps.rustToolBackends.reduce;
            if (!rustReduce) {
                return "Error: Failed to queue ctx_reduce operations. The daemon backend is unavailable.";
            }
            try {
                const response = await rustReduce({
                    sessionId,
                    projectRoot: toolContext.directory,
                    drop: args.drop,
                    commandId: commandIdForInvocation(sessionId, toolContext),
                });
                const value =
                    response !== null && typeof response === "object" && "result" in response
                        ? (response as { result?: unknown }).result
                        : response;
                const record =
                    value !== null && typeof value === "object"
                        ? (value as Record<string, unknown>)
                        : null;
                if (record === null || record.ok !== true) {
                    const error =
                        record && "error" in record
                            ? (record as { error?: unknown }).error
                            : undefined;
                    const errorRecord =
                        error !== null && typeof error === "object"
                            ? (error as { message?: unknown })
                            : undefined;
                    const message =
                        (typeof error === "string" && error.trim() ? error : undefined) ??
                        (typeof errorRecord?.message === "string" && errorRecord.message.trim()
                            ? errorRecord.message
                            : undefined) ??
                        (typeof record?.message === "string" && record.message.trim()
                            ? record.message
                            : "module rejected agent_drops.append");
                    return `Error: Failed to queue ctx_reduce operations. ${message}`;
                }
                const queued = typeof record.queued === "number" ? record.queued : 0;
                if (queued <= 0) {
                    return "All requested tags were already queued or processed. No new action is needed.";
                }
                return `Queued: drop ${formatRawDropForAck(args.drop)}.`;
            } catch (error) {
                return `Error: Failed to queue ctx_reduce operations. ${getErrorMessage(error)}`;
            }
        },
    });
}

export function createCtxReduceTools(deps: CtxReduceToolDeps): Record<string, ToolDefinition> {
    return {
        ctx_reduce: createCtxReduceTool(deps),
    };
}
