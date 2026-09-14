import { describe, expect, it } from "bun:test";
import {
    buildAllowOnlyPermission,
    CONTEXT_RESEARCHER_ALLOWED_TOOLS,
    denyTaskRoutingToAgents,
    denyTaskRoutingToCallerAgents,
    NOTE_CONDITION_COMPILER_ALLOWED_TOOLS,
} from "./permissions";

describe("buildAllowOnlyPermission", () => {
    it("places only the named allows after the wildcard deny so findLast-semantics make them win", () => {
        // Permission.evaluate uses insertion-order rules with `findLast`; a wildcard after a named tool denies that tool.
        // Exact key equality also proves that unlisted tools (task, bash, edit, web) get no entry of their own.
        const perm = buildAllowOnlyPermission(["read", "eidnara_search"]);
        expect(perm).toEqual({ "*": "deny", read: "allow", eidnara_search: "allow" });
        expect(Object.keys(perm)).toEqual(["*", "read", "eidnara_search"]);
    });

    it("returns deny-all when the allow-list is undefined", () => {
        const perm = buildAllowOnlyPermission(undefined, "test-agent");
        expect(perm).toEqual({ "*": "deny" });
    });
});

describe("denyTaskRoutingToAgents", () => {
    it("appends exact agent-ID denies after a whole-permission action", () => {
        expect(denyTaskRoutingToAgents("allow", ["context-researcher"])).toEqual({
            "*": "allow",
            task: { "context-researcher": "deny" },
        });
    });

    it("preserves user task patterns and appends internal denies last", () => {
        const result = denyTaskRoutingToAgents(
            { edit: "ask", task: { "*": "allow", explore: "allow" } },
            ["context-researcher", "note-condition-compiler"],
        );
        expect(result).toEqual({
            edit: "ask",
            task: {
                "*": "allow",
                explore: "allow",
                "context-researcher": "deny",
                "note-condition-compiler": "deny",
            },
        });
        expect(Object.keys((result as { task: Record<string, unknown> }).task)).toEqual([
            "*",
            "explore",
            "context-researcher",
            "note-condition-compiler",
        ]);
    });

    it("expands a task action into a wildcard rule before the denies", () => {
        expect(denyTaskRoutingToAgents({ task: "allow" }, ["context-researcher"])).toEqual({
            task: { "*": "allow", "context-researcher": "deny" },
        });
    });

    it("moves a user allow for an internal agent after the deny so the deny wins", () => {
        const result = denyTaskRoutingToAgents({ task: { "context-researcher": "allow" } }, [
            "context-researcher",
        ]);
        expect(result).toEqual({ task: { "context-researcher": "deny" } });
    });

    it("treats a missing or malformed permission as empty", () => {
        expect(denyTaskRoutingToAgents(undefined, ["context-researcher"])).toEqual({
            task: { "context-researcher": "deny" },
        });
        expect(denyTaskRoutingToAgents(["bogus"], ["context-researcher"])).toEqual({
            task: { "context-researcher": "deny" },
        });
    });
});

describe("denyTaskRoutingToCallerAgents", () => {
    it("adds task denies to the built-in build and plan agents even when unconfigured", () => {
        const result = denyTaskRoutingToCallerAgents({}, ["context-researcher"]);
        expect(result.build).toEqual({ permission: { task: { "context-researcher": "deny" } } });
        expect(result.plan).toEqual({ permission: { task: { "context-researcher": "deny" } } });
    });

    it("adds task denies to user primary agents and leaves subagents untouched", () => {
        const result = denyTaskRoutingToCallerAgents(
            {
                reviewer: { mode: "primary", permission: "allow" },
                explore: { mode: "subagent", permission: "allow" },
                helper: { mode: "all" },
            },
            ["context-researcher"],
        );
        expect(result.reviewer).toEqual({
            mode: "primary",
            permission: { "*": "allow", task: { "context-researcher": "deny" } },
        });
        expect(result.explore).toEqual({ mode: "subagent", permission: "allow" });
        expect(result.helper).toEqual({ mode: "all" });
    });

    it("leaves agents without a mode alone unless they are build or plan", () => {
        const result = denyTaskRoutingToCallerAgents(
            { custom: { permission: "allow" }, build: { model: "m" } },
            ["context-researcher"],
        );
        expect(result.custom).toEqual({ permission: "allow" });
        expect(result.build).toEqual({
            model: "m",
            permission: { task: { "context-researcher": "deny" } },
        });
    });
});

describe("NOTE_CONDITION_COMPILER_ALLOWED_TOOLS", () => {
    it("is empty so the compiler emits text without calling tools", () => {
        expect([...NOTE_CONDITION_COMPILER_ALLOWED_TOOLS]).toEqual([]);
    });
});

describe("CONTEXT_RESEARCHER_ALLOWED_TOOLS", () => {
    it("is exactly eidnara_search plus aft_outline/aft_zoom for navigation: no eidnara_memory, read, write, task, or web tools", () => {
        expect([...CONTEXT_RESEARCHER_ALLOWED_TOOLS]).toEqual([
            "eidnara_search",
            "aft_outline",
            "aft_zoom",
        ]);
    });
});

describe("integration: full hidden-agent permission shape", () => {
    it("note-condition-compiler permission object: `*` denied with no allow entry at all", () => {
        const perm = buildAllowOnlyPermission(NOTE_CONDITION_COMPILER_ALLOWED_TOOLS);
        expect(perm).toEqual({ "*": "deny" });
        expect(Object.keys(perm)).toEqual(["*"]);
    });

    it("context_researcher permission object: `*` denied + read-only retrieval/navigation allowed", () => {
        const perm = buildAllowOnlyPermission(CONTEXT_RESEARCHER_ALLOWED_TOOLS);
        expect(perm).toEqual({
            "*": "deny",
            eidnara_search: "allow",
            aft_outline: "allow",
            aft_zoom: "allow",
        });
    });
});
