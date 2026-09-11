/// <reference types="bun-types" />

import { describe, expect, it, spyOn } from "bun:test";
import { readFileSync } from "node:fs";
import { join } from "node:path";
import { isRecord } from "../../shared/record-type-guard";
import {
    __moduleWireTest,
    buildPagedModuleTransformPayloads,
    encodeOpenCodeMessagesToCk,
    MODULE_ITEM_CONTINUATION_KEY,
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

    it("emits only a tool_result for a later part that repeats an already-emitted call id", () => {
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

    it("keeps the tool_call for a first-seen unfinished tool part without input", () => {
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

    it("keeps non-object parts as unknown opaque blocks", () => {
        const [encoded] = encodeOpenCodeMessagesToCk([
            {
                info: { id: "msg_primitive_parts", role: "user" },
                parts: ["stray", 7, null, { type: "text", text: "real" }],
            },
        ]);
        expect(encoded.ck.content).toEqual([
            {
                kind: {
                    type: "opaque",
                    source: { type: "harness", harness: "opencode" },
                    kind: "unknown",
                    raw: "stray",
                },
            },
            {
                kind: {
                    type: "opaque",
                    source: { type: "harness", harness: "opencode" },
                    kind: "unknown",
                    raw: 7,
                },
            },
            {
                kind: {
                    type: "opaque",
                    source: { type: "harness", harness: "opencode" },
                    kind: "unknown",
                    raw: null,
                },
            },
            { kind: { type: "text", text: "real" } },
        ]);
    });

    it("reads the nested creation timestamp before the flat aliases", () => {
        const [nested, flat, none] = encodeOpenCodeMessagesToCk([
            {
                info: { id: "t1", role: "user", time: { created: 1700000000000, completed: 1 } },
                parts: [],
            },
            { info: { id: "t2", role: "user", time_created: 5 }, parts: [] },
            { info: { id: "t3", role: "user" }, parts: [] },
        ]);
        expect((nested.ck.meta as { created_at_ms?: number }).created_at_ms).toBe(1700000000000);
        expect((flat.ck.meta as { created_at_ms?: number }).created_at_ms).toBe(5);
        expect(none.ck.meta).not.toHaveProperty("created_at_ms");
    });

    it("preserves an explicitly empty reasoning signature", () => {
        const [encoded] = encodeOpenCodeMessagesToCk([
            {
                info: { id: "msg_empty_sig", role: "assistant" },
                parts: [{ type: "reasoning", text: "t", metadata: { signature: "" } }],
            },
        ]);
        expect((encoded.ck.content as Array<{ kind: Record<string, unknown> }>)[0]?.kind).toEqual({
            type: "reasoning",
            text: "t",
            signature: "",
        });
    });

    it("hashes fallback ids over the value JSON serialization emits", () => {
        const when = new Date("2026-01-02T03:04:05.000Z");
        const withToJson = encodeOpenCodeMessagesToCk([
            { info: { role: "user" }, parts: [], when, skip: () => 1 },
        ])[0];
        const asWire = encodeOpenCodeMessagesToCk([
            { info: { role: "user" }, parts: [], when: when.toISOString() },
        ])[0];
        expect(withToJson.mid).toBe(asWire.mid);
    });

    it("preserves an explicitly empty tool-call id as the daemon does", () => {
        const [encoded] = encodeOpenCodeMessagesToCk([
            {
                info: { id: "msg_empty_call", role: "assistant" },
                parts: [
                    {
                        type: "tool",
                        tool: "read",
                        callID: "",
                        id: "part_1",
                        state: { status: "pending" },
                    },
                ],
            },
        ]);
        expect((encoded.ck.content as Array<{ kind: { id: string } }>)[0]?.kind.id).toBe("");
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

    it("ignores explicit ordinals the daemon cannot read as u64", () => {
        const ordinals = encodeOpenCodeMessagesToCk([
            { info: { id: "o1", role: "user" }, parts: [], absolute_ordinal: 1e21 },
            { info: { id: "o2", role: "user" }, parts: [], absolute_ordinal: 2 ** 64 },
            { info: { id: "o3", role: "user" }, parts: [], absolute_ordinal: -1 },
            { info: { id: "o4", role: "user" }, parts: [], absolute_ordinal: 1.5 },
            { info: { id: "o5", role: "user" }, parts: [], absolute_ordinal: 2 ** 63 },
        ]).map((message) => message.ordinal);
        // The first four fall back to `index + 1`; 2^63 survives because its wire text fits u64.
        expect(ordinals).toEqual([1, 2, 3, 4, 2 ** 63]);
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

    it("honors the daemon's tool-name aliases and default", () => {
        const [encoded] = encodeOpenCodeMessagesToCk([
            {
                info: { id: "msg_tool_names", role: "assistant" },
                parts: [
                    {
                        type: "tool",
                        toolName: "todowrite",
                        callID: "c1",
                        state: { status: "pending" },
                    },
                    {
                        type: "tool",
                        name: "ctx_reduce",
                        callID: "c2",
                        state: { status: "pending" },
                    },
                    { type: "tool", callID: "c3", state: { status: "pending" } },
                ],
            },
        ]);
        const names = (encoded.ck.content as Array<{ kind: { name: string } }>).map(
            (block) => block.kind.name,
        );
        expect(names).toEqual(["todowrite", "ctx_reduce", "tool"]);
    });

    it("synthesizes the daemon's deterministic id for a tool part without one", () => {
        const [encoded] = encodeOpenCodeMessagesToCk([
            {
                info: { id: "msg_no_call_id", role: "assistant" },
                absolute_ordinal: 7,
                parts: [
                    { type: "text", text: "lead" },
                    {
                        type: "tool",
                        tool: "read",
                        state: {
                            status: "completed",
                            input: { path: "/tmp/x", limit: 10 },
                            output: "ok",
                        },
                    },
                ],
            },
        ]);
        const kinds = (encoded.ck.content as Array<{ kind: Record<string, unknown> }>).map(
            (block) => block.kind,
        );
        // `synth-tool-{ordinal}-{part_index}-{tool_name}-{stable_hash_prefix(input, 12)}`
        expect(kinds[1]).toMatchObject({
            type: "tool_call",
            id: "synth-tool-7-1-read-aac27fab24fb",
        });
        expect(kinds[2]).toMatchObject({
            type: "tool_result",
            id: "synth-tool-7-1-read-aac27fab24fb",
        });
    });

    it("reads a reasoning signature nested under metadata", () => {
        const [encoded] = encodeOpenCodeMessagesToCk([
            {
                info: { id: "msg_signed_reasoning", role: "assistant" },
                parts: [
                    {
                        type: "reasoning",
                        text: "thinking...",
                        metadata: { anthropic: { signature: "sig-nested" } },
                    },
                    { type: "reasoning", text: "adapter", signature: "sig-top-level" },
                    {
                        type: "reasoning",
                        text: "both",
                        signature: "sig-top-level",
                        metadata: { openai: { signature: "sig-metadata" } },
                    },
                ],
            },
        ]);
        const signatures = (encoded.ck.content as Array<{ kind: { signature?: string } }>).map(
            (block) => block.kind.signature,
        );
        expect(signatures).toEqual(["sig-nested", "sig-top-level", "sig-metadata"]);
    });

    it("reads completion status and output from top-level tool fields", () => {
        const [encoded] = encodeOpenCodeMessagesToCk([
            {
                info: { id: "msg_legacy_tool", role: "assistant" },
                parts: [
                    {
                        type: "tool",
                        tool: "bash",
                        callID: "call_legacy",
                        status: "completed",
                        input: { cmd: "ls" },
                        output: "a b c",
                    },
                    {
                        type: "tool",
                        tool: "bash",
                        callID: "call_failed",
                        status: "error",
                        error: "boom",
                    },
                ],
            },
        ]);

        const kinds = (encoded.ck.content as Array<{ kind: Record<string, unknown> }>).map(
            (block) => block.kind,
        );
        expect(kinds.map((kind) => kind.type)).toEqual([
            "tool_call",
            "tool_result",
            "tool_call",
            "tool_result",
        ]);
        expect(kinds[1]).toMatchObject({
            id: "call_legacy",
            output: { kind: { type: "text", text: "a b c" } },
        });
        expect(kinds[3]).toMatchObject({
            id: "call_failed",
            output: { kind: { type: "error_text", text: "boom" } },
        });
    });

    it("carries tool-result attachments as content result blocks", () => {
        const image = { type: "file", mime: "image/png", url: "data:image/png;base64,QUJD" };
        const other = { type: "note", body: "x" };
        const [encoded] = encodeOpenCodeMessagesToCk([
            {
                info: { id: "msg_attachments", role: "assistant" },
                parts: [
                    {
                        type: "tool",
                        tool: "read",
                        callID: "call_att",
                        state: {
                            status: "completed",
                            input: {},
                            output: "read it",
                            attachments: [image, other, "skipped", null],
                        },
                    },
                    {
                        type: "tool",
                        tool: "read",
                        callID: "call_att_err",
                        state: { status: "error", input: {}, error: "", attachments: [] },
                    },
                ],
            },
        ]);

        const kinds = (encoded.ck.content as Array<{ kind: Record<string, unknown> }>).map(
            (block) => block.kind,
        );
        expect(kinds[1]).toEqual({
            type: "tool_result",
            id: "call_att",
            tool_name: "read",
            output: {
                kind: {
                    type: "content",
                    blocks: [
                        { kind: { type: "text", text: "read it" } },
                        {
                            kind: {
                                type: "media",
                                media: {
                                    kind: "image",
                                    media_type: "image/png",
                                    source: { type: "data_base64", data: "QUJD" },
                                },
                            },
                            provider_extras: { opencode: { rawAttachment: image } },
                        },
                        {
                            kind: {
                                type: "opaque",
                                opaque: {
                                    source: { type: "harness", harness: "opencode" },
                                    kind: "note",
                                    raw: other,
                                },
                            },
                            provider_extras: { opencode: { rawAttachment: other } },
                        },
                    ],
                },
            },
        });
        expect(kinds[3]).toMatchObject({
            output: { kind: { type: "error_content", blocks: [] } },
        });
    });

    it("keeps step-finish parts as opaque blocks with the daemon's source shape", () => {
        const stepFinish = { type: "step-finish", reason: "stop", cost: 0.01 };
        const [encoded] = encodeOpenCodeMessagesToCk([
            {
                info: { id: "msg_step_finish", role: "assistant" },
                parts: [
                    { type: "text", text: "done" },
                    stepFinish,
                    { type: "snapshot", snapshot: "abc" },
                ],
            },
        ]);

        expect(encoded.ck.content).toEqual([
            { kind: { type: "text", text: "done" } },
            {
                kind: {
                    type: "opaque",
                    source: { type: "harness", harness: "opencode" },
                    kind: "step-finish",
                    raw: stepFinish,
                },
            },
        ]);
    });

    it("carries part metadata as the daemon's provider extras", () => {
        const reasoningMetadata = {
            openai: { reasoningEncryptedContent: "enc", itemId: "rs_1", signature: "sig-oa" },
        };
        const textMetadata = { anthropic: { cache: true } };
        const [encoded] = encodeOpenCodeMessagesToCk([
            {
                info: { id: "msg_metadata", role: "assistant" },
                parts: [
                    { type: "reasoning", text: "thinking", metadata: reasoningMetadata },
                    { type: "text", text: "answer", metadata: textMetadata },
                    { type: "text", text: "plain" },
                ],
            },
        ]);

        expect(encoded.ck.content).toEqual([
            {
                kind: { type: "reasoning", text: "thinking", signature: "sig-oa" },
                provider_extras: { opencode: { metadata: reasoningMetadata } },
            },
            {
                kind: { type: "text", text: "answer" },
                provider_extras: { opencode: { metadata: textMetadata } },
            },
            { kind: { type: "text", text: "plain" } },
        ]);
    });

    it("decodes an empty reasoning part with redacted data as redacted reasoning", () => {
        const [encoded] = encodeOpenCodeMessagesToCk([
            {
                info: { id: "msg_redacted", role: "assistant" },
                parts: [
                    { type: "reasoning", text: "", metadata: { redacted: "blob-1" } },
                    { type: "reasoning", text: "", redacted: "blob-2" },
                    { type: "reasoning", text: "visible", metadata: { redacted: "ignored" } },
                ],
            },
        ]);

        const kinds = (encoded.ck.content as Array<{ kind: Record<string, unknown> }>).map(
            (block) => block.kind,
        );
        expect(kinds[0]).toEqual({ type: "redacted_reasoning", data: "blob-1" });
        expect(kinds[1]).toEqual({ type: "redacted_reasoning", data: "blob-2" });
        expect(kinds[2]).toEqual({ type: "reasoning", text: "visible" });
    });

    it("attaches the daemon's approval arc to opaque parts with an approvalId", () => {
        const request = { type: "permission", approvalId: "ap_1", text: "may I?" };
        const response = { type: "permission-response", approvalId: "ap_1", granted: true };
        const stepStart = { type: "step-start", approvalId: "ap_2" };
        const [encoded] = encodeOpenCodeMessagesToCk([
            {
                info: { id: "msg_approval", role: "assistant" },
                parts: [request, response, stepStart, { type: "unknown-kind", x: 1 }],
            },
        ]);

        const kinds = (encoded.ck.content as Array<{ kind: Record<string, unknown> }>).map(
            (block) => block.kind,
        );
        expect(kinds[0]).toMatchObject({
            type: "opaque",
            kind: "permission",
            arc: { kind: "Approval", id: "ap_1", role: "Request" },
        });
        expect(kinds[1]).toMatchObject({
            type: "opaque",
            kind: "permission-response",
            arc: { kind: "Approval", id: "ap_1", role: "Response" },
        });
        expect(kinds[2]).not.toHaveProperty("arc");
        expect(kinds[3]).not.toHaveProperty("arc");
    });

    it("falls back to the daemon's message id chain and stable hash", () => {
        const [topLevel, hashed] = encodeOpenCodeMessagesToCk([
            { info: { role: "user" }, id: "top-level-id", parts: [] },
            {
                info: { role: "user", time: { created: 1700000000000 } },
                parts: [{ type: "text", text: "hi" }],
                absolute_ordinal: 3,
            },
        ]);
        expect(topLevel.mid).toBe("top-level-id");
        // `opencode-hash-` plus `stable_hash_prefix(raw_message, 24)` from the daemon.
        expect(hashed.mid).toBe("opencode-hash-8f8b01a55552065a9c8c32bd");
        expect(hashed.ck.meta).toMatchObject({ harness_id: hashed.mid });
    });

    it("falls back to the top-level role before defaulting to user", () => {
        const [fromInfo, fromRaw, defaulted] = encodeOpenCodeMessagesToCk([
            { info: { id: "r1", role: "assistant" }, role: "user", parts: [] },
            { info: { id: "r2" }, role: "assistant", parts: [] },
            { info: { id: "r3" }, parts: [] },
        ]);
        expect([fromInfo.ck.role, fromRaw.ck.role, defaulted.ck.role]).toEqual([
            "assistant",
            "assistant",
            "user",
        ]);
    });

    it("carries the daemon's message origin from provider and model ids", () => {
        const [nested, flat, none] = encodeOpenCodeMessagesToCk([
            {
                info: {
                    id: "m1",
                    role: "assistant",
                    model: { providerID: "anthropic", modelID: "claude" },
                },
                parts: [],
            },
            {
                info: { id: "m2", role: "assistant", providerID: "openai", modelID: "gpt" },
                parts: [],
            },
            { info: { id: "m3", role: "user" }, parts: [] },
        ]);
        expect(nested.ck.origin).toEqual({
            api: "anthropic",
            provider: "anthropic",
            model: "claude",
        });
        expect(flat.ck.origin).toEqual({ api: "openai", provider: "openai", model: "gpt" });
        expect(none.ck).not.toHaveProperty("origin");
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
            [2 ** 60, "1152921504606847000"],
            [2 ** 63, "9223372036854776000"],
            [-(2 ** 63), "-9223372036854775808"],
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

    it("serializes values the way serde_json::to_string does", () => {
        // Expected strings match `serde_json::to_string` over the wire text `JSON.stringify` emits.
        const numbers: Array<[number, string]> = [
            [1, "1"],
            [-1, "-1"],
            [0, "0"],
            [100, "100"],
            [1.5, "1.5"],
            [0.1, "0.1"],
            [0.00001, "0.00001"],
            [0.000001, "1e-6"],
            [1e-7, "1e-7"],
            [1.5e-10, "1.5e-10"],
            [12345.678, "12345.678"],
            [0.30000000000000004, "0.30000000000000004"],
            [-2.5, "-2.5"],
            [1e15, "1000000000000000"],
            [1e16, "10000000000000000"],
            [1e21, "1e+21"],
            [1e23, "1e+23"],
            [1.5e300, "1.5e+300"],
            [5e-324, "5e-324"],
            [2 ** 60, "1152921504606847000"],
            [2 ** 63, "9223372036854776000"],
            [-(2 ** 63), "-9.223372036854776e+18"],
            [2 ** 64, "1.8446744073709552e+19"],
            [-(2 ** 63) - 2048, "-9.223372036854778e+18"],
        ];
        for (const [value, expected] of numbers) {
            expect(__moduleWireTest.serdeJsonCompact(value)).toBe(expected);
        }
        expect(
            __moduleWireTest.serdeJsonCompact({
                b: [1, 2, { z: true, a: null }],
                a: 'x"y\n\u0001\u007f\u2028é😀',
                "😀": 2,
                "\uE000": 3,
            }),
        ).toBe(
            '{"a":"x\\"y\\n\\u0001\u007f\u2028é😀","b":[1,2,{"a":null,"z":true}],"\uE000":3,"😀":2}',
        );
        expect(__moduleWireTest.serdeJsonCompact({ cost: 0.000001, big: 2 ** 64, f: 1.5 })).toBe(
            '{"big":1.8446744073709552e+19,"cost":1e-6,"f":1.5}',
        );
    });

    it("hashes tool inputs the way the daemon's stable_hash_prefix does", () => {
        // Expected prefixes match `stable_hash_prefix(value, 12)`.
        const cases: Array<[unknown, string]> = [
            [{}, "44136fa355b3"],
            [{ path: "/tmp/x", limit: 10 }, "aac27fab24fb"],
            [
                {
                    b: [1, 2, { z: true, a: null }],
                    a: 'x"y\n\u0001\u007f\u2028é😀',
                    "😀": 2,
                    "\uE000": 3,
                },
                "966855a51ac8",
            ],
            [{ cost: 0.000001, big: 2 ** 64, f: 1.5 }, "1d5924c4d45c"],
            ["just a string", "3fe01def54b1"],
            [[1, "two", null], "597be421a7a1"],
        ];
        for (const [value, expected] of cases) {
            expect(__moduleWireTest.stableHashPrefix(value, 12)).toBe(expected);
        }
    });

    it("orders object keys by Unicode code point like Rust strings", () => {
        // UTF-16 code units place U+1F600 (surrogate pair) before U+E000; code points do not.
        const keys = ["\u{1F600}", "\uE000", "\uFFFF", "z", "\uD7FF"];
        const record = Object.fromEntries(keys.map((key) => [key, 1]));
        expect(__moduleWireTest.canonicalJson(record)).toBe(
            '{"z":1,"\uD7FF":1,"\uE000":1,"\uFFFF":1,"\u{1F600}":1}',
        );
        const sorted = [...keys].sort();
        expect(sorted.indexOf("\u{1F600}")).toBeLessThan(sorted.indexOf("\uE000"));
    });
});

describe("resolveOrdinalsForModule message identity", () => {
    it("resolves a message whose id is only at the top level", async () => {
        const sessionId = "module-wire-top-level-id";
        const unregister = setRawMessageProvider(sessionId, {
            readMessages: () => [],
            readMessageOrdinalPage: () => [],
            getStoredMessageCount: () => 1,
        });
        try {
            const resolved = await resolveOrdinalsForModule({
                sessionId,
                messages: [
                    { info: { role: "user", sessionID: sessionId }, id: "m-1", parts: [] },
                ] as unknown as MessageLike[],
                memo: {
                    generation: 1,
                    memoGeneration: 1,
                    entries: new Map([["m-1", 1]]),
                    anchor: { timeCreated: 1, id: "m-1" },
                    storedCount: 1,
                    canonicalCount: 1,
                },
            });
            expect(resolved.ok).toBe(true);
            if (!resolved.ok) throw new Error(resolved.reason);
            expect(encodeOpenCodeMessagesToCk(resolved.annotatedInput)[0]).toMatchObject({
                mid: "m-1",
                ordinal: 1,
            });
        } finally {
            unregister();
        }
    });

    it("treats an explicitly empty id as an identity, as the encoder and daemon do", async () => {
        const sessionId = "module-wire-empty-id";
        const unregister = setRawMessageProvider(sessionId, {
            readMessages: () => [],
            readMessageOrdinalPage: () => [],
            getStoredMessageCount: () => 1,
        });
        try {
            const resolved = await resolveOrdinalsForModule({
                sessionId,
                messages: [{ info: { id: "", role: "user", sessionID: sessionId }, parts: [] }],
                memo: {
                    generation: 1,
                    memoGeneration: 1,
                    entries: new Map([["", 1]]),
                    anchor: { timeCreated: 1, id: "" },
                    storedCount: 1,
                    canonicalCount: 1,
                },
            });
            expect(resolved.ok).toBe(true);
            if (!resolved.ok) throw new Error(resolved.reason);
            expect(encodeOpenCodeMessagesToCk(resolved.annotatedInput)[0]).toMatchObject({
                mid: "",
                ordinal: 1,
            });
        } finally {
            unregister();
        }
    });
});

describe("resolveOrdinalsForModule stored-count races", () => {
    it("recovers on the next call after a row lands between the page and count reads", async () => {
        const sessionId = "module-wire-count-race";
        const rows = [
            { id: "m-1", timeCreated: 1, contributesOrdinal: true, hasValidInfo: true },
            { id: "m-2", timeCreated: 2, contributesOrdinal: true, hasValidInfo: true },
        ];
        let raceOnce = false;
        const unregister = setRawMessageProvider(sessionId, {
            readMessages: () => rows,
            readMessageOrdinalPage: (after, limit) =>
                rows
                    .filter(
                        (row) =>
                            !after ||
                            row.timeCreated > after.timeCreated ||
                            (row.timeCreated === after.timeCreated && row.id > after.id),
                    )
                    .slice(0, limit),
            getStoredMessageCount: () => {
                // The count read observes a row the page read did not.
                if (raceOnce) {
                    raceOnce = false;
                    rows.push({
                        id: "m-4",
                        timeCreated: 4,
                        contributesOrdinal: true,
                        hasValidInfo: true,
                    });
                }
                return rows.length;
            },
        });
        const messages = ["m-1", "m-2", "m-3"].map((id) => ({
            info: { id, role: "user", sessionID: sessionId },
            parts: [{ type: "text", text: id }],
        })) as MessageLike[];
        const memo = {
            generation: 1,
            memoGeneration: 1,
            entries: new Map<string, number>(),
            anchor: null,
            storedCount: null,
            canonicalCount: 0,
        };
        try {
            const primed = await resolveOrdinalsForModule({
                sessionId,
                messages: messages.slice(0, 2),
                memo,
            });
            expect(primed.ok).toBe(true);
            if (!primed.ok) throw new Error(primed.reason);
            const bundle = {
                ...memo,
                memoGeneration: primed.memoGeneration,
                anchor: primed.memoAnchor,
                storedCount: primed.memoStoredCount,
                canonicalCount: primed.memoCanonicalCount,
            };
            rows.push({ id: "m-3", timeCreated: 3, contributesOrdinal: true, hasValidInfo: true });
            raceOnce = true;
            const raced = await resolveOrdinalsForModule({
                sessionId,
                messages: [
                    ...messages,
                    {
                        info: { id: "m-4", role: "user", sessionID: sessionId },
                        parts: [{ type: "text", text: "m-4" }],
                    } as MessageLike,
                ],
                memo: bundle,
            });
            // The count mismatch triggers a full rescan inside the same call.
            expect(raced.ok).toBe(true);
            if (!raced.ok) throw new Error(raced.reason);
            expect(
                (raced.annotatedInput as Array<{ absolute_ordinal: number }>).map(
                    (message) => message.absolute_ordinal,
                ),
            ).toEqual([1, 2, 3, 4]);
            expect(memo.entries.get("m-1")).toBe(1);
            expect(raced.memoStoredCount).toBe(4);
            expect(raced.memoCanonicalCount).toBe(4);
        } finally {
            unregister();
        }
    });

    it("rescans from the start when a row sorts at or before the anchor", async () => {
        const sessionId = "module-wire-pre-anchor-row";
        const rows = [
            { id: "m-1", timeCreated: 1, contributesOrdinal: true, hasValidInfo: true },
            { id: "m-2", timeCreated: 2, contributesOrdinal: true, hasValidInfo: true },
        ];
        const unregister = setRawMessageProvider(sessionId, {
            readMessages: () => rows,
            readMessageOrdinalPage: (after, limit) =>
                rows
                    .filter(
                        (row) =>
                            !after ||
                            row.timeCreated > after.timeCreated ||
                            (row.timeCreated === after.timeCreated && row.id > after.id),
                    )
                    .sort((a, b) => a.timeCreated - b.timeCreated || (a.id < b.id ? -1 : 1))
                    .slice(0, limit),
            getStoredMessageCount: () => rows.length,
        });
        const messages = ["m-1", "m-2"].map((id) => ({
            info: { id, role: "user", sessionID: sessionId },
            parts: [{ type: "text", text: id }],
        })) as MessageLike[];
        const memo = {
            generation: 1,
            memoGeneration: 1,
            entries: new Map<string, number>(),
            anchor: null as { timeCreated: number; id: string } | null,
            storedCount: null as number | null,
            canonicalCount: 0,
        };
        try {
            const primed = await resolveOrdinalsForModule({ sessionId, messages, memo });
            expect(primed.ok).toBe(true);
            if (!primed.ok) throw new Error(primed.reason);
            const bundle = {
                ...memo,
                memoGeneration: primed.memoGeneration,
                anchor: primed.memoAnchor,
                storedCount: primed.memoStoredCount,
                canonicalCount: primed.memoCanonicalCount,
            };

            // A row with the anchor's timestamp and a lexically smaller id is excluded by the
            // keyset read but increases the stored count. It has no ordinal, so existing ordinals
            // remain unchanged.
            rows.push({
                id: "m-1z",
                timeCreated: 2,
                contributesOrdinal: false,
                hasValidInfo: true,
            });
            const summaryInserted = await resolveOrdinalsForModule({
                sessionId,
                messages,
                memo: bundle,
            });
            expect(summaryInserted.ok).toBe(true);
            if (!summaryInserted.ok) throw new Error(summaryInserted.reason);
            expect(summaryInserted.memoStoredCount).toBe(3);
            expect(summaryInserted.memoCanonicalCount).toBe(2);
            expect(memo.entries.get("m-2")).toBe(2);
            const shifted = {
                ...bundle,
                anchor: summaryInserted.memoAnchor,
                storedCount: summaryInserted.memoStoredCount,
                canonicalCount: summaryInserted.memoCanonicalCount,
            };

            // A contributing row before the anchor moves every later ordinal.
            rows.push({ id: "m-0", timeCreated: 1, contributesOrdinal: true, hasValidInfo: true });
            const conflict = await resolveOrdinalsForModule({
                sessionId,
                messages,
                memo: shifted,
            });
            expect(conflict).toEqual({ ok: false, reason: "mismatch", messageId: "m-1" });
            expect(memo.entries.get("m-1")).toBe(1);
            expect(memo.entries.get("m-2")).toBe(2);
        } finally {
            unregister();
        }
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
        expect(pages[0]?.page as Record<string, unknown>).toEqual(body);
        expect(pages[0]?.bytes).toBe(Buffer.byteLength(JSON.stringify(body)));
    });

    it("keeps sparse array slots at their positions while paging", () => {
        const input: unknown[] = Array.from({ length: 80 }, (_, index) => ({
            mid: `m${index}`,
            ordinal: index + 1,
            ck: { text: "x".repeat(8_000) },
        }));
        delete input[3];
        delete input[40];
        const body = { method: "transform", session_id: "ses-sparse", input };
        const pages = buildPagedModuleTransformPayloads(body);
        const paged = pages.flatMap(
            ({ page }) => JSON.parse(JSON.stringify(page.input)) as unknown[],
        );
        const unpaged = JSON.parse(JSON.stringify(input)) as unknown[];
        expect(paged).toHaveLength(unpaged.length);
        expect(paged[3]).toBeNull();
        expect(paged[40]).toBeNull();
        expect(paged).toEqual(unpaged);
    });

    it("sends an item carrying the reserved continuation key as a continuation", () => {
        const carrier = {
            mid: "carrier",
            ordinal: 1,
            [MODULE_ITEM_CONTINUATION_KEY]: {
                field: "input",
                item_index: 9,
                chunk_index: 0,
                chunk_total: 1,
            },
        };
        const body = {
            method: "transform",
            session_id: "ses-reserved",
            input: [
                carrier,
                ...Array.from({ length: 80 }, (_, index) => ({
                    mid: `m${index}`,
                    ordinal: index + 2,
                    ck: { text: "x".repeat(8_000) },
                })),
            ],
        };
        const pages = buildPagedModuleTransformPayloads(body);
        const units = pages.flatMap(({ page }) => page.input as Array<Record<string, unknown>>);
        expect(units.filter((unit) => unit.mid === "carrier")).toHaveLength(0);
        const marker = units[0]?.[MODULE_ITEM_CONTINUATION_KEY] as Record<string, unknown>;
        expect(marker).toEqual({ field: "input", item_index: 0, chunk_index: 0, chunk_total: 1 });
        expect(JSON.parse(units[0]?.chunk as string)).toEqual(carrier);
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

it("snapshots source toJSON once and serializes each emitted envelope once", async () => {
    const { serializedJsonText } = await import("../../shared/host-client/serialized-json-body");
    let calls = 0;
    const body = {
        method: "transform",
        session_id: "serialized-getter",
        messages: Array.from({ length: 80 }, (_, index) => ({
            index,
            text: "x".repeat(8_000),
        })),
        marker: { toJSON: () => `snapshot-${++calls}` },
    };
    const stringify = spyOn(JSON, "stringify");
    try {
        const pages = buildPagedModuleTransformPayloads(body);
        expect(pages.length).toBeGreaterThan(1);
        expect(calls).toBe(1);
        expect(stringify.mock.calls.filter(([value]) => value === body)).toHaveLength(1);
        expect(pages.at(-1)?.page.marker).toBe("snapshot-1");
        for (const { page } of pages) {
            expect(
                stringify.mock.results.filter(
                    (result) => result.value === serializedJsonText(page),
                ),
            ).toHaveLength(1);
        }
    } finally {
        stringify.mockRestore();
    }
});

it("builds the unpaged carrier without parsing the body", async () => {
    const { serializedJsonText } = await import("../../shared/host-client/serialized-json-body");
    const body = {
        method: "transform",
        session_id: "serialized-unpaged",
        messages: Array.from({ length: 200 }, (_, index) => ({
            index,
            ck: { role: "user", content: [{ kind: { type: "text", text: `m${index}` } }] },
        })),
    };
    const parse = spyOn(JSON, "parse");
    try {
        const pages = buildPagedModuleTransformPayloads(body);
        expect(pages).toHaveLength(1);
        expect(parse).not.toHaveBeenCalled();
        expect(serializedJsonText(pages[0]!.page)).toBe(JSON.stringify(body));
        expect(pages[0]!.page.messages).toBe(body.messages);
    } finally {
        parse.mockRestore();
    }
});

it("measures each paged item once across convergence attempts", () => {
    const body = {
        method: "transform",
        session_id: "serialized-bounds",
        messages: Array.from({ length: 80 }, (_, index) => ({
            index,
            text: "x".repeat(8_000),
        })),
    };
    const stringify = spyOn(JSON, "stringify");
    const parse = spyOn(JSON, "parse");
    try {
        const pages = buildPagedModuleTransformPayloads(body);
        // Two pages force a second convergence attempt after the first assumes one page.
        expect(pages).toHaveLength(2);
        const itemMeasurements = stringify.mock.calls.filter(
            ([value]) => isRecord(value) && value.index === 0 && typeof value.text === "string",
        );
        expect(itemMeasurements).toHaveLength(1);
        // One snapshot of the body for stable bounds; emitted pages are not parsed again.
        const bodyParses = parse.mock.calls.filter(
            ([text]) => typeof text === "string" && text.startsWith('{"method":"transform"'),
        );
        expect(bodyParses).toHaveLength(1);
    } finally {
        stringify.mockRestore();
        parse.mockRestore();
    }
});

it("keeps unpaged boundary bodies accepted when Rust numbers expand", async () => {
    const { serializedTransformCorpus } = await import("./__tests__/serialized-transform-corpus");
    const { serializedJsonText } = await import("../../shared/host-client/serialized-json-body");
    const cases = serializedTransformCorpus().filter((fixture) =>
        ["scalar-number-over", "raw-exponent-over", "raw-negative-zero-over"].includes(
            fixture.name,
        ),
    );
    expect(cases).toHaveLength(3);
    for (const fixture of cases) {
        const expected = JSON.stringify(fixture.body);
        expect(Buffer.byteLength(expected)).toBe(MODULE_PAGE_MAX_BYTES);
        const pages = buildPagedModuleTransformPayloads(fixture.body);
        expect(pages).toHaveLength(1);
        expect(pages[0]?.page.transform_page_index).toBeUndefined();
        expect(serializedJsonText(pages[0]!.page)).toBe(expected);
    }
});
