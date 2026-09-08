import { describe, expect, it } from "bun:test";

import { redactConfigIssuePath } from "./issue-path";

describe("redactConfigIssuePath", () => {
    it("prints static schema fields verbatim", () => {
        expect(redactConfigIssuePath(["memory", "git_commit_indexing", "since_days"])).toEqual([
            "memory",
            "git_commit_indexing",
            "since_days",
        ]);
    });

    it("withholds record keys under a hidden-agent block", () => {
        expect(redactConfigIssuePath(["historian", "tools", "SECRET-TOOL"])).toEqual([
            "historian",
            "tools",
            "<key>",
        ]);
        expect(redactConfigIssuePath(["historian", "permission", "SECRET-TOOL"])).toEqual([
            "historian",
            "permission",
            "<key>",
        ]);
    });

    it("withholds prompt_surface.tool_descriptions keys", () => {
        expect(
            redactConfigIssuePath(["prompt_surface", "tool_descriptions", "SECRET-KEY"]),
        ).toEqual(["prompt_surface", "tool_descriptions", "<key>"]);
    });

    it("keeps `default` and withholds model keys inside a threshold object", () => {
        expect(redactConfigIssuePath(["execute_threshold_percentage", "default"])).toEqual([
            "execute_threshold_percentage",
            "default",
        ]);
        expect(
            redactConfigIssuePath(["execute_threshold_percentage", "openai/SECRET-MODEL"]),
        ).toEqual(["execute_threshold_percentage", "<key>"]);
    });

    it("prints array indices as [n]", () => {
        expect(redactConfigIssuePath(["historian", "disallowed_tools", 0])).toEqual([
            "historian",
            "disallowed_tools",
            "[0]",
        ]);
    });

    it("withholds every segment below an unknown key", () => {
        expect(redactConfigIssuePath(["not_a_field", "child"])).toEqual(["<key>", "<key>"]);
    });
});
