import { ANTI_MEMORY_CATEGORY } from "../../shared/kernel-client/anti-memory";
import type { ImitatedArgRule, ImitatedArgsSchema } from "../unwrap-imitated-reduced-args";
import {
    EIDNARA_MEMORY_ACTIONS,
    EIDNARA_MEMORY_DISPUTE_EVENTS,
    type EidnaraMemoryAction,
} from "./types";

/** Positive-memory categories the tool accepts; the daemon validates categories on commit. */
export const V2_MEMORY_CATEGORIES = [
    "PROJECT_RULES",
    "ARCHITECTURE",
    "CONSTRAINTS",
    "CONFIG_VALUES",
    "NAMING",
] as const;

/** Every category a write may carry: the positive taxonomy plus the anti-memory category. */
export const WRITABLE_MEMORY_CATEGORIES = [...V2_MEMORY_CATEGORIES, ANTI_MEMORY_CATEGORY] as const;

/** One gloss per positive category; the `Record` key type fails to compile when the taxonomy changes without it. */
const V2_MEMORY_CATEGORY_GLOSS: Record<(typeof V2_MEMORY_CATEGORIES)[number], string> = {
    PROJECT_RULES: "project conventions",
    ARCHITECTURE: "design decisions",
    CONSTRAINTS: "requirements and limits",
    CONFIG_VALUES: "settings and operational values",
    NAMING: "names and terminology",
};

const POSITIVE_CATEGORY_LINE = V2_MEMORY_CATEGORIES.map(
    (category) => `${category} (${V2_MEMORY_CATEGORY_GLOSS[category]})`,
).join(", ");

export const EIDNARA_MEMORY_TOOL_NAME = "eidnara_memory";
export const EIDNARA_MEMORY_DESCRIPTION = `Durable project memories shared across sessions, served by the memory daemon.

Memories are addressed by object id (mem_<32hex>). revise and merge supersede their targets with one new object and return its id; no token is passed. A result starting with "Error:" names the memory state and what to do next.

Actions:
- create: content + category, or antiMemory.
- get: up to 20 object ids; hidden and missing objects read the same.
- revise: objectId + content/category or antiMemory.
- archive: objectId.
- merge: objectIds into one survivor + content/category or antiMemory.

Positive categories (exact names): ${POSITIVE_CATEGORY_LINE}.
${ANTI_MEMORY_CATEGORY} requires antiMemory, not content. Do not invent category names.

Memories created here are available through explicit search as labeled model inference. Automatic context injection follows the kernel's admission policy. Agent calls to approve/enforce are rejected.`;

/** Appended to the description when the allowlist admits `dispute`. */
const EIDNARA_MEMORY_DISPUTE_DESCRIPTION = `
dispute: objectId + event (mark_disputed or contradict) + evidenceId, or path + gitObjectId, or both. It marks a served memory disputed or contradicted and cites evidence: a live evidence id of this project, which the daemon checks is live, or a relative repository path at a 40- or 64-character lowercase hex git object id, whose form the daemon checks but whose existence it does not. It can only lower the memory's standing; a dispute on an approval also demotes the memories that rest on it. The result names the applied disposition and whether served visibility changed.`;

/** Appended to the description when the allowlist admits `candidates`. */
const EIDNARA_MEMORY_CANDIDATES_DESCRIPTION = `
candidates: objectId, with optional after (a prior page's next) and limit (1 to 64). A read listing published sources whose text overlaps the memory lexically, each with the dispute form that would confirm it. An overlap is not a judgment; read the source before filing the form.`;

/** `a, b, or c`: how the schema's `action` field names the admitted actions. */
export function actionList(actions: readonly EidnaraMemoryAction[]): string {
    if (actions.length < 2) return actions.join("");
    return `${actions.slice(0, -1).join(", ")}, or ${actions[actions.length - 1]}`;
}

/** The tool description for the actions a caller exposes. */
export function eidnaraMemoryDescription(actions: readonly EidnaraMemoryAction[]): string {
    let description = EIDNARA_MEMORY_DESCRIPTION;
    if (actions.includes("candidates")) {
        description = `${description}\n${EIDNARA_MEMORY_CANDIDATES_DESCRIPTION}`;
    }
    if (actions.includes("dispute")) {
        description = `${description}\n${EIDNARA_MEMORY_DISPUTE_DESCRIPTION}`;
    }
    return description;
}
export const GET_MAX_CLAIMS = 20;
/** Byte budget for the memories a get response serializes, matching the ~16 KiB scale of eidnara_search's 4096-token render budget; entries past the budget are elided and reported. */
export const EIDNARA_MEMORY_RESPONSE_BUDGET_BYTES = 16 * 1024;
/** Merge emits one `supersede_decision` per target; the cap keeps a schema-valid request from failing only at the daemon's envelope limit. */
export const MERGE_MAX_TARGETS = 20;

export const EIDNARA_MEMORY_ANTI_MEMORY_RULE: ImitatedArgRule = {
    type: "object",
    fields: {
        trigger: "string",
        rejectedStrategy: "string",
        rejectionReason: "string",
    },
    optionalFields: {
        saferAlternative: "string",
        preconditions: "string",
        attemptedApproach: "string",
        observedFailure: "string",
        rootCause: "string",
        recovery: "string",
        nonApplicableWhen: "string",
        expiresAt: "number",
    },
};

/** How each `eidnara_memory` argument is recovered from a reduced-args wrapper; every host registers the tool against this one table. */
export const EIDNARA_MEMORY_UNWRAP_RULES: ImitatedArgsSchema = {
    action: { type: "enum", values: EIDNARA_MEMORY_ACTIONS },
    content: "string",
    category: { type: "enum", values: WRITABLE_MEMORY_CATEGORIES },
    antiMemory: EIDNARA_MEMORY_ANTI_MEMORY_RULE,
    objectId: "string",
    objectIds: { type: "array", items: "string", maxItems: 20 },
    reason: "string",
    event: { type: "enum", values: EIDNARA_MEMORY_DISPUTE_EVENTS },
    evidenceId: "string",
    path: "string",
    gitObjectId: "string",
    after: "string",
    limit: "number",
};
