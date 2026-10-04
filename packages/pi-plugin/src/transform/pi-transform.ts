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

import { PiBranchIndex, type PiBranchReader } from "./pi-branch";
import { encodePiRowsToCk, isPiRole, PI_RESERVED_ID_PREFIX, type PiRow } from "./pi-ck";

type Json = Record<string, unknown>;

export function reservedPiId(role: string, entryId: string): string {
    return `${PI_RESERVED_ID_PREFIX}${role}:${entryId}`;
}

interface Produced {
    role: string;
    timestamp: unknown;
    id: string;
}

function produced(entry: SessionEntry): Produced | undefined {
    const record = entry as unknown as Json;
    switch (entry.type) {
        case "message": {
            const message = record.message as Json | undefined;
            return message
                ? { role: String(message.role), timestamp: message.timestamp, id: entry.id }
                : undefined;
        }
        case "custom_message":
            return {
                role: "custom",
                timestamp: Date.parse(String(record.timestamp)),
                id: reservedPiId("custom", entry.id),
            };
        case "branch_summary":
            return record.summary
                ? {
                      role: "branchSummary",
                      timestamp: Date.parse(String(record.timestamp)),
                      id: reservedPiId("branchSummary", entry.id),
                  }
                : undefined;
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
            const id = this.branch.idAt(this.cursor) as string;
            this.cursor -= 1;
            this.entriesRead += 1;
            const entry = this.reader.getEntry(id);
            if (!entry) break;
            const candidate = produced(entry);
            if (!candidate) continue;
            if (candidate.role === message.role && candidate.timestamp === message.timestamp) {
                this.fromEnd.push(candidate.id);
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
    fields(messages: readonly Json[]): Record<string, unknown>;
}

export interface PiPassResult {
    messages?: Json[];
    outcome?: TransformPassOutcome;
    branchEntriesVisited: number;
    entriesAligned: number;
}

export interface PiTransformOptions {
    moduleClient: RustModeModuleClient;
    branchOf?: (sessionId: string) => PiBranchIndex;
    captureAdmission?: TransformCaptureAdmission;
    retainedOutputBudgetBytes?: number;
    unpagedTransformMaxBytes?: number;
}

export function createPiTransform(options: PiTransformOptions) {
    const client = createTransformSessionClient({
        moduleClient: options.moduleClient,
        retainedOutputBudgetBytes: options.retainedOutputBudgetBytes,
        unpagedTransformMaxBytes: options.unpagedTransformMaxBytes,
    });
    const admission = options.captureAdmission ?? defaultTransformCaptureAdmission;
    const branches = new Map<string, PiBranchIndex>();
    const branchOf =
        options.branchOf ??
        ((sessionId: string): PiBranchIndex => {
            let branch = branches.get(sessionId);
            if (!branch) {
                branch = new PiBranchIndex();
                branches.set(sessionId, branch);
            }
            return branch;
        });

    async function run(inputs: PiPassInputs): Promise<PiPassResult> {
        const branch = branchOf(inputs.sessionId);
        const sync = branch.sync(inputs.reader);
        const admitted = admission.admit(inputs.sessionId);
        if ("declined" in admitted)
            return { branchEntriesVisited: sync.visited, entriesAligned: 0 };
        const alignment = new PiAlignment(inputs.messages, branch, inputs.reader);
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
        const outcome = await client.run(inputs.sessionId, admitted.lease, {
            serializerProfile: "pi",
            invocationProfile: "pi-heuristic",
            host: { length: inputs.messages.length, idAt: (index) => alignment.idAt(index) },
            preflight: async () => inputs.projectRoot,
            readWindow: rows,
            idOf: (value) => (value as PiRow).id,
            liveWindow: rows,
            contextLimit: () => inputs.contextLimit,
            prepare: async (members) => {
                const window = members as PiRow[];
                return {
                    encodeInput: () => encodePiRowsToCk(window),
                    fields: inputs.fields(window.map((row) => row.message)),
                };
            },
            publicationRejection: () => null,
            publish(values) {
                replacement = values.map((value) => (value as PiRow).message);
                return undefined;
            },
        });
        return {
            ...(replacement ? { messages: replacement } : {}),
            outcome,
            branchEntriesVisited: sync.visited,
            entriesAligned: alignment.entriesRead,
        };
    }

    return {
        run,
        branch: branchOf,
        state: (sessionId: string) => client.state(sessionId),
        clearSession(sessionId: string): string | null {
            branches.delete(sessionId);
            admission.requestCancel(sessionId, `pi session ${sessionId} cleared`);
            return client.clear(sessionId);
        },
    };
}

export type PiTransform = ReturnType<typeof createPiTransform>;
