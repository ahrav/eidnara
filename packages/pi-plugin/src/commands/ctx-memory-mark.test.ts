import { describe, expect, it } from "bun:test";
import type { ExtensionAPI } from "@earendil-works/pi-coding-agent";
import { createFakePi, fakeContext, fakeKernelResolver } from "../__tests__/test-utils";
import { registerCtxMemoryMarkCommand } from "./ctx-memory-mark";
import type { CtxStatusEntryData } from "./pi-command-utils";

const CWD = "/tmp/pi";

function harness() {
    const fake = createFakePi();
    const entries: CtxStatusEntryData[] = [];
    const confirmations: Array<{ title: string; message: string }> = [];
    let answer = true;
    const pi = {
        ...fake.pi,
        appendEntry: (_type: string, data: CtxStatusEntryData) => {
            entries.push(data);
        },
    } as unknown as ExtensionAPI;
    const resolver = fakeKernelResolver();
    registerCtxMemoryMarkCommand(pi, { kernelClient: resolver.kernelClient });
    const run = async (args: string, hasUI = true, replaceSessionOnConfirm = false) => {
        const command = fake.commands.get("ctx-memory-mark") as {
            handler: (args: string, ctx: unknown) => Promise<void>;
        };
        const base = fakeContext("ses-1", CWD);
        // Pi's real `ctx` throws on every property once its session is replaced; `stale` stands in for that invalidation.
        let stale = false;
        const ctx = new Proxy(
            {
                ...base,
                hasUI,
                ui: {
                    ...base.ui,
                    confirm: async (title: string, message: string) => {
                        confirmations.push({ title, message });
                        if (replaceSessionOnConfirm) stale = true;
                        return answer;
                    },
                },
            },
            {
                get(target, property, receiver) {
                    if (stale) throw new Error("This extension ctx is stale");
                    return Reflect.get(target, property, receiver);
                },
            },
        );
        await command.handler(args, ctx);
        return entries.at(-1) as CtxStatusEntryData;
    };
    return {
        kernel: resolver.kernel,
        entries,
        run,
        confirmations,
        setAnswer: (value: boolean) => {
            answer = value;
        },
    };
}

describe("Pi /ctx-memory-mark", () => {
    it("applies a tightening on a labeled memory without a dialog", async () => {
        const h = harness();
        h.kernel.seedDecision({
            object_id: "mem_rule",
            decision_kind: "PROJECT_RULES",
            summary: "r",
        });
        const entry = await h.run("mark_disputed mem_rule");
        expect(h.confirmations).toEqual([]);
        expect(entry.level).toBe("success");
        expect(entry.text).toContain("active -> disputed");
        expect(h.kernel.objects.get("mem_rule")?.disposition).toBe("disputed");
    });

    it("asks through the dialog when a served surface would change; declining writes nothing", async () => {
        const h = harness();
        h.kernel.seedDecision({
            object_id: "mem_verified",
            decision_kind: "PROJECT_RULES",
            summary: "v",
            labeled: false,
        });
        h.setAnswer(false);
        const declined = await h.run("quarantine mem_verified");
        expect(h.confirmations).toHaveLength(1);
        expect(h.confirmations[0]?.message).toContain("auto_inject: visible -> hidden");
        expect(declined.level).toBe("warning");
        expect(declined.text).toContain("Not Applied");
        expect(h.kernel.receipts.size).toBe(0);

        h.setAnswer(true);
        const applied = await h.run("quarantine mem_verified");
        expect(applied.level).toBe("success");
        expect(applied.text).toContain("receipt #");
        expect(h.kernel.objects.get("mem_verified")?.disposition).toBe("quarantined");
    });

    it("writes nothing and answers nowhere when the session is replaced while the dialog is open", async () => {
        const h = harness();
        h.kernel.seedDecision({
            object_id: "mem_verified",
            decision_kind: "PROJECT_RULES",
            summary: "v",
            labeled: false,
        });
        h.setAnswer(true);
        await h.run("quarantine mem_verified", true, true);
        expect(h.confirmations).toHaveLength(1);
        expect(h.kernel.receipts.size).toBe(0);
        expect(h.kernel.objects.get("mem_verified")?.disposition).toBe("active");
        expect(h.entries).toEqual([]);
    });

    it("falls back to the confirm flag when no dialog is available", async () => {
        const h = harness();
        h.kernel.seedDecision({
            object_id: "mem_verified",
            decision_kind: "PROJECT_RULES",
            summary: "v",
            labeled: false,
        });
        const pending = await h.run("mark_stale mem_verified", false);
        expect(h.confirmations).toEqual([]);
        expect(pending.text).toContain("--yes");
        expect(h.kernel.receipts.size).toBe(0);
        const applied = await h.run("mark_stale mem_verified --yes", false);
        expect(applied.level).toBe("success");
    });

    it("reports a denied relaxation and malformed arguments", async () => {
        const h = harness();
        h.kernel.seedDecision({
            object_id: "mem_q",
            decision_kind: "PROJECT_RULES",
            summary: "q",
            disposition: "quarantined",
        });
        const denied = await h.run("mark_stale mem_q");
        expect(denied.level).toBe("warning");
        expect(denied.text).toContain("Denied");
        expect(h.kernel.objects.get("mem_q")?.disposition).toBe("quarantined");

        const malformed = await h.run("stale mem_q");
        expect(malformed.level).toBe("error");
        expect(malformed.text).toContain("Invalid Arguments");
    });
});
