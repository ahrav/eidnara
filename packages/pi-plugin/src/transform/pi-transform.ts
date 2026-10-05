import type { SessionEntry } from "@earendil-works/pi-coding-agent";
import {
    defaultTransformCaptureAdmission,
    type TransformCaptureAdmission,
} from "@eidnara/opencode/hooks/context/transform-capture";
import {
    createTransformSessionClient,
    type RustModeModuleClient,
    type TransformPassOutcome,
} from "@eidnara/opencode/hooks/context/transform-session-client";

import type { PiBranchIndex, PiBranchReader } from "./pi-branch";
import { encodePiRowsToCk, isPiRole, PI_RESERVED_ID_PREFIX, type PiRow } from "./pi-ck";

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

export interface PiTransformOptions {
    moduleClient: RustModeModuleClient;
    branchOf: (sessionId: string) => PiBranchIndex;
    captureAdmission?: TransformCaptureAdmission;
}

export function createPiTransform(options: PiTransformOptions) {
    const client = createTransformSessionClient({ moduleClient: options.moduleClient });
    const admission = options.captureAdmission ?? defaultTransformCaptureAdmission;
    /** Sessions whose latest admitted pass returned no replacement array. */
    const declined = new Set<string>();

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
        // A declined pass returns nothing, so Pi keeps its own array.
        if (replacement) declined.delete(inputs.sessionId);
        return {
            ...(replacement ? { messages: replacement } : {}),
            outcome,
            branchEntriesVisited,
            entriesAligned: alignment.entriesRead,
        };
    }

    return {
        run,
        /**
         * `folds` returns `false` after a declined pass until a pass supplies replacement
         * messages or `clearSession` clears the session.
         */
        folds(sessionId: string): boolean {
            return !declined.has(sessionId);
        },
        clearSession(sessionId: string): void {
            declined.delete(sessionId);
            admission.requestCancel(sessionId, `pi session ${sessionId} cleared`);
            client.clear(sessionId);
        },
    };
}
