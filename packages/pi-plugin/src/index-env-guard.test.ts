import { afterEach, describe, expect, it } from "bun:test";
import { mkdtempSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { createCountingPi } from "./__tests__/test-utils";
import eidnaraPiExtension, { __test } from "./index";
import { EIDNARA_PI_SUBAGENT_ENV } from "./subagent-runner";

const originalEnv = {
    EIDNARA_PI_SUBAGENT: process.env.EIDNARA_PI_SUBAGENT,
    XDG_CONFIG_HOME: process.env.XDG_CONFIG_HOME,
    XDG_DATA_HOME: process.env.XDG_DATA_HOME,
};

function restoreEnv() {
    for (const [key, value] of Object.entries(originalEnv)) {
        if (value === undefined) delete process.env[key];
        else process.env[key] = value;
    }
}

function isolateXdgEnv() {
    const root = mkdtempSync(join(tmpdir(), "eidnara-pi-index-test-"));
    process.env.XDG_CONFIG_HOME = join(root, "config");
    process.env.XDG_DATA_HOME = join(root, "data");
}

afterEach(() => {
    restoreEnv();
    // The test helper resets the global initialization latch between tests.
    __test.clearPiEidnaraActive();
});

describe("Pi full extension subagent env guard", () => {
    it("no-ops before registering anything inside Eidnara Pi subagents", async () => {
        isolateXdgEnv();
        process.env[EIDNARA_PI_SUBAGENT_ENV] = "1";
        const registrations = createCountingPi();

        await eidnaraPiExtension(registrations.pi);

        expect(registrations.events).toEqual([]);
        expect(registrations.tools).toEqual([]);
        expect(registrations.flags).toEqual([]);
        expect(registrations.commands).toEqual([]);
        expect(registrations.entryRenderers).toEqual([]);
        // The environment guard returns before setting the latch, so a later in-process initialization still registers fully.
        expect(__test.isPiEidnaraActiveInProcess()).toBe(false);
    });

    it("registers the full runtime when the subagent guard is absent", async () => {
        isolateXdgEnv();
        delete process.env[EIDNARA_PI_SUBAGENT_ENV];
        const registrations = createCountingPi();

        await eidnaraPiExtension(registrations.pi);

        expect(registrations.events.length).toBeGreaterThan(0);
        expect(registrations.tools.length).toBeGreaterThan(0);
        expect(registrations.commands.length).toBeGreaterThan(0);
        expect(registrations.entryRenderers).toEqual(["ctx-status"]);
        expect(registrations.events).toContain("before_agent_start");
        // The daemon owns the transform, so the entry registers no `context` handler.
        expect(registrations.events).not.toContain("context");
        expect(registrations.tools).toContain("ctx_search");
        expect(registrations.commands).toContain("ctx-status");
    }, 15_000);
});
