import { describe, expect, it, spyOn } from "bun:test";
import { type SessionEntry, SessionManager } from "@earendil-works/pi-coding-agent";
import {
    ReferenceableAllowance,
    TransformCaptureAdmission,
} from "@eidnara/opencode/hooks/context/transform-capture";
import type { RustModeModuleClient } from "@eidnara/opencode/hooks/context/transform-session-client";
import { HALF_CAP_BLOCKS } from "@eidnara/opencode/hooks/context/window-cap";

import { PiBranchIndex, type PiBranchReader } from "./pi-branch";
import { type PiRow, piRowSize } from "./pi-ck";
import { createPiTransform, entryRow, PiAlignment } from "./pi-transform";

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

    it("names a tool result by the entry that answers the same tool call", () => {
        const manager = SessionManager.inMemory("/project");
        manager.appendMessage(user("run both"));
        const at = clock++;
        const result = (toolCallId: string) => ({
            role: "toolResult" as const,
            toolCallId,
            toolName: "bash",
            content: [{ type: "text" as const, text: toolCallId }],
            isError: false,
            timestamp: at,
        });
        const first = manager.appendMessage(result("call-a"));
        const second = manager.appendMessage(result("call-b"));
        const messages = agentMessages(manager);
        expect(idsOf(messages, manager).slice(1)).toEqual([first, second]);
        // Two results of one millisecond that reach the handler in the other order take no
        // entry's id: each slot's entry answers another call.
        const swapped = [messages[0] as Json, messages[2] as Json, messages[1] as Json];
        expect(idsOf(swapped, manager)).toEqual([undefined, undefined, undefined]);
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
        eidnaraFolds: true,
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

describe("Pi native entry identity", () => {
    it("sends each row with its session entry's type and parentId", async () => {
        const messages: Json[] = [user("first"), toolCall("call-1"), toolResult("call-1")];
        const transport = fakeTransport({
            "transform.boundary": [() => ({ anchors: [] })],
            transform: [rawReply],
        });
        const reader = branchOver(messages);
        const pass = passOver(reader, messages);
        await transformOver(transport.client, pass.branch).run(pass);
        const sent = transport.calls.find((call) => call.method === "transform")?.body ?? {};
        expect(
            (sent.native_messages as PiRow[]).map(({ id, type, parentId }) => ({
                id,
                type,
                parentId,
            })),
        ).toEqual([
            { id: "e1", type: "message", parentId: null },
            { id: "e2", type: "message", parentId: "e1" },
            { id: "e3", type: "message", parentId: "e2" },
        ]);
    });

    it("sends a message a context handler changed without its entry's type and parentId", async () => {
        const call = {
            ...assistant(""),
            content: [
                { type: "toolCall", id: "call-1", name: "bash", arguments: { command: "ls" } },
            ],
            stopReason: "toolUse",
        };
        const persisted: Json[] = [user("first"), call, { ...toolResult("call-1"), isError: true }];
        const identities = async (context: Json[]) => {
            const transport = fakeTransport({
                "transform.boundary": [() => ({ anchors: [] })],
                transform: [rawReply],
            });
            const pass = passOver(branchOver(persisted), context);
            await transformOver(transport.client, pass.branch).run(pass);
            const sent = transport.calls.find((entry) => entry.method === "transform")?.body ?? {};
            return (sent.native_messages as PiRow[]).map((row) => ({
                id: row.id,
                type: row.type,
                parentId: row.parentId,
                message: row.message,
            }));
        };
        const whole = [
            { id: "e1", type: "message", parentId: null },
            { id: "e2", type: "message", parentId: "e1" },
            { id: "e3", type: "message", parentId: "e2" },
        ];
        const named = (rows: { id: string; type?: string; parentId?: string | null }[]) =>
            rows.map(({ id, type, parentId }) => ({ id, type, parentId }));
        // Pi hands a handler a clone of the array: equal messages keep their entries.
        expect(named(await identities(structuredClone(persisted)))).toEqual(whole);
        // The mark of the result alone, in either direction, and its text alone.
        const result = persisted[2] as Json;
        for (const edited of [
            { ...result, isError: false },
            { ...result, isError: undefined },
            { ...result, content: [{ type: "text", text: "pruned" }] },
        ]) {
            const rows = await identities([persisted[0] as Json, call, edited]);
            expect(named(rows)).toEqual([whole[0], whole[1], { id: "e3" }]);
            // The model's context keeps the handler's edit.
            expect(rows[2]?.message).toEqual(JSON.parse(JSON.stringify(edited)));
        }
        // The arguments of the call alone.
        const rerun = {
            ...call,
            content: [{ ...(call.content[0] as Json), arguments: { command: "rm -rf build" } }],
        };
        const rows = await identities([persisted[0] as Json, rerun, result]);
        expect(named(rows)).toEqual([whole[0], { id: "e2" }, whole[2]]);
    });

    it("keeps a row whose entry cannot be read to its id and message", () => {
        const messages: Json[] = [user("first")];
        const reader = branchOver(messages);
        const message = structuredClone(messages[0] as Json);
        const allowance = new ReferenceableAllowance();
        const row = entryRow("e1", message, reader, allowance);
        expect(row).toEqual({ id: "e1", type: "message", parentId: null, message });
        // The row holds the handler's message itself.
        expect(row.message).toBe(message);
        expect(entryRow("e9", message, reader, allowance)).toEqual({ id: "e9", message });
        expect(entryRow("eidnara:custom", message, reader, allowance)).toEqual({
            id: "eidnara:custom",
            message,
        });
    });

    const typed = (row: PiRow) => row.type !== undefined;
    const result = () => ({
        ...toolResult("call-1"),
        isError: true,
        details: { lines: [1, 2] },
        timestamp: 7,
    });
    const rowOf = (message: Json, held: Json, allowance = new ReferenceableAllowance()) =>
        entryRow("e1", message, branchOver([held]), allowance);

    it("reads a message as its entry's when both serialize to one JSON text", () => {
        const persisted = result();
        const row = (message: Json, held: Json = persisted) => rowOf(message, held);
        expect(typed(row(structuredClone(persisted)))).toBe(true);
        // A key that holds `undefined` is absent from the JSON copy an earlier handler returns.
        const sparse = { ...persisted, note: undefined };
        expect(typed(row(JSON.parse(JSON.stringify(sparse)), sparse))).toBe(true);
        expect(typed(row(structuredClone(persisted), sparse))).toBe(true);

        const changed: [string, Json][] = [
            ["a further key", { ...persisted, extra: 1 }],
            ["a missing key", { ...persisted, details: undefined }],
            ["another key under the same count", { ...persisted, details: { rows: [1, 2] } }],
            ["a further item", { ...persisted, details: { lines: [1, 2, 3] } }],
            ["a missing item", { ...persisted, details: { lines: [1] } }],
            ["keys in another order", { details: persisted.details, ...persisted }],
        ];
        for (const [label, message] of changed) {
            expect([label, typed(row(structuredClone(message)))]).toEqual([label, false]);
        }

        // The mark hidden from the request beside a further key: the request carries another
        // message than the entry's.
        const hidden: Json = { ...structuredClone(persisted), extra: true };
        Object.defineProperty(hidden, "isError", { value: true, enumerable: false });
        expect(JSON.parse(JSON.stringify(hidden)).isError).toBeUndefined();
        expect(typed(row(hidden))).toBe(false);
        // A key the entry's message holds as its own and the context message inherits.
        const inherited = JSON.parse('{"role":"user","content":"x","timestamp":1,"__proto__":{}}');
        expect(typed(row({ role: "user", content: "x", timestamp: 1 }, inherited))).toBe(false);
        expect(typed(row(structuredClone(inherited), inherited))).toBe(true);
    });

    it("keeps the type of an unedited clone whose entry holds frozen and null-prototype records", () => {
        // Pi's clone gives a frozen record writable fields and a null-prototype record the
        // plain prototype; the request carries the same text for both.
        const live: Json = result();
        live.details = Object.freeze({
            lines: Object.freeze([1, 2]),
            groups: Object.assign(Object.create(null), { failed: ["a"] }),
        });
        Object.defineProperty(live, "cache", { value: "hidden", enumerable: false });
        const row = rowOf(structuredClone(live), live);
        expect([row.type, row.parentId]).toEqual(["message", null]);
        // The control: the same clone with a changed item.
        const edited = structuredClone(live);
        ((edited.details as Json).groups as Json).failed = ["b"];
        expect(typed(rowOf(edited, live))).toBe(false);
        // A class instance in the live entry reads as changed: the walk admits plain records.
        class Details {
            lines = [1, 2];
        }
        const instanced: Json = { ...result(), details: new Details() };
        expect(JSON.stringify(structuredClone(instanced))).toBe(JSON.stringify(instanced));
        expect(typed(rowOf(structuredClone(instanced), instanced))).toBe(false);
    });

    it("reads a message with an array hole as changed, on either side", () => {
        const filled = () => ({
            ...result(),
            content: [
                { type: "text", text: "planted" },
                { type: "text", text: "ok" },
            ],
        });
        const holed = () => {
            const message = filled();
            delete message.content[0];
            return message;
        };
        // The entry holds the hole and a handler filled it; then the reverse; then both hold it.
        expect(typed(rowOf(filled(), holed()))).toBe(false);
        expect(typed(rowOf(holed(), filled()))).toBe(false);
        expect(typed(rowOf(holed(), holed()))).toBe(false);
        expect(typed(rowOf(filled(), filled()))).toBe(true);
    });

    it("ends the comparison of a cyclic, a deep, and a widely shared message as changed", () => {
        const cyclic = () => {
            const message: Json = result();
            (message.details as Json).self = message;
            return message;
        };
        expect(typed(rowOf(cyclic(), cyclic()))).toBe(false);

        const nested = (levels: number) => {
            const message: Json = result();
            let leaf = message.details as Json;
            for (let level = 0; level < levels; level += 1) {
                leaf.next = {};
                leaf = leaf.next as Json;
            }
            return message;
        };
        expect(typed(rowOf(nested(200), nested(200)))).toBe(true);
        expect(typed(rowOf(nested(300), nested(300)))).toBe(false);
        expect(typed(rowOf(nested(100_000), nested(100_000)))).toBe(false);

        // Sixty levels that each hold one child under two keys: 2^60 paths over 60 objects.
        const shared = () => {
            const message: Json = result();
            let node: Json = {};
            for (let level = 0; level < 60; level += 1) node = { left: node, right: node };
            message.details = node;
            return message;
        };
        const small = result();
        const stringify = spyOn(JSON, "stringify");
        const descriptors = spyOn(Object, "getOwnPropertyDescriptor");
        try {
            // One allowance serves the pass: the row that spends it and the rows after it read
            // as changed, and no text of them is built.
            const allowance = new ReferenceableAllowance(1 << 16);
            expect(typed(rowOf(shared(), shared(), allowance))).toBe(false);
            expect(typed(rowOf(structuredClone(small), small, allowance))).toBe(false);
            expect(stringify.mock.calls.length).toBe(0);
            expect(descriptors.mock.calls.length).toBeLessThan(10_000);
        } finally {
            stringify.mockRestore();
            descriptors.mockRestore();
        }
        // A fresh allowance reads the same small row as its entry's.
        expect(typed(rowOf(structuredClone(small), small))).toBe(true);
    });

    it("runs no code of a message it compares", () => {
        let runs = 0;
        const run = () => {
            runs += 1;
            return [{ type: "text", text: "ok" }];
        };
        const trapped = () =>
            Object.defineProperty(result(), "content", { enumerable: true, get: run }) as Json;
        const hooked = () => ({ ...result(), details: { toJSON: run } });
        for (const build of [trapped, hooked]) {
            expect(typed(rowOf(build(), result()))).toBe(false);
            expect(typed(rowOf(result(), build()))).toBe(false);
        }
        // A map under the plain prototype holds its items in an internal slot. The walk and the
        // request both read it as an empty record, so no item of it is read.
        const slotted = () => {
            const items = new Map([["item", Object.defineProperty({}, "value", { get: run })]]);
            return { ...result(), details: Object.setPrototypeOf(items, Object.prototype) };
        };
        const empty = { ...result(), details: {} };
        const row = rowOf(slotted(), empty);
        expect(typed(row)).toBe(true);
        expect(JSON.stringify(row.message)).toBe(JSON.stringify(empty));
        expect(runs).toBe(0);
    });

    it("tells a raw JSON value from a record of the same shape", () => {
        const raw = (JSON as unknown as { rawJSON?: (text: string) => unknown }).rawJSON;
        if (!raw) return;
        const branded = () => ({ ...result(), details: raw('"make"') });
        const lookalike = () => ({
            ...result(),
            details: Object.freeze(Object.assign(Object.create(null), { rawJSON: '"make"' })),
        });
        expect(JSON.stringify(branded())).not.toBe(JSON.stringify(lookalike()));
        expect(typed(rowOf(lookalike(), branded()))).toBe(false);
        expect(typed(rowOf(branded(), lookalike()))).toBe(false);
        expect(typed(rowOf(lookalike(), lookalike()))).toBe(true);
        // Two messages that hold the same raw text are one text on the wire.
        expect(typed(rowOf(branded(), branded()))).toBe(true);
    });

    describe("while the pass awaits", () => {
        const custom = { role: "custom", customType: "note", content: "kept", timestamp: 9 };
        const persisted = (): Json[] => [user("first"), toolCall("call-1"), result()];
        const reader = (messages: Json[]): PiBranchReader => {
            const base = branchOver(messages);
            const note = {
                type: "custom_message",
                id: "c4",
                parentId: "e3",
                timestamp: new Date(9).toISOString(),
                customType: "note",
                content: "kept",
            } as unknown as SessionEntry;
            return {
                getLeafId: () => "c4",
                getEntry: (id) => (id === "c4" ? note : base.getEntry(id)),
            };
        };
        /** Runs one pass over a clone of the session, with `during` as its `fields` step. */
        const pass = async (during: (context: Json[]) => void) => {
            const held = persisted();
            const context = [...structuredClone(held), structuredClone(custom)];
            const transport = fakeTransport({
                "transform.boundary": [() => ({ anchors: [] })],
                transform: [rawReply],
            });
            const inputs = {
                ...passOver(reader(held), context),
                fields: (messages: readonly Json[]) => {
                    during(messages as Json[]);
                    return { model_key: "faux/faux-model" };
                },
            };
            const outcome = await transformOver(transport.client, inputs.branch).run(inputs);
            const sent = transport.calls.filter((call) => call.method === "transform");
            return { outcome, sent, context, held };
        };
        /** The pass ends before a request is built: no transform is sent and Pi keeps its array. */
        const declinedUnsent = (ran: Awaited<ReturnType<typeof pass>>) => {
            expect(ran.outcome.outcome?.kind).toBe("declined");
            expect(ran.sent).toEqual([]);
            expect(ran.outcome.messages).toBeUndefined();
        };

        it("sends the typed rows of an unchanged window", async () => {
            const ran = await pass(() => {});
            expect(ran.outcome.outcome?.kind).toBe("applied");
            const rows = ran.sent[0]?.body.native_messages as PiRow[];
            expect(rows.map(({ id, type }) => [id, type])).toEqual([
                ["e1", "message"],
                ["e2", "message"],
                ["e3", "message"],
                ["eidnara:custom:c4", "custom_message"],
            ]);
            expect(rows[2]?.message).toEqual(ran.held[2] as Json);
        });

        it("declines when a message that carries its entry's type is edited in place", async () => {
            // An earlier handler that kept the result edits a nested text; then its mark.
            declinedUnsent(
                await pass((messages) => {
                    const part = (messages[2]?.content as Json[])[0] as Json;
                    part.text = "edited after the comparison";
                }),
            );
            declinedUnsent(
                await pass((messages) => {
                    (messages[2] as Json).isError = false;
                }),
            );
        });

        it("compares a row again when the pass reads its window a second time", async () => {
            const held = persisted();
            const context = structuredClone(held);
            // The daemon answers the first request with `boundary_unknown`; an earlier handler
            // edits the result while that request is under way, and the pass reads again.
            const transport = fakeTransport({
                "transform.boundary": [() => ({ anchors: [] }), () => ({ anchors: [] })],
                transform: [
                    () => {
                        (context[2] as Json).isError = false;
                        return { status: "boundary_unknown" };
                    },
                    rawReply,
                ],
            });
            const inputs = passOver(branchOver(held), context);
            const outcome = await transformOver(transport.client, inputs.branch).run(inputs);
            expect(outcome.outcome?.kind).toBe("applied");
            const sent = transport.calls.filter((call) => call.method === "transform");
            const types = (body: Json) =>
                (body.native_messages as PiRow[]).map(({ id, type }) => [id, type]);
            expect(sent.map((call) => types(call.body))).toEqual([
                [
                    ["e1", "message"],
                    ["e2", "message"],
                    ["e3", "message"],
                ],
                [
                    ["e1", "message"],
                    ["e2", "message"],
                    ["e3", undefined],
                ],
            ]);
        });

        it("declines when a custom message row is edited in place", async () => {
            declinedUnsent(
                await pass((messages) => {
                    (messages[3] as Json).content = "rewritten";
                }),
            );
        });

        it("declines when a serialization hook appears on a shared prototype", async () => {
            const plain = Object.prototype as { toJSON?: unknown };
            try {
                declinedUnsent(
                    await pass(() => {
                        Object.defineProperty(plain, "toJSON", {
                            configurable: true,
                            value: () => ({ role: "toolResult", isError: false }),
                        });
                    }),
                );
            } finally {
                delete plain.toJSON;
            }
        });
    });

    it("leaves the session's lease free when the first message's role cannot be read", async () => {
        const messages: Json[] = [user("first"), toolCall("call-1"), result()];
        const admission = new TransformCaptureAdmission();
        const transport = fakeTransport({
            "transform.boundary": [() => ({ anchors: [] })],
            transform: [rawReply],
        });
        const branch = new PiBranchIndex();
        const transform = createPiTransform({
            moduleClient: transport.client,
            branchOf: () => branch,
            captureAdmission: admission,
        });
        const thrown = [
            Object.defineProperty(structuredClone(messages[0] as Json), "role", {
                get() {
                    throw new Error("role");
                },
            }),
            ...structuredClone(messages.slice(1)),
        ];
        await expect(transform.run(passOver(branchOver(messages), thrown, branch))).rejects.toThrow(
            "role",
        );
        expect(admission.activePasses).toBe(0);
        const plain = await transform.run(
            passOver(branchOver(messages), structuredClone(messages), branch),
        );
        expect(plain.outcome?.kind).toBe("applied");
        expect(admission.activePasses).toBe(0);
    });

    it("reads a reserved row's entry by its entry id and copies its own type and parent", () => {
        const custom = { role: "custom", customType: "note", content: "x", timestamp: 9 };
        const entries = new Map<string, SessionEntry>([
            [
                "c7",
                {
                    type: "custom_message",
                    id: "c7",
                    parentId: "root-outside-window",
                    timestamp: new Date(9).toISOString(),
                    customType: "note",
                    content: "x",
                } as unknown as SessionEntry,
            ],
        ]);
        const reader: PiBranchReader = {
            getLeafId: () => "c7",
            getEntry: (id) => entries.get(id),
        };
        expect(entryRow("eidnara:custom:c7", custom, reader, new ReferenceableAllowance())).toEqual(
            {
                id: "eidnara:custom:c7",
                type: "custom_message",
                parentId: "root-outside-window",
                message: custom,
            },
        );
    });
});

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

    it("submits and publishes the whole array once the project withdraws fold authority after a pinned cold pass", async () => {
        const messages: Json[] = Array.from({ length: HALF_CAP_BLOCKS + 10 }, (_, n) =>
            user(`row ${n}`),
        );
        const transport = fakeTransport({
            "transform.boundary": [() => ({ anchors: [] }), () => ({ anchors: [] })],
            transform: [
                () => {
                    throw new Error("queue_full");
                },
                rawReply,
            ],
        });
        const reader = branchOver(messages);
        const pass = passOver(reader, messages);
        const transform = transformOver(transport.client, pass.branch);
        expect((await transform.run(pass)).messages).toBeUndefined();
        const folding = transport.calls.filter((call) => call.method === "transform")[0]?.body;
        expect(folding?.native_messages).toHaveLength(HALF_CAP_BLOCKS);

        const withdrawn = await transform.run({ ...pass, eidnaraFolds: false });
        expect(withdrawn.outcome?.kind).toBe("applied");
        const additive = transport.calls.filter((call) => call.method === "transform")[1]?.body;
        expect(additive?.native_messages).toHaveLength(messages.length);
        expect(withdrawn.messages).toEqual(messages);
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
