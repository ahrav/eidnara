/**
 * Shape rules shared by every `ctx_memory` write path: a create or revise
 * either carries positive content under a taxonomy category or an anti-memory
 * payload under the anti-memory category, never both.
 */

import {
    ANTI_MEMORY_CATEGORY,
    ClaimOperationInputError,
} from "../../shared/kernel-client/anti-memory";
import { WRITABLE_MEMORY_CATEGORIES } from "./constants";
import type { CtxMemoryAction } from "./types";

export interface CtxMemoryWriteShape {
    action?: CtxMemoryAction;
    content?: string;
    category?: string;
    antiMemory?: unknown;
}

export function requireTaxonomyCategory(category: string | undefined): string | undefined {
    if (category === undefined || category === "") return undefined;
    if (!(WRITABLE_MEMORY_CATEGORIES as readonly string[]).includes(category)) {
        throw new ClaimOperationInputError(
            `unknown claim category: ${category} (expected one of ${WRITABLE_MEMORY_CATEGORIES.join(", ")})`,
        );
    }
    return category;
}

/** The wrappers fall back to unvalidated raw arguments when schema parsing fails, so each optional string field is type-checked before any `.trim()` call can throw a TypeError outside the input-error path. `null` stays admitted: every downstream read treats it as absent. commentlint: allow(JUDGE) */
export function assertCtxMemoryFieldTypes(args: CtxMemoryWriteShape): void {
    for (const field of ["content", "category", "reason", "objectId"] as const) {
        const value = (args as Record<string, unknown>)[field];
        if (value !== undefined && value !== null && typeof value !== "string") {
            throw new ClaimOperationInputError(`'${field}' must be a string`);
        }
    }
}

/** `null` in `content` or `antiMemory` counts as absent on both arms, the same admission the field-type check grants, so a raw-argument anti-memory create carrying `content: null` is a valid anti-memory write rather than a mixed one. commentlint: allow(JUDGE) */
export function assertCtxMemoryWriteShape(args: CtxMemoryWriteShape): void {
    assertCtxMemoryFieldTypes(args);
    if (args.action !== "create" && args.action !== "revise") return;
    const category = requireTaxonomyCategory(args.category?.trim());
    const antiArm = category === ANTI_MEMORY_CATEGORY || args.antiMemory != null;
    if (antiArm) {
        if (category !== ANTI_MEMORY_CATEGORY || !args.antiMemory || args.content != null) {
            throw new ClaimOperationInputError(
                `${args.action} anti-memory requires category ${ANTI_MEMORY_CATEGORY}, antiMemory payload, and no content`,
            );
        }
        return;
    }
    if (args.antiMemory != null) {
        throw new ClaimOperationInputError(
            `${args.action} positive memory cannot carry antiMemory`,
        );
    }
    if (args.action === "create" && (!category || !args.content?.trim())) {
        throw new ClaimOperationInputError("create requires non-empty content and category");
    }
}
