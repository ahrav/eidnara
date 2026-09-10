import {
    HistorianConfigSchema,
    SidekickConfigSchema,
} from "@eidnara/opencode/config/schema/eidnara";

export type AgentBlockKind = "historian" | "sidekick";

/**
 * Config loading drops an entire `historian` or `sidekick` block when any field in it fails validation ("invalid agent configuration, ignoring"), so a stale field would silently discard the model setup just wrote.
 * Fields are deleted in place because a comment-json block carries its
 * comments as symbol-keyed metadata that a copy would lose.
 */
export function pruneInvalidAgentFields(
    kind: AgentBlockKind,
    block: Record<string, unknown>,
): string[] {
    const schema = kind === "historian" ? HistorianConfigSchema : SidekickConfigSchema;
    const result = schema.safeParse(block);
    if (result.success) return [];
    const invalid = new Set<string>();
    for (const issue of result.error.issues) {
        const field = issue.path[0];
        if (typeof field === "string") invalid.add(field);
    }
    for (const field of invalid) delete block[field];
    return [...invalid].sort();
}
