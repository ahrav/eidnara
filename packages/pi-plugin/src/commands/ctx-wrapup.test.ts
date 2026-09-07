import { describe, expect, it } from "bun:test";
import type { ExtensionAPI } from "@earendil-works/pi-coding-agent";
import { MAX_WRAPUP_REQUEST_BUDGET_MS } from "@eidnara/opencode/hooks/context/module-transport";
import type { RustModeModuleClient } from "@eidnara/opencode/hooks/context/rust-mode-transform";
import { createFakePi, fakeContext } from "../__tests__/test-utils";
import { registerCtxWrapupCommand } from "./ctx-wrapup";
import { COMPACTION_OFF_COMMAND_UNAVAILABLE } from "./daemon-session-routes";
import type { CtxStatusEntryData } from "./pi-command-utils";

interface RecordedCall {
    method: string;
    body: Record<string, unknown>;
    timeoutMs?: number;
}

function wrapupHarness(respond: (call: RecordedCall) => unknown, compactionOff = false) {
    const calls: RecordedCall[] = [];
    const moduleClient: RustModeModuleClient = {
        call: async (args) => {
            const call: RecordedCall = {
                method: args.method,
                body: args.body as Record<string, unknown>,
                timeoutMs: (args as { timeoutMs?: number }).timeoutMs,
            };
            calls.push(call);
            return respond(call);
        },
    };
    const fake = createFakePi();
    const entries: CtxStatusEntryData[] = [];
    const pi = {
        ...fake.pi,
        appendEntry: (_type: string, data: CtxStatusEntryData) => {
            entries.push(data);
        },
    } as unknown as ExtensionAPI;
    registerCtxWrapupCommand(pi, { moduleClient, projectRoot: "/proj", compactionOff });
    const run = async (args = "") => {
        const command = fake.commands.get("ctx-wrapup") as {
            handler: (args: string, ctx: unknown) => Promise<void>;
        };
        await command.handler(args, { ...fakeContext("ses-1", "/tmp/pi"), hasUI: false });
        return entries;
    };
    return { calls, run };
}

describe("Pi /ctx-wrapup", () => {
    it("sends session.wrapup with the default keep, a minted command id, and the request budget", async () => {
        const { calls, run } = wrapupHarness(() => ({ result: { disposition: "completed" } }));
        const entries = await run();
        expect(calls).toHaveLength(1);
        expect(calls[0]?.method).toBe("session.wrapup");
        expect(calls[0]?.timeoutMs).toBe(MAX_WRAPUP_REQUEST_BUDGET_MS);
        expect(calls[0]?.body).toMatchObject({
            method: "session.wrapup",
            v: 1,
            session_id: "ses-1",
            keep: 20,
        });
        expect(String(calls[0]?.body.command_id)).toMatch(/^opencode-wrapup-/);
        expect(entries[0]?.text).toBe("## Eidnara Wrapup\n\nStarting wrapup…");
        expect(entries[1]?.text).toBe("## Eidnara Wrapup\n\nWrapup completed.");
        expect(entries[1]?.level).toBe("info");
    });

    it("parses an explicit messages_to_keep into the keep field", async () => {
        const { calls, run } = wrapupHarness(() => ({
            result: { disposition: "completed", summary: "Compacted 30 messages.", rounds: 2 },
        }));
        const entries = await run(" 7 ");
        expect(calls[0]?.body.keep).toBe(7);
        expect(entries[1]?.text).toBe("## Eidnara Wrapup\n\nCompacted 30 messages. (2 rounds)");
    });

    it("rejects non-positive or non-numeric keep values without calling the daemon", async () => {
        const { calls, run } = wrapupHarness(() => ({}));
        const entries = await run("0");
        expect(calls).toHaveLength(0);
        expect(entries[0]?.text).toContain("## Eidnara Wrapup — Invalid Arguments");
        expect(entries[0]?.text).toContain("messages_to_keep must be a positive integer.");
        expect(entries[0]?.level).toBe("error");
        const badEntries = await run("lots");
        expect(calls).toHaveLength(0);
        expect(badEntries.at(-1)?.text).toContain("Usage: `/ctx-wrapup [messages_to_keep]`");
    });

    const outcomes: Array<[string, string, CtxStatusEntryData["level"]]> = [
        ["nothing_to_compact", "## Eidnara Wrapup\n\nNothing to compact.", "info"],
        [
            "already_in_progress",
            "## Eidnara Wrapup — Skipped\n\n/ctx-wrapup is already running for this session.",
            "warning",
        ],
        ["retryable", "## Eidnara Wrapup — Partial\n\n", "warning"],
        ["failed", "## Eidnara Wrapup — Failed\n\nWrapup failed; try /ctx-wrapup again.", "error"],
    ];
    for (const [disposition, expected, level] of outcomes) {
        it(`maps the ${disposition} disposition`, async () => {
            const { run } = wrapupHarness(() => ({ result: { disposition } }));
            const entries = await run();
            expect(entries[1]?.text).toContain(expected);
            expect(entries[1]?.level).toBe(level);
        });
    }

    it("renders a transport failure as Failed", async () => {
        const { run } = wrapupHarness(() => {
            throw new Error("request budget exhausted");
        });
        const entries = await run();
        expect(entries[1]?.text).toBe("## Eidnara Wrapup — Failed\n\nrequest budget exhausted");
        expect(entries[1]?.level).toBe("error");
    });

    it("refuses in compaction-off mode without calling the daemon", async () => {
        const { calls, run } = wrapupHarness(() => ({}), true);
        const entries = await run();
        expect(calls).toHaveLength(0);
        expect(entries).toHaveLength(1);
        expect(entries[0]?.text).toBe(COMPACTION_OFF_COMMAND_UNAVAILABLE);
        expect(entries[0]?.level).toBe("warning");
    });
});
