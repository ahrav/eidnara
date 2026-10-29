import { isAvailable, type KernelClient, renderToolStateText } from "../../shared/kernel-client";
import { MemoryInputError } from "../../shared/kernel-client/anti-memory";
import { CANDIDATES_PAGE_MAX } from "../../shared/kernel-client/wire";
import type { EidnaraMemoryArgs } from "./types";

const OVERLAP_NOTE =
    "A candidate is a lexical overlap between a published source and this memory, not a judgment that they contradict.";

const FILE_FORM_NOTE =
    " A row names no readable source, so file a confirm form only when you have read the conflicting source yourself in this session; otherwise report the candidate to the user. The form uses mark_disputed, which lowers the memory's standing and is reversible; contradict removes the memory from every serving surface except audit, so use it only for a source you read that directly contradicts the memory.";

const REPORT_FORM_NOTE =
    " The dispute action is not admitted in this context, so show each form to the user instead of filing it.";

export interface ExecuteCandidatesArgs {
    client: KernelClient;
    /** The trimmed target the caller's `requireTarget` established. */
    objectId: string;
    args: EidnaraMemoryArgs;
    /** Whether this tool admits `dispute`, so the reply says who can file the confirm form. */
    disputeAdmitted: boolean;
    signal?: AbortSignal;
}

/** The wrappers pass raw arguments when schema parsing fails; `after` is typed by the shared field gate, and `limit`, a number, is bounded here for both harnesses. */
function pagingOf(args: EidnaraMemoryArgs): { after?: string | null; limit?: number } {
    const { after, limit } = args as { after?: string | null; limit?: unknown };
    if (
        limit !== undefined &&
        (!Number.isSafeInteger(limit) ||
            (limit as number) < 1 ||
            (limit as number) > CANDIDATES_PAGE_MAX)
    ) {
        throw new MemoryInputError(`'limit' must be an integer from 1 to ${CANDIDATES_PAGE_MAX}`);
    }
    return {
        ...(after === undefined ? {} : { after }),
        ...(limit === undefined ? {} : { limit: limit as number }),
    };
}

export async function executeCandidates(input: ExecuteCandidatesArgs): Promise<string> {
    const { objectId } = input;
    const result = await input.client.candidates({
        decisionObjectId: objectId,
        ...pagingOf(input.args),
        ...(input.signal ? { signal: input.signal } : {}),
    });
    if (!isAvailable(result)) return `Error: ${renderToolStateText(result.state)}`;
    if (result.kind === "terminal") {
        switch (result.terminal) {
            // The contract renders a feature that is off as a plain statement, so Pi reports it as a result and not an error.
            case "disabled":
                return "Contradiction candidates are off in the daemon for this project.";
            case "not_live":
                return `Error: ${objectId} is not a live memory of this project, so it has no candidates (not_live).`;
            case "store_unavailable":
                return "Error: the candidate store is unavailable (store_unavailable); retry later.";
        }
    }
    if (result.items.some((item) => item.decision_object_id !== objectId)) {
        return `Error: the daemon answered candidates for another memory than ${objectId}; nothing was rendered. Retry once; report it to the user if it repeats.`;
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
