import { describe, expect, it } from "bun:test";
import { pruneInvalidAgentFields } from "./agent-config";

describe("pruneInvalidAgentFields", () => {
    it("leaves a valid block untouched and reports nothing", () => {
        const block: Record<string, unknown> = { model: "x", temperature: 0.2, disable: true };
        expect(pruneInvalidAgentFields("historian", block)).toEqual([]);
        expect(block).toEqual({ model: "x", temperature: 0.2, disable: true });
    });

    it("removes only the fields the schema rejects, in place", () => {
        const block: Record<string, unknown> = {
            model: "x",
            temperature: "hot",
            color: "red",
            top_p: 0.5,
        };
        expect(pruneInvalidAgentFields("sidekick", block)).toEqual(["color", "temperature"]);
        expect(block).toEqual({ model: "x", top_p: 0.5 });
    });

    it("keeps unknown fields, which the schema strips rather than rejects", () => {
        const block: Record<string, unknown> = { model: "x", someday: true };
        expect(pruneInvalidAgentFields("historian", block)).toEqual([]);
        expect(block.someday).toBe(true);
    });
});
