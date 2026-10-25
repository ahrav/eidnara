import { Database } from "bun:sqlite";
import { afterAll, describe, expect, it } from "bun:test";

import { join } from "node:path";

import { SOURCE_CORPUS_DIR } from "../source-corpus/corpus.ts";
import {
    freezeOpenCode,
    freezePi,
    frozenPiJsonl,
    loadFrozenOpenCode,
    OPENCODE_CLOSURE,
    PI_CLOSURE,
    requireOpenCodeClosure,
    snapshotStore,
} from "./freeze.ts";
import { CANARY } from "./workspace.ts";

const { mkdtempSync, readdirSync, readFileSync, rmSync } = process.getBuiltinModule("node:fs");
const { tmpdir } = process.getBuiltinModule("node:os");

const scratch: string[] = [];
afterAll(() => {
    for (const dir of scratch) rmSync(dir, { recursive: true, force: true });
});

const SESSION = "ses_0123456789abcdefghijABCD";
const ROOTS = [{ path: "/tmp/scratch-x/checkout", as: "/corpus/ripgrep" }];

function store(): Database {
    const db = new Database(":memory:");
    loadFrozenOpenCode(
        db,
        JSON.stringify({
            session_id: SESSION,
            messages: [
                {
                    id: "msg_0123456789abcdefghijZZZZ",
                    time_created: 2,
                    data: {
                        role: "user",
                        time: { created: 2 },
                        path: { cwd: "/tmp/scratch-x/checkout" },
                    },
                    parts: [
                        {
                            id: "prt_0123456789abcdefghijBBBB",
                            time_created: 2,
                            data: { type: "text", text: "hi" },
                        },
                    ],
                },
                {
                    id: "msg_0123456789abcdefghijAAAA",
                    time_created: 1,
                    data: { role: "assistant", time: { created: 1, completed: 3 } },
                    parts: [
                        {
                            id: "prt_0123456789abcdefghijAAAA",
                            time_created: 1,
                            data: {
                                type: "tool",
                                tool: "bash",
                                callID: "c1",
                                state: {
                                    status: "completed",
                                    output: `token ${CANARY}`,
                                    metadata: { output: CANARY },
                                },
                            },
                        },
                    ],
                },
            ],
        }),
    );
    return db;
}

const committed = (closure: string) =>
    readFileSync(join(SOURCE_CORPUS_DIR, "sessions", closure, "mix.json"), "utf8");

describe("corpus session freeze", () => {
    it("leaves the committed fixtures unchanged when freezing them again", () => {
        const opencode = committed(OPENCODE_CLOSURE);
        const db = new Database(":memory:");
        const sessionId = loadFrozenOpenCode(db, opencode);
        expect(freezeOpenCode(db, sessionId, [])).toBe(opencode);
        const pi = committed(PI_CLOSURE);
        expect(freezePi(frozenPiJsonl(pi), [])).toBe(pi);
    });

    it("freezes the same OpenCode store to the same bytes with paths and ids replaced", () => {
        const first = freezeOpenCode(store(), SESSION, ROOTS);
        expect(freezeOpenCode(store(), SESSION, ROOTS)).toBe(first);
        expect(first).not.toContain("/tmp/scratch-x");
        expect(first).toContain("/corpus/ripgrep");
        expect(first).not.toContain("0123456789abcdefghij");
        const frozen = JSON.parse(first);
        expect(frozen.session_id).toBe("ses_corpus0001");
        expect(frozen.messages.map((m: { id: string }) => m.id)).toEqual([
            "msg_corpus0001",
            "msg_corpus0002",
        ]);
        expect(frozen.messages[0].parts[0].data.canary).toBe(true);
        expect(frozen.messages[1].parts[0].data.canary).toBeUndefined();
    });

    it("refuses to freeze an OpenCode session the store does not hold", () => {
        const absent = "ses_absent0123456789abcdefgh";
        expect(() => freezeOpenCode(store(), absent, ROOTS)).toThrow(absent);
    });

    it("freezes the same Pi file to the same bytes and decodes back to its entries", () => {
        const jsonl = [
            JSON.stringify({ type: "session", id: "3f2c-real", cwd: "/tmp/scratch-x/checkout" }),
            JSON.stringify({
                type: "message",
                id: "a1b2c3d4",
                parentId: "3f2c-real",
                message: {
                    role: "toolResult",
                    toolCallId: "c",
                    toolName: "bash",
                    isError: false,
                    content: [{ type: "text", text: CANARY }],
                },
            }),
        ].join("\n");
        const first = freezePi(jsonl, ROOTS);
        expect(freezePi(jsonl, ROOTS)).toBe(first);
        expect(first).not.toContain("/tmp/scratch-x");
        const entries = frozenPiJsonl(first)
            .split("\n")
            .map((line) => JSON.parse(line));
        expect(entries.map((e) => [e.id, e.parentId])).toEqual([
            ["pi-corpus-session", undefined],
            ["e0001", "pi-corpus-session"],
        ]);
        expect(entries[1].message.content[0].canary).toBe(true);
    });

    it("replaces a kept store snapshot with the next run's store", () => {
        const dir = mkdtempSync(join(tmpdir(), "freeze-snapshot-"));
        scratch.push(dir);
        const target = join(dir, "opencode.db");
        for (const run of ["first", "second"]) {
            const db = new Database(":memory:");
            db.run("CREATE TABLE run (name TEXT)");
            db.prepare("INSERT INTO run VALUES (?)").run(run);
            snapshotStore(db, target);
            db.close();
        }
        const kept = new Database(target, { readonly: true });
        expect(kept.prepare("SELECT name FROM run").all()).toEqual([{ name: "second" }]);
        kept.close();
        expect(readdirSync(dir)).toEqual(["opencode.db"]);
    });

    it("accepts only the OpenCode version its closure names", () => {
        expect(requireOpenCodeClosure("1.18.22\n")).toBe("1.18.22");
        expect(requireOpenCodeClosure("opencode v1.18.22\n")).toBe("1.18.22");
        for (const reported of ["opencode v2.1.1\n", "1.18.220", "11.18.22", ""]) {
            expect(() => requireOpenCodeClosure(reported)).toThrow(OPENCODE_CLOSURE);
        }
    });
});
