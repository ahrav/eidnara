import { describe, expect, test } from "bun:test";
import { readFileSync } from "node:fs";
import path from "node:path";
import {
    ABSTAIN_REASONS,
    ACTIVATION_STATES,
    LIST_TERMINALS,
    MEMORY_REVIEWER_STATES,
    OUTCOMES,
    READ_TERMINALS,
    STATUS_COUNTERS,
} from "./review-wire";

const WIRE_DOC = path.join(
    import.meta.dir,
    "..",
    "..",
    "..",
    "..",
    "docs",
    "host-wire-protocol.md",
);
const wire = readFileSync(WIRE_DOC, "utf8");
const lines = wire.split("\n");

function paragraphStarting(prefix: string): string {
    const line = lines.find((entry) => entry.startsWith(prefix));
    if (line === undefined) throw new Error(`no paragraph starts with ${prefix}`);
    return line;
}

/** Every `` `word` `` in `text`, in order. */
function backticked(text: string): string[] {
    return [...text.matchAll(/`([a-z_]+)`/g)].map((match) => match[1]);
}

/** Every `"word"` in a table type cell such as `` `"ready" \| "starting"` ``. */
function quotedAlternatives(text: string): string[] {
    return [...text.matchAll(/"([a-z_]+)"/g)].map((match) => match[1]);
}

/** `from` through the end of its sentence, where a sentence ends at the first `. ` after `from`. */
function sentence(text: string, from: string): string {
    const start = text.indexOf(from);
    if (start < 0) throw new Error(`no sentence contains ${from}`);
    const end = text.indexOf(". ", start);
    return text.slice(start, end < 0 ? text.length : end);
}

/** The `metrics.memory_reviewer` table rows: first-cell names, in the table's order. */
function memoryReviewerTable(): { names: string[]; types: Map<string, string> } {
    const first = lines.findIndex((line) => line.startsWith("| `memory_reviewer_state` |"));
    if (first < 0) throw new Error("no memory_reviewer table");
    const names: string[] = [];
    const types = new Map<string, string>();
    for (let index = first; index < lines.length && lines[index].startsWith("|"); index++) {
        const cells = lines[index].split(" | ");
        const rowNames = backticked(cells[0]);
        names.push(...rowNames);
        for (const name of rowNames) types.set(name, cells[1] ?? "");
    }
    return { names, types };
}

describe("review vocabularies are pinned to docs/host-wire-protocol.md", () => {
    test("review.list outcomes and abstention reasons match the item vocabulary exactly", () => {
        const paragraph = paragraphStarting("`review.list` carries");
        const outcomesText = sentence(paragraph, '"outcome": ');
        const outcomes = quotedAlternatives(
            outcomesText.slice(0, outcomesText.indexOf('"selected"')),
        );
        expect(outcomes.slice(1)).toEqual([...OUTCOMES]);
        const reasons = backticked(sentence(paragraph, "one of `"));
        expect(reasons).toEqual([...ABSTAIN_REASONS]);
    });

    test("review.read terminals are exactly the documented set plus the shared disabled terminal", () => {
        const read = backticked(
            sentence(paragraphStarting("`review.read` carries"), "with `t` one of"),
        );
        const shared = paragraphStarting("**Review operations (protocol 3).**");
        const listTerminals = [...shared.matchAll(/"terminal":"([a-z_]+)"/g)].map((m) => m[1]);
        expect(listTerminals).toEqual([...LIST_TERMINALS]);
        expect(new Set(READ_TERMINALS)).toEqual(
            new Set([...read.filter((t) => t !== "t"), ...listTerminals]),
        );
    });

    test("status states and counters match the metrics.memory_reviewer table in its order", () => {
        const { names, types } = memoryReviewerTable();
        expect(names.slice(0, 3)).toEqual([
            "memory_reviewer_state",
            "activation_state",
            "sampled_at_ms",
        ]);
        expect(names.slice(3)).toEqual([...STATUS_COUNTERS]);
        expect(quotedAlternatives(types.get("memory_reviewer_state") ?? "")).toEqual([
            ...MEMORY_REVIEWER_STATES,
        ]);
        expect(quotedAlternatives(types.get("activation_state") ?? "")).toEqual([
            ...ACTIVATION_STATES,
        ]);
    });
});
