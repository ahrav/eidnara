import { describe, expect, it } from "bun:test";

import { redactConfigIssuePath } from "./issue-path";

describe("redactConfigIssuePath", () => {
    it("prints static fields and array indices, and withholds record keys and unknown segments", () => {
        const cases: Array<[string, Array<string | number>, string[]]> = [
            [
                "static schema fields print verbatim",
                ["memory", "git_commit_indexing", "since_days"],
                ["memory", "git_commit_indexing", "since_days"],
            ],
            [
                "record keys under a hidden-agent tools block are withheld",
                ["historian", "tools", "SECRET-TOOL"],
                ["historian", "tools", "<key>"],
            ],
            [
                "record keys under a hidden-agent permission block are withheld",
                ["historian", "permission", "SECRET-TOOL"],
                ["historian", "permission", "<key>"],
            ],
            [
                "prompt_surface.tool_descriptions keys are withheld",
                ["prompt_surface", "tool_descriptions", "SECRET-KEY"],
                ["prompt_surface", "tool_descriptions", "<key>"],
            ],
            [
                "`default` inside a threshold object is kept",
                ["execute_threshold_percentage", "default"],
                ["execute_threshold_percentage", "default"],
            ],
            [
                "model keys inside a threshold object are withheld",
                ["execute_threshold_percentage", "openai/SECRET-MODEL"],
                ["execute_threshold_percentage", "<key>"],
            ],
            [
                "array indices print as [n]",
                ["historian", "disallowed_tools", 0],
                ["historian", "disallowed_tools", "[0]"],
            ],
            [
                "every segment below an unknown key is withheld",
                ["not_a_field", "child"],
                ["<key>", "<key>"],
            ],
        ];
        for (const [title, path, expected] of cases) {
            expect([title, redactConfigIssuePath(path)]).toEqual([title, expected]);
        }
    });
});
