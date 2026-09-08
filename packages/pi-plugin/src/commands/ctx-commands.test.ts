import { describe, expect, it } from "bun:test";
import type { ExtensionAPI } from "@earendil-works/pi-coding-agent";
import { COMPACTION_ENABLED_PATH } from "@eidnara/opencode/config/agent-disable";
import { resolveProjectRootDirectory } from "@eidnara/opencode/features/context/project-identity";
import type { RustModeModuleClient } from "@eidnara/opencode/hooks/context/rust-mode-transform";
import { createFakePi, fakeContext, fakeKernelResolver } from "../__tests__/test-utils";
import { registerCtxFlushCommand } from "./ctx-flush";
import { registerCtxRecompCommand } from "./ctx-recomp";
import { registerCtxStatusCommand } from "./ctx-status";
import { COMPACTION_OFF_COMMAND_UNAVAILABLE } from "./daemon-session-routes";
import type { CtxStatusEntryData } from "./pi-command-utils";

/** The harness cwd is not a git checkout, so the route root is its canonical spelling. */
const CWD = "/tmp/pi";
const CWD_ROOT = resolveProjectRootDirectory(CWD);

interface RecordedCall {
    method: string;
    body: Record<string, unknown>;
    timeoutMs?: number;
    projectRoot: string;
}

function fakeModuleClient(respond: (call: RecordedCall) => unknown) {
    const calls: RecordedCall[] = [];
    const client: RustModeModuleClient = {
        call: async (args) => {
            const call: RecordedCall = {
                method: args.method,
                body: args.body as Record<string, unknown>,
                timeoutMs: (args as { timeoutMs?: number }).timeoutMs,
                projectRoot: args.projectRoot,
            };
            calls.push(call);
            return respond(call);
        },
    };
    return { calls, client };
}

function harness() {
    const fake = createFakePi();
    const entries: CtxStatusEntryData[] = [];
    const pi = {
        ...fake.pi,
        appendEntry: (_type: string, data: CtxStatusEntryData) => {
            entries.push(data);
        },
    } as unknown as ExtensionAPI;
    const run = async (name: string, args = "", ctx: Record<string, unknown> = {}) => {
        const command = fake.commands.get(name) as {
            handler: (args: string, ctx: unknown) => Promise<void>;
        };
        await command.handler(args, { ...fakeContext("ses-1", CWD), hasUI: false, ...ctx });
        return entries;
    };
    return { pi, run };
}

describe("Pi /ctx-flush", () => {
    it("routes session.flush through the module client and reports the armed result", async () => {
        const { pi, run } = harness();
        const module = fakeModuleClient(() => ({ result: { armed: true } }));
        registerCtxFlushCommand(pi, { moduleClient: module.client });
        const [entry] = await run("ctx-flush");
        expect(module.calls).toEqual([
            {
                method: "session.flush",
                body: { method: "session.flush", v: 1, session_id: "ses-1" },
                timeoutMs: undefined,
                projectRoot: CWD_ROOT,
            },
        ]);
        expect(entry?.text).toContain("Flushed: Changes take effect on next message.");
        expect(entry?.level).toBe("success");
    });

    it("routes on the project root of the invocation cwd, so /cd moves later commands", async () => {
        const { pi, run } = harness();
        const module = fakeModuleClient(() => ({ result: { armed: true } }));
        registerCtxFlushCommand(pi, { moduleClient: module.client });
        await run("ctx-flush");
        await run("ctx-flush", "", { cwd: "/tmp/other-project" });
        expect(module.calls.map((call) => call.projectRoot)).toEqual([
            CWD_ROOT,
            resolveProjectRootDirectory("/tmp/other-project"),
        ]);
    });

    it("reports nothing pending when the daemon is not armed", async () => {
        const { pi, run } = harness();
        const module = fakeModuleClient(() => ({ armed: false }));
        registerCtxFlushCommand(pi, { moduleClient: module.client });
        const [entry] = await run("ctx-flush");
        expect(entry?.text).toContain("No pending operations to flush.");
    });

    it("renders a module failure as an error entry", async () => {
        const { pi, run } = harness();
        const module = fakeModuleClient(() => {
            throw new Error("daemon down");
        });
        registerCtxFlushCommand(pi, { moduleClient: module.client });
        const [entry] = await run("ctx-flush");
        expect(entry?.text).toContain("Error: Failed to flush context operations. daemon down");
        expect(entry?.level).toBe("error");
    });

    it("refuses in compaction-off mode without calling the daemon", async () => {
        const { pi, run } = harness();
        const module = fakeModuleClient(() => ({}));
        registerCtxFlushCommand(pi, {
            moduleClient: module.client,
            compactionOff: true,
        });
        const [entry] = await run("ctx-flush");
        expect(entry?.text).toBe(COMPACTION_OFF_COMMAND_UNAVAILABLE);
        expect(entry?.text).toContain(`${COMPACTION_ENABLED_PATH}=false`);
        expect(module.calls).toHaveLength(0);
    });
});

describe("Pi /ctx-status", () => {
    const statusDeps = (moduleClient: RustModeModuleClient, compactionOff = false) => ({
        moduleClient,
        compactionOff,
        kernelClient: fakeKernelResolver().kernelClient,
        resolveProjectSettings: () => ({ projectIdentity: "proj" }),
    });
    const DAEMON_STATUS = {
        usage: { current_total_input_tokens: 42_000, context_limit_tokens: 100_000 },
        boundary_present: true,
        coverage_ordinal: 12,
        compartment_count: 4,
        tail_hygiene: { u: 6_510, t: 10_000, severity: 0.651, evaluable: true },
    };
    function dialogUi() {
        let opened = 0;
        const ui = {
            notify: () => undefined,
            async custom(factory: unknown) {
                opened += 1;
                const component = (
                    factory as (
                        tui: unknown,
                        theme: unknown,
                        keybindings: unknown,
                        done: () => void,
                    ) => { dispose?: () => void }
                )(
                    { requestRender: () => undefined },
                    { fg: (_name: string, text: string) => text, bold: (text: string) => text },
                    undefined,
                    () => undefined,
                );
                // Disposing clears the dialog's refresh interval so the test process exits cleanly.
                component.dispose?.();
                return undefined;
            },
        };
        return { ui, opened: () => opened };
    }

    it("renders the daemon status fields as text when no TUI is available", async () => {
        const { pi, run } = harness();
        const module = fakeModuleClient(() => ({ result: DAEMON_STATUS }));
        registerCtxStatusCommand(pi, statusDeps(module.client));
        const [entry] = await run("ctx-status");
        expect(module.calls[0]?.method).toBe("session.status");
        expect(module.calls[0]?.projectRoot).toBe(CWD_ROOT);
        expect(module.calls[0]?.body).toEqual({
            method: "session.status",
            v: 1,
            session_id: "ses-1",
        });
        expect(entry?.text).toContain("### Module Cache");
        expect(entry?.text).toContain(
            `- Usage: ${(42_000).toLocaleString()} / ${(100_000).toLocaleString()} tokens`,
        );
        expect(entry?.text).toContain("- Boundary: present");
        expect(entry?.text).toContain("- Coverage ordinal: 12");
        expect(entry?.text).toContain("- Compartments: 4");
        expect(entry?.text).toContain("### Tail Hygiene");
        expect(entry?.text).toContain("65.1%");
    });

    it("renders the window derivation only when the daemon limit is absent or equals the usable window", async () => {
        const reservedModel = {
            model: {
                provider: "anthropic",
                id: "claude",
                contextWindow: 100_000,
                maxTokens: 20_000,
            },
            getContextUsage: () => ({ tokens: 42_000, percent: 42, contextWindow: 100_000 }),
        };
        const statuses = [
            { ...DAEMON_STATUS.usage },
            { current_total_input_tokens: 42_000, context_limit_tokens: 80_000 },
            { current_total_input_tokens: 42_000 },
        ];
        const { pi, run } = harness();
        const module = fakeModuleClient(() => ({
            result: { ...DAEMON_STATUS, usage: statuses.shift() },
        }));
        registerCtxStatusCommand(pi, statusDeps(module.client));

        const [differing] = await run("ctx-status", "", reservedModel);
        expect(differing?.text).toContain("- Usage: 42,000 / 100,000 tokens");
        expect(differing?.text).not.toContain("usable (");

        const [, agreeing] = await run("ctx-status", "", reservedModel);
        expect(agreeing?.text).toContain("- Usage: 42,000 / 80,000 tokens");
        expect(agreeing?.text).toContain("42k / 80k usable (52.5%)");

        const [, , noLimit] = await run("ctx-status", "", reservedModel);
        expect(noLimit?.text).toContain("- Usage: 42,000 tokens");
        expect(noLimit?.text).toContain("42k / 80k usable (52.5%)");
    });

    it("resolves project settings and the daemon route from the same invoking cwd", async () => {
        const { pi, run } = harness();
        const module = fakeModuleClient(() => ({ result: {} }));
        const resolvedFor: string[] = [];
        registerCtxStatusCommand(pi, {
            ...statusDeps(module.client),
            resolveProjectSettings: ({ cwd }) => {
                resolvedFor.push(cwd);
                return { projectIdentity: `proj:${cwd}` };
            },
        });
        await run("ctx-status", "", { cwd: "/tmp/project-a" });
        await run("ctx-status", "", { cwd: "/tmp/project-b" });
        expect(resolvedFor).toEqual(["/tmp/project-a", "/tmp/project-b"]);
        expect(module.calls.map((call) => call.projectRoot)).toEqual([
            resolveProjectRootDirectory("/tmp/project-a"),
            resolveProjectRootDirectory("/tmp/project-b"),
        ]);
    });

    it("reports the unavailable line when session.status fails and notes compaction-off", async () => {
        const { pi, run } = harness();
        const module = fakeModuleClient(() => {
            throw new Error("socket closed");
        });
        registerCtxStatusCommand(pi, statusDeps(module.client, true));
        const [entry] = await run("ctx-status");
        expect(entry?.text).toContain("Session status is unavailable: socket closed");
        expect(entry?.text).toContain(
            `**Compaction:** disabled (${COMPACTION_ENABLED_PATH}: false)`,
        );
        expect(entry?.text).not.toContain("### Module Cache");
    });

    it("opens the dialog with a daemon answer and routes the read through the cwd root", async () => {
        const { pi, run } = harness();
        const module = fakeModuleClient(() => ({ result: DAEMON_STATUS }));
        registerCtxStatusCommand(pi, statusDeps(module.client));
        const { ui, opened } = dialogUi();
        const entries = await run("ctx-status", "", { hasUI: true, ui });
        expect(opened()).toBe(1);
        expect(entries).toHaveLength(0);
        expect(module.calls.map((call) => call.method)).toEqual(["session.status"]);
        expect(module.calls[0]?.projectRoot).toBe(CWD_ROOT);
    });

    it("reports a status failure as text instead of opening a dialog of zero counts", async () => {
        const { pi, run } = harness();
        const module = fakeModuleClient(() => {
            throw new Error("socket closed");
        });
        registerCtxStatusCommand(pi, statusDeps(module.client));
        const { ui, opened } = dialogUi();
        const [entry] = await run("ctx-status", "", { hasUI: true, ui });
        expect(opened()).toBe(0);
        expect(entry?.text).toContain("Session status is unavailable: socket closed");
    });

    it("reports compaction-off as text instead of a dialog that cannot show the mode", async () => {
        const { pi, run } = harness();
        const module = fakeModuleClient(() => ({ result: DAEMON_STATUS }));
        registerCtxStatusCommand(pi, statusDeps(module.client, true));
        const { ui, opened } = dialogUi();
        const [entry] = await run("ctx-status", "", { hasUI: true, ui });
        expect(opened()).toBe(0);
        expect(entry?.text).toContain(
            `**Compaction:** disabled (${COMPACTION_ENABLED_PATH}: false)`,
        );
        expect(entry?.text).toContain("### Module Cache");
    });
});

describe("Pi /ctx-recomp", () => {
    const cases: Array<[string, string, CtxStatusEntryData["level"]]> = [
        ["started", "Historian recomp started.", "info"],
        ["already_in_progress", "## Eidnara Recomp — Skipped", "warning"],
        ["nothing_to_do", "Nothing to rebuild", "info"],
        ["failed", "## Eidnara Recomp — Failed", "error"],
    ];
    for (const [disposition, expected, level] of cases) {
        it(`maps the ${disposition} disposition`, async () => {
            const { pi, run } = harness();
            const module = fakeModuleClient(() => ({ result: { disposition } }));
            registerCtxRecompCommand(pi, { moduleClient: module.client });
            const [entry] = await run("ctx-recomp");
            const body = module.calls[0]?.body ?? {};
            expect(module.calls[0]?.method).toBe("session.recomp");
            expect(module.calls[0]?.projectRoot).toBe(CWD_ROOT);
            expect(body).toMatchObject({ method: "session.recomp", v: 1, session_id: "ses-1" });
            expect(String(body.command_id)).toMatch(/^opencode-recomp-/);
            expect(entry?.text).toContain(expected);
            expect(entry?.level).toBe(level);
        });
    }

    it("refuses a message range without calling the daemon", async () => {
        const { pi, run } = harness();
        const module = fakeModuleClient(() => ({}));
        registerCtxRecompCommand(pi, { moduleClient: module.client });
        const [entry] = await run("ctx-recomp", "1-40");
        expect(module.calls).toHaveLength(0);
        expect(entry?.text).toContain("## Eidnara Recomp — Unsupported");
        expect(entry?.text).toContain("Requested range: `1-40`");
        expect(entry?.text).toContain("`session.recomp` accepts no message range.");
    });

    it("refuses in compaction-off mode", async () => {
        const { pi, run } = harness();
        const module = fakeModuleClient(() => ({}));
        registerCtxRecompCommand(pi, {
            moduleClient: module.client,
            compactionOff: true,
        });
        const [entry] = await run("ctx-recomp");
        expect(entry?.text).toBe(COMPACTION_OFF_COMMAND_UNAVAILABLE);
        expect(module.calls).toHaveLength(0);
    });
});
