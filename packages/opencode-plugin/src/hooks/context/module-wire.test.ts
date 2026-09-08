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

    it("preserves an explicit zero absolute ordinal", () => {
        const [encoded] = encodeOpenCodeMessagesToCk([
            {
                info: { id: "msg_leading_synthetic", role: "user" },
                parts: [{ type: "text", text: "preamble", synthetic: true }],
                absolute_ordinal: 0,
            },
        ]);

        expect(encoded.ordinal).toBe(0);
        expect(encoded.ck.meta).toMatchObject({ ordinal: 0, synthetic: true });
    });

    it("propagates provider-executed metadata to the call and result blocks", () => {
        const [encoded] = encodeOpenCodeMessagesToCk([
            {
                info: { id: "msg_server_tool", role: "assistant" },
                parts: [
                    {
                        type: "tool",
                        tool: "web_search",
                        callID: "call_1",
                        metadata: { providerExecuted: true },
                        state: { status: "completed", input: { q: "x" }, output: "done" },
                    },
                    {
                        type: "tool",
                        tool: "read",
                        callID: "call_2",
                        state: { status: "completed", input: {}, output: "ok" },
                    },
                ],
            },
        ]);

        const kinds = (encoded.ck.content as Array<{ kind: Record<string, unknown> }>).map(
            (block) => block.kind,
        );
        expect(kinds[0]).toMatchObject({
            type: "tool_call",
            id: "call_1",
            provider_executed: true,
        });
        expect(kinds[1]).toMatchObject({
            type: "tool_result",
            id: "call_1",
            provider_executed: true,
        });
        expect(kinds[2]).toEqual({ type: "tool_call", id: "call_2", name: "read", input: {} });
        expect(kinds[3]).not.toHaveProperty("provider_executed");
    });

    it("encodes file and image parts as media blocks", () => {
        const [encoded] = encodeOpenCodeMessagesToCk([
            {
                info: { id: "msg_media", role: "user" },
                parts: [
                    {
                        type: "file",
                        mime: "image/png",
                        filename: "shot.png",
                        url: "data:image/png;base64,AAAA",
                    },
                    { type: "file", mime: "application/pdf", url: "https://example.test/a.pdf" },
                    { type: "image", mime: "image/jpeg", data: "BBBB" },
                    { type: "file", mime: "text/plain", name: "notes.txt" },
                ],
            },
        ]);

        expect(encoded.ck.content).toEqual([
            {
                kind: {
                    type: "media",
                    kind: "image",
                    media_type: "image/png",
                    filename: "shot.png",
                    source: { type: "data_base64", data: "AAAA" },
                },
            },
            {
                kind: {
                    type: "media",
                    kind: "document",
                    media_type: "application/pdf",
                    source: { type: "url", url: "https://example.test/a.pdf" },
                },
            },
            {
                kind: {
                    type: "media",
                    kind: "image",
                    media_type: "image/jpeg",
                    source: { type: "data_base64", data: "BBBB" },
                },
            },
            {
                kind: {
                    type: "media",
                    kind: "file",
                    media_type: "text/plain",
                    filename: "notes.txt",
                    source: {
                        type: "opaque",
                        raw: { type: "file", mime: "text/plain", name: "notes.txt" },
                    },
                },
            },
        ]);
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

describe("transform page digest canonical JSON", () => {
    it("renders numbers the way the daemon's canonical_number does", () => {
        // Expected strings match `canonical_number` in `crates/daemon/src/lib.rs`.
        const cases: Array<[number, string]> = [
            [1e-7, "0.0000001"],
            [1.5e-10, "0.00000000015"],
            [0.000001, "0.000001"],
            [0.1, "0.1"],
            [0.30000000000000004, "0.30000000000000004"],
            [123.456, "123.456"],
            [-1.5, "-1.5"],
            [100, "100"],
            [1, "1"],
            [1e21, "1000000000000000000000"],
            [1e23, "99999999999999991611392"],
            [9007199254740992, "9007199254740992"],
            [2 ** 64, "18446744073709551616"],
            [-(2 ** 63) - 2048, "-9223372036854777856"],
        ];
        for (const [value, expected] of cases) {
            expect(__moduleWireTest.canonicalJson(value)).toBe(expected);
        }
        expect(__moduleWireTest.canonicalJson({ b: [1e-7, "x"], a: null })).toBe(
            '{"a":null,"b":[0.0000001,"x"]}',
        );
    });
});

describe("synthetic message classification", () => {
    it("uses one predicate for the ordinal resolver and the CK encoder", async () => {
        const sessionId = "module-wire-mixed-synthetic";
        const unregister = setRawMessageProvider(sessionId, {
            readMessages: () => [],
            readMessageOrdinalPage: () => [],
            getStoredMessageCount: () => 2,
        });
        const messages = [
            {
                info: { id: "m-1", role: "user", sessionID: sessionId },
                parts: [{ type: "text", text: "first" }],
            },
            {
                info: { id: "injected", role: "user", sessionID: sessionId },
                parts: [
                    { type: "text", text: "authored", synthetic: false },
                    { type: "text", text: "marker", synthetic: true },
                ],
            },
            {
                info: { id: "m-2", role: "assistant", sessionID: sessionId },
                parts: [{ type: "text", text: "second" }],
            },
        ] as MessageLike[];
        try {
            const resolved = await resolveOrdinalsForModule({
                sessionId,
                messages,
                memo: {
                    generation: 1,
                    memoGeneration: 1,
                    entries: new Map([
                        ["m-1", 1],
                        ["m-2", 2],
                    ]),
                    anchor: { timeCreated: 2, id: "m-2" },
                    storedCount: 2,
                    canonicalCount: 2,
                },
            });
            // A mixed authored/synthetic message is not synthetic, so it cannot borrow an
            // ordinal and stays unresolved instead of reaching the daemon as a duplicate.
            expect(resolved).toEqual({
                ok: false,
                reason: "unresolved",
                messageId: "injected",
                messageIndex: 1,
                messageRole: "user",
            });
            expect(encodeOpenCodeMessagesToCk(messages)[1]?.ck.meta).toMatchObject({
                synthetic: false,
            });
        } finally {
            unregister();
        }
    });

    it("lets a wholly synthetic message borrow the preceding ordinal", async () => {
        const sessionId = "module-wire-wholly-synthetic";
        const unregister = setRawMessageProvider(sessionId, {
            readMessages: () => [],
            readMessageOrdinalPage: () => [],
            getStoredMessageCount: () => 2,
        });
        const messages = [
            {
                info: { id: "m-1", role: "user", sessionID: sessionId },
                parts: [{ type: "text", text: "first" }],
            },
            {
                info: { id: "injected", role: "user", sessionID: sessionId },
                parts: [{ type: "text", text: "marker", synthetic: true }],
            },
            {
                info: { id: "m-2", role: "assistant", sessionID: sessionId },
                parts: [{ type: "text", text: "second" }],
            },
        ] as MessageLike[];
        try {
            const resolved = await resolveOrdinalsForModule({
                sessionId,
                messages,
                memo: {
                    generation: 1,
                    memoGeneration: 1,
                    entries: new Map([
                        ["m-1", 1],
                        ["m-2", 2],
                    ]),
                    anchor: { timeCreated: 2, id: "m-2" },
                    storedCount: 2,
                    canonicalCount: 2,
                },
            });
            expect(resolved.ok).toBe(true);
            if (!resolved.ok) throw new Error(resolved.reason);
            const encoded = encodeOpenCodeMessagesToCk(resolved.annotatedInput);
            expect(encoded.map((message) => message.ordinal)).toEqual([1, 1, 2]);
            expect(
                encoded.map((message) => (message.ck.meta as { synthetic: boolean }).synthetic),
            ).toEqual([false, true, false]);
        } finally {
            unregister();
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
