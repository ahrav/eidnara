import { describe, expect, it } from "bun:test";
import {
    buildAllowOnlyPermission,
    denyTaskRoutingToAgents,
    denyTaskRoutingToCallerAgents,
    SIDEKICK_ALLOWED_TOOLS,
    SMART_NOTE_COMPILER_ALLOWED_TOOLS,
} from "./permissions";

describe("buildAllowOnlyPermission", () => {
    it("places only the named allows after the wildcard deny so findLast-semantics make them win", () => {
        // Permission.evaluate uses insertion-order rules with `findLast`; a wildcard after a named tool denies that tool.
        // Exact key equality also proves that unlisted tools (task, bash, edit, web) get no entry of their own.
        const perm = buildAllowOnlyPermission(["read", "ctx_search"]);
        expect(perm).toEqual({ "*": "deny", read: "allow", ctx_search: "allow" });
        expect(Object.keys(perm)).toEqual(["*", "read", "ctx_search"]);
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
    it("is exactly ctx_search plus aft_outline/aft_zoom for navigation: no ctx_memory, read, write, task, or web tools", () => {
        expect([...SIDEKICK_ALLOWED_TOOLS]).toEqual(["ctx_search", "aft_outline", "aft_zoom"]);
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
