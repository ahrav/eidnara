/// <reference types="bun-types" />

import { beforeEach, describe, expect, it, mock } from "bun:test";
import { RAW_PART_VERSION_KEY } from "./read-session-raw";
import type { MessageLike, ThinkingLikePart } from "./tag-content-primitives";
import {
    createToolDropTarget,
    extractToolCallObservation,
    hasMeaningfulPart,
    partHasCompletedResult,
    type ToolCallIndex,
    ToolMutationBatch,
} from "./tool-drop-target";

function message(id: string, role: string, parts: unknown[]): MessageLike {
    return {
        info: { id, role, sessionID: "ses-1" },
        parts,
    };
}

function hasCall(messages: MessageLike[], callId: string): boolean {
    for (const msg of messages) {
        for (const part of msg.parts) {
            const observation = extractToolCallObservation(part);
            if (observation?.callId === callId) {
                return true;
            }
        }
    }

    return false;
}

function buildIndex(messages: MessageLike[]): ToolCallIndex {
    const index: ToolCallIndex = new Map();
    for (const msg of messages) {
        for (const part of msg.parts) {
            const observation = extractToolCallObservation(part);
            if (observation) {
                const entry = index.get(observation.callId) ?? {
                    occurrences: [],
                    hasResult: false,
                };
                entry.occurrences.push({ message: msg, part, kind: observation.kind });
                // OpenCode `{ type: "tool" }` parts are result observations while pending; set `hasResult` only after completion.
                if (observation.kind === "result" && partHasCompletedResult(part))
                    entry.hasResult = true;
                index.set(observation.callId, entry);
            }
        }
    }
    return index;
}

describe("tool-drop-target", () => {
    let buildOutput: ReturnType<typeof mock<(suffix: string) => string>>;

    beforeEach(() => {
        buildOutput = mock((suffix: string) => `output-${suffix}`);
    });

    describe("extractToolCallObservation", () => {
        describe("#given supported and unsupported tool part shapes", () => {
            describe("#when extracting tool call observations", () => {
                it("#then it classifies invocation/result parts and ignores invalid shapes", () => {
                    expect(extractToolCallObservation({ type: "tool", callID: "call-a" })).toEqual({
                        callId: "call-a",
                        kind: "result",
                    });
                    expect(
                        extractToolCallObservation({ type: "tool-invocation", callID: "call-b" }),
                    ).toEqual({
                        callId: "call-b",
                        kind: "invocation",
                    });
                    expect(extractToolCallObservation({ type: "tool_use", id: "call-c" })).toEqual({
                        callId: "call-c",
                        kind: "invocation",
                    });
                    expect(
                        extractToolCallObservation({ type: "tool_result", tool_use_id: "call-d" }),
                    ).toEqual({
                        callId: "call-d",
                        kind: "result",
                    });
                    expect(
                        extractToolCallObservation({ type: "tool_result", tool_use_id: "" }),
                    ).toBeNull();
                    expect(extractToolCallObservation({ type: "text", text: "plain" })).toBeNull();
                });
            });
        });
    });

    describe("partHasCompletedResult", () => {
        it("counts a completed OpenCode tool part (output string) as closed", () => {
            expect(
                partHasCompletedResult({ type: "tool", callID: "c", state: { output: "done" } }),
            ).toBe(true);
        });

        it("counts an errored OpenCode tool part (status error, no output) as closed", () => {
            expect(
                partHasCompletedResult({
                    type: "tool",
                    callID: "c",
                    state: { status: "error", error: "boom", input: { content: "x".repeat(600) } },
                }),
            ).toBe(true);
        });

        it("counts a completed OpenCode tool part with non-string output as closed", () => {
            expect(
                partHasCompletedResult({
                    type: "tool",
                    callID: "c",
                    state: { status: "completed", output: null, input: {} },
                }),
            ).toBe(true);
            expect(
                partHasCompletedResult({
                    type: "tool",
                    callID: "c",
                    state: { status: "completed" },
                }),
            ).toBe(true);
        });

        it("counts a flat OpenCode tool part with top-level terminal fields as closed", () => {
            expect(
                partHasCompletedResult({
                    type: "tool",
                    callID: "c",
                    status: "completed",
                    input: { q: "a" },
                    output: "done",
                }),
            ).toBe(true);
            expect(
                partHasCompletedResult({ type: "tool", callID: "c", status: "running", input: {} }),
            ).toBe(false);
            expect(partHasCompletedResult({ type: "tool", callID: "c" })).toBe(false);
        });

        it("falls back to the top-level status when the nested status is not a string", () => {
            expect(
                partHasCompletedResult({
                    type: "tool",
                    callID: "c",
                    status: "completed",
                    state: { status: null, attachments: [{ type: "file", mime: "image/png" }] },
                }),
            ).toBe(true);
            expect(
                partHasCompletedResult({
                    type: "tool",
                    callID: "c",
                    status: "running",
                    state: { status: "completed" },
                }),
            ).toBe(true);
        });

        it("keeps a running OpenCode tool part (no output, no error) open", () => {
            expect(
                partHasCompletedResult({
                    type: "tool",
                    callID: "c",
                    state: { status: "running", input: { prompt: "p" } },
                }),
            ).toBe(false);
        });

        it("keeps a pending OpenCode tool part open", () => {
            expect(
                partHasCompletedResult({ type: "tool", callID: "c", state: { status: "pending" } }),
            ).toBe(false);
        });

        it("counts an Anthropic tool_result part as closed", () => {
            expect(partHasCompletedResult({ type: "tool_result", tool_use_id: "c" })).toBe(true);
        });

        it("excludes invocation-shaped parts and non-records", () => {
            expect(partHasCompletedResult({ type: "tool-invocation", callID: "c" })).toBe(false);
            expect(partHasCompletedResult({ type: "tool_use", id: "c" })).toBe(false);
            expect(partHasCompletedResult(null)).toBe(false);
            expect(partHasCompletedResult({ type: "tool" })).toBe(false);
        });
    });

    describe("createToolDropTarget", () => {
        describe("#given a complete invocation/result tool pair", () => {
            describe("#when dropping the call", () => {
                it("#then it marks for removal, and finalize removes parts, prunes empty wrappers, and clears thinking", () => {
                    const messages: MessageLike[] = [
                        message("m-inv", "assistant", [
                            { type: "tool-invocation", callID: "call-1" },
                        ]),
                        message("m-tool", "tool", [
                            {
                                type: "tool",
                                callID: "call-1",
                                state: { output: buildOutput("tool") },
                            },
                        ]),
                        message("m-wrapper", "assistant", [
                            { type: "step-start", snapshot: "snap-1" },
                            { type: "tool_use", id: "call-1" },
                            {
                                type: "tool_result",
                                tool_use_id: "call-1",
                                content: buildOutput("result"),
                            },
                            { type: "step-finish", reason: "tool-calls" },
                        ]),
                        message("m-keep", "assistant", [{ type: "text", text: "keep me" }]),
                    ];
                    const thinkingParts: ThinkingLikePart[] = [
                        { type: "thinking", thinking: "private" },
                        { type: "reasoning", text: "trace" },
                    ];
                    const index = buildIndex(messages);
                    const batch = new ToolMutationBatch(messages);

                    const target = createToolDropTarget("call-1", thinkingParts, index, batch, 1);
                    const result = target.drop();

                    expect(result).toBe("removed");
                    expect(buildOutput).toHaveBeenCalledTimes(2);

                    batch.finalize();

                    expect(hasCall(messages, "call-1")).toBe(false);
                    expect(messages).toHaveLength(1);
                    expect(messages[0]?.info.id).toBe("m-keep");
                    expect(thinkingParts[0]?.thinking).toBe("[cleared]");
                    expect(thinkingParts[1]?.text).toBe("[cleared]");
                });

                it("#then signed reasoning stays byte-identical while unsigned reasoning is cleared", () => {
                    const signedNested = {
                        type: "reasoning",
                        text: "signed nested",
                        metadata: { anthropic: { signature: "sig-1" } },
                    };
                    const signedTop = {
                        type: "thinking",
                        thinking: "signed top",
                        signature: "sig-2",
                    };
                    const unsigned = { type: "reasoning", text: "unsigned" };
                    const pristineNested = JSON.stringify(signedNested);
                    const pristineTop = JSON.stringify(signedTop);
                    const messages: MessageLike[] = [
                        message("m-inv", "assistant", [{ type: "tool_use", id: "call-sig" }]),
                        message("m-res", "tool", [
                            { type: "tool", callID: "call-sig", state: { output: "out" } },
                        ]),
                    ];
                    const index = buildIndex(messages);
                    const batch = new ToolMutationBatch(messages);
                    const target = createToolDropTarget(
                        "call-sig",
                        [signedNested, signedTop, unsigned],
                        index,
                        batch,
                        23,
                    );

                    expect(target.truncate()).toBe("truncated");
                    expect(target.drop()).toBe("removed");

                    expect(JSON.stringify(signedNested)).toBe(pristineNested);
                    expect(JSON.stringify(signedTop)).toBe(pristineTop);
                    expect(unsigned.text).toBe("[cleared]");
                });

                it("#then redacted reasoning stays verbatim in every persisted form", () => {
                    const topData = { type: "reasoning", text: "", data: "opaque-1" };
                    const topRedacted = { type: "reasoning", text: "", redacted: "opaque-2" };
                    const metaRedacted = {
                        type: "reasoning",
                        text: "",
                        metadata: { redacted: "opaque-3" },
                    };
                    const pristine = [topData, topRedacted, metaRedacted].map((p) =>
                        JSON.stringify(p),
                    );
                    const messages: MessageLike[] = [
                        message("m-inv", "assistant", [{ type: "tool_use", id: "call-red" }]),
                        message("m-res", "tool", [
                            { type: "tool", callID: "call-red", state: { output: "out" } },
                        ]),
                    ];
                    const index = buildIndex(messages);
                    const batch = new ToolMutationBatch(messages);
                    const target = createToolDropTarget(
                        "call-red",
                        [topData, topRedacted, metaRedacted],
                        index,
                        batch,
                        24,
                    );

                    expect(target.drop()).toBe("removed");

                    expect(
                        [topData, topRedacted, metaRedacted].map((p) => JSON.stringify(p)),
                    ).toEqual(pristine);
                });

                it("#then clearing unsigned reasoning advances the part version once", () => {
                    const reasoning = { type: "reasoning", text: "reasoning" };
                    Object.defineProperty(reasoning, RAW_PART_VERSION_KEY, {
                        value: 1_700_000_000_000,
                        enumerable: false,
                        configurable: true,
                    });
                    const versionOf = (p: object): unknown =>
                        (p as Record<string, unknown>)[RAW_PART_VERSION_KEY];
                    const messages: MessageLike[] = [
                        message("m-res", "tool", [
                            { type: "tool", callID: "call-ver", state: { output: "out" } },
                        ]),
                    ];
                    const index = buildIndex(messages);
                    const batch = new ToolMutationBatch(messages);
                    const target = createToolDropTarget("call-ver", [reasoning], index, batch, 25);

                    expect(target.truncate()).toBe("truncated");
                    const stamped = versionOf(reasoning);
                    expect(reasoning.text).toBe("[cleared]");
                    expect(typeof stamped).toBe("string");

                    // A second clear over already-cleared text is a no-op and keeps the stamp.
                    expect(target.drop()).toBe("removed");
                    expect(versionOf(reasoning)).toBe(stamped);
                });

                it("#then the clamped clone carries a fresh version even when the harness version is enumerable", () => {
                    const toolPart = {
                        type: "tool",
                        callID: "call-clone-ver",
                        version: 7,
                        state: { output: "x".repeat("[dropped \u00a733\u00a7]".length) },
                    };
                    const messages: MessageLike[] = [
                        message("m-clone-ver", "assistant", [toolPart]),
                    ];
                    const index = buildIndex(messages);
                    const batch = new ToolMutationBatch(messages);
                    const target = createToolDropTarget("call-clone-ver", [], index, batch, 33);

                    expect(target.truncate()).toBe("truncated");

                    const wire = messages[0]?.parts[0] as Record<string, unknown>;
                    expect(wire.version).toBe(7);
                    expect(typeof wire[RAW_PART_VERSION_KEY]).toBe("string");
                    expect(RAW_PART_VERSION_KEY in toolPart).toBe(false);
                });
            });
        });

        describe("#given only a tool invocation without any result", () => {
            describe("#when dropping the call", () => {
                it("#then it reports incomplete and leaves messages unchanged", () => {
                    const messages: MessageLike[] = [
                        message("m-inv", "assistant", [
                            { type: "tool-invocation", callID: "call-orphan" },
                        ]),
                    ];
                    const thinkingParts: ThinkingLikePart[] = [
                        { type: "thinking", thinking: "keep" },
                    ];
                    const index = buildIndex(messages);
                    const batch = new ToolMutationBatch(messages);

                    const target = createToolDropTarget(
                        "call-orphan",
                        thinkingParts,
                        index,
                        batch,
                        2,
                    );
                    const result = target.drop();

                    expect(result).toBe("incomplete");
                    expect(hasCall(messages, "call-orphan")).toBe(true);
                    expect(thinkingParts[0]?.thinking).toBe("keep");
                });
            });
        });

        describe("#given no matching tool call id exists", () => {
            describe("#when dropping the call", () => {
                it("#then it reports absent without mutation", () => {
                    const messages: MessageLike[] = [
                        message("m-other", "assistant", [
                            { type: "tool-invocation", callID: "call-other" },
                        ]),
                    ];
                    const thinkingParts: ThinkingLikePart[] = [
                        { type: "thinking", thinking: "unchanged" },
                    ];
                    const index: ToolCallIndex = new Map();
                    const batch = new ToolMutationBatch(messages);

                    const target = createToolDropTarget(
                        "call-missing",
                        thinkingParts,
                        index,
                        batch,
                        3,
                    );
                    const result = target.drop();

                    expect(result).toBe("absent");
                    expect(hasCall(messages, "call-other")).toBe(true);
                    expect(thinkingParts[0]?.thinking).toBe("unchanged");
                });
            });
        });

        describe("#given a complete tool pair with both tool and tool_result outputs", () => {
            describe("#when setContent is called", () => {
                it("#then it updates only result content and supports dropped-content removal", () => {
                    const toolResultPart = {
                        type: "tool_result",
                        tool_use_id: "call-2",
                        content: "old-result",
                    };
                    const toolPart = {
                        type: "tool",
                        callID: "call-2",
                        state: { output: "old-tool" },
                    };
                    const messages: MessageLike[] = [
                        message("m-inv", "assistant", [{ type: "tool_use", id: "call-2" }]),
                        message("m-res", "tool", [toolPart]),
                        message("m-res-2", "assistant", [toolResultPart]),
                    ];
                    const thinkingParts: ThinkingLikePart[] = [
                        { type: "thinking", thinking: "to clear" },
                    ];
                    const index = buildIndex(messages);
                    const batch = new ToolMutationBatch(messages);
                    const target = createToolDropTarget("call-2", thinkingParts, index, batch, 4);

                    target.setContent("replacement-content");

                    expect(toolPart.state.output).toBe("replacement-content");
                    expect(toolResultPart.content).toBe("replacement-content");
                    expect(hasCall(messages, "call-2")).toBe(true);

                    target.setContent("[dropped §2§]");
                    batch.finalize();

                    expect(hasCall(messages, "call-2")).toBe(false);
                    expect(thinkingParts[0]?.thinking).toBe("[cleared]");
                });

                it("#then content that merely begins with [dropped is stored, not treated as a drop", () => {
                    const toolPart = {
                        type: "tool",
                        callID: "call-2",
                        state: { output: "old-tool" },
                    };
                    const messages: MessageLike[] = [
                        message("m-inv", "assistant", [{ type: "tool_use", id: "call-2" }]),
                        message("m-res", "tool", [toolPart]),
                    ];
                    const index = buildIndex(messages);
                    const batch = new ToolMutationBatch(messages);
                    const target = createToolDropTarget("call-2", [], index, batch, 4);

                    expect(target.setContent("[dropped 10 stale rows]")).toBe(true);
                    batch.finalize();

                    expect(toolPart.state.output).toBe("[dropped 10 stale rows]");
                    expect(hasCall(messages, "call-2")).toBe(true);
                });

                it("#then a drop sentinel on an incomplete or absent target reports false and changes nothing", () => {
                    const runningPart = {
                        type: "tool",
                        callID: "call-run",
                        state: { status: "running", input: { prompt: "p" } },
                    };
                    const messages: MessageLike[] = [message("m-run", "assistant", [runningPart])];
                    const index = buildIndex(messages);
                    const batch = new ToolMutationBatch(messages);

                    const incomplete = createToolDropTarget("call-run", [], index, batch, 4);
                    expect(incomplete.setContent("[dropped §4§]")).toBe(false);

                    const absent = createToolDropTarget("call-missing", [], index, batch, 4);
                    expect(absent.setContent("[dropped §4§]")).toBe(false);

                    batch.finalize();
                    expect(messages[0]?.parts[0]).toBe(runningPart);
                    expect(runningPart.state.input).toEqual({ prompt: "p" });
                });
            });
        });

        describe("#given truncate has already replaced the wire part with a clone", () => {
            describe("#when drop runs on the same target", () => {
                it("#then finalize removes the clone from the outgoing messages", () => {
                    const toolPart = {
                        type: "tool",
                        callID: "call-esc",
                        state: { input: { q: "a" }, output: "big" },
                    };
                    const messages: MessageLike[] = [
                        message("m-inv", "assistant", [{ type: "tool_use", id: "call-esc" }]),
                        message("m-res", "tool", [toolPart]),
                    ];
                    const index = buildIndex(messages);
                    const batch = new ToolMutationBatch(messages);
                    const target = createToolDropTarget("call-esc", [], index, batch, 6);

                    expect(target.truncate()).toBe("truncated");
                    expect(messages[1]?.parts[0]).not.toBe(toolPart);
                    expect(target.drop()).toBe("removed");
                    batch.finalize();

                    expect(hasCall(messages, "call-esc")).toBe(false);
                    expect(toolPart.state.output).toBe("big");
                });
            });
        });

        describe("#given drop is called twice on the same callId", () => {
            describe("#when the second drop runs", () => {
                it("#then it returns absent (idempotent)", () => {
                    const messages: MessageLike[] = [
                        message("m-inv", "assistant", [
                            { type: "tool-invocation", callID: "call-1" },
                        ]),
                        message("m-tool", "tool", [
                            { type: "tool", callID: "call-1", state: { output: "out" } },
                        ]),
                    ];
                    const index = buildIndex(messages);
                    const batch = new ToolMutationBatch(messages);
                    const target = createToolDropTarget("call-1", [], index, batch, 5);

                    expect(target.drop()).toBe("removed");
                    expect(target.drop()).toBe("absent");
                });
            });
        });

        describe("#given a complete tool pair with structured input", () => {
            describe("#when truncate is called", () => {
                it("#then it keeps small inputs intact while truncating result content", () => {
                    const toolResultPart = {
                        type: "tool_result",
                        tool_use_id: "call-3",
                        content: "old-result",
                    };
                    const toolPart = {
                        type: "tool",
                        callID: "call-3",
                        state: {
                            input: {
                                query: "abcdef",
                                short: "abc",
                                files: ["a", "b"],
                                metadata: { nested: true },
                                exact: true,
                                limit: 2,
                            },
                            output: "old-tool",
                        },
                    };
                    const messages: MessageLike[] = [
                        message("m-inv", "assistant", [{ type: "tool_use", id: "call-3" }]),
                        message("m-res", "tool", [toolPart]),
                        message("m-res-2", "assistant", [toolResultPart]),
                    ];
                    const thinkingParts: ThinkingLikePart[] = [
                        { type: "thinking", thinking: "to clear" },
                        { type: "reasoning", text: "trace" },
                    ];
                    const index = buildIndex(messages);
                    const batch = new ToolMutationBatch(messages);
                    const target = createToolDropTarget("call-3", thinkingParts, index, batch, 7);

                    expect(target.truncate()).toBe("truncated");

                    batch.finalize();

                    expect(hasCall(messages, "call-3")).toBe(true);
                    expect(messages).toHaveLength(3);

                    // `ToolMutationBatch` replaces the message part with a clamped sentinel clone and does not mutate OpenCode's original part.
                    const wireToolPart = messages[1]?.parts[0] as {
                        state: Record<string, unknown>;
                    };
                    expect(wireToolPart.state).toEqual({
                        input: {
                            query: "abcdef",
                            short: "abc",
                            files: ["a", "b"],
                            metadata: { nested: true },
                            exact: true,
                            limit: 2,
                        },
                        output: "[dropped \u00a77\u00a7]",
                    });
                    const wireResultPart = messages[2]?.parts[0] as { content: string };
                    expect(wireResultPart.content).toBe("[dropped \u00a77\u00a7]");

                    // `ToolMutationBatch` replaces the message part with a clamped sentinel clone and does not mutate OpenCode's original part.
                    // `ToolMutationBatch` replaces the message part with a clamped sentinel clone and does not mutate OpenCode's original part.
                    // `ToolMutationBatch` replaces the message part with a clamped sentinel clone and does not mutate OpenCode's original part.
                    expect(toolPart.state as Record<string, unknown>).toEqual({
                        input: {
                            query: "abcdef",
                            short: "abc",
                            files: ["a", "b"],
                            metadata: { nested: true },
                            exact: true,
                            limit: 2,
                        },
                        output: "old-tool",
                    });
                    expect(toolResultPart.content).toBe("old-result");
                    expect(wireToolPart).not.toBe(toolPart);
                    expect(wireResultPart).not.toBe(toolResultPart);
                    expect(thinkingParts[0]?.thinking).toBe("[cleared]");
                    expect(thinkingParts[1]?.text).toBe("[cleared]");
                });

                it("#then truncates large inputs before keeping the tool structure", () => {
                    const largeQuery = "x".repeat(600);
                    const toolPart = {
                        type: "tool",
                        callID: "call-4",
                        state: {
                            input: {
                                query: largeQuery,
                                files: ["a", "b"],
                                metadata: { nested: true },
                            },
                            output: "old-tool",
                        },
                    };
                    const messages: MessageLike[] = [
                        message("m-inv", "assistant", [
                            {
                                type: "tool-invocation",
                                callID: "call-4",
                                args: { query: largeQuery },
                            },
                        ]),
                        message("m-res", "tool", [toolPart]),
                    ];
                    const index = buildIndex(messages);
                    const batch = new ToolMutationBatch(messages);
                    const target = createToolDropTarget("call-4", [], index, batch, 9);

                    expect(target.truncate()).toBe("truncated");

                    // `ToolMutationBatch` replaces the message part with a clamped sentinel clone and does not mutate OpenCode's original part.
                    const wireToolPart = messages[1]?.parts[0] as {
                        state: Record<string, unknown>;
                    };
                    expect(wireToolPart.state).toEqual({
                        input: {
                            query: "xxxxx...[truncated]",
                            files: "[2 items]",
                            metadata: "[object]",
                        },
                        output: "[dropped \u00a79\u00a7]",
                    });

                    // `ToolMutationBatch` replaces the message part with a clamped sentinel clone and does not mutate OpenCode's original part.
                    // `ToolMutationBatch` replaces the message part with a clamped sentinel clone and does not mutate OpenCode's original part.
                    expect(toolPart.state as Record<string, unknown>).toEqual({
                        input: {
                            query: largeQuery,
                            files: ["a", "b"],
                            metadata: { nested: true },
                        },
                        output: "old-tool",
                    });
                    expect(wireToolPart).not.toBe(toolPart);
                });
            });
        });

        describe("#given a pending/running OpenCode tool part (no result output yet)", () => {
            describe("#when any drop/clamp selector runs", () => {
                it("#then it is treated as an open arc: never targeted and left byte-identical", () => {
                    const longPrompt = `Fix the auth bug and add coverage. ${"x".repeat(600)}`;
                    const taskPart = {
                        type: "tool",
                        tool: "task",
                        callID: "call-task",
                        state: {
                            status: "running",
                            input: { prompt: longPrompt, subagent_type: "mason" },
                        },
                    };
                    const pristine = JSON.stringify(taskPart);
                    const messages: MessageLike[] = [message("m-task", "assistant", [taskPart])];
                    const index = buildIndex(messages);
                    const batch = new ToolMutationBatch(messages);
                    const target = createToolDropTarget("call-task", [], index, batch, 11);

                    // Invocations without completed results are ineligible for canDrop, drop, truncate, and setContent.
                    expect(target.canDrop()).toBe(false);
                    expect(target.truncate()).toBe("incomplete");
                    expect(target.drop()).toBe("incomplete");
                    expect(target.setContent("replacement")).toBe(false);

                    // `ToolMutationBatch` does not mutate parts that OpenCode still references.
                    // `ToolMutationBatch` does not mutate parts that OpenCode still references.
                    expect(JSON.stringify(taskPart)).toBe(pristine);
                    expect(messages[0]?.parts[0]).toBe(taskPart);
                });
            });
        });

        describe("#given a completed tool part with a long argument (background task shape)", () => {
            describe("#when truncate runs", () => {
                it("#then the wire copy carries the clamp while the live object stays intact", () => {
                    const longPrompt = `Investigate and fix the regression. ${"y".repeat(600)}`;
                    const taskPart = {
                        type: "tool",
                        tool: "task",
                        callID: "call-bg",
                        state: {
                            status: "completed",
                            input: { prompt: longPrompt, subagent_type: "mason" },
                            output: '<task state="running">started</task>',
                        },
                    };
                    const pristine = JSON.stringify(taskPart);
                    const messages: MessageLike[] = [message("m-bg", "assistant", [taskPart])];
                    const index = buildIndex(messages);
                    const batch = new ToolMutationBatch(messages);
                    const target = createToolDropTarget("call-bg", [], index, batch, 12);

                    expect(target.canDrop()).toBe(true);
                    expect(target.truncate()).toBe("truncated");

                    // `ToolMutationBatch` replaces the message part with a clamped sentinel clone and does not mutate OpenCode's original part.
                    const wire = messages[0]?.parts[0] as {
                        state: { input: Record<string, unknown>; output: string };
                    };
                    expect(wire).not.toBe(taskPart);
                    expect(wire.state.output).toBe("[dropped \u00a712\u00a7]");
                    expect(wire.state.input.prompt).toBe("Inves...[truncated]");

                    // `ToolMutationBatch` replaces the message part with a clamped sentinel clone and does not mutate OpenCode's original part.
                    // `ToolMutationBatch` replaces the message part with a clamped sentinel clone and does not mutate OpenCode's original part.
                    // `ToolMutationBatch` replaces the message part with a clamped sentinel clone and does not mutate OpenCode's original part.
                    expect(JSON.stringify(taskPart)).toBe(pristine);
                    expect(taskPart.state.input.prompt).toBe(longPrompt);
                });
            });
        });

        describe("#given an errored OpenCode tool part (status error, no output, large input)", () => {
            describe("#when the selectors run", () => {
                it("#then it IS clamp-eligible (closed arm) and the clamp stays on the wire only", () => {
                    const bigContent = "z".repeat(600);
                    const failedWrite = {
                        type: "tool",
                        tool: "write",
                        callID: "call-err",
                        state: {
                            status: "error",
                            error: "permission denied",
                            input: { filePath: "/spec.md", content: bigContent },
                        },
                    };
                    const pristine = JSON.stringify(failedWrite);
                    const messages: MessageLike[] = [message("m-err", "assistant", [failedWrite])];
                    const index = buildIndex(messages);
                    const batch = new ToolMutationBatch(messages);
                    const target = createToolDropTarget("call-err", [], index, batch, 13);

                    // Errored completed results remain eligible for canDrop, drop, and truncate.
                    expect(target.canDrop()).toBe(true);
                    expect(target.truncate()).toBe("truncated");

                    // `ToolMutationBatch` replaces the message part with a clamped sentinel clone and does not mutate OpenCode's original part.
                    const wire = messages[0]?.parts[0] as {
                        state: Record<string, unknown> & { input: Record<string, unknown> };
                    };
                    expect(wire).not.toBe(failedWrite);
                    expect(wire.state.error).toBe("[dropped \u00a713\u00a7]");
                    expect("output" in wire.state).toBe(false);
                    expect(wire.state.input.content).toBe("zzzzz...[truncated]");

                    // `ToolMutationBatch` replaces the message part with a clamped sentinel clone and does not mutate OpenCode's original part.
                    // `ToolMutationBatch` replaces the message part with a clamped sentinel clone and does not mutate OpenCode's original part.
                    expect(JSON.stringify(failedWrite)).toBe(pristine);
                    expect(failedWrite.state.error).toBe("permission denied");
                    expect(failedWrite.state.input.content).toBe(bigContent);
                });

                it("#then setContent replaces the error payload the wire serializes", () => {
                    const failedRead = {
                        type: "tool",
                        callID: "call-err-2",
                        state: { status: "error", error: "ENOENT: long stack trace" },
                    };
                    const messages: MessageLike[] = [message("m-err-2", "assistant", [failedRead])];
                    const index = buildIndex(messages);
                    const batch = new ToolMutationBatch(messages);
                    const target = createToolDropTarget("call-err-2", [], index, batch, 14);

                    expect(target.setContent("ENOENT")).toBe(true);
                    expect(failedRead.state.error).toBe("ENOENT");
                    // A second identical write is a no-op because the error field is what is compared.
                    expect(target.setContent("ENOENT")).toBe(false);
                });
            });
        });

        describe("#given a flat OpenCode tool part with top-level status, input, and output", () => {
            describe("#when it is truncated or replaced", () => {
                it("#then the top-level fields are the ones rewritten and the input is clamped", () => {
                    const flat = {
                        type: "tool",
                        callID: "call-flat",
                        status: "completed",
                        input: { content: "f".repeat(600) },
                        output: "big output",
                    };
                    const pristine = JSON.stringify(flat);
                    const messages: MessageLike[] = [message("m-flat", "assistant", [flat])];
                    const index = buildIndex(messages);
                    const batch = new ToolMutationBatch(messages);
                    const target = createToolDropTarget("call-flat", [], index, batch, 26);

                    expect(target.canDrop()).toBe(true);
                    expect(target.readInput()).toEqual({ content: "f".repeat(600) });
                    expect(target.truncate()).toBe("truncated");

                    const wire = messages[0]?.parts[0] as Record<string, unknown> & {
                        input: { content: string };
                    };
                    expect(wire.output).toBe("[dropped \u00a726\u00a7]");
                    expect(wire.input.content).toBe("fffff...[truncated]");
                    expect("state" in wire).toBe(false);
                    expect(JSON.stringify(flat)).toBe(pristine);

                    expect(target.setContent("replaced")).toBe(true);
                    expect(wire.output).toBe("replaced");
                    expect("state" in wire).toBe(false);
                });

                it("#then a nested state with a top-level input fallback clamps that input", () => {
                    const mixed = {
                        type: "tool",
                        callID: "call-mixed",
                        args: { query: "g".repeat(600) },
                        state: { status: "completed", output: "done" },
                    };
                    const messages: MessageLike[] = [message("m-mixed", "assistant", [mixed])];
                    const index = buildIndex(messages);
                    const batch = new ToolMutationBatch(messages);
                    const target = createToolDropTarget("call-mixed", [], index, batch, 27);

                    expect(target.readInput()).toBe(mixed.args);
                    expect(target.truncate()).toBe("truncated");

                    const wire = messages[0]?.parts[0] as {
                        args: { query: string };
                        state: { output: string };
                    };
                    expect(wire.args.query).toBe("ggggg...[truncated]");
                    expect(wire.state.output).toBe("[dropped \u00a727\u00a7]");
                    expect(mixed.args.query).toBe("g".repeat(600));
                });

                it("#then a large top-level array input collapses to an item count", () => {
                    const arrayInput = {
                        type: "tool",
                        callID: "call-arr",
                        state: {
                            status: "completed",
                            output: "done",
                            input: Array(200).fill("item"),
                        },
                    };
                    const messages: MessageLike[] = [message("m-arr", "assistant", [arrayInput])];
                    const index = buildIndex(messages);
                    const batch = new ToolMutationBatch(messages);
                    const target = createToolDropTarget("call-arr", [], index, batch, 28);

                    expect(target.readInput()).toBeNull();
                    expect(target.truncate()).toBe("truncated");

                    const wire = messages[0]?.parts[0] as { state: { input: unknown } };
                    expect(wire.state.input).toBe("[200 items]");
                    expect(Array.isArray(arrayInput.state.input)).toBe(true);
                });

                it("#then the result payload resolves as a group, state first", () => {
                    const grouped = {
                        type: "tool",
                        callID: "call-grp",
                        output: "top-level output",
                        state: { status: "completed", error: "e".repeat(600) },
                    };
                    const messages: MessageLike[] = [message("m-grp", "assistant", [grouped])];
                    const index = buildIndex(messages);
                    const batch = new ToolMutationBatch(messages);
                    const target = createToolDropTarget("call-grp", [], index, batch, 29);

                    expect(target.truncate()).toBe("truncated");

                    const wire = messages[0]?.parts[0] as Record<string, unknown> & {
                        state: Record<string, unknown>;
                    };
                    expect(wire.state.error).toBe("[dropped \u00a729\u00a7]");
                    expect("output" in wire).toBe(false);
                    expect(grouped.output).toBe("top-level output");
                });
            });
        });

        describe("#given a tool_result part whose payload is under output or result", () => {
            describe("#when it is truncated or replaced", () => {
                it("#then the existing payload field is overwritten rather than shadowed", () => {
                    const viaOutput = {
                        type: "tool_result",
                        tool_use_id: "call-out",
                        output: "big",
                    };
                    const viaResult = {
                        type: "tool_result",
                        tool_use_id: "call-res",
                        result: "big",
                    };
                    const messages: MessageLike[] = [
                        message("m-inv", "assistant", [
                            { type: "tool_use", id: "call-out" },
                            { type: "tool_use", id: "call-res" },
                        ]),
                        message("m-res", "assistant", [viaOutput, viaResult]),
                    ];
                    const index = buildIndex(messages);
                    const batch = new ToolMutationBatch(messages);

                    expect(createToolDropTarget("call-out", [], index, batch, 30).truncate()).toBe(
                        "truncated",
                    );
                    const wireOut = messages[1]?.parts[0] as Record<string, unknown>;
                    expect(wireOut.output).toBe("[dropped \u00a730\u00a7]");
                    expect("content" in wireOut).toBe(false);

                    const resTarget = createToolDropTarget("call-res", [], index, batch, 31);
                    expect(resTarget.setContent("small")).toBe(true);
                    expect(viaResult.result).toBe("small");
                    expect("content" in viaResult).toBe(false);
                });
            });
        });

        describe("#given an OpenCode tool part keyed by a call-id alias", () => {
            describe("#when the index is built", () => {
                it("#then callId and id resolve in codec order", () => {
                    expect(extractToolCallObservation({ type: "tool", callId: "alias-1" })).toEqual(
                        {
                            callId: "alias-1",
                            kind: "result",
                        },
                    );
                    expect(extractToolCallObservation({ type: "tool", id: "prt-2" })).toEqual({
                        callId: "prt-2",
                        kind: "result",
                    });
                    expect(
                        extractToolCallObservation({
                            type: "tool",
                            callID: "primary",
                            id: "prt-3",
                        }),
                    ).toEqual({ callId: "primary", kind: "result" });

                    const aliased = {
                        type: "tool",
                        callId: "call-alias",
                        state: { status: "completed", output: "done" },
                    };
                    const messages: MessageLike[] = [message("m-alias", "assistant", [aliased])];
                    const index = buildIndex(messages);
                    const batch = new ToolMutationBatch(messages);
                    const target = createToolDropTarget("call-alias", [], index, batch, 32);
                    expect(target.canDrop()).toBe(true);
                    expect(target.drop()).toBe("removed");
                });
            });
        });

        describe("#given tool input measured for the clamp cutoff", () => {
            describe("#when the input is multibyte text", () => {
                it("#then the cutoff counts UTF-8 bytes, not UTF-16 code units", () => {
                    // 200 CJK characters: 200 code units, 600 UTF-8 bytes.
                    const cjk = "\u4e2d".repeat(200);
                    const toolPart = {
                        type: "tool",
                        callID: "call-cjk",
                        state: { input: { text: cjk }, output: "done" },
                    };
                    const messages: MessageLike[] = [message("m-cjk", "assistant", [toolPart])];
                    const index = buildIndex(messages);
                    const batch = new ToolMutationBatch(messages);
                    const target = createToolDropTarget("call-cjk", [], index, batch, 15);

                    expect(target.truncate()).toBe("truncated");

                    const wire = messages[0]?.parts[0] as { state: { input: { text: string } } };
                    expect(wire.state.input.text).toBe(`${"\u4e2d".repeat(5)}...[truncated]`);
                });
            });

            describe("#when a long argument ends with the truncation sentinel", () => {
                it("#then only the exact five-scalar clamped shape is exempt from re-clamping", () => {
                    const longTail = `${"y".repeat(600)}...[truncated]`;
                    const toolPart = {
                        type: "tool",
                        callID: "call-tail",
                        state: {
                            input: {
                                payload: longTail,
                                clamped: "abcde...[truncated]",
                                shortHead: "abc...[truncated]",
                            },
                            output: "done",
                        },
                    };
                    const messages: MessageLike[] = [message("m-tail", "assistant", [toolPart])];
                    const index = buildIndex(messages);
                    const batch = new ToolMutationBatch(messages);
                    const target = createToolDropTarget("call-tail", [], index, batch, 16);

                    expect(target.truncate()).toBe("truncated");

                    const wire = messages[0]?.parts[0] as {
                        state: { input: { payload: string; clamped: string; shortHead: string } };
                    };
                    expect(wire.state.input.payload).toBe("yyyyy...[truncated]");
                    expect(wire.state.input.clamped).toBe("abcde...[truncated]");
                    expect(wire.state.input.shortHead).toBe("abc.....[truncated]");
                });
            });

            describe("#when an argument contains non-BMP characters", () => {
                it("#then the clamp counts Unicode scalars, keeping five or fewer intact", () => {
                    const fiveEmoji = "\u{1F600}".repeat(5);
                    const sixEmoji = "\u{1F600}".repeat(6);
                    const toolPart = {
                        type: "tool",
                        callID: "call-emoji",
                        state: {
                            input: { five: fiveEmoji, six: sixEmoji, pad: "p".repeat(600) },
                            output: "done",
                        },
                    };
                    const messages: MessageLike[] = [message("m-emoji", "assistant", [toolPart])];
                    const index = buildIndex(messages);
                    const batch = new ToolMutationBatch(messages);
                    const target = createToolDropTarget("call-emoji", [], index, batch, 19);

                    expect(target.truncate()).toBe("truncated");

                    const wire = messages[0]?.parts[0] as {
                        state: { input: { five: string; six: string } };
                    };
                    expect(wire.state.input.five).toBe(fiveEmoji);
                    expect(wire.state.input.six).toBe(`${fiveEmoji}...[truncated]`);
                });
            });
        });

        describe("#given a completed OpenCode tool that carries attachments", () => {
            describe("#when the result is truncated or replaced", () => {
                it("#then truncate removes attachments from the wire clone only", () => {
                    const toolPart = {
                        type: "tool",
                        callID: "call-att",
                        attachments: [{ type: "file", mime: "image/png", url: "data:..." }],
                        state: {
                            attachments: [{ type: "file", mime: "image/png", url: "data:..." }],
                            output: "screenshot taken",
                        },
                    };
                    const pristine = JSON.stringify(toolPart);
                    const messages: MessageLike[] = [message("m-att", "assistant", [toolPart])];
                    const index = buildIndex(messages);
                    const batch = new ToolMutationBatch(messages);
                    const target = createToolDropTarget("call-att", [], index, batch, 20);

                    expect(target.truncate()).toBe("truncated");

                    const wire = messages[0]?.parts[0] as Record<string, unknown> & {
                        state: Record<string, unknown>;
                    };
                    expect(wire.state.output).toBe("[dropped \u00a720\u00a7]");
                    expect("attachments" in wire.state).toBe(false);
                    expect("attachments" in wire).toBe(false);
                    expect(JSON.stringify(toolPart)).toBe(pristine);
                });

                it("#then setContent removes attachments and reports the change", () => {
                    const toolPart = {
                        type: "tool",
                        callID: "call-att-2",
                        state: {
                            attachments: [{ type: "file", mime: "image/png", url: "data:..." }],
                            output: "same text",
                        },
                    };
                    const messages: MessageLike[] = [message("m-att-2", "assistant", [toolPart])];
                    const index = buildIndex(messages);
                    const batch = new ToolMutationBatch(messages);
                    const target = createToolDropTarget("call-att-2", [], index, batch, 21);

                    // Same text, but the attachment removal is itself a change.
                    expect(target.setContent("same text")).toBe(true);
                    expect("attachments" in toolPart.state).toBe(false);
                    expect(target.setContent("same text")).toBe(false);
                });

                it("#then setContent with different text also removes attachments", () => {
                    const toolPart = {
                        type: "tool",
                        callID: "call-att-3",
                        state: {
                            attachments: [{ type: "file", mime: "image/png", url: "data:..." }],
                            output: "old text",
                        },
                    };
                    const messages: MessageLike[] = [message("m-att-3", "assistant", [toolPart])];
                    const index = buildIndex(messages);
                    const batch = new ToolMutationBatch(messages);
                    const target = createToolDropTarget("call-att-3", [], index, batch, 22);

                    expect(target.setContent("new text")).toBe(true);
                    expect(toolPart.state.output).toBe("new text");
                    expect("attachments" in toolPart.state).toBe(false);
                });
            });
        });

        describe("#given a partless message the batch never touched", () => {
            describe("#when finalize prunes emptied wrappers", () => {
                it("#then the untouched partless message survives", () => {
                    const partlessUser = message("m-user-fence", "user", []);
                    const messages: MessageLike[] = [
                        message("m-inv", "assistant", [{ type: "tool_use", id: "call-old" }]),
                        message("m-res", "tool", [
                            { type: "tool", callID: "call-old", state: { output: "old" } },
                        ]),
                        partlessUser,
                    ];
                    const index = buildIndex(messages);
                    const batch = new ToolMutationBatch(messages);
                    const target = createToolDropTarget("call-old", [], index, batch, 17);

                    expect(target.drop()).toBe("removed");
                    batch.finalize();

                    expect(messages).toHaveLength(1);
                    expect(messages[0]).toBe(partlessUser);
                });
            });
        });

        describe("#given an affected message whose only other part is ignored text", () => {
            describe("#when the tool in it is dropped", () => {
                it("#then finalize prunes the message because ignored text never reaches the provider", () => {
                    const messages: MessageLike[] = [
                        message("m-inv", "assistant", [{ type: "tool_use", id: "call-ign" }]),
                        message("m-res", "assistant", [
                            { type: "text", text: "## Routing Status", ignored: true },
                            { type: "tool", callID: "call-ign", state: { output: "out" } },
                        ]),
                        message("m-keep", "assistant", [{ type: "text", text: "keep me" }]),
                    ];
                    const index = buildIndex(messages);
                    const batch = new ToolMutationBatch(messages);
                    const target = createToolDropTarget("call-ign", [], index, batch, 18);

                    expect(target.drop()).toBe("removed");
                    batch.finalize();

                    expect(messages).toHaveLength(1);
                    expect(messages[0]?.info.id).toBe("m-keep");
                });
            });
        });
    });

    describe("hasMeaningfulPart", () => {
        it("returns false for empty text", () => {
            expect(hasMeaningfulPart({ type: "text", text: "" })).toBe(false);
            expect(hasMeaningfulPart({ type: "text", text: "   " })).toBe(false);
        });

        it("returns false for text containing only tag prefixes", () => {
            expect(hasMeaningfulPart({ type: "text", text: "§424§ " })).toBe(false);
            expect(hasMeaningfulPart({ type: "text", text: "§424§" })).toBe(false);
            expect(hasMeaningfulPart({ type: "text", text: "§424§   " })).toBe(false);
            expect(hasMeaningfulPart({ type: "text", text: "§1§ §2§ " })).toBe(false);
            expect(hasMeaningfulPart({ type: "text", text: '§15298">§15298§ ' })).toBe(false);
            expect(hasMeaningfulPart({ type: "text", text: '§15298">§ ' })).toBe(false);
        });

        it("returns true for text with actual content", () => {
            expect(hasMeaningfulPart({ type: "text", text: "hello" })).toBe(true);
            expect(hasMeaningfulPart({ type: "text", text: "§424§ hello" })).toBe(true);
            expect(hasMeaningfulPart({ type: "text", text: '§15298">§15298§ hello' })).toBe(true);
        });

        it("returns false for ignored text even when it has content", () => {
            expect(hasMeaningfulPart({ type: "text", text: "hidden", ignored: true })).toBe(false);
            expect(hasMeaningfulPart({ type: "text", text: "shown", ignored: false })).toBe(true);
        });

        it("returns true for tools", () => {
            expect(hasMeaningfulPart({ type: "tool" })).toBe(true);
            expect(hasMeaningfulPart({ type: "tool_result" })).toBe(true);
        });

        it("returns false for non-record types", () => {
            expect(hasMeaningfulPart(null)).toBe(false);
            expect(hasMeaningfulPart(undefined)).toBe(false);
            expect(hasMeaningfulPart("string")).toBe(false);
            expect(hasMeaningfulPart(123)).toBe(false);
        });

        it("returns false for ignored part types", () => {
            expect(hasMeaningfulPart({ type: "step-start" })).toBe(false);
            expect(hasMeaningfulPart({ type: "step-finish" })).toBe(false);
            expect(hasMeaningfulPart({ type: "thinking" })).toBe(false);
            expect(hasMeaningfulPart({ type: "reasoning" })).toBe(false);
            expect(hasMeaningfulPart({ type: "redacted_thinking" })).toBe(false);
            expect(hasMeaningfulPart({ type: "meta" })).toBe(false);
            expect(hasMeaningfulPart({ type: "snapshot" })).toBe(false);
            expect(hasMeaningfulPart({ type: "patch" })).toBe(false);
            expect(hasMeaningfulPart({ type: "agent" })).toBe(false);
            expect(hasMeaningfulPart({ type: "retry" })).toBe(false);
        });
    });
});
