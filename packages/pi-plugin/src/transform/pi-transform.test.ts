import { describe, expect, it } from "bun:test";
import { type SessionEntry, SessionManager } from "@earendil-works/pi-coding-agent";
import { TransformCaptureAdmission } from "@eidnara/opencode/hooks/context/transform-capture";
import type { RustModeModuleClient } from "@eidnara/opencode/hooks/context/transform-session-client";

import { PiBranchIndex, type PiBranchReader } from "./pi-branch";
import { createPiTransform, PiAlignment, reservedPiId } from "./pi-transform";

type Json = Record<string, unknown>;

let clock = 1_700_000_000_000;
const user = (text: string) => ({ role: "user" as const, content: text, timestamp: clock++ });
const assistant = (text: string, stopReason: "stop" | "error" = "stop") => ({
    role: "assistant" as const,
    content: [{ type: "text" as const, text }],
    api: "faux",
    provider: "faux",
    model: "faux-model",
    usage: {
        input: 10,
        output: 1,
        cacheRead: 0,
        cacheWrite: 0,
        totalTokens: 11,
        cost: { input: 0, output: 0, cacheRead: 0, cacheWrite: 0, total: 0 },
    },
    stopReason,
    timestamp: clock++,
});

/** Models Pi's agent context by filtering assistant messages with `stopReason: "error"`. */
function agentMessages(manager: SessionManager): Json[] {
    return (manager.buildSessionContext().messages as unknown as Json[]).filter(
        (message) => !(message.role === "assistant" && message.stopReason === "error"),
    );
}

function idsOf(messages: readonly Json[], manager: PiBranchReader, branch = new PiBranchIndex()) {
    branch.sync(manager);
    const alignment = new PiAlignment(messages, branch, manager);
    return messages.map((_, index) => alignment.idAt(index));
}

describe("Pi id alignment", () => {
    it("names every message by its persisted entry id, identically after an index rebuild", () => {
        const manager = SessionManager.inMemory("/project");
        const ids = [
            manager.appendMessage(user("one")),
            manager.appendMessage(assistant("two")),
            manager.appendMessage(user("three")),
        ];
        manager.appendModelChange("faux", "faux-model");
        ids.push(manager.appendMessage(assistant("four")));
        const messages = agentMessages(manager);
        expect(idsOf(messages, manager)).toEqual(ids);
        expect(idsOf(structuredClone(messages), manager)).toEqual(ids);
    });

    it("skips a failed response Pi dropped before its retry, so no later id shifts", () => {
        const manager = SessionManager.inMemory("/project");
        const first = manager.appendMessage(user("question"));
        manager.appendMessage(assistant("overloaded", "error"));
        const retry = manager.appendMessage(assistant("answer"));
        const next = manager.appendMessage(user("follow-up"));
        expect(idsOf(agentMessages(manager), manager)).toEqual([first, retry, next]);
    });

    it("gives the latest compaction, a branch summary, and a custom message stable reserved ids", () => {
        const manager = SessionManager.inMemory("/project");
        const old = manager.appendMessage(user("old"));
        const kept = manager.appendMessage(assistant("kept"));
        const compaction = manager.appendCompaction("summary", kept, 100);
        const custom = manager.appendCustomMessageEntry("note", "custom text", true);
        const after = manager.appendMessage(user("after"));
        manager.branchWithSummary(after, "left a branch");
        const summary = manager.getLeafId() as string;
        const last = manager.appendMessage(user("last"));
        const messages = agentMessages(manager);
        expect(messages.map((message) => message.role)).toEqual([
            "compactionSummary",
            "assistant",
            "custom",
            "user",
            "branchSummary",
            "user",
        ]);
        const expected = [
            reservedPiId("compactionSummary", compaction),
            kept,
            reservedPiId("custom", custom),
            after,
            reservedPiId("branchSummary", summary),
            last,
        ];
        expect(idsOf(messages, manager)).toEqual(expected);
        expect(idsOf(messages, manager)).toEqual(expected);
        expect(expected).not.toContain(old);
    });

    it("leaves every slot before a mismatch unnamed", () => {
        const manager = SessionManager.inMemory("/project");
        manager.appendMessage(user("one"));
        const two = manager.appendMessage(user("two"));
        const messages = agentMessages(manager);
        messages[0] = { ...messages[0], timestamp: 1 };
        expect(idsOf(messages, manager)).toEqual([undefined, two]);
    });
});

function syntheticBranch(count: number): {
    reader: PiBranchReader & { reads: number };
    messages: Json[];
    append(): void;
} {
    const entries = new Map<string, SessionEntry>();
    const messages: Json[] = [];
    let leaf: string | null = null;
    const append = () => {
        const n = entries.size + 1;
        const message = { role: n % 2 ? "user" : "assistant", content: `m${n}`, timestamp: n };
        const id = `e${n}`;
        entries.set(id, {
            type: "message",
            id,
            parentId: leaf,
            timestamp: new Date(n).toISOString(),
            message,
        } as unknown as SessionEntry);
        messages.push(message);
        leaf = id;
    };
    for (let index = 0; index < count; index += 1) append();
    const reader = {
        reads: 0,
        getLeafId: () => leaf,
        getEntry(id: string) {
            reader.reads += 1;
            return entries.get(id);
        },
    };
    return { reader, messages, append };
}

describe("Pi per-turn work", () => {
    it.each([
        10_000, 100_000, 1_000_000,
    ])("is bounded by the new entries and the window at %i entries", (count) => {
        const { reader, messages, append } = syntheticBranch(count);
        const branch = new PiBranchIndex();
        expect(branch.sync(reader)).toEqual({ visited: count, rebuilt: true });
        for (let index = 0; index < 3; index += 1) append();
        reader.reads = 0;
        expect(branch.sync(reader)).toEqual({ visited: 3, rebuilt: false });
        expect(reader.reads).toBe(3);
        reader.reads = 0;
        const alignment = new PiAlignment(messages, branch, reader);
        const window = 300;
        expect(alignment.idAt(messages.length - window)).toBe(`e${count + 3 - window + 1}`);
        expect(alignment.entriesRead).toBe(window);
        expect(reader.reads).toBe(window);
    }, 60_000);

    it("rewalks only from a navigated leaf to the branch point", () => {
        const manager = SessionManager.inMemory("/project");
        const ids = Array.from({ length: 6 }, (_, n) => manager.appendMessage(user(`m${n}`)));
        const branch = new PiBranchIndex();
        branch.sync(manager);
        manager.branch(ids[2] as string);
        expect(branch.sync(manager)).toEqual({ visited: 0, rebuilt: false });
        expect(branch.length).toBe(3);
        const fresh = manager.appendMessage(user("fresh"));
        expect(branch.sync(manager)).toEqual({ visited: 1, rebuilt: false });
        expect(idsOf(agentMessages(manager), manager, branch)).toEqual([...ids.slice(0, 3), fresh]);
    });
});

type Reply = (body: Json) => unknown;

function fakeTransport(replies: Record<string, Reply[]>) {
    const calls: { method: string; body: Json }[] = [];
    const client: RustModeModuleClient = {
        async call({ method, body }) {
            calls.push({ method, body: body as Json });
            const reply = replies[method]?.shift();
            if (!reply) throw new Error(`no scripted reply for ${method}`);
            return reply(body as Json);
        },
    };
    return { client, calls };
}

const m0 = { id: "eidnara:synthetic:m0", message: { role: "user", content: "m0", timestamp: 0 } };

function foldReply(boundary: { mid: string; sequence: number }): Reply {
    return (body) => ({
        status: "ok",
        action: "HARD",
        boundary,
        base_revision: body.base_revision,
        output_revision: `out-${String(body.base_revision)}`,
        operations: [
            { op: "insert", values: [m0] },
            {
                op: "keep",
                source: "input",
                start: 0,
                count: (body.native_messages as unknown[]).length,
            },
        ],
    });
}

describe("Pi publication by return", () => {
    function session() {
        const manager = SessionManager.inMemory("/project");
        manager.appendMessage(user("old"));
        const kept = manager.appendMessage(assistant("kept"));
        manager.appendCompaction("pi summary", kept, 100);
        const head = manager.appendMessage(user("head"));
        manager.appendMessage(assistant("reply"));
        manager.appendMessage(user("now"));
        return { manager, head };
    }

    it("returns the applied values over the captured window and nothing before its head", async () => {
        const { manager, head } = session();
        const messages = agentMessages(manager);
        const transport = fakeTransport({
            "transform.boundary": [() => ({ anchors: [{ mid: head, sequence: 2 }] })],
            transform: [foldReply({ mid: head, sequence: 2 })],
        });
        const transform = createPiTransform({
            moduleClient: transport.client,
            captureAdmission: new TransformCaptureAdmission(),
        });
        const result = await transform.run({
            sessionId: "ses",
            messages,
            reader: manager,
            projectRoot: "/project",
            contextLimit: undefined,
            fields: () => ({ model_key: "faux/faux-model" }),
        });
        expect(result.outcome).toEqual({ kind: "applied", boundary: { mid: head, sequence: 2 } });
        const sent = transport.calls.find((call) => call.method === "transform")?.body ?? {};
        expect(sent.serializer_profile).toBe("pi");
        expect((sent.native_messages as { id: string }[]).map((row) => row.id)[0]).toBe(head);
        expect(result.messages).toEqual([m0.message, ...messages.slice(2)]);
        expect(JSON.stringify(result.messages)).not.toContain("pi summary");
    });

    it("returns nothing when the pass declines, so Pi keeps its array", async () => {
        const { manager } = session();
        const transport = fakeTransport({
            "transform.boundary": [
                () => {
                    throw new Error("daemon unavailable");
                },
            ],
        });
        const transform = createPiTransform({
            moduleClient: transport.client,
            captureAdmission: new TransformCaptureAdmission(),
        });
        const result = await transform.run({
            sessionId: "ses",
            messages: agentMessages(manager),
            reader: manager,
            projectRoot: "/project",
            contextLimit: undefined,
            fields: () => ({}),
        });
        expect(result.outcome?.kind).toBe("declined");
        expect(result.messages).toBeUndefined();
    });
});
