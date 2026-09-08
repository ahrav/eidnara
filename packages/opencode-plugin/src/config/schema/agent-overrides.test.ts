import { describe, expect, it } from "bun:test";

import { AgentOverrideConfigSchema } from "./agent-overrides";

describe("AgentOverrideConfigSchema permission", () => {
    it("keeps permission keys that are not named in the schema", () => {
        const result = AgentOverrideConfigSchema.safeParse({
            permission: {
                edit: "allow",
                read: "deny",
                task: "deny",
                websearch: "ask",
                mcp_github_create_issue: "deny",
                "*": "ask",
            },
        });

        expect(result.success).toBe(true);
        if (!result.success) {
            return;
        }
        expect(result.data.permission).toEqual({
            edit: "allow",
            read: "deny",
            task: "deny",
            websearch: "ask",
            mcp_github_create_issue: "deny",
            "*": "ask",
        });
    });

    it("accepts a pattern map for an unnamed permission key", () => {
        const result = AgentOverrideConfigSchema.safeParse({
            permission: { read: { "*.env": "deny", "*": "allow" } },
        });

        expect(result.success).toBe(true);
        if (!result.success) {
            return;
        }
        expect(result.data.permission).toEqual({ read: { "*.env": "deny", "*": "allow" } });
    });

    it("rejects an invalid action on an unnamed permission key", () => {
        const result = AgentOverrideConfigSchema.safeParse({
            permission: { read: "maybe" },
        });

        expect(result.success).toBe(false);
    });

    it("rejects an invalid action inside an unnamed key's pattern map", () => {
        const result = AgentOverrideConfigSchema.safeParse({
            permission: { task: { "*": "sometimes" } },
        });

        expect(result.success).toBe(false);
    });

    it("still rejects an invalid action on a named key", () => {
        const result = AgentOverrideConfigSchema.safeParse({
            permission: { edit: "maybe" },
        });

        expect(result.success).toBe(false);
    });
});
