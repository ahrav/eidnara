import { describe, expect, it } from "bun:test";
import { CONTEXT_RESEARCHER_AGENT } from "./context-researcher";
import {
    buildHiddenAgentConfig,
    buildHiddenAgentRegistrations,
    HIDDEN_AGENT_DESCRIPTION_MARKER,
} from "./hidden-agent-registrations";
import { NOTE_CONDITION_COMPILER_AGENT } from "./note-condition-compiler";
import {
    CONTEXT_RESEARCHER_ALLOWED_TOOLS,
    NOTE_CONDITION_COMPILER_ALLOWED_TOOLS,
} from "./permissions";

describe("buildHiddenAgentRegistrations", () => {
    const context_researcherOverrides = { model: "m" };
    const registrations = buildHiddenAgentRegistrations({
        noteConditionCompilerPrompt: "c",
        context_researcherPrompt: "s",
        context_researcherOverrides,
    });

    it("registers exactly the conditional-note compiler and context_researcher, in that order", () => {
        expect(registrations.map((r) => r.id)).toEqual([
            NOTE_CONDITION_COMPILER_AGENT,
            CONTEXT_RESEARCHER_AGENT,
        ]);
    });

    it("marks every registration primary, hidden, and internal", () => {
        for (const registration of registrations) {
            expect(registration.mode).toBe("primary");
            expect(registration.hidden).toBe(true);
            expect(registration.description).toContain(HIDDEN_AGENT_DESCRIPTION_MARKER);
        }
    });

    it("locks the compiler to its empty allow-list", () => {
        const compiler = registrations.find((r) => r.id === NOTE_CONDITION_COMPILER_AGENT);
        expect(compiler?.prompt).toBe("c");
        expect(compiler?.allowedTools).toEqual([...NOTE_CONDITION_COMPILER_ALLOWED_TOOLS]);
        expect(compiler?.lockPermissions).toBe(true);
        expect(compiler?.overrides).toBeUndefined();
    });

    it("gives context_researcher its allow-list and passes user overrides through", () => {
        const context_researcher = registrations.find((r) => r.id === CONTEXT_RESEARCHER_AGENT);
        expect(context_researcher?.prompt).toBe("s");
        expect(context_researcher?.allowedTools).toEqual([...CONTEXT_RESEARCHER_ALLOWED_TOOLS]);
        expect(context_researcher?.overrides).toBe(context_researcherOverrides);
        expect(context_researcher?.lockPermissions).toBeUndefined();
    });
});

describe("buildHiddenAgentConfig", () => {
    const overrides = {
        steps: 99,
        maxSteps: 5,
        permission: { edit: "allow" },
        tools: { bash: true },
        prompt: "x",
        system: "user system",
    };

    it("clamps step limits to the cap and merges user permission, tools, prompt, and system", () => {
        const config = buildHiddenAgentConfig("p", ["eidnara_search"], 40, overrides);
        expect(config.steps).toBe(40);
        expect(config.maxSteps).toBe(5);
        expect(config.permission).toEqual({ "*": "deny", eidnara_search: "allow", edit: "allow" });
        expect((config as Record<string, unknown>).tools).toEqual({ bash: true });
        expect(config.prompt).toBe("x");
        expect((config as Record<string, unknown>).system).toBe("user system");
        expect(config.mode).toBe("primary");
        expect(config.hidden).toBe(true);

        // An override above the cap on maxSteps alone clamps it and leaves steps at the cap.
        const clamped = buildHiddenAgentConfig("p", ["eidnara_search"], 40, { maxSteps: 100_000 });
        expect(clamped.maxSteps).toBe(40);
        expect(clamped.steps).toBe(40);
    });

    it("drops user permission, tools, prompt, and system overrides when permissions are locked", () => {
        const config = buildHiddenAgentConfig(
            "p",
            ["eidnara_search"],
            40,
            overrides,
            "label",
            true,
        );
        expect(config.permission).toEqual({ "*": "deny", eidnara_search: "allow" });
        expect("edit" in config.permission).toBe(false);
        expect("tools" in config).toBe(false);
        expect("system" in config).toBe(false);
        expect(config.prompt).toBe("p");
        expect(config.steps).toBe(40);
        expect(config.maxSteps).toBe(5);
    });

    it("falls back to the cap for a step limit that is not a positive integer", () => {
        for (const bad of [0, -1, 2.5, Number.NaN, Number.POSITIVE_INFINITY, "7", null]) {
            const config = buildHiddenAgentConfig("p", ["eidnara_search"], 40, {
                steps: bad,
                maxSteps: bad,
            });
            expect([bad, config.steps, config.maxSteps]).toEqual([bad, 40, 40]);
        }
    });

    it("keeps a positive integer step limit at or under the cap", () => {
        const config = buildHiddenAgentConfig("p", ["eidnara_search"], 40, {
            steps: 1,
            maxSteps: 40,
        });
        expect(config.steps).toBe(1);
        expect(config.maxSteps).toBe(40);
    });
});
