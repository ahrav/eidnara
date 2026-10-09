import { afterEach, describe, expect, it } from "bun:test";
import {
    chmodSync,
    existsSync,
    lstatSync,
    mkdtempSync,
    readFileSync,
    rmSync,
    writeFileSync,
} from "node:fs";
import { createServer, type Server, Socket } from "node:net";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { HostClient } from "@eidnara/opencode/shared/host-client";
import { COMPRESSION_FIDELITY_CORPUS_SHA256 } from "../compression-fidelity/corpus";
import {
    __hermeticHostTest,
    buildDirectHostFixture,
    detectRustModePrereqs,
    HermeticHostStack,
    type ScriptMessage,
} from "./hermetic-host";

const fixturePrereqs = detectRustModePrereqs();

const temporaryRoots: string[] = [];

function mode(path: string): number {
    return lstatSync(path).mode & 0o777;
}

async function waitFor<T>(read: () => Promise<T>, predicate: (value: T) => boolean): Promise<T> {
    const deadline = Date.now() + 20_000;
    for (;;) {
        const value = await read();
        if (predicate(value)) return value;
        if (Date.now() >= deadline) throw new Error("fixture state did not converge");
        await Bun.sleep(10);
    }
}

function model_executionCall(client: HostClient, prompt: string): Promise<Record<string, unknown>> {
    return client.call<Record<string, unknown>>(
        "model_execution",
        "session.send",
        {
            prompt,
            model: { provider: "fixture", model: "deterministic" },
            tools: [],
            generation: { max_output_tokens: 1_024, temperature: 0.1 },
        },
        { targetKind: "management_surface" },
    );
}

async function rawControl(path: string, request: Buffer): Promise<Record<string, unknown>> {
    const socket = await new Promise<Socket>((resolveSocket, rejectSocket) => {
        const candidate = new Socket();
        candidate.once("error", rejectSocket);
        candidate.connect(path, () => {
            candidate.off("error", rejectSocket);
            resolveSocket(candidate);
        });
    });
    socket.write(request);
    const response = await new Promise<Buffer>((resolveResponse, rejectResponse) => {
        let bytes = Buffer.alloc(0);
        socket.on("data", (chunk: Buffer) => {
            bytes = Buffer.concat([bytes, chunk]);
            const newline = bytes.indexOf(0x0a);
            if (newline >= 0) resolveResponse(bytes.subarray(0, newline));
            if (bytes.byteLength > __hermeticHostTest.maxLineBytes + 1) {
                rejectResponse(new Error("raw fixture response exceeded cap"));
            }
        });
        socket.once("error", rejectResponse);
    });
    socket.destroy();
    return JSON.parse(response.toString("utf8")) as Record<string, unknown>;
}

async function mockControl(
    responder: (request: Record<string, unknown>, socket: Socket) => void,
): Promise<{
    client: InstanceType<typeof __hermeticHostTest.FixtureControlClient>;
    server: Server;
}> {
    const root = mkdtempSync(join(tmpdir(), "eidnara-control-client-"));
    temporaryRoots.push(root);
    const path = join(root, "control.sock");
    const server = createServer((socket) => {
        socket.on("data", (chunk: Buffer) => {
            const line = chunk.toString("utf8").trim();
            responder(JSON.parse(line) as Record<string, unknown>, socket);
        });
    });
    await new Promise<void>((resolveListen) => server.listen(path, resolveListen));
    const client = new __hermeticHostTest.FixtureControlClient(path, 500);
    await client.connect();
    return { client, server };
}

/** The text a corpus message presents: its text parts, joined as the presenter joins parts. */
function messageText(message: ScriptMessage): string {
    return message.parts
        .filter((part) => part.type === "text")
        .map((part) => String(part.text ?? ""))
        .join(" / ");
}

/** A summarizer-shaped prompt presenting `texts` at ordinals 1.. in the format the daemon renders. */
function summarizerPrompt(texts: readonly string[]): string {
    const lines = texts.map((text, index) => `[${index + 1}] U: ${text}`);
    return `Summarize.\n<new_messages>\n${lines.join("\n")}\n</new_messages>`;
}

afterEach(() => {
    for (const root of temporaryRoots.splice(0)) rmSync(root, { recursive: true, force: true });
});

describe("direct host fixture contract", () => {
    it("parses only the bounded readiness schema and reaps only stale PID records", () => {
        const valid = Buffer.from(
            JSON.stringify({
                status: "ready",
                wire_version: 4,
                catalog: ["context", "local_embeddings", "model_execution"],
                debug_assertions: true,
                model_workers: 8,
            }),
        );
        expect(__hermeticHostTest.parseReadyRecord(valid).status).toBe("ready");
        const priorFixture = Buffer.from(
            JSON.stringify({
                status: "ready",
                wire_version: 4,
                catalog: ["context", "local_embeddings", "model_execution"],
            }),
        );
        expect(__hermeticHostTest.parseReadyRecord(priorFixture).status).toBe("ready");
        for (const partial of [
            { debug_assertions: true },
            { model_workers: 8 },
            { debug_assertions: "yes", model_workers: 8 },
            { debug_assertions: true, model_workers: 1.5 },
        ]) {
            expect(() =>
                __hermeticHostTest.parseReadyRecord(
                    Buffer.from(
                        JSON.stringify({
                            status: "ready",
                            wire_version: 4,
                            catalog: ["context", "local_embeddings", "model_execution"],
                            ...partial,
                        }),
                    ),
                ),
            ).toThrow();
        }
        expect(() =>
            __hermeticHostTest.parseReadyRecord(
                Buffer.from('{"status":"ready","wire_version":4,"catalog":[],"key":"secret"}'),
            ),
        ).toThrow();
        expect(() =>
            __hermeticHostTest.parseReadyRecord(
                Buffer.alloc(__hermeticHostTest.maxLineBytes + 1, 0x78),
            ),
        ).toThrow();

        const nowMs = 10 * __hermeticHostTest.stalePidAgeMs;
        expect(
            __hermeticHostTest.isStaleRustE2ePidRecord(
                nowMs - __hermeticHostTest.stalePidAgeMs + 1,
                nowMs,
            ),
        ).toBe(false);
        expect(
            __hermeticHostTest.isStaleRustE2ePidRecord(
                nowMs - __hermeticHostTest.stalePidAgeMs,
                nowMs,
            ),
        ).toBe(true);
        expect(__hermeticHostTest.isStaleRustE2ePidRecord(nowMs + 1, nowMs)).toBe(false);
    });

    it("rejects readiness emitted before control and secure publication exist", async () => {
        const root = mkdtempSync(join(tmpdir(), "opencode-e2e-early-ready-"));
        temporaryRoots.push(root);
        const fixtureBin = join(root, "early-ready-fixture.sh");
        writeFileSync(
            fixtureBin,
            `#!/bin/sh\nprintf '%s\\n' '{"status":"ready","wire_version":4,"catalog":["context","local_embeddings","model_execution"],"debug_assertions":true,"model_workers":8}'\nsleep 1\nmkdir -p "$2/eidnara/run"\n: > "$2/direct-host-control.sock"\n: > "$2/eidnara/run/connection.json"\nsleep 60\n`,
            { mode: 0o700 },
        );

        const startupError = await HermeticHostStack.start({
            dataDir: root,
            fixtureBin,
            startTimeoutMs: 2_000,
        }).catch((error: unknown) => error);
        expect(String(startupError)).toContain("direct host readiness preceded secure publication");
        expect(existsSync(root)).toBe(false);
        temporaryRoots.splice(temporaryRoots.indexOf(root), 1);
    }, 15_000);

    it("rejects malformed, unknown, oversized, mismatched, and duplicate responses", async () => {
        const cases: Array<(request: Record<string, unknown>, socket: Socket) => void> = [
            (_request, socket) => socket.write("not-json\n"),
            (request, socket) =>
                socket.write(
                    `${JSON.stringify({ id: request.id, ok: true, result: { accepted: true }, extra: true })}\n`,
                ),
            (_request, socket) =>
                socket.write(`${"x".repeat(__hermeticHostTest.maxLineBytes + 1)}\n`),
            (_request, socket) =>
                socket.write(
                    `${JSON.stringify({ id: 999, ok: true, result: { accepted: true } })}\n`,
                ),
        ];
        for (const responder of cases) {
            const { client, server } = await mockControl(responder);
            expect(await client.backendSuccess().catch((error: unknown) => error)).toBeInstanceOf(
                Error,
            );
            client.close();
            await new Promise<void>((resolveClose) => server.close(() => resolveClose()));
        }

        const { client, server } = await mockControl((request, socket) => {
            const response = `${JSON.stringify({ id: request.id, ok: true, result: { accepted: true } })}\n`;
            socket.write(response + response);
        });
        await client.backendSuccess();
        await Bun.sleep(20);
        expect(await client.counters().catch((error: unknown) => error)).toBeInstanceOf(Error);
        client.close();
        await new Promise<void>((resolveClose) => server.close(() => resolveClose()));
    });

    it.skipIf(!fixturePrereqs.ok)(
        "proves permissions, controls, managed readiness, redaction, and JSONL shutdown",
        async () => {
            const fixtureBin = await buildDirectHostFixture();
            const root = mkdtempSync(join(tmpdir(), "opencode-e2e-direct-host-"));
            temporaryRoots.push(root);
            chmodSync(root, 0o700);
            const stack = await HermeticHostStack.start({ dataDir: root, fixtureBin });
            const sentinel = "u7-request-sentinel-DO-NOT-LOG";
            try {
                expect(mode(root)).toBe(0o700);
                expect(mode(stack.controlPath)).toBe(0o600);
                expect(mode(stack.connectionFile)).toBe(0o600);

                const publicationText = readFileSync(stack.connectionFile, "utf8");
                const publication = JSON.parse(publicationText) as {
                    key: number[];
                    daemon_id: number[];
                };
                const secretRenderings = [
                    publication.key.join(","),
                    publication.key.join(", "),
                    Buffer.from(publication.key).toString("hex"),
                    Buffer.from(publication.key).toString("hex").toUpperCase(),
                    Buffer.from(publication.key).toString("base64"),
                    publication.daemon_id.join(","),
                    publication.daemon_id.join(", "),
                    Buffer.from(publication.daemon_id).toString("hex"),
                    Buffer.from(publication.daemon_id).toString("hex").toUpperCase(),
                    Buffer.from(publication.daemon_id).toString("base64"),
                    publicationText,
                    JSON.stringify(publication),
                ];

                const before = await stack.backendCounters();
                const controlResponses: string[] = [];
                const thrownErrors: string[] = [];
                const malformedControls = [
                    Buffer.from(`{"id":30,"sentinel":"${sentinel}","command":\n`),
                    Buffer.from(
                        `${JSON.stringify({ id: 31, sentinel, command: { name: "unknown" } })}\n`,
                    ),
                    Buffer.concat([
                        Buffer.from(`{"id":32,"sentinel":"${sentinel}","padding":"`),
                        Buffer.alloc(__hermeticHostTest.maxLineBytes + 1, 0x78),
                        Buffer.from('"}\n'),
                    ]),
                ];
                for (const bytes of malformedControls) {
                    try {
                        const response = await rawControl(stack.controlPath, bytes);
                        expect(response.ok).toBe(false);
                        controlResponses.push(JSON.stringify(response));
                    } catch (error) {
                        thrownErrors.push(String(error));
                    }
                }
                expect(controlResponses.length + thrownErrors.length).toBe(
                    malformedControls.length,
                );
                expect(await stack.backendCounters()).toEqual(before);

                const callFor = async (session: string, prompt: string): Promise<void> => {
                    const client = await HostClient.connect({
                        connectionFile: stack.connectionFile,
                        identity: { project_root: root, harness: "opencode", session },
                        targetKind: "management_surface",
                    });
                    try {
                        expect((await model_executionCall(client, prompt)).run_id).toBeString();
                    } finally {
                        await client.closeAsync();
                    }
                };

                await stack.backendSuccess();
                await callFor("fixture-success", sentinel);
                await waitFor(
                    () => stack.backendCounters(),
                    (counters) => counters.completed === before.completed + 1,
                );

                expect(await stack.releaseBlockedBackendCall()).toBe(false);
                await stack.blockNextBackendCall();
                await callFor("fixture-blocked", "blocked request body");
                await waitFor(
                    () => stack.backendCounters(),
                    (counters) => counters.blocked === before.blocked + 1,
                );
                expect(await stack.releaseBlockedBackendCall()).toBe(true);
                await waitFor(
                    () => stack.backendCounters(),
                    (counters) => counters.released === before.released + 1,
                );

                await stack.failNextBackendCall();
                await callFor("fixture-failure", "typed outage");
                await waitFor(
                    () => stack.backendCounters(),
                    (counters) => counters.failed === before.failed + 1,
                );
                const diagnostics = __hermeticHostTest.diagnostics(stack);
                const observedOutputs = [
                    ...controlResponses,
                    ...thrownErrors,
                    diagnostics.stdout,
                    diagnostics.stderr,
                    diagnostics.retainedLog,
                    stack.hostLog(),
                ];
                const forbidden = [sentinel, ...secretRenderings];
                expect(
                    observedOutputs.some((output) =>
                        forbidden.some((secret) => secret.length > 0 && output.includes(secret)),
                    ),
                ).toBe(false);
                await stack.stop();
                expect(existsSync(root)).toBe(false);
                temporaryRoots.splice(temporaryRoots.indexOf(root), 1);
            } finally {
                await stack.stop();
            }
        },
        180_000,
    );

    it.skipIf(!fixturePrereqs.ok)(
        "binds queued corpus scripts to presented ordinals and fails mismatch and exhaustion typed",
        async () => {
            const fixtureBin = await buildDirectHostFixture();
            const root = mkdtempSync(join(tmpdir(), "opencode-e2e-direct-host-script-"));
            temporaryRoots.push(root);
            const stack = await HermeticHostStack.start({ dataDir: root, fixtureBin });
            const send = async (session: string, prompt: string): Promise<void> => {
                const client = await HostClient.connect({
                    connectionFile: stack.connectionFile,
                    identity: { project_root: root, harness: "opencode", session },
                    targetKind: "management_surface",
                });
                try {
                    expect((await model_executionCall(client, prompt)).run_id).toBeString();
                } finally {
                    await client.closeAsync();
                }
            };
            const settled = (before: number) =>
                waitFor(
                    () => stack.backendCounters(),
                    (counters) => counters.completed + counters.failed === before + 1,
                );
            try {
                const source = await stack.scriptSource("C1.S1");
                expect(source).toMatchObject({
                    corpusSha256: COMPRESSION_FIDELITY_CORPUS_SHA256,
                    case: "C1",
                    scenario: "C1.S1",
                    source: "C1.V1",
                });
                expect(source.leakProbes.length).toBeGreaterThan(0);
                expect(source.messages.length).toBe(6);
                const caseTexts = source.messages.map(messageText);

                const rejection = (promise: Promise<unknown>) =>
                    promise.then(
                        () => "accepted",
                        (error: unknown) => String(error),
                    );
                expect(await rejection(stack.scriptCases(["C9.S1"]))).toContain("unknown_scenario");
                expect(await rejection(stack.scriptSource("C1.V1"))).toContain("unknown_scenario");
                expect(await rejection(stack.scriptCases([]))).toContain("empty_queue");
                expect(await stack.userHintOutcome()).toBeNull();
                // The kernel opens with the first project session, so a fresh stack reports
                // kernel_unavailable for both admission controls.
                expect(
                    await rejection(stack.memoryAdmission("mem_missing", "quarantine")),
                ).toContain("kernel_unavailable");
                expect(
                    await rejection(
                        stack.memorySeed("mem_missing", "mem_new", "ARCHITECTURE", "x"),
                    ),
                ).toContain("kernel_unavailable");
                expect(await rejection(stack.scriptCases(Array(9).fill("C1.S1")))).toContain(
                    "queue_too_long",
                );
                const oversized = await rawControl(
                    stack.controlPath,
                    Buffer.from(
                        `${JSON.stringify({
                            id: 40,
                            command: {
                                name: "script-cases",
                                entries: Array(8).fill(
                                    "C1.S1".padEnd(__hermeticHostTest.maxLineBytes / 4, " "),
                                ),
                            },
                        })}\n`,
                    ),
                );
                expect(oversized).toMatchObject({
                    ok: false,
                    error: { code: "request_too_large" },
                });
                const idle = {
                    corpusSha256: COMPRESSION_FIDELITY_CORPUS_SHA256,
                    armed: false,
                    remaining: 0,
                    bound: 0,
                    filled: 0,
                    mismatched: 0,
                    exhausted: 0,
                    fillerImportance: 30,
                    echoImportance: 50,
                    bindings: [],
                };
                expect(await stack.scriptStatus()).toEqual(idle);

                // Unarmed, a summarizer prompt gets the default scripted answer.
                let counters = await stack.backendCounters();
                const unarmed = counters;
                await send("script-default", summarizerPrompt(caseTexts));
                counters = await settled(counters.completed + counters.failed);
                expect(counters.completed).toBe(unarmed.completed + 1);
                expect(await stack.scriptStatus()).toEqual(idle);

                await stack.scriptCases(["filler", "C1.S1", "C1.S1", "C2.S1"]);
                expect(await stack.scriptStatus()).toMatchObject({ armed: true, remaining: 4 });

                // A filler entry answers with compact fixture-authored segments.
                const filled = counters;
                await send("script-filler", summarizerPrompt(["filler one", "filler two"]));
                counters = await settled(filled.completed + filled.failed);
                expect(counters.completed).toBe(filled.completed + 1);
                expect(await stack.scriptStatus()).toMatchObject({
                    remaining: 3,
                    bound: 0,
                    filled: 1,
                });

                // A lead-in record before the case and a trailing record after it.
                const before = counters;
                await send(
                    "script-bound",
                    summarizerPrompt(["lead-in note", ...caseTexts, "trailing note"]),
                );
                counters = await settled(before.completed + before.failed);
                expect(counters.completed).toBe(before.completed + 1);
                const bound = await stack.scriptStatus();
                expect(bound).toMatchObject({
                    remaining: 2,
                    bound: 1,
                    filled: 1,
                    mismatched: 0,
                    exhausted: 0,
                });
                expect(bound.bindings).toEqual([
                    {
                        scenario: "C1.S1",
                        source: "C1.V1",
                        start: 2,
                        end: 7,
                        unprocessedFrom: 8,
                        outputSha256: expect.stringMatching(/^[0-9a-f]{64}$/),
                    },
                ]);

                // A scheduled typed failure consumes no entry.
                const scheduled = counters;
                await stack.failNextBackendCall();
                await send("script-scheduled-failure", summarizerPrompt(caseTexts));
                counters = await settled(scheduled.completed + scheduled.failed);
                expect(counters.failed).toBe(scheduled.failed + 1);
                expect((await stack.scriptStatus()).remaining).toBe(2);

                // A blocked call is answered from the script once released.
                const blocked = counters;
                await stack.blockNextBackendCall();
                await send("script-blocked", summarizerPrompt(caseTexts));
                await waitFor(
                    () => stack.backendCounters(),
                    (value) => value.blocked === blocked.blocked + 1,
                );
                expect((await stack.scriptStatus()).bindings).toHaveLength(1);
                expect(await stack.releaseBlockedBackendCall()).toBe(true);
                counters = await settled(blocked.completed + blocked.failed);
                expect(counters.completed).toBe(blocked.completed + 1);
                const released = await stack.scriptStatus();
                expect(released).toMatchObject({ remaining: 1, bound: 2 });
                expect(released.bindings.map((binding) => [binding.start, binding.end])).toEqual([
                    [2, 7],
                    [1, 6],
                ]);

                // A non-summarizer prompt consumes nothing and keeps its default answer.
                const plain = counters;
                await send("script-plain", "not a summarizer prompt");
                counters = await settled(plain.completed + plain.failed);
                expect(counters.completed).toBe(plain.completed + 1);
                expect((await stack.scriptStatus()).remaining).toBe(1);

                // C2.S1 is next; C1's messages do not match it.
                const mismatch = counters;
                await send("script-mismatch", summarizerPrompt(caseTexts));
                counters = await settled(mismatch.completed + mismatch.failed);
                expect(counters.failed).toBe(mismatch.failed + 1);
                expect(await stack.scriptStatus()).toMatchObject({
                    remaining: 0,
                    bound: 2,
                    mismatched: 1,
                    exhausted: 0,
                });

                const exhausted = counters;
                await send("script-exhausted", summarizerPrompt(caseTexts));
                counters = await settled(exhausted.completed + exhausted.failed);
                expect(counters.failed).toBe(exhausted.failed + 1);
                expect(counters.completed).toBe(exhausted.completed);
                expect(await stack.scriptStatus()).toMatchObject({
                    remaining: 0,
                    bound: 2,
                    mismatched: 1,
                    exhausted: 1,
                });
            } finally {
                await stack.stop();
            }
        },
        180_000,
    );

    it.skipIf(!fixturePrereqs.ok)(
        "routes SIGTERM through fixture cleanup",
        async () => {
            const fixtureBin = await buildDirectHostFixture();
            const root = mkdtempSync(join(tmpdir(), "opencode-e2e-direct-host-term-"));
            temporaryRoots.push(root);
            const stack = await HermeticHostStack.start({ dataDir: root, fixtureBin });
            await stack.blockNextBackendCall();
            const client = await HostClient.connect({
                connectionFile: stack.connectionFile,
                identity: { project_root: root, harness: "opencode", session: "sigterm" },
                targetKind: "management_surface",
            });
            await model_executionCall(client, "sigterm sentinel request");
            await waitFor(
                () => stack.backendCounters(),
                (counters) => counters.blocked >= 1,
            );
            await client.closeAsync().catch(() => undefined);
            await stack.terminateHost();
            expect(existsSync(stack.controlPath)).toBe(false);
            expect(existsSync(stack.connectionFile)).toBe(false);
            await stack.stop();
            expect(existsSync(root)).toBe(false);
            temporaryRoots.splice(temporaryRoots.indexOf(root), 1);
        },
        180_000,
    );
});
