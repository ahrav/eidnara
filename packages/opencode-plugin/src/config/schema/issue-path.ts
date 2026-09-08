import { EidnaraConfigSchema } from "./eidnara";

interface ZodDefView {
    type: string;
    innerType?: unknown;
    in?: unknown;
    options?: unknown[];
    shape?: Record<string, unknown>;
    catchall?: unknown;
    valueType?: unknown;
    element?: unknown;
}

function defOf(schema: unknown): ZodDefView | undefined {
    return (schema as { _zod?: { def?: ZodDefView } } | undefined)?._zod?.def;
}

/** A segment is static when an object shape declares it; a record key, catchall key, or array index is dynamic. */
function stepInto(schema: unknown, segment: PropertyKey): { isStatic: boolean; next: unknown[] } {
    const def = defOf(schema);
    if (!def) return { isStatic: false, next: [] };
    switch (def.type) {
        case "optional":
        case "default":
        case "nullable":
            return stepInto(def.innerType, segment);
        case "pipe":
            return stepInto(def.in, segment);
        case "union": {
            let isStatic = false;
            const next: unknown[] = [];
            for (const option of def.options ?? []) {
                const step = stepInto(option, segment);
                isStatic ||= step.isStatic;
                next.push(...step.next);
            }
            return { isStatic, next };
        }
        case "object": {
            const key = String(segment);
            if (def.shape && Object.hasOwn(def.shape, key)) {
                return { isStatic: true, next: [def.shape[key]] };
            }
            return { isStatic: false, next: def.catchall ? [def.catchall] : [] };
        }
        case "record":
            return { isStatic: false, next: def.valueType ? [def.valueType] : [] };
        case "array":
            return { isStatic: false, next: def.element ? [def.element] : [] };
        default:
            return { isStatic: false, next: [] };
    }
}

/**
 * Record and catchall keys are user-authored, and `{env:}` or `{file:}` substitution runs on the
 * raw text, so such a key can carry a resolved secret and is withheld from the rendered path.
 */
export function redactConfigIssuePath(path: readonly PropertyKey[]): string[] {
    let candidates: unknown[] = [EidnaraConfigSchema];
    const parts: string[] = [];
    for (const segment of path) {
        let isStatic = false;
        const next: unknown[] = [];
        for (const candidate of candidates) {
            const step = stepInto(candidate, segment);
            isStatic ||= step.isStatic;
            next.push(...step.next);
        }
        if (isStatic) parts.push(String(segment));
        else parts.push(typeof segment === "number" ? `[${segment}]` : "<key>");
        candidates = next;
    }
    return parts;
}
