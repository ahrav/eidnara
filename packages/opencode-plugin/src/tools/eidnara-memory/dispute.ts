/**
 * The `eidnara_memory` `dispute` action: a model-filed `mark_disputed` or
 * `contradict` that cites evidence. It runs the shared `/eidnara-memory-mark`
 * runner, so the daemon validates the evidence exactly as it does for the
 * command, under the tool's actor, the `assistant` source kind, and the
 * `(session, tool-call id)` identity every tool write uses. The runner's
 * preview supplies whether served visibility changes; the action never asks
 * for confirmation.
 */

import {
    type KernelClient,
    OPERATION_KEY_SEPARATOR,
    renderToolStateText,
    stateKey,
} from "../../shared/kernel-client";
import { runMemoryMarkCommand } from "../../shared/memory-mark-command";
import { EIDNARA_MEMORY_DISPUTE_EVENTS, type EidnaraMemoryArgs } from "./types";

const DISPUTE_USAGE =
    "Error: dispute needs objectId, event (mark_disputed or contradict), and evidenceId, or path with gitObjectId, or both.";

export interface ExecuteDisputeArgs {
    client: KernelClient;
    args: EidnaraMemoryArgs;
    identity: { sessionId: string; toolCallId: string };
    actor: string;
}

function isDisputeEvent(value: unknown): value is (typeof EIDNARA_MEMORY_DISPUTE_EVENTS)[number] {
    return (EIDNARA_MEMORY_DISPUTE_EVENTS as readonly unknown[]).includes(value);
}

export async function executeDispute(input: ExecuteDisputeArgs): Promise<string> {
    const { args, identity } = input;
    const objectId = args.objectId?.trim();
    const evidenceId = args.evidenceId ?? undefined;
    const path = args.path ?? undefined;
    const gitObjectId = args.gitObjectId ?? undefined;
    if (!objectId) return DISPUTE_USAGE;
    if (!isDisputeEvent(args.event)) {
        return `Error: dispute event '${String(args.event)}' is not supported; use mark_disputed or contradict.`;
    }
    if ((path === undefined) !== (gitObjectId === undefined)) return DISPUTE_USAGE;
    if (evidenceId === undefined && path === undefined) return DISPUTE_USAGE;
    const outcome = await runMemoryMarkCommand({
        client: input.client,
        operationId: `${identity.sessionId}${OPERATION_KEY_SEPARATOR}${identity.toolCallId}`,
        actor: input.actor,
        cause: `eidnara_memory dispute ${identity.toolCallId}`,
        sourceKind: "assistant",
        args: {
            event: args.event,
            objectId,
            ...(evidenceId === undefined ? {} : { evidenceId }),
            ...(path === undefined || gitObjectId === undefined
                ? {}
                : { evidenceRef: { path, git_object_id: gitObjectId } }),
            confirmed: true,
        },
    });
    switch (outcome.kind) {
        case "applied":
        case "denied":
            return JSON.stringify({
                action: "dispute",
                outcome: outcome.kind,
                objectId: outcome.result.object_id,
                event: outcome.result.event,
                previousDisposition: outcome.result.previous_disposition,
                disposition: outcome.result.disposition,
                ...(outcome.kind === "applied"
                    ? {
                          commitSeq: outcome.commitSeq,
                          replayed: outcome.replayed,
                          ...(outcome.visibilityChanged === undefined
                              ? {}
                              : { visibilityChanged: outcome.visibilityChanged }),
                      }
                    : {}),
            });
        case "refused":
            return `Error: dispute refused at ${outcome.step}: ${renderToolStateText(outcome.state)} (${stateKey(outcome.state)})`;
        default:
            // `confirmed: true` means the runner never stops for confirmation.
            return `Error: dispute stopped at ${outcome.kind}; nothing was written.`;
    }
}
