import { describe, expect, it } from "bun:test";
import { CONTEXT_RESEARCHER_AGENT } from "./context-researcher";
import {
    buildHiddenAgentConfig,
    buildHiddenAgentRegistrations,
    HIDDEN_AGENT_DESCRIPTION_MARKER,
} from "./hidden-agent-registrations";
import { CONTEXT_RESEARCHER_ALLOWED_TOOLS } from "./permissions";

describe("buildHiddenAgentRegistrations", () => {
    it("registers only context_researcher as hidden internal agent", () => {
        const overrides = { model: "m" };
        const registrations = buildHiddenAgentRegistrations({
            context_researcherPrompt: "s",
            context_researcherOverrides: overrides,
        });
        expect(registrations.map(({ id }) => id)).toEqual([CONTEXT_RESEARCHER_AGENT]);
        expect(registrations[0]).toMatchObject({
            mode: "primary",
            hidden: true,
            prompt: "s",
            allowedTools: [...CONTEXT_RESEARCHER_ALLOWED_TOOLS],
            overrides,
        });
        expect(registrations[0]?.description).toContain(HIDDEN_AGENT_DESCRIPTION_MARKER);
    });
});

describe("buildHiddenAgentConfig", () => {
    it("clamps limits and merges ordinary overrides", () => {
        const config = buildHiddenAgentConfig("p", ["eidnara_search"], 40, {
            steps: 99,
            maxSteps: 5,
            permission: { edit: "allow" },
            tools: { bash: true },
            prompt: "x",
        });
        expect(config.steps).toBe(40);
        expect(config.maxSteps).toBe(5);
        expect(config.permission).toEqual({ "*": "deny", eidnara_search: "allow", edit: "allow" });
        expect(config.prompt).toBe("x");
    });

    it("passes user tools and system overrides through and keeps the built-in prompt when none is given", () => {
        const config = buildHiddenAgentConfig(
            "p",
            ["eidnara_search"],
            40,
            { tools: { bash: true }, system: "s", fallback_models: ["m"] },
            "context-researcher",
            "d",
        );
        expect(config).toMatchObject({
            prompt: "p",
            tools: { bash: true },
            system: "s",
            fallback_models: ["m"],
            description: "d",
            mode: "primary",
            hidden: true,
        });
        expect(config.permission).toEqual({ "*": "deny", eidnara_search: "allow" });
    });

    it("falls back to cap for invalid limits", () => {
        for (const bad of [0, -1, 2.5, Number.NaN, Number.POSITIVE_INFINITY, "7", null]) {
            const config = buildHiddenAgentConfig("p", [], 40, { steps: bad, maxSteps: bad });
            expect([config.steps, config.maxSteps]).toEqual([40, 40]);
        }
    });
});
