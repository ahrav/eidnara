import { describe, expect, it } from "bun:test";
import {
    buildSyntheticTodoPart,
    computeSyntheticCallId,
    isSyntheticTodoPart,
    normalizeTodoStateJson,
} from "./todo-view";

describe("normalizeTodoStateJson", () => {
    it("returns null for non-array input and for any malformed todo item", () => {
        for (const input of [
            null,
            undefined,
            "not an array",
            { todos: [] },
            [{ content: "Interop", status: "done" }],
            [{ content: "Urgent", status: "pending", priority: "urgent" }],
            [{ content: "Valid", status: "pending", priority: "high" }, { content: "No status" }],
        ]) {
            expect(normalizeTodoStateJson(input)).toBeNull();
        }
    });

    it("keeps order, strips extra fields, and defaults a missing priority to medium", () => {
        const cases: Array<[unknown, unknown[]]> = [
            [[], []],
            [
                [
                    { content: "First", status: "in_progress", priority: "high" },
                    { content: "Second", status: "pending", priority: "medium" },
                ],
                [
                    { content: "First", status: "in_progress", priority: "high" },
                    { content: "Second", status: "pending", priority: "medium" },
                ],
            ],
            [
                [{ id: "1", content: "Task", status: "pending", priority: "high" }],
                [{ content: "Task", status: "pending", priority: "high" }],
            ],
            [
                [{ content: "Task", status: "pending" }],
                [{ content: "Task", status: "pending", priority: "medium" }],
            ],
        ];
        for (const [input, expected] of cases) {
            expect(JSON.parse(normalizeTodoStateJson(input) ?? "null")).toEqual(expected);
        }
    });
});

describe("buildSyntheticTodoPart", () => {
    const validState = JSON.stringify([
        { content: "Active task", status: "in_progress", priority: "high" },
        { content: "Done task", status: "completed", priority: "medium" },
    ]);

    it("returns null for empty, invalid, or all-terminal state", () => {
        for (const state of [
            "",
            "not json",
            "[]",
            JSON.stringify([
                { content: "A", status: "completed", priority: "high" },
                { content: "B", status: "cancelled", priority: "low" },
            ]),
        ]) {
            expect(buildSyntheticTodoPart(state)).toBeNull();
        }
    });

    it("produces a valid OpenCode tool part shape", () => {
        const part = buildSyntheticTodoPart(validState);
        expect(part).not.toBeNull();
        if (!part) throw new Error("part null");
        expect(part.type).toBe("tool");
        expect(part.tool).toBe("todowrite");
        expect(part.callID).toMatch(/^synthetic_todo_[0-9a-f]{16}$/);
        expect(part.state.status).toBe("completed");
        expect(part.state.input.todos).toHaveLength(2);
        expect(part.state.metadata.todos).toHaveLength(2);
        expect(part.state.metadata.truncated).toBe(false);
        expect(part.syntheticTodoMarker).toBe(true);
    });

    it("output field is JSON-stringified todos with 2-space indent (matches OpenCode todo.ts)", () => {
        const part = buildSyntheticTodoPart(validState);
        if (!part) throw new Error("part null");
        const todos = JSON.parse(validState);
        expect(part.state.output).toBe(JSON.stringify(todos, null, 2));
    });

    it("title reflects active count only", () => {
        const part = buildSyntheticTodoPart(validState);
        if (!part) throw new Error("part null");
        // The active-count calculation includes the in_progress item and excludes the completed item.
        expect(part.state.title).toBe("1 todos");
    });

    it("time start equals end (synthetic signal)", () => {
        const part = buildSyntheticTodoPart(validState);
        if (!part) throw new Error("part null");
        expect(part.state.time.start).toBe(part.state.time.end);
    });

    it("derives callID from the state: same state repeats it, different state changes it", () => {
        const otherState = JSON.stringify([
            { content: "Different", status: "pending", priority: "low" },
        ]);
        const a = buildSyntheticTodoPart(validState);
        const b = buildSyntheticTodoPart(validState);
        const c = buildSyntheticTodoPart(otherState);
        expect(a?.callID).toBe(b?.callID);
        expect(a?.callID).not.toBe(c?.callID);
    });
});

describe("computeSyntheticCallId", () => {
    it("returns a 16-hex-char id with the synthetic prefix", () => {
        const id = computeSyntheticCallId("[]");
        expect(id).toMatch(/^synthetic_todo_[0-9a-f]{16}$/);
    });

    it("is a pure function of the input: same input repeats, different input differs", () => {
        expect(computeSyntheticCallId("foo")).toBe(computeSyntheticCallId("foo"));
        expect(computeSyntheticCallId("foo")).not.toBe(computeSyntheticCallId("bar"));
    });
});

describe("isSyntheticTodoPart", () => {
    it("detects parts with the syntheticTodoMarker flag", () => {
        const validState = JSON.stringify([
            { content: "X", status: "in_progress", priority: "high" },
        ]);
        const part = buildSyntheticTodoPart(validState);
        expect(isSyntheticTodoPart(part)).toBe(true);
    });

    it("detects synthetic parts by callID prefix even without the marker", () => {
        const part = {
            type: "tool",
            tool: "todowrite",
            callID: "synthetic_todo_0123456789abcdef",
        };
        expect(isSyntheticTodoPart(part)).toBe(true);
    });

    it("rejects real tool parts", () => {
        const realPart = {
            type: "tool",
            tool: "todowrite",
            callID: "toolu_01N63ZiXgCock1HUZHeRFtLP",
            state: { status: "completed" },
        };
        expect(isSyntheticTodoPart(realPart)).toBe(false);
    });

    it("rejects non-objects and unrelated shapes", () => {
        expect(isSyntheticTodoPart(null)).toBe(false);
        expect(isSyntheticTodoPart(undefined)).toBe(false);
        expect(isSyntheticTodoPart("string")).toBe(false);
        expect(isSyntheticTodoPart({ type: "text", text: "hi" })).toBe(false);
    });
});
