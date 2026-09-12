import { describe, expect, it } from "bun:test";
import type { z } from "zod";

import { AgentOverrideConfigSchema } from "./agent-overrides";

type Permission = NonNullable<z.input<typeof AgentOverrideConfigSchema>["permission"]>;

describe("AgentOverrideConfigSchema permission", () => {
    it("keeps unnamed keys and accepts pattern maps on unnamed and rule-capable named keys", () => {
        const accepted: Array<[string, Permission]> = [
            [
                "unnamed action keys",
                {
                    edit: "allow",
                    read: "deny",
                    task: "deny",
                    websearch: "ask",
                    mcp_github_create_issue: "deny",
                    "*": "ask",
                },
            ],
            ["pattern map on an unnamed key", { read: { "*.env": "deny", "*": "allow" } }],
            [
                "pattern maps on the named rule-capable keys",
                {
                    edit: { "*": "deny", "src/**": "allow" },
                    bash: { "git *": "allow", "*": "ask" },
                    external_directory: { "/tmp/**": "allow" },
                },
            ],
        ];
        for (const [title, permission] of accepted) {
            const result = AgentOverrideConfigSchema.safeParse({ permission });
            expect([title, result.success]).toEqual([title, true]);
            if (!result.success) {
                continue;
            }
            expect([title, result.data.permission]).toEqual([title, permission]);
        }
    });

    it("rejects invalid actions on any key and pattern maps on action-only named keys", () => {
        const rejected: Array<[string, Record<string, unknown>]> = [
            ["pattern map on action-only webfetch", { webfetch: { "*": "allow" } }],
            ["pattern map on action-only doom_loop", { doom_loop: { "*": "allow" } }],
            ["invalid action on an unnamed key", { read: "maybe" }],
            ["invalid action inside an unnamed key's pattern map", { task: { "*": "sometimes" } }],
            ["invalid action on a named key", { edit: "maybe" }],
        ];
        for (const [title, permission] of rejected) {
            expect([title, AgentOverrideConfigSchema.safeParse({ permission }).success]).toEqual([
                title,
                false,
            ]);
        }
    });
});
