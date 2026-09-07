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

    test.each(ALL_STATE_KEYS)("%s renders non-empty tool text", (key) => {
        expect(renderToolStateText(stateFor(key)).length).toBeGreaterThan(0);
    });

    test("unavailable and stale tool text never tells the caller to retry", () => {
        for (const key of ALL_STATE_KEYS) {
            if (!key.startsWith("unavailable") && key !== "stale") continue;
            expect(renderToolStateText(stateFor(key)).toLowerCase()).not.toContain("retry");
        }
    });
});
