import type { ToolDefinition } from "@earendil-works/pi-coding-agent";
import { resolveProjectRootDirectory } from "@eidnara/opencode/features/context/project-identity";
import type { RustToolBackends } from "@eidnara/opencode/plugin/rust-tool-backends";
import { getErrorMessage } from "@eidnara/opencode/shared/error-message";
import { CTX_REDUCE_DESCRIPTION } from "@eidnara/opencode/tools/ctx-reduce/constants";
import { unwrapImitatedReducedArgs } from "@eidnara/opencode/tools/unwrap-imitated-reduced-args";
import { type Static, Type } from "typebox";
import { boundedCommandId } from "./command-id";

const ParamsSchema = Type.Object(
    {
        drop: Type.Optional(
            Type.String({
                description: "Tag IDs to drop entirely. Ranges: '3-5', '1,2,9'",
            }),
        ),
    },
    { additionalProperties: true },
);

type CtxReduceParams = Static<typeof ParamsSchema>;

function ok(text: string) {
    return { content: [{ type: "text" as const, text }], details: undefined };
}

function err(text: string) {
    return {
        content: [{ type: "text" as const, text }],
        details: undefined,
        isError: true,
    };
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

export interface CtxReduceToolDeps {
    rustToolBackends: RustToolBackends;
}

export function createCtxReduceTool(deps: CtxReduceToolDeps): ToolDefinition<typeof ParamsSchema> {
    let fallbackCommandSequence = 0;

    // A call without a provider id gets a per-instance sequence so retries of distinct calls never share a command.
    const commandIdForInvocation = (sessionId: string, toolCallId: string | undefined): string => {
        const callId = toolCallId?.trim();
        if (callId) return boundedCommandId(`pi-${sessionId}-${callId}`);
        fallbackCommandSequence += 1;
        return boundedCommandId(`pi-${sessionId}-${fallbackCommandSequence}`);
    };

    return {
        name: "ctx_reduce",
        label: "Eidnara: Reduce",
        description: CTX_REDUCE_DESCRIPTION,
        parameters: ParamsSchema,
        async execute(toolCallId, params: CtxReduceParams, signal, _onUpdate, ctx) {
            params = unwrapImitatedReducedArgs(params, ["drop"], { drop: "string" });
            const sessionId = ctx.sessionManager.getSessionId();

            if (!params.drop) {
                return err("Error: 'drop' must be provided.");
            }

            const rustReduce = deps.rustToolBackends.reduce;
            if (!rustReduce) {
                return err(
                    "Error: Failed to queue ctx_reduce operations. The daemon backend is unavailable.",
                );
            }
            try {
                const response = await rustReduce({
                    sessionId,
                    projectRoot: resolveProjectRootDirectory(ctx.cwd),
                    drop: params.drop,
                    commandId: commandIdForInvocation(sessionId, toolCallId),
                    ...(signal ? { signal } : {}),
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
                    return err(`Error: Failed to queue ctx_reduce operations. ${message}`);
                }
                const queued = typeof record.queued === "number" ? record.queued : 0;
                if (queued <= 0) {
                    return ok(
                        "All requested tags were already queued or processed. No new action is needed.",
                    );
                }
                return ok(`Queued: drop ${formatRawDropForAck(params.drop)}.`);
            } catch (error) {
                return err(
                    `Error: Failed to queue ctx_reduce operations. ${getErrorMessage(error)}`,
                );
            }
        },
    };
}
