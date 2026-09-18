/// <reference types="bun-types" />

import { afterEach, describe, expect, it, jest, mock, spyOn } from "bun:test";
import { mkdtempSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import {
    clearHookInitFailure,
    getLastHookInitFailure,
} from "../../features/context/fail-closed-block";
import {
    __resetProjectIdentityForTests,
    resolveProjectIdentityForSession,
} from "../../features/context/project-identity";
import { createMessagesTransformHandler } from "../../plugin/messages-transform";
import * as logger from "../../shared/logger";
import { TimeoutError } from "../../shared/with-timeout";
import * as permissionAvailability from "./eidnara-reduce-availability";
import {
    clearToolPermissionDenied,
    peekToolPermissionDeniedForTest,
} from "./eidnara-reduce-availability";
import { createEidnaraHook, type EidnaraDeps } from "./hook";
import {
    createKernelClient,
    resetKernelClientsForTest,
    sharedStateForTest,
} from "./kernel-transport";
import { createLiveSessionState } from "./live-session-state";
import { isModuleCallBodyValid } from "./module-transport";
import { setRawMessageProvider } from "./read-session-chunk";
import { closeReadOnlySessionDb } from "./read-session-db";
import type { RawMessage } from "./read-session-raw";
import type { RustModeModuleClient } from "./rust-mode-transform";
import type { MessageLike } from "./tag-content-primitives";
import { defaultTransformCaptureAdmission } from "./transform-capture";

type RecordedCall = {
    sessionId: string;
    projectRoot: string;
    method: string;
    body: unknown;
    signal?: AbortSignal;
};

type FakeModuleClient = {
    client: RustModeModuleClient;
    calls: RecordedCall[];
    deleteSession: ReturnType<typeof mock>;
    closeSession: ReturnType<typeof mock>;
};

const HOOK_KEYS = [
    "experimental.chat.messages.transform",
    "experimental.chat.system.transform",
    "experimental.text.complete",
    "chat.message",
    "event",
    "command.execute.before",
    "tool.execute.after",
].sort();

const tempDirs: string[] = [];
const unregisterProviders: Array<() => void> = [];
const originalXdgDataHome = process.env.XDG_DATA_HOME;

/** An empty data home has no `opencode.db`, so tool verdicts freeze fail-open and prompt hashes persist. */
function useTempDataHome(prefix: string): string {
    const dir = mkdtempSync(join(tmpdir(), prefix));
    tempDirs.push(dir);
    process.env.XDG_DATA_HOME = dir;
    return dir;
}

/** One raw user message lets the transform resolve ordinals without an OpenCode session DB. */
function installOneRawMessage(sessionId: string): MessageLike[] {
    const row = { id: "m-1", timeCreated: 1, contributesOrdinal: true, hasValidInfo: true };
    unregisterProviders.push(
        setRawMessageProvider(sessionId, {
            readMessages: () => [row] as unknown as RawMessage[],
            readMessageOrdinalPage: (after) => (after ? [] : [row]),
            getStoredMessageCount: () => 1,
        }),
    );
    return [
        {
            info: { id: row.id, role: "user", sessionID: sessionId },
            parts: [{ type: "text", text: "hello" }],
        },
    ];
}

afterEach(() => {
    expect(defaultTransformCaptureAdmission.activePasses).toBe(0);
    expect(defaultTransformCaptureAdmission.chargedBytes).toBe(0);
    jest.useRealTimers();
    mock.restore();
    closeReadOnlySessionDb();
    resetKernelClientsForTest();
    for (const unregister of unregisterProviders.splice(0)) unregister();
    __resetProjectIdentityForTests();
    clearHookInitFailure();
    process.env.XDG_DATA_HOME = originalXdgDataHome;
    for (const dir of tempDirs) {
        try {
            rmSync(dir, { recursive: true, force: true, maxRetries: 10, retryDelay: 100 });
        } catch {
            /* Windows can retain file handles after tests; cleanup tolerates deletion failures. */
        }
    }
    tempDirs.length = 0;
});

/** A daemon answer that inserts `output` whole, bound to the transform body's base revision. */
function recipeResponse(
    body: unknown,
    output: readonly unknown[],
    extra: Record<string, unknown> = {},
): Record<string, unknown> {
    const request = body as Record<string, unknown>;
    return {
        ...extra,
        base_revision: request.base_revision,
        output_revision: `out-${crypto.randomUUID()}`,
        operations: output.length === 0 ? [] : [{ op: "insert", values: output }],
    };
}

function createFakeModuleClient(
    respond: (call: RecordedCall) => unknown = () => ({ ok: true }),
): FakeModuleClient {
    const calls: RecordedCall[] = [];
    const deleteSession = mock(async () => {});
    const closeSession = mock(() => {});
    const client: RustModeModuleClient = {
        call: async ({ sessionId, projectRoot, method, body, signal }) => {
            if (!isModuleCallBodyValid(method, body)) {
                throw new TypeError(`invalid fake module body for ${method}`);
            }
            const call = { sessionId, projectRoot, method, body, signal };
            calls.push(call);
            return respond(call);
        },
        deleteSession,
        closeSession,
    };
    return { client, calls, deleteSession, closeSession };
}

function createClientMock(promptMock = mock(() => undefined), sessionDirectory?: string) {
    return {
        session: {
            prompt: promptMock,
            promptAsync: mock(async () => undefined),
            get: mock(async () => ({
                data: sessionDirectory === undefined ? {} : { directory: sessionDirectory },
            })),
        },
        app: { agents: mock(async () => ({ data: [] })) },
        tui: { showToast: mock(async () => undefined) },
    } as unknown as EidnaraDeps["client"];
}

function createDeps(overrides: Partial<EidnaraDeps> = {}): EidnaraDeps {
    return {
        client: createClientMock(),
        directory: "/tmp",
        config: { protected_tags: 3, cache_ttl: "5m", transform_mode: "rust" },
        rustModeModuleClient: createFakeModuleClient().client,
        ...overrides,
    };
}

function requireHook(
    hook: ReturnType<typeof createEidnaraHook>,
): NonNullable<ReturnType<typeof createEidnaraHook>> {
    expect(hook).not.toBeNull();
    return hook!;
}

async function expectSentinel(promise: Promise<unknown>, sentinel: string): Promise<void> {
    try {
        await promise;
        throw new Error(`Expected sentinel ${sentinel}`);
    } catch (error) {
        expect(String(error)).toContain(sentinel);
    }
}

describe("eidnara hook", () => {
    for (const entry of ["hook", "wrapper"] as const) {
        it.each([
            "info",
            "messages",
        ])(`accepts hidden own %s data through the actual ${entry}`, async (field) => {
            useTempDataHome("hook-hidden-data-");
            const sessionId = `ses-hidden-${entry}-${field}`;
            const messages = installOneRawMessage(sessionId);
            const approved = {
                info: { id: "approved" },
                parts: [{ type: "text", text: "result" }],
            };
            const fake = createFakeModuleClient(({ body }) => recipeResponse(body, [approved]));
            const hook = requireHook(
                createEidnaraHook(createDeps({ rustModeModuleClient: fake.client })),
            );
            const output = { messages };
            if (field === "info") Object.defineProperty(messages[0], field, { enumerable: false });
            else Object.defineProperty(output, field, { enumerable: false });
            if (entry === "wrapper") {
                const wrapper = createMessagesTransformHandler({
                    eidnara: hook,
                    transformMode: "rust",
                });
                const result: unknown = await wrapper({}, output as never);
                expect(result).toBe(messages);
            } else await hook["experimental.chat.messages.transform"]({}, output);
            expect(fake.calls.map((call) => call.method)).toEqual(["transform"]);
            expect(output.messages).toBe(messages);
            expect(messages[0]).toBe(approved);
        });

        for (const unsupported of [
            "output-proxy",
            "root-proxy",
            "message-proxy",
            "index-getter",
            "nested-getter",
            "then",
        ] as const) {
            it(`rejects ${unsupported} at the actual ${entry} entry without triggering reads`, async () => {
                useTempDataHome("hook-source-traps-");
                const sessionId = `ses-${entry}-${unsupported}`;
                const fake = createFakeModuleClient(({ body }) => recipeResponse(body, []));
                const client = createClientMock();
                const hook = requireHook(
                    createEidnaraHook(createDeps({ client, rustModeModuleClient: fake.client })),
                );
                const messages = installOneRawMessage(sessionId);
                const member = messages[0];
                const trap = mock((_target?: object, key?: PropertyKey): never => {
                    throw new Error(`source trap invoked: ${String(key)}`);
                });
                const handlers = {
                    get: trap,
                    ownKeys: trap,
                    getPrototypeOf: trap,
                    getOwnPropertyDescriptor: trap,
                };
                let array = messages;
                if (unsupported === "root-proxy")
                    array = new Proxy<MessageLike[]>(messages, handlers);
                else if (unsupported === "message-proxy")
                    messages[0] = new Proxy<MessageLike>(member, handlers);
                else if (unsupported === "index-getter")
                    Object.defineProperty(messages, "0", {
                        get: trap,
                        enumerable: true,
                        configurable: true,
                    });
                else if (unsupported === "nested-getter")
                    Object.defineProperty(member, "parts", { get: trap });
                else if (unsupported === "then")
                    Object.defineProperty(messages, unsupported, { get: trap });
                const slotBefore = Object.getOwnPropertyDescriptor(messages, "0");
                const output = { messages: array };
                const argument =
                    unsupported === "output-proxy"
                        ? new Proxy<typeof output>(output, handlers)
                        : output;
                if (entry === "wrapper") {
                    const wrapper = createMessagesTransformHandler({
                        eidnara: hook,
                        transformMode: "rust",
                    });
                    const returned: unknown = await wrapper({}, argument as never);
                    if (
                        unsupported === "output-proxy" ||
                        unsupported === "root-proxy" ||
                        unsupported === "then"
                    )
                        expect(returned).toBeUndefined();
                    else expect(returned).toBe(array);
                } else await hook["experimental.chat.messages.transform"]({}, argument);
                expect(output.messages).toBe(array);
                expect(Object.getOwnPropertyDescriptor(messages, "0")).toEqual(slotBefore);
                expect(trap).not.toHaveBeenCalled();
                expect(client.session.get).not.toHaveBeenCalled();
                expect(client.app.agents).not.toHaveBeenCalled();
                expect(fake.calls).toHaveLength(0);
            });
        }

        it(`preserves host array and returned payload identity through the actual ${entry}`, async () => {
            useTempDataHome("hook-publish-identity-");
            const sessionId = `ses-publish-${entry}`;
            const returned = {
                info: { id: "returned", sessionID: sessionId, role: "user" },
                parts: [{ type: "text", text: "transformed" }],
            };
            const fake = createFakeModuleClient(({ body }) => recipeResponse(body, [returned]));
            const hook = requireHook(
                createEidnaraHook(createDeps({ rustModeModuleClient: fake.client })),
            );
            const messages = installOneRawMessage(sessionId);
            const output = { messages };
            if (entry === "wrapper") {
                const wrapper = createMessagesTransformHandler({
                    eidnara: hook,
                    transformMode: "rust",
                });
                expect((await wrapper({}, output as never)) as unknown[]).toBe(messages);
            } else await hook["experimental.chat.messages.transform"]({}, output);
            expect(output.messages).toBe(messages);
            expect(output.messages).toHaveLength(1);
            expect(output.messages[0]).toBe(returned);
            expect(output.messages[0].parts).toBe(returned.parts);
            expect(fake.calls.map((call) => call.method)).toEqual(["transform"]);
        });
    }

    it("logs a byte-budget decline at warn before preflight", async () => {
        useTempDataHome("hook-source-limit-");
        const fake = createFakeModuleClient(({ body }) => recipeResponse(body, []));
        const client = createClientMock();
        const hook = requireHook(
            createEidnaraHook(createDeps({ client, rustModeModuleClient: fake.client })),
        );
        const sessionId = "ses-byte-budget";
        // Forty two-MiB strings charge more than the 64 MiB owner can grant.
        const messages = Array.from({ length: 40 }, (_, index) => ({
            info: { id: `m-${index}`, role: "user", sessionID: sessionId },
            parts: [{ type: "text", text: "x".repeat(2 ** 21) }],
        }));
        const warn = spyOn(logger.sessionLog, "warn");
        try {
            await hook["experimental.chat.messages.transform"]({}, { messages });
            expect(
                warn.mock.calls.some(
                    ([session, message]) =>
                        session === sessionId &&
                        String(message).includes("pass declined: capture_bytes"),
                ),
            ).toBe(true);
        } finally {
            warn.mockRestore();
        }
        expect(client.session.get).not.toHaveBeenCalled();
        expect(fake.calls).toHaveLength(0);
    });

    it("captures the source once per pass", async () => {
        useTempDataHome("hook-single-capture-");
        const sessionId = "ses-single-capture";
        const messages = installOneRawMessage(sessionId);
        const member = messages[0]!;
        const approved = { info: { id: "approved" }, parts: [{ type: "text", text: "result" }] };
        const fake = createFakeModuleClient(({ body }) => recipeResponse(body, [approved]));
        const hook = requireHook(
            createEidnaraHook(createDeps({ rustModeModuleClient: fake.client })),
        );
        const define = Object.defineProperty;
        let memberSlotDefinitions = 0;
        const spy = spyOn(Object, "defineProperty").mockImplementation(
            (target, key, descriptor) => {
                if (Array.isArray(target) && "value" in descriptor && descriptor.value === member)
                    memberSlotDefinitions += 1;
                return define(target, key, descriptor);
            },
        );
        try {
            await hook["experimental.chat.messages.transform"]({}, { messages });
        } finally {
            spy.mockRestore();
        }
        expect(fake.calls.map((call) => call.method)).toEqual(["transform"]);
        expect(messages[0]).toBe(approved);
        expect(memberSlotDefinitions).toBe(1);
    });

    it("rejects source mutation during hook directory lookup before direct transform", async () => {
        useTempDataHome("hook-source-directory-");
        const sessionId = "ses-source-directory";
        const messages = installOneRawMessage(sessionId);
        const fake = createFakeModuleClient(({ body }) => recipeResponse(body, []));
        const client = createClientMock();
        const started = Promise.withResolvers<void>();
        const release = Promise.withResolvers<void>();
        client.session.get = mock(async () => {
            started.resolve();
            await release.promise;
            return { data: { directory: "/tmp" } };
        }) as never;
        const hook = requireHook(
            createEidnaraHook(createDeps({ client, rustModeModuleClient: fake.client })),
        );
        const trap = mock(() => "changed");
        const output = { messages };
        const pending = hook["experimental.chat.messages.transform"]({}, output);
        try {
            await Promise.race([started.promise, pending]);
            expect(client.session.get).toHaveBeenCalledTimes(1);
            Object.defineProperty(messages[0], "parts", { get: trap });
            release.resolve();
            await pending;
            expect(trap).not.toHaveBeenCalled();
            expect(fake.calls).toHaveLength(0);
            expect(output.messages).toBe(messages);
            expect(messages.length).toBe(1);
            expect(Object.getOwnPropertyDescriptor(messages[0], "parts")?.get).toBe(trap);
        } finally {
            release.resolve();
            await pending;
        }
    });

    it("admits before the actual hook directory await and never restores a rebound output", async () => {
        useTempDataHome("hook-directory-admission-");
        const sessionId = "ses-directory-admission";
        const started = Promise.withResolvers<void>();
        const directory = Promise.withResolvers<unknown>();
        const client = createClientMock();
        const get = mock(() => {
            started.resolve();
            return directory.promise;
        });
        client.session.get = get as never;
        const fake = createFakeModuleClient(({ body }) => recipeResponse(body, []));
        const hook = requireHook(
            createEidnaraHook(createDeps({ client, rustModeModuleClient: fake.client })),
        );
        const messages = installOneRawMessage(sessionId);
        const original = messages[0];
        const output = { messages };
        const first = hook["experimental.chat.messages.transform"]({}, output);
        const debugSpy = spyOn(logger.sessionLog, "debug");
        try {
            await Promise.race([started.promise, first]);
            expect(get).toHaveBeenCalledTimes(1);
            const replacement = [
                { info: { id: "host", role: "user", sessionID: sessionId }, parts: [] },
            ];
            output.messages = replacement;
            const secondMessages = installOneRawMessage(sessionId);
            const secondMember = secondMessages[0];
            const secondOutput = { messages: secondMessages };
            await hook["experimental.chat.messages.transform"]({}, secondOutput);
            expect(get).toHaveBeenCalledTimes(1);
            expect(fake.calls).toHaveLength(0);
            // The second call declines on the held session slot, not on any capture check.
            expect(
                debugSpy.mock.calls
                    .filter(([session]) => session === sessionId)
                    .map(([, message]) => message),
            ).toContain("rust transform declined before dispatch: session_busy");
            expect(secondOutput.messages).toBe(secondMessages);
            expect(secondOutput.messages[0]).toBe(secondMember);
            directory.resolve({ data: { directory: "/tmp" } });
            await first;
            expect(output.messages).toBe(replacement);
            expect(output.messages[0]).toBe(replacement[0]);
            expect(messages[0]).toBe(original);
            expect(fake.calls).toHaveLength(0);
        } finally {
            directory.resolve({ data: { directory: "/tmp" } });
            await first;
            debugSpy.mockRestore();
        }
    });

    for (const failure of ["rejection", "timeout"] as const) {
        it(`keeps todowrite absent after a cached deny and live ${failure}`, async () => {
            useTempDataHome("hook-permission-failure-");
            jest.useFakeTimers();
            const sessionId = `ses-deny-read-${failure}`;
            const logMock = Object.assign(
                mock(() => {}),
                {
                    debug: mock(() => {}),
                    info: mock(() => {}),
                    warn: mock(() => {}),
                    error: mock(() => {}),
                },
            );
            const logSpy = spyOn(logger, "sessionLog").mockImplementation(logMock);
            Object.assign(logSpy, logMock);
            clearToolPermissionDenied(sessionId);
            const client = createClientMock();
            const agents = mock(async () => ({
                data: [{ name: "build", permission: { todowrite: "deny" } }],
            }));
            client.app.agents = agents as never;
            const fake = createFakeModuleClient(({ body }) => recipeResponse(body, []));
            const hook = requireHook(
                createEidnaraHook(createDeps({ client, rustModeModuleClient: fake.client })),
            );
            const messages = installOneRawMessage(sessionId);
            Object.assign(messages[0]!.info!, { agent: "build" });
            const transform = () =>
                hook["experimental.chat.messages.transform"]({}, { messages: [...messages] });
            await transform();
            expect(peekToolPermissionDeniedForTest(sessionId, "todowrite", "build")).toBe(true);

            for (const consume of [
                transform,
                () =>
                    hook["tool.execute.after"]({
                        tool: "todowrite",
                        sessionID: sessionId,
                        agent: "build",
                        args: {
                            todos: [{ content: "Blocked", status: "pending", priority: "high" }],
                        },
                    }),
            ]) {
                if (failure === "rejection") {
                    await hook.event({
                        event: { type: "session.updated", properties: { info: { id: sessionId } } },
                    });
                } else {
                    jest.advanceTimersByTime(30_000);
                }
                const cachedDenyAtEntry =
                    peekToolPermissionDeniedForTest(sessionId, "todowrite", "build") === true;
                const started = Promise.withResolvers<void>();
                const read = Promise.withResolvers<never>();
                agents.mockImplementationOnce(() => {
                    started.resolve();
                    return read.promise;
                });
                const callsBefore = agents.mock.calls.length;
                const logsBefore = logSpy.mock.calls.length;
                const pending = consume();
                await started.promise;
                if (failure === "rejection") read.reject(new Error("permission unavailable"));
                else jest.advanceTimersByTime(2_000);
                await pending;
                const readError = logSpy.mock.calls
                    .slice(logsBefore)
                    .find(
                        ([id, text]) => id === sessionId && text.includes("permission read failed"),
                    )?.[2];
                expect(readError).toBeInstanceOf(failure === "timeout" ? TimeoutError : Error);
                const liveReadFailed =
                    agents.mock.calls.length === callsBefore + 1 && readError instanceof Error;
                expect(
                    cachedDenyAtEntry && liveReadFailed,
                    "todowrite-deny-then-read-failure-is-exercised",
                ).toBe(true);
                expect(peekToolPermissionDeniedForTest(sessionId, "todowrite", "build")).toBe(true);
            }
            expect(fake.calls.filter((call) => call.method === "todo_state.set")).toEqual([]);
            const bodies = fake.calls
                .filter((call) => call.method === "transform")
                .map((call) => call.body as { todo_tool_present: boolean });
            expect(bodies.map((body) => body.todo_tool_present)).toEqual([false, false]);
        });
    }

    it("shares fresh permission hits between transform and capture without SDK reads", async () => {
        useTempDataHome("hook-permission-hit-");
        const sessionId = "ses-permission-hit";
        clearToolPermissionDenied(sessionId);
        const client = createClientMock();
        const agents = mock(async () => ({ data: [{ name: "build" }] }));
        client.app.agents = agents as never;
        const fake = createFakeModuleClient(({ body }) => recipeResponse(body, []));
        const hook = requireHook(
            createEidnaraHook(createDeps({ client, rustModeModuleClient: fake.client })),
        );
        const messages = installOneRawMessage(sessionId);
        Object.assign(messages[0]!.info!, { agent: "build" });
        await hook["experimental.chat.messages.transform"]({}, { messages: [...messages] });
        await hook["tool.execute.after"]({
            tool: "todowrite",
            sessionID: sessionId,
            agent: "build",
            args: { todos: [{ content: "Allowed", status: "pending", priority: "high" }] },
        });
        await hook["experimental.chat.messages.transform"]({}, { messages: [...messages] });
        expect(agents).toHaveBeenCalledTimes(1);
        expect(fake.calls.some((call) => call.method === "todo_state.set")).toBe(true);
    });

    it("suppresses both consumers when the permission client is unavailable", async () => {
        useTempDataHome("hook-permission-unavailable-");
        const sessionId = "ses-permission-unavailable";
        clearToolPermissionDenied(sessionId);
        const fake = createFakeModuleClient(({ body }) => recipeResponse(body, []));
        const hook = requireHook(
            createEidnaraHook(createDeps({ client: undefined, rustModeModuleClient: fake.client })),
        );
        const messages = installOneRawMessage(sessionId);
        await hook["experimental.chat.messages.transform"]({}, { messages: [...messages] });
        await hook["tool.execute.after"]({
            tool: "todowrite",
            sessionID: sessionId,
            args: { todos: [{ content: "Blocked", status: "pending", priority: "high" }] },
        });
        expect(fake.calls.filter((call) => call.method === "todo_state.set")).toEqual([]);
        expect(fake.calls.find((call) => call.method === "transform")?.body).toMatchObject({
            todo_tool_present: false,
        });
    });

    for (const boundary of ["session.updated", "session.compacted", "eidnara-flush"] as const) {
        it(`refreshes all session agents after ${boundary} without changing other sessions`, async () => {
            useTempDataHome("hook-permission-invalidate-");
            const sessionId = `ses-permission-${boundary}`;
            const otherSession = `${sessionId}-other`;
            clearToolPermissionDenied(sessionId);
            clearToolPermissionDenied(otherSession);
            const client = createClientMock();
            let action = "allow";
            const agents = mock(async () => ({
                data: ["build", "plan"].map((name) => ({
                    name,
                    permission: { todowrite: action },
                })),
            }));
            client.app.agents = agents as never;
            const fake = createFakeModuleClient(({ body }) =>
                recipeResponse(body, [], { result: { armed: false } }),
            );
            const hook = requireHook(
                createEidnaraHook(createDeps({ client, rustModeModuleClient: fake.client })),
            );
            const messages = installOneRawMessage(sessionId);
            Object.assign(messages[0]!.info!, { agent: "build" });
            const capture = (sessionID: string, agent: string) =>
                hook["tool.execute.after"]({
                    tool: "todowrite",
                    sessionID,
                    agent,
                    args: { todos: [{ content: "Snapshot", status: "pending", priority: "high" }] },
                });
            await capture(sessionId, "build");
            await capture(sessionId, "plan");
            await capture(otherSession, "build");
            await Bun.sleep(0);
            for (action of ["deny", "allow"]) {
                if (boundary === "eidnara-flush") {
                    await expectSentinel(
                        hook["command.execute.before"](
                            { command: "eidnara-flush", sessionID: sessionId, arguments: "" },
                            { parts: [{ type: "text", text: "" }] },
                        ),
                        "__CONTEXT_MANAGEMENT_EIDNARA-FLUSH_HANDLED__",
                    );
                } else {
                    await hook.event({
                        event: { type: boundary, properties: { info: { id: sessionId } } },
                    });
                }
                const reads = agents.mock.calls.length;
                const captures = fake.calls.filter(
                    (call) => call.method === "todo_state.set",
                ).length;
                await hook["experimental.chat.messages.transform"]({}, { messages: [...messages] });
                await capture(sessionId, "build");
                await capture(sessionId, "plan");
                await capture(otherSession, "build");
                await Bun.sleep(0);
                expect(agents).toHaveBeenCalledTimes(reads + 2);
                expect(fake.calls.filter((call) => call.method === "todo_state.set")).toHaveLength(
                    captures + (action === "allow" ? 3 : 1),
                );
                expect(
                    fake.calls.filter((call) => call.method === "transform").at(-1)?.body,
                ).toMatchObject({ todo_tool_present: action === "allow" });
            }
            await hook.event({
                event: { type: "session.deleted", properties: { info: { id: sessionId } } },
            });
            expect(
                peekToolPermissionDeniedForTest(sessionId, "todowrite", "build"),
            ).toBeUndefined();
            expect(peekToolPermissionDeniedForTest(sessionId, "todowrite", "plan")).toBeUndefined();
            expect(peekToolPermissionDeniedForTest(otherSession, "todowrite", "build")).toBe(false);
        });
    }

    it("suppresses an in-flight capture allow invalidated before a newer transform deny", async () => {
        useTempDataHome("hook-permission-race-");
        const sessionId = "ses-permission-race";
        clearToolPermissionDenied(sessionId);
        const started = Promise.withResolvers<void>();
        const response = Promise.withResolvers<unknown>();
        const client = createClientMock();
        const agents = mock(async () => ({
            data: [{ name: "build", permission: { todowrite: "deny" } }],
        }));
        agents.mockImplementationOnce(() => {
            started.resolve();
            return response.promise as never;
        });
        client.app.agents = agents as never;
        const fake = createFakeModuleClient(({ body }) => recipeResponse(body, []));
        const hook = requireHook(
            createEidnaraHook(createDeps({ client, rustModeModuleClient: fake.client })),
        );
        const pending = hook["tool.execute.after"]({
            tool: "todowrite",
            sessionID: sessionId,
            agent: "build",
            args: { todos: [{ content: "Blocked", status: "pending", priority: "high" }] },
        });
        await started.promise;
        await hook.event({
            event: { type: "session.updated", properties: { info: { id: sessionId } } },
        });
        const messages = installOneRawMessage(sessionId);
        Object.assign(messages[0]!.info!, { agent: "build" });
        await hook["experimental.chat.messages.transform"]({}, { messages: [...messages] });
        response.resolve({ data: [] });
        await pending;
        expect(peekToolPermissionDeniedForTest(sessionId, "todowrite", "build")).toBe(true);
        expect(fake.calls.filter((call) => call.method === "todo_state.set")).toEqual([]);
        expect(fake.calls.find((call) => call.method === "transform")?.body).toMatchObject({
            todo_tool_present: false,
        });
    });

    for (const [agent, action] of [
        ["build", "allow"],
        ["", "allow"],
        ["", "deny"],
    ] as const) {
        it(`shares an overlapping transform and capture ${action} for host agent ${JSON.stringify(agent)}`, async () => {
            useTempDataHome("hook-permission-overlap-");
            const sessionId = `ses-permission-overlap-${agent}`;
            clearToolPermissionDenied(sessionId);
            const started = Promise.withResolvers<void>();
            const response = Promise.withResolvers<unknown>();
            const client = createClientMock();
            const get = mock(() => {
                started.resolve();
                return response.promise;
            });
            const agents = mock(async () => ({ data: agent ? [{ name: agent }] : [] }));
            client.app.agents = agents as never;
            const resolver = spyOn(permissionAvailability, "todowritePermissionDenied");
            const fake = createFakeModuleClient(({ body }) => recipeResponse(body, []));
            const hook = requireHook(
                createEidnaraHook(createDeps({ client, rustModeModuleClient: fake.client })),
            );
            await hook.resolveSessionDirectory(sessionId);
            client.session.get = get as never;
            const messages = installOneRawMessage(sessionId);
            Object.assign(messages[0]!.info!, { agent });
            const transform = hook["experimental.chat.messages.transform"](
                {},
                { messages: [...messages] },
            );
            await started.promise;
            const capture = hook["tool.execute.after"]({
                tool: "todowrite",
                sessionID: sessionId,
                agent: agent || undefined,
                args: { todos: [{ content: "Allowed", status: "pending", priority: "high" }] },
            });
            await Bun.sleep(0);
            expect(resolver).toHaveBeenCalledTimes(2);
            response.resolve({ data: { permission: { todowrite: action } } });
            await Promise.all([transform, capture]);
            await hook["tool.execute.after"]({
                tool: "todowrite",
                sessionID: sessionId,
                agent,
                args: { todos: [{ content: "Cached", status: "pending", priority: "high" }] },
            });
            await Bun.sleep(0);
            expect(fake.calls.find((call) => call.method === "transform")?.body).toMatchObject({
                todo_tool_present: action === "allow",
            });
            expect(fake.calls.filter((call) => call.method === "todo_state.set")).toHaveLength(
                action === "allow" ? 2 : 0,
            );
            expect(agents).toHaveBeenCalledTimes(agent ? 1 : 0);
            expect(get).toHaveBeenCalledTimes(1);
        });
    }

    it("returns exactly the session hook keys and attaches rustToolBackends non-enumerably", () => {
        useTempDataHome("hook-keys-");
        const fake = createFakeModuleClient();
        const hook = requireHook(
            createEidnaraHook(createDeps({ rustModeModuleClient: fake.client })),
        );

        expect(Object.keys(hook).sort()).toEqual(HOOK_KEYS);
        for (const key of HOOK_KEYS) {
            expect(typeof hook[key as keyof typeof hook]).toBe("function");
        }
        expect("tool.definition" in hook).toBe(false);
        expect("config" in hook).toBe(false);
        expect(Object.keys(hook.rustToolBackends).sort()).toEqual(["note", "reduce"]);
        expect(typeof hook.resolveSessionDirectory).toBe("function");
        expect("noteEvaluationAvailable" in hook.rustToolBackends).toBe(false);
    });

    it("attaches the daemon tool backends in ts mode and leaves the messages transform a no-op", async () => {
        useTempDataHome("hook-ts-mode-");
        const fake = createFakeModuleClient();
        const hook = requireHook(
            createEidnaraHook(
                createDeps({
                    rustModeModuleClient: fake.client,
                    config: { protected_tags: 3, cache_ttl: "5m", transform_mode: "ts" },
                }),
            ),
        );

        expect(Object.keys(hook).sort()).toEqual(HOOK_KEYS);
        expect(Object.keys(hook.rustToolBackends).sort()).toEqual(["note", "reduce"]);

        await hook.rustToolBackends.reduce?.({
            sessionId: "ses-ts",
            projectRoot: "/repo",
            drop: "1",
            commandId: "cmd-ts",
        });
        expect(fake.calls.map((call) => call.method)).toEqual(["agent_drops.append"]);

        const messages = [{ info: { sessionID: "ses-ts" } }];
        const output = { messages: [...messages] };
        await hook["experimental.chat.messages.transform"]({}, output);
        expect(output.messages).toEqual(messages);
        expect(fake.calls).toHaveLength(1);
    });

    it("returns null and records no_project when no project identity resolves", () => {
        useTempDataHome("hook-no-project-");
        const home = process.env.HOME ?? process.env.USERPROFILE ?? "/";
        const hook = createEidnaraHook(
            createDeps({
                directory: home,
                rustModeModuleClient: createFakeModuleClient().client,
                config: {
                    protected_tags: 3,
                    cache_ttl: "5m",
                    transform_mode: "rust",
                    allow_home_project: false,
                },
            }),
        );

        expect(hook).toBeNull();
        expect(getLastHookInitFailure()).toEqual({ type: "no_project" });
    });

    it("forwards todowrite snapshots to the daemon as todo_state.set", async () => {
        useTempDataHome("hook-todo-");
        const fake = createFakeModuleClient();
        const hook = requireHook(
            createEidnaraHook(createDeps({ rustModeModuleClient: fake.client })),
        );

        await hook["tool.execute.after"]({
            tool: "todowrite",
            sessionID: "ses-todo",
            args: {
                todos: [{ status: "pending", priority: "high", content: "Forward me" }],
                owner_message_id: "msg-owner",
            },
        });
        await Bun.sleep(0);

        expect(fake.calls).toEqual([
            {
                sessionId: "ses-todo",
                projectRoot: "/tmp",
                method: "todo_state.set",
                body: {
                    method: "todo_state.set",
                    v: 1,
                    session_id: "ses-todo",
                    state_json: '[{"content":"Forward me","status":"pending","priority":"high"}]',
                    owner_message_id: "msg-owner",
                },
            },
        ]);
    });

    it("routes todo snapshots by the session's own directory", async () => {
        useTempDataHome("hook-todo-route-");
        const fake = createFakeModuleClient();
        const liveSessionState = createLiveSessionState();
        const hook = requireHook(
            createEidnaraHook(
                createDeps({
                    client: createClientMock(undefined, "/other/repo"),
                    rustModeModuleClient: fake.client,
                    liveSessionState,
                }),
            ),
        );

        await hook["tool.execute.after"]({
            tool: "todowrite",
            sessionID: "ses-todo-routed",
            args: { todos: [{ status: "pending", priority: "high", content: "Route me" }] },
        });
        await Bun.sleep(0);

        expect(fake.calls.map((call) => [call.method, call.projectRoot])).toEqual([
            ["todo_state.set", "/other/repo"],
        ]);
        expect(liveSessionState.sessionDirectoryBySession.get("ses-todo-routed")).toBe(
            "/other/repo",
        );
    });

    it("classifies a restored child before forwarding its first todo snapshot", async () => {
        useTempDataHome("hook-todo-restored-child-");
        const fake = createFakeModuleClient();
        const liveSessionState = createLiveSessionState();
        let postResolutionGate = false;
        const hasSubagent = liveSessionState.subagentSessions.has.bind(
            liveSessionState.subagentSessions,
        );
        liveSessionState.subagentSessions.has = mock((sessionId: string) => {
            const result = hasSubagent(sessionId);
            if (result) postResolutionGate = true;
            return result;
        });
        const client = createClientMock(undefined, "/other/repo") as unknown as {
            session: { get: ReturnType<typeof mock> };
        };
        client.session.get = mock(async () => ({
            data: { directory: "/other/repo", parentID: "ses-parent" },
        }));
        const hook = requireHook(
            createEidnaraHook(
                createDeps({
                    client: client as unknown as EidnaraDeps["client"],
                    rustModeModuleClient: fake.client,
                    liveSessionState,
                }),
            ),
        );

        await hook["tool.execute.after"]({
            tool: "todowrite",
            sessionID: "ses-restored-todo-child",
            args: { todos: [{ status: "pending", priority: "high", content: "Child task" }] },
        });
        for (let attempt = 0; attempt < 20 && !postResolutionGate; attempt += 1) {
            await Bun.sleep(0);
        }

        expect(postResolutionGate).toBe(true);
        expect(liveSessionState.subagentSessions.has("ses-restored-todo-child")).toBe(true);
        expect(fake.calls.filter((call) => call.method === "todo_state.set")).toHaveLength(0);
    });

    it("drops a detached todo snapshot whose session was deleted while it awaited the directory", async () => {
        useTempDataHome("hook-todo-deleted-race-");
        const fake = createFakeModuleClient();
        const liveSessionState = createLiveSessionState();
        let releaseDirectoryRead: (() => void) | undefined;
        const client = createClientMock(undefined, "/other/repo") as unknown as {
            session: { get: ReturnType<typeof mock> };
        };
        client.session.get = mock(
            () =>
                new Promise<{
                    data: { directory: string; parentID: string; title: string };
                }>((resolve) => {
                    releaseDirectoryRead = () =>
                        resolve({
                            data: {
                                directory: "/other/repo",
                                parentID: "ses-parent",
                                title: "eidnara-late-child",
                            },
                        });
                }),
        );
        const hook = requireHook(
            createEidnaraHook(
                createDeps({
                    client: client as unknown as EidnaraDeps["client"],
                    rustModeModuleClient: fake.client,
                    liveSessionState,
                }),
            ),
        );

        await hook["tool.execute.after"]({
            tool: "todowrite",
            sessionID: "ses-todo-deleted",
            args: { todos: [{ status: "pending", priority: "high", content: "Too late" }] },
        });
        while (releaseDirectoryRead === undefined) await Bun.sleep(0);
        await hook.event({
            event: { type: "session.deleted", properties: { info: { id: "ses-todo-deleted" } } },
        });
        releaseDirectoryRead?.();
        await Bun.sleep(0);
        await Bun.sleep(0);

        expect(fake.calls.filter((call) => call.method === "todo_state.set")).toHaveLength(0);
        expect(liveSessionState.sessionDirectoryBySession.has("ses-todo-deleted")).toBe(false);
        expect(liveSessionState.sessionMetadataReadStateBySession.has("ses-todo-deleted")).toBe(
            false,
        );
        expect(liveSessionState.subagentSessions.has("ses-todo-deleted")).toBe(false);
        expect(liveSessionState.internalChildSessions.has("ses-todo-deleted")).toBe(false);
    });

    it("skips a todo snapshot without a second directory read when the session is already deleted", async () => {
        useTempDataHome("hook-todo-already-deleted-");
        const fake = createFakeModuleClient();
        const client = createClientMock(undefined, "/other/repo") as unknown as {
            session: { get: ReturnType<typeof mock> };
        };
        const hook = requireHook(
            createEidnaraHook(
                createDeps({
                    client: client as unknown as EidnaraDeps["client"],
                    rustModeModuleClient: fake.client,
                }),
            ),
        );
        const sessionId = "ses-todo-already-deleted";
        await hook.event({
            event: { type: "session.deleted", properties: { info: { id: sessionId } } },
        });
        client.session.get.mockClear();

        await hook["tool.execute.after"]({
            tool: "todowrite",
            sessionID: sessionId,
            args: { todos: [{ status: "pending", priority: "high", content: "Too late" }] },
        });
        await Bun.sleep(0);

        expect(client.session.get).toHaveBeenCalledTimes(1);
        expect(fake.calls.filter((call) => call.method === "todo_state.set")).toHaveLength(0);
    });

    it("skips the transform for a hidden eidnara- child restored after a restart", async () => {
        useTempDataHome("hook-internal-child-rehydrate-");
        const fake = createFakeModuleClient(({ method, body }) =>
            method === "transform"
                ? recipeResponse(body, [], { decision: "PASSTHROUGH" })
                : { ok: true },
        );
        const liveSessionState = createLiveSessionState();
        const client = createClientMock(undefined, "/other/repo") as unknown as {
            session: { get: ReturnType<typeof mock> };
        };
        client.session.get = mock(async () => ({
            data: {
                directory: "/other/repo",
                parentID: "ses-parent",
                title: "eidnara-context_researcher",
            },
        }));
        const hook = requireHook(
            createEidnaraHook(
                createDeps({
                    client: client as unknown as EidnaraDeps["client"],
                    rustModeModuleClient: fake.client,
                    liveSessionState,
                }),
            ),
        );

        const messages = installOneRawMessage("ses-restored-internal");
        await hook["experimental.chat.messages.transform"]({}, { messages: [...messages] });

        expect(liveSessionState.internalChildSessions.has("ses-restored-internal")).toBe(true);
        expect(fake.calls.filter((call) => call.method === "transform")).toHaveLength(0);
    });

    it("treats a restored child session as a subagent from the host's parentID", async () => {
        useTempDataHome("hook-subagent-rehydrate-");
        const fake = createFakeModuleClient(({ method, body }) =>
            method === "transform"
                ? recipeResponse(body, [], { decision: "PASSTHROUGH" })
                : { ok: true },
        );
        const liveSessionState = createLiveSessionState();
        const client = createClientMock(undefined, "/other/repo") as unknown as {
            session: { get: ReturnType<typeof mock> };
        };
        client.session.get = mock(async () => ({
            data: { directory: "/other/repo", parentID: "ses-parent" },
        }));
        const hook = requireHook(
            createEidnaraHook(
                createDeps({
                    client: client as unknown as EidnaraDeps["client"],
                    rustModeModuleClient: fake.client,
                    liveSessionState,
                }),
            ),
        );

        const messages = installOneRawMessage("ses-restored-child");
        await hook["experimental.chat.messages.transform"]({}, { messages: [...messages] });

        expect(liveSessionState.subagentSessions.has("ses-restored-child")).toBe(true);
        const transformBody = fake.calls.find((call) => call.method === "transform")?.body as
            | { is_subagent?: boolean }
            | undefined;
        expect(transformBody?.is_subagent).toBe(true);
    });

    it("routes rustToolBackends.reduce through the session directory fallback", async () => {
        useTempDataHome("hook-reduce-");
        const fake = createFakeModuleClient();
        const hook = requireHook(
            createEidnaraHook(createDeps({ rustModeModuleClient: fake.client })),
        );

        await hook.rustToolBackends?.reduce?.({
            sessionId: "ses-reduce",
            drop: "tool output summary",
            commandId: "cmd-1",
        });

        expect(fake.calls).toEqual([
            {
                sessionId: "ses-reduce",
                projectRoot: "/tmp",
                method: "agent_drops.append",
                body: {
                    method: "agent_drops.append",
                    v: 1,
                    session_id: "ses-reduce",
                    drop: "tool output summary",
                    command_id: "cmd-1",
                },
            },
        ]);
    });

    it("routes eidnara_note through the pinned root and derives its memory project", async () => {
        useTempDataHome("hook-note-");
        const fake = createFakeModuleClient(() => ({ result: { note_id: 7 } }));
        const liveSessionState = createLiveSessionState();
        liveSessionState.sessionDirectoryBySession.set("ses-note", "/pinned/repo");
        const hook = requireHook(
            createEidnaraHook(
                createDeps({
                    client: createClientMock(undefined, "/other/repo"),
                    rustModeModuleClient: fake.client,
                    liveSessionState,
                }),
            ),
        );
        const memoryProject = resolveProjectIdentityForSession("/pinned/repo");
        expect(memoryProject).toBeDefined();

        const response = await hook.rustToolBackends?.note?.({
            commandId: "cmd-note",
            sessionId: "ses-note",
            action: "write",
            content: "Remember the build flag",
            surfaceCondition: "file exists",
            compiledProvider: "quickjs",
            compiledConfig: "{}",
            compiledAt: 123,
            compileStatus: "compiled",
        });

        expect(response).toEqual({ result: { note_id: 7 } });
        expect(fake.calls).toEqual([
            {
                sessionId: "ses-note",
                projectRoot: "/pinned/repo",
                method: "eidnara_note",
                body: {
                    name: "eidnara_note",
                    arguments: {
                        command_id: "cmd-note",
                        action: "write",
                        content: "Remember the build flag",
                        memory_project: memoryProject,
                        surface_condition: "file exists",
                        compiled_provider: "quickjs",
                        compiled_config: "{}",
                        compiled_at: 123,
                        compile_status: "compiled",
                        filter: undefined,
                        limit: undefined,
                        offset: undefined,
                        note_id: undefined,
                    },
                },
            },
        ]);
    });

    it("omits compiled fields from eidnara_note arguments without a compile status", async () => {
        useTempDataHome("hook-note-plain-");
        const fake = createFakeModuleClient();
        const hook = requireHook(
            createEidnaraHook(createDeps({ rustModeModuleClient: fake.client })),
        );

        await hook.rustToolBackends?.note?.({
            sessionId: "ses-note",
            action: "read",
            filter: "active",
            limit: 5,
        });

        const args = (fake.calls[0]?.body as { arguments: Record<string, unknown> }).arguments;
        expect("command_id" in args).toBe(false);
        expect("compile_status" in args).toBe(false);
        expect(args).toEqual(
            expect.objectContaining({ action: "read", filter: "active", limit: 5 }),
        );
    });

    it("gates both rust tool backends before and after a pending session directory read", async () => {
        useTempDataHome("hook-tools-deleted-race-");
        const fake = createFakeModuleClient();
        const liveSessionState = createLiveSessionState();
        let releaseDirectoryRead: (() => void) | undefined;
        const client = createClientMock(undefined, "/other/repo") as unknown as {
            session: { get: ReturnType<typeof mock> };
        };
        client.session.get = mock(
            () =>
                new Promise<{
                    data: { directory: string; parentID: string; title: string };
                }>((resolve) => {
                    releaseDirectoryRead = () =>
                        resolve({
                            data: {
                                directory: "/other/repo",
                                parentID: "ses-parent",
                                title: "eidnara-late-child",
                            },
                        });
                }),
        );
        const hook = requireHook(
            createEidnaraHook(
                createDeps({
                    client: client as unknown as EidnaraDeps["client"],
                    rustModeModuleClient: fake.client,
                    liveSessionState,
                }),
            ),
        );
        const sessionId = "ses-tools-deleted";

        const reduce = hook.rustToolBackends?.reduce?.({
            sessionId,
            drop: "1",
            commandId: "cmd-reduce",
        });
        const note = hook.rustToolBackends?.note?.({ sessionId, action: "read" });
        while (releaseDirectoryRead === undefined) await Bun.sleep(0);
        await hook.event({
            event: { type: "session.deleted", properties: { info: { id: sessionId } } },
        });
        releaseDirectoryRead();

        const outcomes = await Promise.allSettled([reduce, note]);
        expect(outcomes).toHaveLength(2);
        for (const outcome of outcomes) {
            expect(outcome.status).toBe("rejected");
            if (outcome.status === "rejected") {
                expect(outcome.reason).toHaveProperty(
                    "message",
                    "Session was deleted before the Rust tool could run.",
                );
            }
        }
        expect(
            fake.calls.filter(
                (call) => call.method === "agent_drops.append" || call.method === "eidnara_note",
            ),
        ).toHaveLength(0);
        expect(liveSessionState.sessionDirectoryBySession.has(sessionId)).toBe(false);
        expect(liveSessionState.sessionMetadataReadStateBySession.has(sessionId)).toBe(false);
        expect(liveSessionState.subagentSessions.has(sessionId)).toBe(false);
        expect(liveSessionState.internalChildSessions.has(sessionId)).toBe(false);

        const readsAfterRace = client.session.get.mock.calls.length;
        await expect(
            hook.rustToolBackends?.reduce?.({
                sessionId,
                drop: "2",
                commandId: "cmd-after-delete",
            }),
        ).rejects.toThrow("Session was deleted before the Rust tool could run.");
        await expect(hook.rustToolBackends?.note?.({ sessionId, action: "read" })).rejects.toThrow(
            "Session was deleted before the Rust tool could run.",
        );
        expect(client.session.get).toHaveBeenCalledTimes(readsAfterRace);
    });

    it("routes the transform by the session's own directory and skips hidden eidnara- children", async () => {
        useTempDataHome("hook-transform-route-");
        const fake = createFakeModuleClient(({ method, body }) =>
            method === "transform"
                ? recipeResponse(body, [], { decision: "PASSTHROUGH" })
                : { ok: true },
        );
        const liveSessionState = createLiveSessionState();
        const hook = requireHook(
            createEidnaraHook(
                createDeps({
                    client: createClientMock(undefined, "/other/repo"),
                    rustModeModuleClient: fake.client,
                    liveSessionState,
                }),
            ),
        );

        const messages = installOneRawMessage("ses-routed");
        await hook["experimental.chat.messages.transform"]({}, { messages: [...messages] });
        expect(fake.calls.map((call) => [call.method, call.projectRoot])).toEqual([
            ["transform", "/other/repo"],
        ]);
        expect(liveSessionState.sessionDirectoryBySession.get("ses-routed")).toBe("/other/repo");

        liveSessionState.internalChildSessions.add("ses-hidden");
        const hidden = installOneRawMessage("ses-hidden");
        await hook["experimental.chat.messages.transform"]({}, { messages: [...hidden] });
        expect(fake.calls).toHaveLength(1);
    });

    it("skips a transform for a session deleted while its directory read is pending", async () => {
        useTempDataHome("hook-transform-deleted-race-");
        const fake = createFakeModuleClient(({ method, body }) =>
            method === "transform"
                ? recipeResponse(body, [], { decision: "PASSTHROUGH" })
                : { ok: true },
        );
        const liveSessionState = createLiveSessionState();
        let releaseDirectoryRead: (() => void) | undefined;
        const client = createClientMock(undefined, "/other/repo") as unknown as {
            session: { get: ReturnType<typeof mock> };
        };
        client.session.get = mock(
            () =>
                new Promise<{
                    data: { directory: string; parentID: string; title: string };
                }>((resolve) => {
                    releaseDirectoryRead = () =>
                        resolve({
                            data: {
                                directory: "/other/repo",
                                parentID: "ses-parent",
                                title: "eidnara-late-child",
                            },
                        });
                }),
        );
        const hook = requireHook(
            createEidnaraHook(
                createDeps({
                    client: client as unknown as EidnaraDeps["client"],
                    rustModeModuleClient: fake.client,
                    liveSessionState,
                }),
            ),
        );
        const sessionId = "ses-transform-deleted-race";
        const messages = installOneRawMessage(sessionId);
        const pass = hook["experimental.chat.messages.transform"]({}, { messages: [...messages] });
        while (releaseDirectoryRead === undefined) await Bun.sleep(0);

        await hook.event({
            event: { type: "session.deleted", properties: { info: { id: sessionId } } },
        });
        releaseDirectoryRead();
        await pass;

        expect(fake.deleteSession).toHaveBeenCalled();
        expect(fake.calls.filter((call) => call.method === "transform")).toHaveLength(0);
        expect(liveSessionState.sessionDirectoryBySession.has(sessionId)).toBe(false);
        expect(liveSessionState.sessionMetadataReadStateBySession.has(sessionId)).toBe(false);
        expect(liveSessionState.subagentSessions.has(sessionId)).toBe(false);
        expect(liveSessionState.internalChildSessions.has(sessionId)).toBe(false);
    });

    it("skips a transform for an already deleted session without reading its directory", async () => {
        useTempDataHome("hook-transform-already-deleted-");
        const fake = createFakeModuleClient();
        const client = createClientMock(undefined, "/other/repo") as unknown as {
            session: { get: ReturnType<typeof mock> };
        };
        const hook = requireHook(
            createEidnaraHook(
                createDeps({
                    client: client as unknown as EidnaraDeps["client"],
                    rustModeModuleClient: fake.client,
                }),
            ),
        );
        const sessionId = "ses-transform-already-deleted";
        await hook.event({
            event: { type: "session.deleted", properties: { info: { id: sessionId } } },
        });
        client.session.get.mockClear();
        const messages = installOneRawMessage(sessionId);

        await hook["experimental.chat.messages.transform"]({}, { messages: [...messages] });

        expect(client.session.get).not.toHaveBeenCalled();
        expect(fake.calls.filter((call) => call.method === "transform")).toHaveLength(0);
    });

    it("clears the transform session and prompt state on session.deleted", async () => {
        useTempDataHome("hook-session-deleted-");
        const fake = createFakeModuleClient(({ method, body }) =>
            method === "transform"
                ? recipeResponse(body, [], { decision: "PASSTHROUGH" })
                : { ok: true },
        );
        const liveSessionState = createLiveSessionState();
        const kernelConfig = {
            host: { connection_file: "/nonexistent/eidnara-hook-test/host.json" },
        };
        const hook = requireHook(
            createEidnaraHook(
                createDeps({
                    client: createClientMock(undefined, "/other/repo"),
                    rustModeModuleClient: fake.client,
                    liveSessionState,
                    config: {
                        protected_tags: 3,
                        cache_ttl: "5m",
                        transform_mode: "rust",
                        ...kernelConfig,
                    },
                }),
            ),
        );
        const sessionId = "ses-deleted";
        // A memory read holds a kernel route on the shared transport until the session closes it.
        createKernelClient({ sessionId, projectRoot: "/other/repo", config: kernelConfig });
        const sharedKernel = sharedStateForTest(kernelConfig)?.module;
        if (!sharedKernel) throw new Error("the kernel client must resolve a shared transport");
        const closedKernelSessions: string[] = [];
        sharedKernel.closeSession = (closing: string) => {
            closedKernelSessions.push(closing);
        };
        const selectModel = () =>
            hook["chat.message"]({
                sessionID: sessionId,
                model: { providerID: "provider", modelID: "model" },
            });

        await selectModel();
        const messages = installOneRawMessage(sessionId);
        await hook["experimental.chat.messages.transform"]({}, { messages: [...messages] });
        await hook["experimental.chat.system.transform"](
            { sessionID: sessionId },
            { system: ["first prompt"] },
        );
        await hook["experimental.chat.system.transform"](
            { sessionID: sessionId },
            { system: ["second prompt"] },
        );
        expect(liveSessionState.historyRefreshSessions.has(sessionId)).toBe(true);

        await hook.event({
            event: { type: "session.deleted", properties: { info: { id: sessionId } } },
        });
        await Bun.sleep(0);

        // Session deletion uses the transform's recorded project root, not the plugin launch directory.
        expect(fake.deleteSession).toHaveBeenCalledWith(sessionId, "/other/repo");
        expect(fake.closeSession).toHaveBeenCalledWith(sessionId);
        expect(closedKernelSessions).toEqual([sessionId]);
        expect(liveSessionState.liveModelBySession.has(sessionId)).toBe(false);
        expect(liveSessionState.sessionDirectoryBySession.has(sessionId)).toBe(false);
        expect(liveSessionState.historyRefreshSessions.has(sessionId)).toBe(false);

        await expect(
            hook.resolveSessionDirectory(sessionId, "/late/tool-directory"),
        ).rejects.toThrow("Session was deleted before the Rust tool could run.");
        expect(liveSessionState.sessionDirectoryBySession.has(sessionId)).toBe(false);

        // A prompt change after deletion finds no persisted hash, so it initializes instead of flagging a change.
        await selectModel();
        await hook["experimental.chat.system.transform"](
            { sessionID: sessionId },
            { system: ["third prompt"] },
        );
        expect(liveSessionState.historyRefreshSessions.has(sessionId)).toBe(false);
    });

    it("routes rust cleanup from the deleted event directory without an SDK read", async () => {
        useTempDataHome("hook-session-deleted-route-");
        const fake = createFakeModuleClient();
        const client = createClientMock(undefined, "/wrong/repo") as unknown as {
            session: { get: ReturnType<typeof mock> };
        };
        const hook = requireHook(
            createEidnaraHook(
                createDeps({
                    client: client as unknown as EidnaraDeps["client"],
                    rustModeModuleClient: fake.client,
                }),
            ),
        );

        await hook.event({
            event: {
                type: "session.deleted",
                properties: { info: { id: "ses-delete-route", directory: "/actual/repo" } },
            },
        });
        await Bun.sleep(0);

        expect(client.session.get).not.toHaveBeenCalled();
        expect(fake.deleteSession).toHaveBeenCalledWith("ses-delete-route", "/actual/repo");
    });

    it("preserves an existing route pin when the deleted event reports another directory", async () => {
        useTempDataHome("hook-session-deleted-pinned-route-");
        const fake = createFakeModuleClient();
        const liveSessionState = createLiveSessionState();
        liveSessionState.sessionDirectoryBySession.set("ses-delete-pinned", "/pinned/repo");
        const hook = requireHook(
            createEidnaraHook(
                createDeps({
                    rustModeModuleClient: fake.client,
                    liveSessionState,
                }),
            ),
        );

        await hook.event({
            event: {
                type: "session.deleted",
                properties: { info: { id: "ses-delete-pinned", directory: "/event/repo" } },
            },
        });
        await Bun.sleep(0);

        expect(fake.deleteSession).toHaveBeenCalledWith("ses-delete-pinned", "/pinned/repo");
    });

    it("closes local route state without invoking daemon deletion in ts mode", async () => {
        useTempDataHome("hook-session-deleted-ts-");
        const fake = createFakeModuleClient();
        const hook = requireHook(
            createEidnaraHook(
                createDeps({
                    rustModeModuleClient: fake.client,
                    config: { protected_tags: 3, cache_ttl: "5m", transform_mode: "ts" },
                }),
            ),
        );

        await hook.event({
            event: {
                type: "session.deleted",
                properties: { info: { id: "ses-delete-ts", directory: "/actual/repo" } },
            },
        });
        await Bun.sleep(0);

        expect(fake.deleteSession).not.toHaveBeenCalled();
        expect(fake.closeSession).toHaveBeenCalledWith("ses-delete-ts");
    });

    it("deletes daemon state for a ts-mode session with an existing route", async () => {
        useTempDataHome("hook-session-deleted-ts-route-");
        const fake = createFakeModuleClient();
        fake.client.hasSessionRoute = (sessionId) => sessionId === "ses-delete-ts-routed";
        const hook = requireHook(
            createEidnaraHook(
                createDeps({
                    rustModeModuleClient: fake.client,
                    config: { protected_tags: 3, cache_ttl: "5m", transform_mode: "ts" },
                }),
            ),
        );

        await hook.event({
            event: {
                type: "session.deleted",
                properties: {
                    info: { id: "ses-delete-ts-routed", directory: "/actual/repo" },
                },
            },
        });
        await Bun.sleep(0);

        expect(fake.deleteSession).toHaveBeenCalledWith("ses-delete-ts-routed", "/actual/repo");
        expect(fake.closeSession).toHaveBeenCalledWith("ses-delete-ts-routed");
    });

    it("forwards /eidnara-flush to session.flush and throws the sentinel", async () => {
        useTempDataHome("hook-flush-");
        const fake = createFakeModuleClient(() => ({ result: { armed: false } }));
        const promptMock = mock((..._args: unknown[]) => undefined);
        const liveSessionState = createLiveSessionState();
        const hook = requireHook(
            createEidnaraHook(
                createDeps({
                    client: createClientMock(promptMock),
                    rustModeModuleClient: fake.client,
                    liveSessionState,
                }),
            ),
        );

        await expectSentinel(
            hook["command.execute.before"](
                { command: "eidnara-flush", sessionID: "ses-flush", arguments: "" },
                { parts: [{ type: "text", text: "" }] },
            ),
            "__CONTEXT_MANAGEMENT_EIDNARA-FLUSH_HANDLED__",
        );

        expect(fake.calls).toEqual([
            {
                sessionId: "ses-flush",
                projectRoot: "/tmp",
                method: "session.flush",
                body: { method: "session.flush", v: 1, session_id: "ses-flush" },
            },
        ]);
        expect(liveSessionState.historyRefreshSessions.has("ses-flush")).toBe(true);
        expect(liveSessionState.systemPromptRefreshSessions.has("ses-flush")).toBe(true);
        expect(liveSessionState.pendingMaterializationSessions.has("ses-flush")).toBe(true);
        expect(promptMock).toHaveBeenCalledTimes(1);
        const callArg = promptMock.mock.calls[0]?.[0] as Record<string, unknown>;
        expect(callArg).toEqual(
            expect.objectContaining({
                path: { id: "ses-flush" },
                body: expect.objectContaining({
                    parts: [
                        {
                            type: "text",
                            text: expect.stringContaining("No pending operations to flush."),
                            ignored: true,
                        },
                    ],
                }),
            }),
        );
    });

    it("routes ctx commands by the session's own directory like the transform", async () => {
        useTempDataHome("hook-command-route-");
        const fake = createFakeModuleClient(() => ({ result: { armed: false } }));
        const liveSessionState = createLiveSessionState();
        const hook = requireHook(
            createEidnaraHook(
                createDeps({
                    client: createClientMock(undefined, "/other/repo"),
                    rustModeModuleClient: fake.client,
                    liveSessionState,
                }),
            ),
        );

        await expectSentinel(
            hook["command.execute.before"](
                { command: "eidnara-flush", sessionID: "ses-routed-cmd", arguments: "" },
                { parts: [{ type: "text", text: "" }] },
            ),
            "__CONTEXT_MANAGEMENT_EIDNARA-FLUSH_HANDLED__",
        );

        expect(fake.calls.map((call) => [call.method, call.projectRoot])).toEqual([
            ["session.flush", "/other/repo"],
        ]);
        expect(liveSessionState.sessionDirectoryBySession.get("ses-routed-cmd")).toBe(
            "/other/repo",
        );
    });

    it("clears late command-route metadata when the session is deleted during resolution", async () => {
        useTempDataHome("hook-command-deleted-race-");
        const fake = createFakeModuleClient();
        const liveSessionState = createLiveSessionState();
        let releaseDirectoryRead: (() => void) | undefined;
        const client = createClientMock(undefined, "/other/repo") as unknown as {
            session: { get: ReturnType<typeof mock> };
        };
        client.session.get = mock(
            () =>
                new Promise<{
                    data: { directory: string; parentID: string; title: string };
                }>((resolve) => {
                    releaseDirectoryRead = () =>
                        resolve({
                            data: {
                                directory: "/other/repo",
                                parentID: "ses-parent",
                                title: "eidnara-late-child",
                            },
                        });
                }),
        );
        const hook = requireHook(
            createEidnaraHook(
                createDeps({
                    client: client as unknown as EidnaraDeps["client"],
                    rustModeModuleClient: fake.client,
                    liveSessionState,
                }),
            ),
        );
        const sessionId = "ses-command-deleted-race";
        const command = hook["command.execute.before"](
            { command: "eidnara-flush", sessionID: sessionId, arguments: "" },
            { parts: [{ type: "text", text: "" }] },
        );
        while (releaseDirectoryRead === undefined) await Bun.sleep(0);

        await hook.event({
            event: { type: "session.deleted", properties: { info: { id: sessionId } } },
        });
        releaseDirectoryRead();
        await expectSentinel(command, "__CONTEXT_MANAGEMENT_EIDNARA-FLUSH_HANDLED__");

        expect(fake.calls.filter((call) => call.method === "session.flush")).toHaveLength(0);
        expect(liveSessionState.sessionDirectoryBySession.has(sessionId)).toBe(false);
        expect(liveSessionState.sessionMetadataReadStateBySession.has(sessionId)).toBe(false);
        expect(liveSessionState.subagentSessions.has(sessionId)).toBe(false);
        expect(liveSessionState.internalChildSessions.has(sessionId)).toBe(false);
    });
});

it("sends a serialized body carrier from the live transform hook", async () => {
    const { serializedJsonText } = await import("../../shared/host-client/serialized-json-body");
    useTempDataHome("hook-serialized-body-");
    const sessionId = "hook-serialized-body";
    const fake = createFakeModuleClient(({ body }) => recipeResponse(body, []));
    const hook = requireHook(createEidnaraHook(createDeps({ rustModeModuleClient: fake.client })));
    const messages = installOneRawMessage(sessionId);
    await hook["experimental.chat.messages.transform"]({}, { messages });
    const calls = fake.calls.filter((call) => call.method === "transform");
    expect(calls).toHaveLength(1);
    const text = serializedJsonText(calls[0]!.body);
    expect(text).toBeString();
    expect(JSON.parse(text!)).toMatchObject({ method: "transform", session_id: sessionId });
    expect(Object.isFrozen(calls[0]!.body)).toBe(true);
});

describe("rust-mode guidance fetch", () => {
    it("sends guidance.get with the transform's prompt-surface fields, a 5 s budget, and appends the bytes", async () => {
        useTempDataHome("hook-guidance-parity-");
        const fake = createFakeModuleClient(({ method, body }) => {
            if (method === "transform")
                return recipeResponse(body, [], { decision: "PASSTHROUGH" });
            if (method === "guidance.get") return { bytes: "## Eidnara\n\nGuidance block." };
            return { ok: true };
        });
        const hook = requireHook(
            createEidnaraHook(
                createDeps({
                    rustModeModuleClient: fake.client,
                    config: {
                        protected_tags: 3,
                        cache_ttl: "5m",
                        transform_mode: "rust",
                        prompt_surface: {
                            default: "light",
                            tool_descriptions: { eidnara_search: "x" },
                        },
                    },
                }),
            ),
        );
        const sessionId = "ses-guidance-parity";
        await hook["chat.message"]({
            sessionID: sessionId,
            model: { providerID: "provider", modelID: "model" },
        });
        const messages = installOneRawMessage(sessionId);
        await hook["experimental.chat.messages.transform"]({}, { messages: [...messages] });
        const output = { system: ["host prompt"] };
        await hook["experimental.chat.system.transform"]({ sessionID: sessionId }, output);

        const transform = fake.calls.find((call) => call.method === "transform");
        const guidance = fake.calls.find((call) => call.method === "guidance.get");
        if (!transform || !guidance) throw new Error("both routes must have been called");
        const transformBody = transform.body as Record<string, unknown>;
        const guidanceBody = guidance.body as Record<string, unknown>;
        // The daemon freezes the first prompt-surface selection per session, so both routes
        // must describe the same one.
        const surfaceKeys = Object.keys(transformBody).filter((key) =>
            key.startsWith("prompt_surface_"),
        );
        expect(surfaceKeys.sort()).toEqual([
            "prompt_surface_config_identity",
            "prompt_surface_guidance_override",
            "prompt_surface_model_key",
            "prompt_surface_preset",
            "prompt_surface_tool_descriptions",
        ]);
        // The fixture's raw message names no model, so the transform reports none; the
        // system-prompt path resolves the live model selected by `chat.message`.
        for (const key of surfaceKeys) {
            if (key === "prompt_surface_model_key") continue;
            expect(guidanceBody[key]).toEqual(transformBody[key]);
        }
        expect(guidanceBody.prompt_surface_tool_descriptions).toEqual({ eidnara_search: "x" });
        expect(guidanceBody).toMatchObject({
            prompt_surface_model_key: "provider/model",
            tool_present: transformBody.tool_present,
            serializer_profile: "opencode-aisdk",
        });
        expect(guidance.signal).toBeInstanceOf(AbortSignal);
        expect(output.system).toEqual(["host prompt\n\n## Eidnara\n\nGuidance block."]);
    });
});
