import { randomBytes } from "node:crypto";
import { type ToolDefinition, tool } from "@opencode-ai/plugin";
import type { RustToolBackends } from "../../plugin/rust-tool-backends";
import { boundedCommandId, toolCallIdFromContext } from "../../plugin/rust-tool-backends";
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

/** Consecutive tag numbers collapse into one `§a§-§b§` range so a large drop stays short. */
function formatAcceptedTagsForAck(accepted: readonly number[]): string {
    const sorted = [...new Set(accepted)].sort((a, b) => a - b);
    const runs: string[] = [];
    let index = 0;
    while (index < sorted.length) {
        const start = sorted[index] as number;
        let end = start;
        while (index + 1 < sorted.length && sorted[index + 1] === end + 1) {
            index += 1;
            end = sorted[index] as number;
        }
        runs.push(start === end ? `§${start}§` : `§${start}§-§${end}§`);
        index += 1;
    }
    return runs.join(", ");
}

function tagNumberList(record: Record<string, unknown>, key: string): number[] | undefined {
    const value = record[key];
    return Array.isArray(value)
        ? value.filter((entry): entry is number => typeof entry === "number")
        : undefined;
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
    // The daemon's reduce_command_ledger keeps command ids for the session's
    // lifetime, so a fallback id must not repeat after the tool is rebuilt:
    // the incarnation nonce keeps `oc-<session>-<nonce>-1` from colliding with
    // the same counter value issued by an earlier tool instance.
    const incarnation = randomBytes(6).toString("hex");
    let fallbackCommandSequence = 0;

    const commandIdForInvocation = (sessionId: string, toolContext: unknown): string => {
        const callId = toolCallIdFromContext(toolContext);
        if (callId) return boundedCommandId(`oc-${sessionId}-${callId}`);
        fallbackCommandSequence += 1;
        return boundedCommandId(`oc-${sessionId}-${incarnation}-${fallbackCommandSequence}`);
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
                const accepted = tagNumberList(record, "accepted");
                const alreadyQueued = tagNumberList(record, "already_queued") ?? [];
                const unknown = tagNumberList(record, "unknown") ?? [];
                const details: string[] = [];
                if (alreadyQueued.length > 0) {
                    details.push(`tags ${alreadyQueued.join(", ")} already queued`);
                }
                if (unknown.length > 0) details.push(`tags ${unknown.join(", ")} not found`);
                if (queued <= 0) {
                    return unknown.length > 0
                        ? `All known requested tags were already queued or processed. No new action is needed. Tags ${unknown.join(", ")} not found.`
                        : "All requested tags were already queued or processed. No new action is needed.";
                }
                // A daemon that reports the accepted tags is authoritative; the raw
                // request is echoed only when the response carries no such list.
                if (accepted === undefined) {
                    return `Queued: drop ${formatRawDropForAck(args.drop)}.`;
                }
                const pendingBefore = new Set(alreadyQueued);
                const newlyQueued = accepted.filter((tag) => !pendingBefore.has(tag));
                const targets = formatAcceptedTagsForAck(
                    newlyQueued.length > 0 ? newlyQueued : accepted,
                );
                return `Queued: ${[`drop ${targets}`, ...details].join("; ")}.`;
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
