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

/** The system prompt is Pi's whole invocation surface. `append` keeps an existing block; an empty `replace` removes the block and is still `applied_replacement`; a candidate the window refuses is `keep` with the prompt unchanged. */
export function editSystemPrompt(
    systemPrompt: string,
    action: PackedAction,
    preparationId: string,
    body: string,
    usableContextLimit: number | undefined,
): PiEdit {
    const block = `${OPEN_PREFIX}${preparationId}${OPEN_SUFFIX}${body}${CLOSE}`;
    const start = ownedBlockStart(systemPrompt);
    let candidate: Edit<string>;
    if (action === "append") {
        candidate =
            body.length === 0 || start >= 0
                ? { surface: systemPrompt, outcome: "keep" }
                : { surface: `${systemPrompt}${block}`, outcome: "append" };
    } else {
        const stripped = start >= 0 ? systemPrompt.slice(0, start) : systemPrompt;
        candidate = {
            surface: body.length > 0 ? `${stripped}${block}` : stripped,
            outcome: "applied_replacement",
        };
    }
    const validation = validatePiInvocation(candidate.surface, systemPrompt, usableContextLimit);
    return validation.ok
        ? { ...candidate, validation }
        : { surface: systemPrompt, outcome: "keep", validation };
}
