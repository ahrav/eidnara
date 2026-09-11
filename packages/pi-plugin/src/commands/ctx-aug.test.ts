import { afterEach, describe, expect, it, mock, spyOn } from "bun:test";
import { mkdtempSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import {
    __resetProjectIdentityForTests,
    __setProjectIdentityTestHooks,
} from "@eidnara/opencode/features/context/project-identity";
import { createFakePi, fakeContext } from "../__tests__/test-utils";
import * as subagentModule from "../subagent-runner";
import { registerCtxAugCommand } from "./ctx-aug";

function installRunner(result: unknown) {
    const run = mock(async () => result);
    const runnerConstructor = spyOn(subagentModule, "PiSubagentRunner").mockImplementation(
        () => ({ harness: "pi", run }) as never,
    );
    return { run, constructor: runnerConstructor };
}

describe("registerCtxAugCommand", () => {
    afterEach(() => {
        mock.restore();
    });

    it("sends the prompt with sidekick augmentation when sidekick returns context", async () => {
        const runner = installRunner({
            ok: true,
            assistantText: "Relevant memory context.",
            durationMs: 5,
        });
        try {
            const fake = createFakePi();
            registerCtxAugCommand(fake.pi as never, {
                model: "test/model",
                timeoutMs: 123,
            });
            const command = fake.commands.get("ctx-aug") as {
                handler: (args: string, ctx: never) => Promise<void>;
            };

            await command.handler(" implement feature ", fakeContext("ses-aug") as never);

            expect(runner.run).toHaveBeenCalledWith(
                expect.objectContaining({
                    model: "test/model",
                    userMessage: "implement feature",
                    timeoutMs: 123,
                }),
            );
            expect(fake.sentMessages).toEqual([
                "implement feature\n\n<sidekick-augmentation>\nRelevant memory context.\n</sidekick-augmentation>",
            ]);
        } finally {
            runner.constructor.mockRestore();
        }
    });

    it("surfaces not configured when sidekick config is absent", async () => {
        const fake = createFakePi();
        const notify = mock(() => undefined);
        registerCtxAugCommand(fake.pi as never, undefined);
        const command = fake.commands.get("ctx-aug") as {
            handler: (args: string, ctx: never) => Promise<void>;
        };
        const ctx = { ...fakeContext("ses-aug"), ui: { notify } };

        await command.handler("implement feature", ctx as never);

        expect(notify).toHaveBeenCalledWith(
            expect.stringContaining("Sidekick is not configured"),
            "warning",
        );
        expect(fake.sentMessages).toEqual([]);
    });

    it("sends the original prompt unchanged for an empty sidekick result", async () => {
        const runner = installRunner({
            ok: true,
            assistantText: "No relevant memories found.",
            durationMs: 5,
        });
        try {
            const fake = createFakePi();
            registerCtxAugCommand(fake.pi as never, { model: "test/model" });
            const command = fake.commands.get("ctx-aug") as {
                handler: (args: string, ctx: never) => Promise<void>;
            };

            await command.handler("implement feature", fakeContext("ses-aug") as never);

            expect(fake.sentMessages).toEqual(["implement feature"]);
        } finally {
            runner.constructor.mockRestore();
        }
    });

    it("does not send the prompt when the sidekick run was aborted", async () => {
        const runner = installRunner({
            ok: false,
            reason: "abort",
            error: "pi subagent aborted by caller",
            durationMs: 5,
        });
        try {
            const fake = createFakePi();
            const notify = mock(() => undefined);
            registerCtxAugCommand(fake.pi as never, { model: "test/model" });
            const command = fake.commands.get("ctx-aug") as {
                handler: (args: string, ctx: never) => Promise<void>;
            };
            const controller = new AbortController();
            controller.abort();
            const ctx = { ...fakeContext("ses-aug"), signal: controller.signal, ui: { notify } };

            await command.handler("implement feature", ctx as never);

            expect(runner.run).toHaveBeenCalledTimes(1);
            expect(fake.sentMessages).toEqual([]);
            expect(notify).toHaveBeenCalledWith(expect.stringContaining("cancelled"), "info");
        } finally {
            runner.constructor.mockRestore();
        }
    });

    it("sends the original prompt when the sidekick fails for another reason", async () => {
        const runner = installRunner({
            ok: false,
            reason: "timeout",
            error: "pi subagent timed out",
            durationMs: 5,
        });
        try {
            const fake = createFakePi();
            registerCtxAugCommand(fake.pi as never, { model: "test/model" });
            const command = fake.commands.get("ctx-aug") as {
                handler: (args: string, ctx: never) => Promise<void>;
            };

            await command.handler("implement feature", fakeContext("ses-aug") as never);

            expect(fake.sentMessages).toEqual(["implement feature"]);
        } finally {
            runner.constructor.mockRestore();
        }
    });

    it("notifies the user when the session directory has no project identity", async () => {
        const runner = installRunner({ ok: true, assistantText: "unused", durationMs: 1 });
        const home = mkdtempSync(join(tmpdir(), "eidnara-pi-ctx-aug-home-"));
        try {
            __setProjectIdentityTestHooks({ homeDirectory: () => home });
            const fake = createFakePi();
            const notify = mock(() => undefined);
            registerCtxAugCommand(fake.pi as never, { model: "test/model" });
            const command = fake.commands.get("ctx-aug") as {
                handler: (args: string, ctx: never) => Promise<void>;
            };
            const ctx = { ...fakeContext("ses-aug", home), ui: { notify } };

            await command.handler("implement feature", ctx as never);

            expect(runner.run).not.toHaveBeenCalled();
            expect(fake.sentMessages).toEqual([]);
            expect(notify).toHaveBeenCalledWith(
                expect.stringContaining("no project identity"),
                "warning",
            );
        } finally {
            runner.constructor.mockRestore();
            __resetProjectIdentityForTests();
            rmSync(home, { recursive: true, force: true });
        }
    });

    it("sends the original prompt without a UI when the session directory has no project identity", async () => {
        const runner = installRunner({ ok: true, assistantText: "unused", durationMs: 1 });
        const home = mkdtempSync(join(tmpdir(), "eidnara-pi-ctx-aug-home-"));
        try {
            __setProjectIdentityTestHooks({ homeDirectory: () => home });
            const fake = createFakePi();
            const notify = mock(() => undefined);
            registerCtxAugCommand(fake.pi as never, { model: "test/model" });
            const command = fake.commands.get("ctx-aug") as {
                handler: (args: string, ctx: never) => Promise<void>;
            };
            const ctx = { ...fakeContext("ses-aug", home), hasUI: false, ui: { notify } };

            await command.handler("implement feature", ctx as never);

            expect(runner.run).not.toHaveBeenCalled();
            expect(notify).not.toHaveBeenCalled();
            expect(fake.sentMessages).toEqual(["implement feature"]);
        } finally {
            runner.constructor.mockRestore();
            __resetProjectIdentityForTests();
            rmSync(home, { recursive: true, force: true });
        }
    });

    it("does not send the prompt without a UI when the signal is aborted and there is no project identity", async () => {
        const runner = installRunner({ ok: true, assistantText: "unused", durationMs: 1 });
        const home = mkdtempSync(join(tmpdir(), "eidnara-pi-ctx-aug-home-"));
        try {
            __setProjectIdentityTestHooks({ homeDirectory: () => home });
            const fake = createFakePi();
            registerCtxAugCommand(fake.pi as never, { model: "test/model" });
            const command = fake.commands.get("ctx-aug") as {
                handler: (args: string, ctx: never) => Promise<void>;
            };
            const controller = new AbortController();
            controller.abort();
            const ctx = {
                ...fakeContext("ses-aug", home),
                hasUI: false,
                signal: controller.signal,
            };

            await command.handler("implement feature", ctx as never);

            expect(runner.run).not.toHaveBeenCalled();
            expect(fake.sentMessages).toEqual([]);
        } finally {
            runner.constructor.mockRestore();
            __resetProjectIdentityForTests();
            rmSync(home, { recursive: true, force: true });
        }
    });
});
