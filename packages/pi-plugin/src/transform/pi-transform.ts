import type { CompactionEntry, SessionEntry } from "@earendil-works/pi-coding-agent";
import {
    defaultTransformCaptureAdmission,
    type TransformCaptureAdmission,
} from "@eidnara/opencode/hooks/context/transform-capture";
import {
    createTransformSessionClient,
    type RustModeModuleClient,
    type TransformBoundary,
    type TransformPassOutcome,
} from "@eidnara/opencode/hooks/context/transform-session-client";

import type { PiBranchIndex, PiBranchReader } from "./pi-branch";
import { encodePiRowsToCk, isPiRole, PI_RESERVED_ID_PREFIX, type PiRow, piRowSize } from "./pi-ck";

type Json = Record<string, unknown>;

function reservedPiId(role: string, entryId: string): string {
    return `${PI_RESERVED_ID_PREFIX}${role}:${entryId}`;
}

/**
 * The id of the message `entry` contributes to Pi's context when `message` is that message,
 * `null` when it is another, and `undefined` when the entry contributes none. A live custom
 * message is stamped before its entry is written, so custom and branch summary messages match
 * by content rather than by timestamp.
 */
function entryIdFor(entry: SessionEntry, message: Json): string | null | undefined {
    const record = entry as unknown as Json;
    switch (entry.type) {
        case "message": {
            const own = record.message as Json | undefined;
            if (!own) return undefined;
            return own.role === message.role && own.timestamp === message.timestamp
                ? entry.id
                : null;
        }
        case "custom_message":
            return message.role === "custom" &&
                message.customType === record.customType &&
                JSON.stringify(message.content) === JSON.stringify(record.content)
                ? reservedPiId("custom", entry.id)
                : null;
        case "branch_summary":
            if (!record.summary) return undefined;
            return message.role === "branchSummary" &&
                message.summary === record.summary &&
                message.fromId === record.fromId
                ? reservedPiId("branchSummary", entry.id)
                : null;
        default:
            return undefined;
    }
}

function failedAssistant(entry: SessionEntry): boolean {
    const message = (entry as unknown as Json).message as Json | undefined;
    return (
        entry.type === "message" && message?.role === "assistant" && message.stopReason === "error"
    );
}

export class PiAlignment {
    /** Ids from the end: `fromEnd[k]` names message `messages.length - 1 - k`. */
    private readonly fromEnd: string[] = [];
    private cursor: number;
    private failed = false;
    entriesRead = 0;

    constructor(
        private readonly messages: readonly Json[],
        private readonly branch: PiBranchIndex,
        private readonly reader: PiBranchReader,
    ) {
        this.cursor = branch.length - 1;
    }

    idAt(index: number): string | undefined {
        const k = this.messages.length - 1 - index;
        if (k < 0) return undefined;
        while (!this.failed && this.fromEnd.length <= k) this.step();
        return this.fromEnd[k];
    }

    holds(id: string): boolean {
        let entryId = id;
        let role: string | undefined;
        if (id.startsWith(PI_RESERVED_ID_PREFIX)) {
            const rest = id.slice(PI_RESERVED_ID_PREFIX.length);
            const separator = rest.indexOf(":");
            if (separator < 0) return false;
            role = rest.slice(0, separator);
            entryId = rest.slice(separator + 1);
        }
        const position = this.branch.indexOf(entryId);
        if (position === undefined) return false;
        const compaction = this.branch.latestCompaction();
        if (role === "compactionSummary") return position === compaction;
        if (compaction === undefined || position >= compaction) return true;
        const compactionId = this.branch.idAt(compaction);
        const entry = compactionId === undefined ? undefined : this.reader.getEntry(compactionId);
        const firstKept = (entry as { firstKeptEntryId?: unknown } | undefined)?.firstKeptEntryId;
        const keptFrom = typeof firstKept === "string" ? this.branch.indexOf(firstKept) : undefined;
        return keptFrom === undefined || position >= keptFrom;
    }

    private step(): void {
        const next = this.messages.length - 1 - this.fromEnd.length;
        const message = this.messages[next] as Json;
        if (next === 0 && message.role === "compactionSummary") {
            const compaction = this.branch.latestCompaction();
            const id = compaction === undefined ? undefined : this.branch.idAt(compaction);
            if (id === undefined) this.failed = true;
            else this.fromEnd.push(reservedPiId("compactionSummary", id));
            return;
        }
        while (this.cursor >= 0) {
            const id = this.branch.idAt(this.cursor);
            this.cursor -= 1;
            this.entriesRead += 1;
            const entry = id === undefined ? undefined : this.reader.getEntry(id);
            if (!entry) break;
            const matched = entryIdFor(entry, message);
            if (matched === undefined) continue;
            if (matched !== null) {
                this.fromEnd.push(matched);
                return;
            }
            if (failedAssistant(entry)) continue;
            break;
        }
        this.failed = true;
    }
}

export interface PiPassInputs {
    sessionId: string;
    messages: readonly Json[];
    reader: PiBranchReader;
    projectRoot: string;
    contextLimit: number | undefined;
    /** `completedAtMs(index)` returns the session-write timestamp for the message at `index`. */
    fields(
        messages: readonly Json[],
        completedAtMs: (index: number) => number | undefined,
    ): Record<string, unknown>;
}

export interface PiPassResult {
    /** Pi's replacement array; present only when this pass applied the daemon's output. */
    messages?: Json[];
    outcome?: TransformPassOutcome;
    /** Work counters: branch entries the index sync and the alignment read. */
    branchEntriesVisited: number;
    entriesAligned: number;
}

/** The compaction Pi commits for an eviction (spec D3, D13). */
export interface PiEviction {
    summary: string;
    firstKeptEntryId: string;
    tokensBefore: number;
    details: { sequence: number };
}

/** The rendered boundary the daemon acknowledged and the m0 text it rendered with it. */
interface Acknowledged {
    boundary: TransformBoundary;
    summary: string;
}

function textOf(message: Json | undefined): string | undefined {
    const content = message?.content;
    if (typeof content === "string") return content;
    if (!Array.isArray(content)) return undefined;
    return content
        .map((part) => ((part as Json).type === "text" ? String((part as Json).text) : ""))
        .join("");
}

export interface PiTransformOptions {
    moduleClient: RustModeModuleClient;
    branchOf: (sessionId: string) => PiBranchIndex;
    captureAdmission?: TransformCaptureAdmission;
}

/** The latest compaction entry on `branch`, read through `reader`. */
function latestCompaction(
    branch: PiBranchIndex,
    reader: PiBranchReader,
): CompactionEntry | undefined {
    const latest = branch.latestCompaction();
    const entry = latest === undefined ? undefined : reader.getEntry(branch.idAt(latest) ?? "");
    return entry?.type === "compaction" ? entry : undefined;
}

export function createPiTransform(options: PiTransformOptions) {
    const client = createTransformSessionClient({ moduleClient: options.moduleClient });
    const admission = options.captureAdmission ?? defaultTransformCaptureAdmission;
    /** Sessions whose latest admitted pass returned no replacement array. */
    const declined = new Set<string>();
    const acknowledged = new Map<string, Acknowledged>();

    async function run(inputs: PiPassInputs): Promise<PiPassResult> {
        const branch = options.branchOf(inputs.sessionId);
        const branchEntriesVisited = branch.sync(inputs.reader);
        const admitted = admission.admit(inputs.sessionId);
        if ("declined" in admitted) return { branchEntriesVisited, entriesAligned: 0 };
        declined.add(inputs.sessionId);
        const alignment = new PiAlignment(inputs.messages, branch, inputs.reader);
        // The client's capture rechecks compare slot references, so a slot keeps one row.
        const slotRows = new Map<number, PiRow>();
        const rows = (start: number, end: number): PiRow[] | undefined => {
            const window: PiRow[] = [];
            for (let index = end - 1; index >= start; index -= 1) {
                let row = slotRows.get(index);
                if (!row) {
                    const id = alignment.idAt(index);
                    const message = inputs.messages[index] as Json;
                    if (id === undefined || !isPiRole(message.role)) return undefined;
                    row = { id, message };
                    slotRows.set(index, row);
                }
                window.push(row);
            }
            return window.reverse();
        };
        let replacement: Json[] | undefined;
        // Pi hands each handler its own clone of the array, so no other code can change or
        // resize it while the pass awaits, and a publication cannot be refused.
        const outcome = await client.run(inputs.sessionId, admitted.lease, {
            serializerProfile: "pi",
            invocationProfile: "pi-heuristic",
            host: {
                length: inputs.messages.length,
                idAt: (index) => alignment.idAt(index),
                holds: (id) => alignment.holds(id),
            },
            preflight: async () => inputs.projectRoot,
            readWindow: rows,
            measure: () => (index) => {
                const row = rows(index, index + 1)?.[0];
                return row && piRowSize(row);
            },
            // After a store reset the compaction summary heads a cold import as ordinary content.
            coldLead: inputs.messages[0]?.role === "compactionSummary" ? 1 : 0,
            idOf: (value) => (value as PiRow).id,
            liveWindow: rows,
            privateWindow: true,
            contextLimit: () => inputs.contextLimit,
            prepare: async (members) => {
                const window = members as PiRow[];
                const writtenAt = (index: number): number | undefined => {
                    const id = window[index]?.id;
                    const entry = id === undefined ? undefined : inputs.reader.getEntry(id);
                    return entry ? Date.parse(entry.timestamp) : undefined;
                };
                return {
                    encodeInput: () => encodePiRowsToCk(window),
                    fields: inputs.fields(
                        window.map((row) => row.message),
                        writtenAt,
                    ),
                };
            },
            publicationRejection: () => null,
            failOpen: false,
            publish(values) {
                replacement = values.map((value) => (value as PiRow).message);
                return undefined;
            },
        });
        // An applied pass at a rendered boundary leads with m0, the text an eviction carries; an
        // applied pass with no boundary, as after a store reset, leaves nothing to evict.
        if (replacement && outcome.kind === "applied") {
            const summary = textOf(replacement[0]);
            if (outcome.boundary && summary)
                acknowledged.set(inputs.sessionId, { boundary: outcome.boundary, summary });
            else acknowledged.delete(inputs.sessionId);
        }
        // A declined pass returns nothing, so Pi keeps its own array.
        if (replacement) declined.delete(inputs.sessionId);
        return {
            ...(replacement ? { messages: replacement } : {}),
            outcome,
            branchEntriesVisited,
            entriesAligned: alignment.entriesRead,
        };
    }

    /**
     * The compaction that evicts the history before the acknowledged rendered boundary: its end
     * entry is the first kept entry, so the boundary stays in view (C3). `undefined` when no
     * boundary is acknowledged, its end entry is off the branch (as after a navigation), or the
     * branch's latest compaction already evicted through it, whichever path committed that one.
     */
    function eviction(
        sessionId: string,
        reader: PiBranchReader,
        tokensBefore: number,
    ): PiEviction | undefined {
        const ack = acknowledged.get(sessionId);
        if (!ack) return undefined;
        const branch = options.branchOf(sessionId);
        branch.sync(reader);
        if (branch.indexOf(ack.boundary.mid) === undefined) return undefined;
        const committed = latestCompaction(branch, reader);
        // A compaction that already evicted through this boundary, or one that keeps less, leaves nothing to evict.
        if (
            committed &&
            ((committed.firstKeptEntryId === ack.boundary.mid &&
                (committed.details as { sequence?: unknown } | undefined)?.sequence ===
                    ack.boundary.sequence) ||
                (branch.indexOf(committed.firstKeptEntryId) ?? -1) >
                    (branch.indexOf(ack.boundary.mid) ?? -1))
        )
            return undefined;
        return {
            summary: ack.summary,
            firstKeptEntryId: ack.boundary.mid,
            tokensBefore,
            details: { sequence: ack.boundary.sequence },
        };
    }

    return {
        run,
        eviction,
        /** Whether the branch's latest compaction is one an Eidnara eviction committed. */
        ownsCompaction(sessionId: string, reader: PiBranchReader): boolean {
            const branch = options.branchOf(sessionId);
            branch.sync(reader);
            const committed = latestCompaction(branch, reader);
            return (
                committed?.fromHook === true &&
                typeof (committed.details as { sequence?: unknown } | undefined)?.sequence ===
                    "number"
            );
        },
        /**
         * `folds` returns `false` after a declined pass until a pass supplies replacement
         * messages or `clearSession` clears the session.
         */
        folds(sessionId: string): boolean {
            return !declined.has(sessionId);
        },
        clearSession(sessionId: string): void {
            declined.delete(sessionId);
            acknowledged.delete(sessionId);
            admission.requestCancel(sessionId, `pi session ${sessionId} cleared`);
            client.clear(sessionId);
        },
    };
}
