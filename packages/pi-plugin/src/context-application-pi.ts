import type { Edit, PackedAction } from "@eidnara/opencode/hooks/context/context-application";
import {
    type InvocationValidation,
    validateInvocation,
} from "@eidnara/opencode/hooks/context/invocation-budget";

const OPEN_PREFIX = '\n<eidnara-packed preparation="';
const OPEN_SUFFIX = '">\n';
const CLOSE = "\n</eidnara-packed>";

/** Charged on top of the heuristic estimate because the estimator undercounts relative to the provider's tokenizer. */
export const PI_INVOCATION_HEADROOM_PERMILLE = 250;

/** The owned block is the prompt's tail; a block-shaped run anywhere else is prompt text, not ownership. */
function ownedBlockStart(systemPrompt: string): number {
    if (!systemPrompt.endsWith(CLOSE)) return -1;
    return systemPrompt.lastIndexOf(OPEN_PREFIX);
}

/**
 * A block that contains the open delimiter past its own opening cannot be found again by
 * `ownedBlockStart`: `replace` would strip only its tail and `append` would write a second block
 * beside the residue. The tail anchor already ignores a close delimiter inside the body.
 */
function locatable(block: string): boolean {
    return block.indexOf(OPEN_PREFIX, 1) === -1;
}

export function hasPackedBlock(systemPrompt: string): boolean {
    return ownedBlockStart(systemPrompt) >= 0;
}

/** The whole system prompt is charged, not the packed block alone; the limit is Pi's usable window when the host reports one. */
export function validatePiInvocation(
    candidate: string,
    incoming: string,
    usableContextLimit: number | undefined,
): InvocationValidation {
    return validateInvocation([candidate.length], [incoming.length], {
        maxTokens: usableContextLimit,
        headroomPermille: PI_INVOCATION_HEADROOM_PERMILLE,
        profile: "pi-heuristic",
    });
}

export type PiEdit = Edit<string> & { validation: InvocationValidation };

/** An empty `replace` is still `applied_replacement`; a window refusal and an unlocatable block both confirm as `keep` with the prompt unchanged. */
export function editSystemPrompt(
    systemPrompt: string,
    action: PackedAction,
    preparationId: string,
    body: string,
    usableContextLimit: number | undefined,
): PiEdit {
    const block = `${OPEN_PREFIX}${preparationId}${OPEN_SUFFIX}${body}${CLOSE}`;
    const start = ownedBlockStart(systemPrompt);
    const unchanged: Edit<string> = { surface: systemPrompt, outcome: "keep" };
    let candidate: Edit<string>;
    if (body.length > 0 && !locatable(block)) {
        candidate = unchanged;
    } else if (action === "append") {
        candidate =
            body.length === 0 || start >= 0
                ? unchanged
                : { surface: `${systemPrompt}${block}`, outcome: "append" };
    } else {
        const stripped = start >= 0 ? systemPrompt.slice(0, start) : systemPrompt;
        candidate = {
            surface: body.length > 0 ? `${stripped}${block}` : stripped,
            outcome: "applied_replacement",
        };
    }
    const validation = validatePiInvocation(candidate.surface, systemPrompt, usableContextLimit);
    return validation.ok ? { ...candidate, validation } : { ...unchanged, validation };
}
