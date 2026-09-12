import { describe, expect, test } from "bun:test";
import { renderToolStateText } from "./render";
import { ALL_STATE_KEYS, type MemoryState, type StateKey } from "./state";

function stateFor(key: StateKey): MemoryState {
    const [kind, reason] = key.split(":");
    if (kind === "stale" || kind === "abstained") {
        return { kind, lag_positions: 1, oldest_unconsumed_age_ms: 1 };
    }
    return (reason === undefined ? { kind } : { kind, reason }) as MemoryState;
}

describe("render", () => {
    test("tool text is one sentence keyed by state", () => {
        expect(renderToolStateText({ kind: "unavailable", reason: "daemon_absent" })).toContain(
            "daemon is not running",
        );
        expect(renderToolStateText({ kind: "disabled" })).toContain("memory.enabled");
    });

    test("every state key renders non-empty tool text", () => {
        const empty = ALL_STATE_KEYS.filter(
            (key) => renderToolStateText(stateFor(key)).length === 0,
        );
        expect(empty).toEqual([]);
    });
});
