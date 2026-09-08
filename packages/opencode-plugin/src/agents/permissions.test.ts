import { describe, expect, it } from "bun:test";
import {
    buildAllowOnlyPermission,
    denyTaskRoutingToAgents,
    denyTaskRoutingToCallerAgents,
    SIDEKICK_ALLOWED_TOOLS,
    SMART_NOTE_COMPILER_ALLOWED_TOOLS,
} from "./permissions";

describe("buildAllowOnlyPermission", () => {
    it("starts with wildcard deny so nothing is allowed by default", () => {
        const perm = buildAllowOnlyPermission([]);
        expect(perm["*"]).toBe("deny");
    });

    it("layers the allow-list on top of the wildcard deny", () => {
        const perm = buildAllowOnlyPermission(["read", "ctx_search"]);
        expect(perm["*"]).toBe("deny");
        expect(perm.read).toBe("allow");
        expect(perm.ctx_search).toBe("allow");
    });

    it("places named allows AFTER the wildcard deny so findLast-semantics make them win", () => {
        // Permission.evaluate uses insertion-order rules with `findLast`; a wildcard after a named tool denies that tool.
        const perm = buildAllowOnlyPermission(["read"]);
        const keys = Object.keys(perm);
        const wildcardIdx = keys.indexOf("*");
        const readIdx = keys.indexOf("read");
        expect(wildcardIdx).toBeLessThan(readIdx);
    });

    it("never accidentally allows `task`, `bash`, or `edit` unless explicitly listed", () => {
        const perm = buildAllowOnlyPermission(["read"]);
        expect(perm.task).toBeUndefined();
        expect(perm.bash).toBeUndefined();
        expect(perm.edit).toBeUndefined();
        expect(perm.webfetch).toBeUndefined();
        expect(perm.websearch).toBeUndefined();
        // The wildcard deny covers omitted tools through `findLast`.
    });

    it("returns an empty allow-list as just the wildcard deny", () => {
        const perm = buildAllowOnlyPermission([]);
        expect(Object.keys(perm)).toEqual(["*"]);
    });

    it("returns deny-all when the allow-list is undefined", () => {
        const perm = buildAllowOnlyPermission(undefined, "test-agent");
        expect(perm).toEqual({ "*": "deny" });
    });
});

describe("denyTaskRoutingToAgents", () => {
    it("appends exact agent-ID denies after a whole-permission action", () => {
        expect(denyTaskRoutingToAgents("allow", ["sidekick"])).toEqual({
            "*": "allow",
            task: { sidekick: "deny" },
        });
    });

    it("preserves user task patterns and appends internal denies last", () => {
        const result = denyTaskRoutingToAgents(
            { edit: "ask", task: { "*": "allow", explore: "allow" } },
            ["sidekick", "smart-note-compiler"],
        );
        expect(result).toEqual({
            edit: "ask",
            task: {
                "*": "allow",
                explore: "allow",
                sidekick: "deny",
                "smart-note-compiler": "deny",
            },
        });
        expect(Object.keys((result as { task: Record<string, unknown> }).task)).toEqual([
            "*",
            "explore",
            "sidekick",
            "smart-note-compiler",
        ]);
    });

    it("expands a task action into a wildcard rule before the denies", () => {
        expect(denyTaskRoutingToAgents({ task: "allow" }, ["sidekick"])).toEqual({
            task: { "*": "allow", sidekick: "deny" },
        });
    });

    it("moves a user allow for an internal agent after the deny so the deny wins", () => {
        const result = denyTaskRoutingToAgents({ task: { sidekick: "allow" } }, ["sidekick"]);
        expect(result).toEqual({ task: { sidekick: "deny" } });
    });

    it("treats a missing or malformed permission as empty", () => {
        expect(denyTaskRoutingToAgents(undefined, ["sidekick"])).toEqual({
            task: { sidekick: "deny" },
        });
        expect(denyTaskRoutingToAgents(["bogus"], ["sidekick"])).toEqual({
            task: { sidekick: "deny" },
        });
    });
});

describe("denyTaskRoutingToCallerAgents", () => {
    it("adds task denies to the built-in build and plan agents even when unconfigured", () => {
        const result = denyTaskRoutingToCallerAgents({}, ["sidekick"]);
        expect(result.build).toEqual({ permission: { task: { sidekick: "deny" } } });
        expect(result.plan).toEqual({ permission: { task: { sidekick: "deny" } } });
    });

    it("adds task denies to user primary agents and leaves subagents untouched", () => {
        const result = denyTaskRoutingToCallerAgents(
            {
                reviewer: { mode: "primary", permission: "allow" },
                explore: { mode: "subagent", permission: "allow" },
                helper: { mode: "all" },
            },
            ["sidekick"],
        );
        expect(result.reviewer).toEqual({
            mode: "primary",
            permission: { "*": "allow", task: { sidekick: "deny" } },
        });
        expect(result.explore).toEqual({ mode: "subagent", permission: "allow" });
        expect(result.helper).toEqual({ mode: "all" });
    });

    it("leaves agents without a mode alone unless they are build or plan", () => {
        const result = denyTaskRoutingToCallerAgents(
            { custom: { permission: "allow" }, build: { model: "m" } },
            ["sidekick"],
        );
        expect(result.custom).toEqual({ permission: "allow" });
        expect(result.build).toEqual({ model: "m", permission: { task: { sidekick: "deny" } } });
    });
});

describe("SMART_NOTE_COMPILER_ALLOWED_TOOLS", () => {
    it("is empty so the compiler emits text without calling tools", () => {
        expect([...SMART_NOTE_COMPILER_ALLOWED_TOOLS]).toEqual([]);
    });
});

describe("SIDEKICK_ALLOWED_TOOLS", () => {
    it("includes ctx_search but not ctx_memory for retrieval", () => {
        expect(SIDEKICK_ALLOWED_TOOLS).toContain("ctx_search");
        expect(SIDEKICK_ALLOWED_TOOLS).not.toContain("ctx_memory");
    });

    it("includes `aft_outline` and `aft_zoom` for lightweight structural context", () => {
        expect(SIDEKICK_ALLOWED_TOOLS).toContain("aft_outline");
        expect(SIDEKICK_ALLOWED_TOOLS).toContain("aft_zoom");
    });

    it("does NOT include `read` (use aft_outline/aft_zoom for navigation instead)", () => {
        expect(SIDEKICK_ALLOWED_TOOLS).not.toContain("read");
    });

    it("does NOT include `task` or any edit / bash / web tool", () => {
        for (const denied of ["task", "bash", "edit", "write", "webfetch", "websearch"]) {
            expect(SIDEKICK_ALLOWED_TOOLS).not.toContain(denied);
        }
    });
});

describe("integration: full hidden-agent permission shape", () => {
    it("smart-note-compiler permission object: `*` denied with no allow entry at all", () => {
        const perm = buildAllowOnlyPermission(SMART_NOTE_COMPILER_ALLOWED_TOOLS);
        expect(perm).toEqual({ "*": "deny" });
        expect(Object.keys(perm)).toEqual(["*"]);
    });

    it("sidekick permission object: `*` denied + read-only retrieval/navigation allowed", () => {
        const perm = buildAllowOnlyPermission(SIDEKICK_ALLOWED_TOOLS);
        expect(perm).toEqual({
            "*": "deny",
            ctx_search: "allow",
            aft_outline: "allow",
            aft_zoom: "allow",
        });
    });
});
