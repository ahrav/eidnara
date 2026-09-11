import { afterEach, describe, expect, it } from "bun:test";
import { mkdirSync, mkdtempSync, rmSync, writeFileSync } from "node:fs";
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
    const root = mkdtempSync(join(tmpdir(), "eidnara-pi-latch-test-"));
    process.env.XDG_CONFIG_HOME = join(root, "config");
    process.env.XDG_DATA_HOME = join(root, "data");
    return root;
}

function userConfigPath(root: string): string {
    return join(root, "config", "eidnara", "eidnara.jsonc");
}

afterEach(() => {
    restoreEnv();
    // Clear the process-global latch between tests; otherwise one test's initialization suppresses the next.
    __test.clearPiEidnaraActive();
});

describe("Pi in-process re-init latch (#247)", () => {
    it("second init in the same process is a no-op (no duplicate registrations)", async () => {
        isolateXdgEnv();
        delete process.env[EIDNARA_PI_SUBAGENT_ENV];
        __test.clearPiEidnaraActive();

        const first = createCountingPi();
        await eidnaraPiExtension(first.pi);

        expect(first.events.length).toBeGreaterThan(0);
        expect(first.tools.length).toBeGreaterThan(0);
        expect(first.commands.length).toBeGreaterThan(0);
        expect(first.entryRenderers).toEqual(["ctx-status"]);

        expect(__test.isPiEidnaraActiveInProcess()).toBe(true);

        const second = createCountingPi();
        await eidnaraPiExtension(second.pi);

        expect(second.events).toEqual([]);
        expect(second.tools).toEqual([]);
        expect(second.flags).toEqual([]);
        expect(second.commands).toEqual([]);
        expect(second.entryRenderers).toEqual([]);
    }, 15_000);

    it("clearing the latch (dispose) allows a full re-init", async () => {
        isolateXdgEnv();
        delete process.env[EIDNARA_PI_SUBAGENT_ENV];
        __test.clearPiEidnaraActive();

        const first = createCountingPi();
        await eidnaraPiExtension(first.pi);
        expect(first.tools.length).toBeGreaterThan(0);

        __test.clearPiEidnaraActive();
        expect(__test.isPiEidnaraActiveInProcess()).toBe(false);

        const second = createCountingPi();
        await eidnaraPiExtension(second.pi);

        expect(second.events.length).toBeGreaterThan(0);
        expect(second.tools.length).toBeGreaterThan(0);
        expect(second.commands.length).toBeGreaterThan(0);
        expect(second.entryRenderers).toEqual(["ctx-status"]);
    }, 15_000);

    it("a disabled configuration leaves the latch clear so /reload can register an enabled one", async () => {
        const root = isolateXdgEnv();
        delete process.env[EIDNARA_PI_SUBAGENT_ENV];
        __test.clearPiEidnaraActive();
        mkdirSync(join(root, "config", "eidnara"), { recursive: true });
        writeFileSync(userConfigPath(root), JSON.stringify({ enabled: false }));

        const disabled = createCountingPi();
        await eidnaraPiExtension(disabled.pi);
        expect(disabled.events).toEqual([]);
        expect(disabled.tools).toEqual([]);
        expect(disabled.commands).toEqual([]);
        // No `session_shutdown` handler was registered, so nothing else could clear the latch.
        expect(__test.isPiEidnaraActiveInProcess()).toBe(false);

        // `/reload` after the user enables Eidnara re-runs the factory without a process restart.
        rmSync(userConfigPath(root));
        const enabled = createCountingPi();
        await eidnaraPiExtension(enabled.pi);
        expect(enabled.events).toContain("session_shutdown");
        expect(enabled.tools.length).toBeGreaterThan(0);
        expect(enabled.commands.length).toBeGreaterThan(0);
        expect(__test.isPiEidnaraActiveInProcess()).toBe(true);
    }, 15_000);
});
