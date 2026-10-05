import { describe, expect, it } from "bun:test";
import { type SessionEntry, SessionManager } from "@earendil-works/pi-coding-agent";
import { TransformCaptureAdmission } from "@eidnara/opencode/hooks/context/transform-capture";
import type { RustModeModuleClient } from "@eidnara/opencode/hooks/context/transform-session-client";
import { HALF_CAP_BLOCKS } from "@eidnara/opencode/hooks/context/window-cap";

import { PiBranchIndex, type PiBranchReader } from "./pi-branch";
import { type PiRow, piRowSize } from "./pi-ck";
import { createPiTransform, PiAlignment } from "./pi-transform";

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

    it("gives the latest compaction, a branch summary, and custom messages stable reserved ids", () => {
        const manager = SessionManager.inMemory("/project");
        const old = manager.appendMessage(user("old"));
        const first = manager.appendCompaction("first summary", old, 100);
        const kept = manager.appendMessage(assistant("kept"));
        const latest = manager.appendCompaction("latest summary", kept, 100);
        const note = manager.appendCustomMessageEntry("note", "custom text", true);
        const other = manager.appendCustomMessageEntry("note", "other text", true);
        const after = manager.appendMessage(user("after"));
        manager.branchWithSummary(after, "left a branch");
        const summary = manager.getLeafId() as string;
        const last = manager.appendMessage(user("last"));
        const messages = agentMessages(manager);
        expect(messages.map((message) => message.role)).toEqual([
            "compactionSummary",
            "assistant",
            "custom",
            "custom",
            "user",
            "branchSummary",
            "user",
        ]);
        const expected = [
            `eidnara:compactionSummary:${latest}`,
            kept,
            `eidnara:custom:${note}`,
            `eidnara:custom:${other}`,
            after,
            `eidnara:branchSummary:${summary}`,
            last,
        ];
        expect(idsOf(messages, manager)).toEqual(expected);
        expect(idsOf(structuredClone(messages), manager)).toEqual(expected);
        expect(idsOf(messages, manager)).not.toContain(`eidnara:compactionSummary:${first}`);
    });

    it("matches a live custom message stamped before its entry was written", () => {
        const manager = SessionManager.inMemory("/project");
        const question = manager.appendMessage(user("question"));
        const note = manager.appendCustomMessageEntry(
            "note",
            [{ type: "text", text: "hint" }],
            false,
        );
        const messages = agentMessages(manager);
        // Pi stamps a live custom message when it is created, before the entry is appended.
        messages[1] = { ...messages[1], timestamp: (messages[1]?.timestamp as number) - 5 };
        expect(idsOf(messages, manager)).toEqual([question, `eidnara:custom:${note}`]);
    });

    it("leaves a custom message unnamed when its type or content differs from the entry", () => {
        for (const change of [{ customType: "other" }, { content: "edited" }]) {
            const manager = SessionManager.inMemory("/project");
            manager.appendMessage(user("question"));
            manager.appendCustomMessageEntry("note", "custom text", true);
            const last = manager.appendMessage(user("after"));
            const messages = agentMessages(manager);
            messages[1] = { ...messages[1], ...change };
            expect(idsOf(messages, manager)).toEqual([undefined, undefined, last]);
        }
    });

    it("leaves a branch summary unnamed when its summary or origin differs from the entry", () => {
        for (const change of [{ summary: "other summary" }, { fromId: "elsewhere" }]) {
            const manager = SessionManager.inMemory("/project");
            const question = manager.appendMessage(user("question"));
            manager.branchWithSummary(question, "left a branch");
            const last = manager.appendMessage(user("after"));
            const messages = agentMessages(manager);
            expect(messages.map((message) => message.role)).toEqual([
                "user",
                "branchSummary",
                "user",
            ]);
            messages[1] = { ...messages[1], ...change };
            expect(idsOf(messages, manager)).toEqual([undefined, undefined, last]);
        }
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
    navigate(n: number): void;
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
    const navigate = (n: number) => {
        leaf = `e${n}`;
        messages.length = n;
    };
    return { reader, messages, append, navigate };
}

describe("Pi per-turn work", () => {
    it.each([
        10_000, 100_000, 1_000_000,
    ])("is bounded by the new entries and the window at %i entries", (count) => {
        const { reader, messages, append } = syntheticBranch(count);
        const branch = new PiBranchIndex();
        expect(branch.sync(reader)).toBe(count);
        for (let index = 0; index < 3; index += 1) append();
        reader.reads = 0;
        expect(branch.sync(reader)).toBe(3);
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
        expect(branch.sync(manager)).toBe(0);
        expect(branch.length).toBe(3);
        const fresh = manager.appendMessage(user("fresh"));
        expect(branch.sync(manager)).toBe(1);
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

function passOver(
    manager: PiBranchReader,
    messages: readonly Json[],
    branch = new PiBranchIndex(),
) {
    return {
        sessionId: "ses",
        messages,
        reader: manager,
        projectRoot: "/project",
        contextLimit: undefined,
        fields: () => ({ model_key: "faux/faux-model" }),
        branch,
    };
}

function transformOver(client: RustModeModuleClient, branch: PiBranchIndex) {
    return createPiTransform({
        moduleClient: client,
        branchOf: () => branch,
        captureAdmission: new TransformCaptureAdmission(),
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
        const pass = passOver(manager, messages);
        const result = await transformOver(transport.client, pass.branch).run(pass);
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
        const pass = passOver(manager, agentMessages(manager));
        const result = await transformOver(transport.client, pass.branch).run(pass);
        expect(result.outcome?.kind).toBe("declined");
        expect(result.messages).toBeUndefined();
    });

    it("returns nothing when a pass after an applied one declines", async () => {
        const { manager, head } = session();
        const transport = fakeTransport({
            "transform.boundary": [() => ({ anchors: [{ mid: head, sequence: 2 }] })],
            transform: [
                foldReply({ mid: head, sequence: 2 }),
                () => ({ status: "error", code: "session_busy", message: "busy" }),
            ],
        });
        const branch = new PiBranchIndex();
        const transform = transformOver(transport.client, branch);
        const first = await transform.run(passOver(manager, agentMessages(manager), branch));
        expect(first.outcome?.kind).toBe("applied");
        manager.appendMessage(assistant("more"));
        const second = await transform.run(passOver(manager, agentMessages(manager), branch));
        expect(second.outcome).toEqual({ kind: "declined", servedLastApplied: false });
        expect(second.messages).toBeUndefined();
    });

    it.each([
        10_000, 1_000_000,
    ])("reads only new entries and the window at %i entries", async (count) => {
        const { reader, messages, append } = syntheticBranch(count);
        const window = 300;
        const anchor = { mid: `e${count - window + 1}`, sequence: 7 };
        const transport = fakeTransport({
            "transform.boundary": [() => ({ anchors: [anchor] })],
            transform: [foldReply(anchor), foldReply(anchor)],
        });
        const branch = new PiBranchIndex();
        const transform = transformOver(transport.client, branch);
        await transform.run(passOver(reader, messages, branch));
        append();
        append();
        const steady = await transform.run(passOver(reader, messages, branch));
        expect(steady.outcome?.kind).toBe("applied");
        expect(steady.branchEntriesVisited).toBe(2);
        expect(steady.entriesAligned).toBe(window + 2);
        const sent = transport.calls.filter((call) => call.method === "transform").at(-1)?.body;
        expect((sent?.native_messages as unknown[]).length).toBe(window + 2);
    }, 60_000);

    it.each([
        10_000, 1_000_000,
    ])("aligns only the window after a navigation drops the boundary at %i entries", async (count) => {
        const { reader, messages, navigate } = syntheticBranch(count);
        const window = 300;
        const newer = { mid: `e${count - window + 1}`, sequence: 8 };
        const older = { mid: `e${count - 2 * window + 1}`, sequence: 7 };
        const transport = fakeTransport({
            "transform.boundary": [
                () => ({ anchors: [newer, older] }),
                () => ({ anchors: [newer, older] }),
            ],
            transform: [foldReply(newer), foldReply(older)],
        });
        const branch = new PiBranchIndex();
        const transform = transformOver(transport.client, branch);
        expect((await transform.run(passOver(reader, messages, branch))).outcome?.kind).toBe(
            "applied",
        );
        navigate(count - window);
        const navigated = await transform.run(passOver(reader, messages, branch));
        expect(navigated.outcome).toEqual({ kind: "applied", boundary: older });
        expect(navigated.entriesAligned).toBe(window);
        const sent = transport.calls.filter((call) => call.method === "transform").at(-1)?.body;
        expect((sent?.native_messages as { id: string }[])[0]?.id).toBe(older.mid);
    }, 60_000);
});

function branchOver(messages: readonly Json[]): PiBranchReader {
    const entries = new Map<string, SessionEntry>();
    let leaf: string | null = null;
    for (const [index, message] of messages.entries()) {
        const id = `e${index + 1}`;
        entries.set(id, {
            type: "message",
            id,
            parentId: leaf,
            timestamp: new Date(index + 1).toISOString(),
            message,
        } as unknown as SessionEntry);
        leaf = id;
    }
    return { getLeafId: () => leaf, getEntry: (id) => entries.get(id) };
}

const toolCall = (id: string) => ({
    ...assistant(""),
    content: [{ type: "toolCall", id, name: "read", arguments: { path: "a.ts" } }],
    stopReason: "toolUse",
});
const toolResult = (id: string) => ({
    role: "toolResult",
    toolCallId: id,
    toolName: "read",
    content: [{ type: "text", text: "ok" }],
    isError: false,
    timestamp: clock++,
});

function rawReply(body: Json) {
    return {
        status: "ok",
        action: "SOFT",
        boundary: null,
        base_revision: body.base_revision,
        output_revision: `raw-${String(body.base_revision)}`,
        operations: [
            {
                op: "keep",
                source: "input",
                start: 0,
                count: (body.native_messages as unknown[]).length,
            },
        ],
    };
}

async function coldWindow(messages: Json[]): Promise<{ id: string; role: unknown }[]> {
    const transport = fakeTransport({
        "transform.boundary": [() => ({ anchors: [] })],
        transform: [rawReply],
    });
    const reader = branchOver(messages);
    const pass = passOver(reader, messages);
    const result = await transformOver(transport.client, pass.branch).run(pass);
    expect(result.outcome?.kind).toBe("applied");
    const sent = transport.calls.find((call) => call.method === "transform")?.body ?? {};
    return (sent.native_messages as PiRow[]).map((row) => ({
        id: row.id,
        role: row.message.role,
    }));
}

describe("Pi cold import", () => {
    it("starts its suffix after a tool result whose call the half cap leaves out", async () => {
        const newest = Array.from({ length: HALF_CAP_BLOCKS - 1 }, (_, n) => user(`new ${n}`));
        const result = toolResult("call-split");
        const call = toolCall("call-split");
        // The newest rows and the result fill the half cap exactly; the call would pass it.
        for (const row of [...newest, result, call])
            expect(piRowSize({ id: "x", message: row })?.blocks).toBe(1);
        const older = Array.from({ length: 300 }, (_, n) => user(`old ${n}`));
        const messages: Json[] = [...older, call, result, ...newest];

        const sent = await coldWindow(messages);
        expect(sent[0]?.role).not.toBe("toolResult");
        expect(sent[0]?.id).toBe(`e${older.length + 3}`);
        expect(sent).toHaveLength(newest.length);
    });

    it("keeps a newest run of tool results past half the cap with their call", async () => {
        const calls = Array.from({ length: HALF_CAP_BLOCKS + 1 }, (_, n) => `call-${n}`);
        const call = {
            ...assistant(""),
            content: calls.map((id) => ({ type: "toolCall", id, name: "read", arguments: {} })),
            stopReason: "toolUse",
        };
        const messages: Json[] = [
            ...Array.from({ length: 10 }, (_, n) => user(`m${n}`)),
            call,
            ...calls.map((id) => toolResult(id)),
        ];
        const sent = await coldWindow(messages);
        expect(sent[0]).toEqual({ id: "e11", role: "assistant" });
        expect(sent).toHaveLength(calls.length + 1);
    });
});

describe("Pi eviction at a reserved boundary row", () => {
    it.each([
        [
            "custom message",
            (manager: SessionManager) => manager.appendCustomMessageEntry("note", "hint", true),
            "custom",
        ],
        [
            "branch summary",
            (manager: SessionManager) => {
                manager.branchWithSummary(manager.getLeafId() as string, "left a branch");
                return manager.getLeafId() as string;
            },
            "branchSummary",
        ],
    ] as const)("evicts through a %s by its entry id", async (_, append, role) => {
        const manager = SessionManager.inMemory("/project");
        manager.appendMessage(user("one"));
        manager.appendMessage(assistant("two"));
        const entry = append(manager);
        manager.appendMessage(user("now"));
        const boundary = { mid: `eidnara:${role}:${entry}`, sequence: 3 };
        const transport = fakeTransport({
            "transform.boundary": [() => ({ anchors: [boundary] })],
            transform: [foldReply(boundary)],
        });
        const pass = passOver(manager, agentMessages(manager));
        const transform = transformOver(transport.client, pass.branch);
        const result = await transform.run(pass);
        expect(result.outcome).toEqual({ kind: "applied", boundary });

        expect(transform.eviction("ses", manager, 42)).toEqual({
            summary: "m0",
            firstKeptEntryId: entry,
            tokensBefore: 42,
            details: { sequence: 3 },
        });
        manager.appendCompaction("m0", entry, 42, { sequence: 3 }, true);
        expect(transform.eviction("ses", manager, 42)).toBeUndefined();
    });
});
