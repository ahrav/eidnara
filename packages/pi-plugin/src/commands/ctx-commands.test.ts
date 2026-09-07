import { describe, expect, it } from "bun:test";
import type { ExtensionAPI } from "@earendil-works/pi-coding-agent";
import { COMPACTION_ENABLED_PATH } from "@eidnara/opencode/config/agent-disable";
import type { RustModeModuleClient } from "@eidnara/opencode/hooks/context/rust-mode-transform";
import { createFakePi, fakeContext, fakeKernelResolver } from "../__tests__/test-utils";
import { registerCtxFlushCommand } from "./ctx-flush";
import { registerCtxRecompCommand } from "./ctx-recomp";
import { registerCtxStatusCommand } from "./ctx-status";
import { COMPACTION_OFF_COMMAND_UNAVAILABLE } from "./daemon-session-routes";
import type { CtxStatusEntryData } from "./pi-command-utils";

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
    const run = async (name: string, args = "") => {
        const command = fake.commands.get(name) as {
            handler: (args: string, ctx: unknown) => Promise<void>;
        };
        await command.handler(args, { ...fakeContext("ses-1", "/tmp/pi"), hasUI: false });
        return entries;
    };
    return { pi, run };
}

describe("Pi /ctx-flush", () => {
    it("routes session.flush through the module client and reports the armed result", async () => {
        const { pi, run } = harness();
        const module = fakeModuleClient(() => ({ result: { armed: true } }));
        registerCtxFlushCommand(pi, { moduleClient: module.client, projectRoot: "/proj" });
        const [entry] = await run("ctx-flush");
        expect(module.calls).toEqual([
            {
                method: "session.flush",
                body: { method: "session.flush", v: 1, session_id: "ses-1" },
                timeoutMs: undefined,
                projectRoot: "/proj",
            },
        ]);
        expect(entry?.text).toContain("Flushed: Changes take effect on next message.");
        expect(entry?.level).toBe("success");
    });

    it("reports nothing pending when the daemon is not armed", async () => {
        const { pi, run } = harness();
        const module = fakeModuleClient(() => ({ armed: false }));
        registerCtxFlushCommand(pi, { moduleClient: module.client, projectRoot: "/proj" });
        const [entry] = await run("ctx-flush");
        expect(entry?.text).toContain("No pending operations to flush.");
    });

    it("renders a module failure as an error entry", async () => {
        const { pi, run } = harness();
        const module = fakeModuleClient(() => {
            throw new Error("daemon down");
        });
        registerCtxFlushCommand(pi, { moduleClient: module.client, projectRoot: "/proj" });
        const [entry] = await run("ctx-flush");
        expect(entry?.text).toContain("Error: Failed to flush context operations. daemon down");
        expect(entry?.level).toBe("error");
    });

    it("refuses in compaction-off mode without calling the daemon", async () => {
        const { pi, run } = harness();
        const module = fakeModuleClient(() => ({}));
        registerCtxFlushCommand(pi, {
            moduleClient: module.client,
            projectRoot: "/proj",
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
        projectRoot: "/proj",
        compactionOff,
        kernelClient: fakeKernelResolver().kernelClient,
        projectIdentity: "proj",
    });

    it("renders the daemon status fields as text when no TUI is available", async () => {
        const { pi, run } = harness();
        const module = fakeModuleClient(() => ({
            result: {
                usage: { current_total_input_tokens: 42_000, context_limit_tokens: 100_000 },
                boundary_present: true,
                coverage_ordinal: 12,
                compartment_count: 4,
                tail_hygiene: { u: 6_510, t: 10_000, severity: 0.651, evaluable: true },
            },
        }));
        registerCtxStatusCommand(pi, statusDeps(module.client));
        const [entry] = await run("ctx-status");
        expect(module.calls[0]?.method).toBe("session.status");
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
            registerCtxRecompCommand(pi, { moduleClient: module.client, projectRoot: "/proj" });
            const [entry] = await run("ctx-recomp");
            const body = module.calls[0]?.body ?? {};
            expect(module.calls[0]?.method).toBe("session.recomp");
            expect(body).toMatchObject({ method: "session.recomp", v: 1, session_id: "ses-1" });
            expect(String(body.command_id)).toMatch(/^opencode-recomp-/);
            expect(entry?.text).toContain(expected);
            expect(entry?.level).toBe(level);
        });
    }

    it("refuses a message range without calling the daemon", async () => {
        const { pi, run } = harness();
        const module = fakeModuleClient(() => ({}));
        registerCtxRecompCommand(pi, { moduleClient: module.client, projectRoot: "/proj" });
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
            projectRoot: "/proj",
            compactionOff: true,
        });
        const [entry] = await run("ctx-recomp");
        expect(entry?.text).toBe(COMPACTION_OFF_COMMAND_UNAVAILABLE);
        expect(module.calls).toHaveLength(0);
    });
});
