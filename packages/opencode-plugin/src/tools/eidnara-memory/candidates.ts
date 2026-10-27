import { isAvailable, type KernelClient, renderToolStateText } from "../../shared/kernel-client";
import type { EidnaraMemoryArgs } from "./types";

const CANDIDATES_NOTE =
    "A candidate is a lexical overlap between a published source and this memory, not a judgment that they contradict. Read the source before filing its dispute form, choosing mark_disputed or contradict.";

export interface ExecuteCandidatesArgs {
    client: KernelClient;
    args: EidnaraMemoryArgs;
    signal?: AbortSignal;
}

export async function executeCandidates(input: ExecuteCandidatesArgs): Promise<string> {
    const objectId = input.args.objectId?.trim();
    if (!objectId) return "Error: 'objectId' is required when action is 'candidates'.";
    const result = await input.client.candidates({
        decisionObjectId: objectId,
        after: input.args.after ?? null,
        ...(input.args.limit === undefined ? {} : { limit: input.args.limit }),
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
    return JSON.stringify({
        action: "candidates",
        objectId,
        note: CANDIDATES_NOTE,
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
