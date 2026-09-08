import { afterEach, describe, expect, it, spyOn } from "bun:test";
import { mkdirSync, mkdtempSync, realpathSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import * as loggerModule from "@eidnara/opencode/shared/logger";
import { createCountingPi } from "./__tests__/test-utils";
import eidnaraPiExtension, { __test } from "./index";
import { EIDNARA_PI_SUBAGENT_ENV } from "./subagent-runner";

const originalEnv = {
    EIDNARA_PI_SUBAGENT: process.env.EIDNARA_PI_SUBAGENT,
    XDG_CONFIG_HOME: process.env.XDG_CONFIG_HOME,
    XDG_DATA_HOME: process.env.XDG_DATA_HOME,
};
const originalCwd = process.cwd();

function restoreEnv() {
    for (const [key, value] of Object.entries(originalEnv)) {
        if (value === undefined) delete process.env[key];
        else process.env[key] = value;
    }
}

function isolateXdgEnv() {
    const root = mkdtempSync(join(tmpdir(), "eidnara-pi-root-test-"));
    process.env.XDG_CONFIG_HOME = join(root, "config");
    process.env.XDG_DATA_HOME = join(root, "data");
}

/** A checkout root with a project config and a nested working directory. */
function checkoutWithProjectConfig() {
    const root = realpathSync.native(mkdtempSync(join(tmpdir(), "eidnara-pi-checkout-")));
    mkdirSync(join(root, ".git"));
    mkdirSync(join(root, ".eidnara"));
    const configPath = join(root, ".eidnara", "eidnara.jsonc");
    writeFileSync(configPath, JSON.stringify({ protected_tags: 7 }));
    const nested = join(root, "packages", "inner");
    mkdirSync(nested, { recursive: true });
    return { root, nested, configPath };
}

function configLoadMessages(logSpy: ReturnType<typeof spyOn>): string[] {
    return logSpy.mock.calls
        .map(([message]) => String(message))
        .filter((message) => message.includes("config loaded from:"));
}

afterEach(() => {
    process.chdir(originalCwd);
    restoreEnv();
    __test.clearPiEidnaraActive();
    __test.resetLoggedPiConfigDirs();
});

describe("Pi project config resolves from the checkout root", () => {
    it("loads the root project config when the process starts in a nested directory", async () => {
        isolateXdgEnv();
        delete process.env[EIDNARA_PI_SUBAGENT_ENV];
        __test.clearPiEidnaraActive();
        const { nested, configPath } = checkoutWithProjectConfig();
        process.chdir(nested);
        const logSpy = spyOn(loggerModule, "log").mockImplementation(() => undefined);
        try {
            await eidnaraPiExtension(createCountingPi().pi);
            expect(configLoadMessages(logSpy).some((m) => m.includes(configPath))).toBe(true);
        } finally {
            logSpy.mockRestore();
        }
    });

    it("loads the root project config for a /cd into a nested directory of another checkout", async () => {
        isolateXdgEnv();
        delete process.env[EIDNARA_PI_SUBAGENT_ENV];
        __test.clearPiEidnaraActive();
        const registrations = createCountingPi();
        await eidnaraPiExtension(registrations.pi);
        const beforeAgentStart = registrations.handlers.get("before_agent_start") as
            | ((event: { systemPrompt: string }, ctx: unknown) => Promise<unknown>)
            | undefined;
        if (!beforeAgentStart) throw new Error("before_agent_start not registered");

        const { nested, configPath } = checkoutWithProjectConfig();
        const logSpy = spyOn(loggerModule, "log").mockImplementation(() => undefined);
        try {
            await beforeAgentStart(
                { systemPrompt: "You are a helpful assistant." },
                { cwd: nested, sessionManager: { getSessionId: () => "ses-nested" } },
            );
            expect(configLoadMessages(logSpy).some((m) => m.includes(configPath))).toBe(true);
        } finally {
            logSpy.mockRestore();
        }
    });
});
