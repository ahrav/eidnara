import {
    type CandidatesResult,
    isAvailable,
    type KernelClient,
    renderToolStateText,
} from "../../shared/kernel-client";
import { MemoryInputError } from "../../shared/kernel-client/anti-memory";
import { CANDIDATES_PAGE_MAX, type CandidateItem } from "../../shared/kernel-client/wire";
import { EIDNARA_MEMORY_RESPONSE_BUDGET_BYTES } from "./constants";
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

type CandidatesPage = Extract<CandidatesResult, { kind: "page" }>;

/** One rendered row: the ids and integers the model reads and the dispute form that would confirm the row. */
function renderRow(item: CandidateItem) {
    return {
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
    };
}

/** How many leading rows render inside `EIDNARA_MEMORY_RESPONSE_BUDGET_BYTES` beside `envelope`, which carries every other field of the reply, `next` included; the ids' caps keep one row inside it. */
function rowsWithinBudget(envelope: Record<string, unknown>, rows: unknown[]): number {
    let used = Buffer.byteLength(JSON.stringify({ ...envelope, candidates: [] }), "utf8");
    for (const [index, row] of rows.entries()) {
        used += Buffer.byteLength(JSON.stringify(row), "utf8") + 1;
        if (used > EIDNARA_MEMORY_RESPONSE_BUDGET_BYTES) return index;
    }
    return rows.length;
}

export async function executeCandidates(input: ExecuteCandidatesArgs): Promise<string> {
    const { objectId } = input;
    const paging = pagingOf(input.args);
    const ask = (limit?: number) =>
        input.client.candidates({
            decisionObjectId: objectId,
            ...paging,
            ...(limit === undefined ? {} : { limit }),
            ...(input.signal ? { signal: input.signal } : {}),
        });
    const judge = (result: CandidatesResult, limit: number) => {
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
        if (result.items.length > limit) {
            return `Error: the daemon answered ${result.items.length} candidates for a page of ${limit}; nothing was rendered. Retry once; report it to the user if it repeats.`;
        }
        if (result.items.some((item) => item.decision_object_id !== objectId)) {
            return `Error: the daemon answered candidates for another memory than ${objectId}; nothing was rendered. Retry once; report it to the user if it repeats.`;
        }
        return result;
    };
    const note = OVERLAP_NOTE + (input.disputeAdmitted ? FILE_FORM_NOTE : REPORT_FORM_NOTE);
    // The reply's fields beside the rows, measured with the rows; `next` is a cursor of up to about 1 KiB.
    const envelope = (page: CandidatesPage) => ({
        action: "candidates",
        objectId,
        note,
        asOf: page.as_of,
        next: page.next,
    });
    let page = judge(await ask(), paging.limit ?? CANDIDATES_PAGE_MAX);
    if (typeof page === "string") return page;
    let rows = page.items.map(renderRow);
    const fitted = rowsWithinBudget(envelope(page), rows);
    if (fitted < rows.length) {
        // The cursor is opaque, so the page of the fitted size comes from the daemon: its next lands after the last rendered row.
        const limit = Math.max(fitted, 1);
        page = judge(await ask(limit), limit);
        if (typeof page === "string") return page;
        rows = page.items.map(renderRow);
        if (rowsWithinBudget(envelope(page), rows) < rows.length) {
            // The second page holds other rows than the first, so the store changed between the two reads.
            return `Error: the candidates of ${objectId} changed between two reads; nothing was rendered. Retry once; report it to the user if it repeats.`;
        }
    }
    return JSON.stringify({ ...envelope(page), candidates: rows });
}
