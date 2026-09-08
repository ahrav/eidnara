/// <reference types="bun-types" />

import { describe, expect, it } from "bun:test";
import { readFileSync } from "node:fs";
import { join } from "node:path";
import {
    __moduleWireTest,
    buildPagedModuleTransformPayloads,
    encodeOpenCodeMessagesToCk,
    MODULE_PAGE_MAX_BYTES,
    resolveOrdinalsForModule,
} from "./module-wire";
import { setRawMessageProvider } from "./read-session-chunk";
import type { MessageLike } from "./tag-content-primitives";

describe("encodeOpenCodeMessagesToCk", () => {
    it("marks a collapsed synthetic todo pair as synthetic CK ingress", () => {
        const [encoded] = encodeOpenCodeMessagesToCk([
            {
                info: { id: "msg_synthetic_todo", role: "assistant" },
                parts: [
                    {
                        type: "tool",
                        tool: "todowrite",
                        callID: "eidnara_synthetic_todo_deadbeefdeadbeef",
                        syntheticTodoMarker: true,
                        state: {
                            status: "completed",
                            input: { todos: [] },
                            output: "[]",
                        },
                    },
                ],
            },
        ]);

        expect(encoded.ck.meta).toMatchObject({
            harness_id: "msg_synthetic_todo",
            synthetic: true,
        });
    });

    it("emits only a tool_result for a finished tool part that carries no input", () => {
        const kinds = encodeOpenCodeMessagesToCk([
            {
                info: { id: "asst", role: "assistant" },
                parts: [{ type: "tool", tool: "read", callID: "tc-1", state: { input: {} } }],
            },
            {
                info: { id: "user", role: "user" },
                parts: [
                    {
                        type: "tool",
                        tool: "read",
                        callID: "tc-1",
                        state: { status: "completed", output: "done" },
                    },
                ],
            },
        ]).map((message) =>
            (message.ck.content as Array<{ kind: { type: string } }>).map((c) => c.kind.type),
        );

        expect(kinds).toEqual([["tool_call"], ["tool_result"]]);
    });

    it("keeps the tool_call for an unfinished tool part without input", () => {
        const [encoded] = encodeOpenCodeMessagesToCk([
            {
                info: { id: "asst", role: "assistant" },
                parts: [
                    { type: "tool", tool: "read", callID: "tc-1", state: { status: "pending" } },
                ],
            },
        ]);

        expect(
            (encoded.ck.content as Array<{ kind: { type: string } }>).map((c) => c.kind.type),
        ).toEqual(["tool_call"]);
    });

    it("matches the module golden generated from raw OpenCode reasoning parts", () => {
        const golden = JSON.parse(
            readFileSync(
                join(
                    import.meta.dir,
                    "../../../../../crates/daemon/testdata/merged-reasoning-adapter-golden.json",
                ),
                "utf8",
            ),
        ) as {
            generator_version: number;
            cases: Array<{
                name: string;
                raw_messages: unknown[];
                encoded_input: unknown[];
            }>;
        };

        expect(golden.generator_version).toBe(1);
        expect(golden.cases.map((fixture) => fixture.name)).toEqual([
            "reasoning",
            "thinking",
            "redacted_thinking",
            "reasoning_cache_control",
        ]);
        for (const fixture of golden.cases) {
            expect(encodeOpenCodeMessagesToCk(fixture.raw_messages)).toEqual(fixture.encoded_input);
        }
    });
});

describe("resolveOrdinalsForModule provisional tails", () => {
    async function resolveTail(count: number) {
        const sessionId = `module-wire-provisional-${count}`;
        const persistedTail: Array<{
            id: string;
            timeCreated: number;
            contributesOrdinal: boolean;
            hasValidInfo: boolean;
        }> = [];
        const unregister = setRawMessageProvider(sessionId, {
            readMessages: () => persistedTail,
            readMessageOrdinalPage: (after, limit) =>
                persistedTail
                    .filter(
                        (row) =>
                            !after ||
                            row.timeCreated > after.timeCreated ||
                            (row.timeCreated === after.timeCreated && row.id > after.id),
                    )
                    .slice(0, limit),
            getStoredMessageCount: () => 500 + persistedTail.length,
        });
        const messages = Array.from({ length: count }, (_, index) => ({
            info: {
                id: `m-${501 + index}`,
                role: "user",
                sessionID: sessionId,
            },
            parts: [{ type: "text", text: `unpersisted ${index + 1}` }],
        })) as MessageLike[];
        const memo = new Map<string, number>([["m-500", 500]]);
        try {
            const first = await resolveOrdinalsForModule({
                sessionId,
                messages,
                memo: {
                    generation: 1,
                    memoGeneration: 1,
                    entries: memo,
                    anchor: { timeCreated: 500, id: "m-500" },
                    storedCount: 500,
                    canonicalCount: 500,
                },
                provisionalBase: 500,
            });
            expect(first.ok).toBe(true);
            if (!first.ok) throw new Error(first.reason);
            return { first, messages, memo, persistedTail, unregister, sessionId };
        } catch (error) {
            unregister();
            throw error;
        }
    }

    it("continues wholly fresh post-descent arrays from the durable provisional base", async () => {
        const sessionId = "module-wire-wholly-fresh-descent";
        const unregister = setRawMessageProvider(sessionId, {
            readMessages: () => [],
            readMessageOrdinalPage: () => [],
            getStoredMessageCount: () => 0,
        });
        const messages = [
            {
                info: { id: "summary", role: "user", sessionID: sessionId },
                parts: [{ type: "text", text: "continuation summary" }],
            },
            {
                info: { id: "tail", role: "assistant", sessionID: sessionId },
                parts: [{ type: "text", text: "continued answer" }],
            },
        ] as MessageLike[];
        try {
            const resolved = await resolveOrdinalsForModule({
                sessionId,
                messages,
                memo: {
                    generation: 1,
                    memoGeneration: 1,
                    entries: new Map(),
                    anchor: null,
                    storedCount: 0,
                    canonicalCount: 0,
                },
                provisionalBase: 97,
            });
            expect(resolved.ok).toBe(true);
            if (!resolved.ok) throw new Error(resolved.reason);
            expect(
                encodeOpenCodeMessagesToCk(resolved.annotatedInput as MessageLike[]).map(
                    (message) => message.ck.meta.ordinal,
                ),
            ).toEqual([98, 99]);
        } finally {
            unregister();
        }
    });

    it("assigns one unpersisted append the next absolute ordinal", async () => {
        const result = await resolveTail(1);
        try {
            expect(result.first.annotatedInput).toEqual([
                expect.objectContaining({ absolute_ordinal: 501 }),
            ]);
            expect(
                encodeOpenCodeMessagesToCk(result.first.annotatedInput as MessageLike[])[0]?.ck
                    .meta,
            ).toEqual(expect.objectContaining({ ordinal: 501 }));
        } finally {
            result.unregister();
        }
    });

    it("assigns two unpersisted appends distinct absolute ordinals", async () => {
        const result = await resolveTail(2);
        try {
            expect(
                (result.first.annotatedInput as Array<{ absolute_ordinal: number }>).map(
                    (message) => message.absolute_ordinal,
                ),
            ).toEqual([501, 502]);
            expect(
                encodeOpenCodeMessagesToCk(result.first.annotatedInput as MessageLike[]).map(
                    (message) => message.ck.meta.ordinal,
                ),
            ).toEqual([501, 502]);
        } finally {
            result.unregister();
        }
    });

    it("reconciles provisional ordinals when the appended rows persist", async () => {
        const result = await resolveTail(2);
        try {
            result.persistedTail.push(
                { id: "m-501", timeCreated: 501, contributesOrdinal: true, hasValidInfo: true },
                { id: "m-502", timeCreated: 502, contributesOrdinal: true, hasValidInfo: true },
            );
            const reconciled = await resolveOrdinalsForModule({
                sessionId: result.sessionId,
                messages: result.messages,
                memo: {
                    generation: 1,
                    memoGeneration: result.first.memoGeneration,
                    entries: result.memo,
                    anchor: result.first.memoAnchor,
                    storedCount: result.first.memoStoredCount,
                    canonicalCount: result.first.memoCanonicalCount,
                },
            });
            expect(reconciled.ok).toBe(true);
            if (reconciled.ok) {
                expect(reconciled.memoCanonicalCount).toBe(502);
                expect(result.memo.get("m-501")).toBe(501);
                expect(result.memo.get("m-502")).toBe(502);
            }
        } finally {
            result.unregister();
        }
    });
});

describe("buildPagedModuleTransformPayloads byte reuse", () => {
    it("returns the first stringify length on the unpaged path", () => {
        const body = {
            method: "transform",
            session_id: "ses-unpaged",
            input: [{ mid: "m1", ordinal: 1, ck: { text: "hi" } }],
        };
        const pages = buildPagedModuleTransformPayloads(body);
        expect(pages).toHaveLength(1);
        expect(pages[0]?.page).toBe(body);
        expect(pages[0]?.bytes).toBe(Buffer.byteLength(JSON.stringify(body)));
    });

    it("returns paging sizes that match a later stringify of each page", () => {
        const body = {
            method: "transform",
            session_id: "ses-paged",
            input: Array.from({ length: 80 }, (_, index) => ({
                mid: `m${index}`,
                ordinal: index + 1,
                ck: { text: "x".repeat(8_000) },
            })),
        };
        expect(Buffer.byteLength(JSON.stringify(body))).toBeGreaterThan(MODULE_PAGE_MAX_BYTES);
        const pages = buildPagedModuleTransformPayloads(body);
        expect(pages.length).toBeGreaterThan(1);
        for (const { page, bytes } of pages) {
            expect(bytes).toBe(Buffer.byteLength(JSON.stringify(page)));
        }
    });
});

describe("transform page array fields", () => {
    it("pins the pageable array field list against the daemon's Rust literal", () => {
        const rustSource = readFileSync(
            join(import.meta.dir, "../../../../../crates/daemon/src/lib.rs"),
            "utf8",
        );
        const match = rustSource.match(
            /const TRANSFORM_PAGE_ARRAY_FIELDS: \[&str; (\d+)\] = \[([^\]]*)\];/,
        );
        if (!match)
            throw new Error("TRANSFORM_PAGE_ARRAY_FIELDS not found in crates/daemon/src/lib.rs");
        const rustFields = [...match[2]!.matchAll(/"([a-z_]+)"/g)].map((entry) => entry[1]);
        expect(rustFields).toHaveLength(Number(match[1]));

        const body: Record<string, unknown> = { method: "transform", session_id: "s" };
        for (const field of rustFields) body[field] = [{ pad: "x".repeat(MODULE_PAGE_MAX_BYTES) }];
        const pages = buildPagedModuleTransformPayloads(body);
        expect(pages.length).toBeGreaterThan(1);
        for (const { page } of pages) {
            for (const field of rustFields) expect(Array.isArray(page[field])).toBe(true);
        }
        // Every field the daemon pages reaches a page; an oversize item is chunked into continuations.
        for (const field of rustFields) {
            expect(pages.some(({ page }) => (page[field] as unknown[]).length > 0)).toBe(true);
        }
        // A list field the daemon does not page rides as a scalar on the completing page only.
        const unpaged = buildPagedModuleTransformPayloads({ ...body, other_list: [1, 2, 3] });
        expect(unpaged.filter(({ page }) => "other_list" in page)).toHaveLength(1);
        expect(unpaged.at(-1)?.page.other_list).toEqual([1, 2, 3]);
        expect(__moduleWireTest.buildPagedModuleTransformPayloads).toBe(
            buildPagedModuleTransformPayloads,
        );
    });
});
