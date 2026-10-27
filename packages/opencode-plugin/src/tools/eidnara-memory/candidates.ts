import { isAvailable, type KernelClient, renderToolStateText } from "../../shared/kernel-client";
import { MemoryInputError } from "../../shared/kernel-client/anti-memory";
import type { EidnaraMemoryArgs } from "./types";

const OVERLAP_NOTE =
    "A candidate is a lexical overlap between a published source and this memory, not a judgment that they contradict.";

const FILE_FORM_NOTE =
    " File a confirm form only with independent evidence that the source conflicts with the memory; otherwise report the candidate to the user. The form uses mark_disputed; change event to contradict only when the source directly contradicts the memory.";

const REPORT_FORM_NOTE =
    " The dispute action is not admitted in this context, so show each form to the user instead of filing it.";

export interface ExecuteCandidatesArgs {
    client: KernelClient;
    args: EidnaraMemoryArgs;
    /** Whether this tool admits `dispute`, so the reply says who can file the confirm form. */
    disputeAdmitted: boolean;
    signal?: AbortSignal;
}

/** The wrappers pass raw arguments when schema parsing fails, so the paging fields are checked here for both harnesses. */
function pagingOf(args: EidnaraMemoryArgs): { after?: string | null; limit?: number } {
    const { after, limit } = args as { after?: unknown; limit?: unknown };
    if (after !== undefined && after !== null && typeof after !== "string") {
        throw new MemoryInputError("'after' must be a string or null");
    }
    if (
        limit !== undefined &&
        (!Number.isSafeInteger(limit) || (limit as number) < 1 || (limit as number) > 64)
    ) {
        throw new MemoryInputError("'limit' must be an integer from 1 to 64");
    }
    return {
        ...(after === undefined ? {} : { after }),
        ...(limit === undefined ? {} : { limit: limit as number }),
    };
}

export async function executeCandidates(input: ExecuteCandidatesArgs): Promise<string> {
    const objectId = input.args.objectId?.trim();
    if (!objectId) return "Error: 'objectId' is required when action is 'candidates'.";
    const result = await input.client.candidates({
        decisionObjectId: objectId,
        ...pagingOf(input.args),
        ...(input.signal ? { signal: input.signal } : {}),
    });
    if (!isAvailable(result)) return renderToolStateText(result.state);
    if (result.kind === "terminal") {
        switch (result.terminal) {
            case "disabled":
                return "Contradiction candidates are off in the daemon for this project.";
            case "not_live":
                return `Error: ${objectId} is not a live memory of this project, so it has no candidates (not_live).`;
            case "store_unavailable":
                return "Error: the candidate store is unavailable (store_unavailable); retry later.";
        }
    }
    if (result.items.some((item) => item.decision_object_id !== objectId)) {
        return `Error: the daemon answered candidates for another memory than ${objectId}; nothing was rendered.`;
    }
    return JSON.stringify({
        action: "candidates",
        objectId,
        note: OVERLAP_NOTE + (input.disputeAdmitted ? FILE_FORM_NOTE : REPORT_FORM_NOTE),
        asOf: result.as_of,
        candidates: result.items.map((item) => ({
            evidenceId: item.evidence_id,
            decisionId: item.decision_id,
            score: item.score,
            matchedTerms: item.matched_terms,
            decisionTerms: item.decision_terms,
            publishTip: item.publish_tip,
            createdAtMs: item.created_at_ms,
            confirm: {
                action: "dispute",
                event: "mark_disputed",
                objectId: item.decision_object_id,
                evidenceId: item.evidence_id,
            },
        })),
        next: result.next,
    });
}
