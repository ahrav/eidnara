/// <reference types="bun-types" />

import { afterEach, describe, expect, it, mock } from "bun:test";
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

type RecordedCall = { sessionId: string; projectRoot: string; method: string; body: unknown };

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

function createFakeModuleClient(
    respond: (call: RecordedCall) => unknown = () => ({ ok: true }),
): FakeModuleClient {
    const calls: RecordedCall[] = [];
    const deleteSession = mock(async () => {});
    const closeSession = mock(() => {});
    const client: RustModeModuleClient = {
        call: async ({ sessionId, projectRoot, method, body }) => {
            if (!isModuleCallBodyValid(method, body)) {
                throw new TypeError(`invalid fake module body for ${method}`);
            }
            const call = { sessionId, projectRoot, method, body };
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
        const fake = createFakeModuleClient(({ method }) =>
            method === "transform"
                ? { decision: "PASSTHROUGH", native_messages: [] }
                : { ok: true },
        );
        const liveSessionState = createLiveSessionState();
        const client = createClientMock(undefined, "/other/repo") as unknown as {
            session: { get: ReturnType<typeof mock> };
        };
        client.session.get = mock(async () => ({
            data: { directory: "/other/repo", parentID: "ses-parent", title: "eidnara-sidekick" },
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
        const fake = createFakeModuleClient(({ method }) =>
            method === "transform"
                ? { decision: "PASSTHROUGH", native_messages: [] }
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

    it("routes ctx_note through the pinned root and derives its memory project", async () => {
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
                method: "ctx_note",
                body: {
                    name: "ctx_note",
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

    it("omits compiled fields from ctx_note arguments without a compile status", async () => {
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
                (call) => call.method === "agent_drops.append" || call.method === "ctx_note",
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
        const fake = createFakeModuleClient(({ method }) =>
            method === "transform"
                ? { decision: "PASSTHROUGH", native_messages: [] }
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
        const fake = createFakeModuleClient(({ method }) =>
            method === "transform"
                ? { decision: "PASSTHROUGH", native_messages: [] }
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
        const fake = createFakeModuleClient(({ method }) =>
            method === "transform"
                ? { decision: "PASSTHROUGH", native_messages: [] }
                : { ok: true },
        );
        const liveSessionState = createLiveSessionState();
        const kernelConfig = {
            subc: { connection_file: "/nonexistent/eidnara-hook-test/subc.json" },
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

    it("forwards /ctx-flush to session.flush and throws the sentinel", async () => {
        useTempDataHome("hook-flush-");
        const fake = createFakeModuleClient(() => ({ result: { armed: false } }));
        const promptMock = mock(() => undefined);
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
                { command: "ctx-flush", sessionID: "ses-flush", arguments: "" },
                { parts: [{ type: "text", text: "" }] },
            ),
            "__CONTEXT_MANAGEMENT_CTX-FLUSH_HANDLED__",
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
                { command: "ctx-flush", sessionID: "ses-routed-cmd", arguments: "" },
                { parts: [{ type: "text", text: "" }] },
            ),
            "__CONTEXT_MANAGEMENT_CTX-FLUSH_HANDLED__",
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
            { command: "ctx-flush", sessionID: sessionId, arguments: "" },
            { parts: [{ type: "text", text: "" }] },
        );
        while (releaseDirectoryRead === undefined) await Bun.sleep(0);

        await hook.event({
            event: { type: "session.deleted", properties: { info: { id: sessionId } } },
        });
        releaseDirectoryRead();
        await expectSentinel(command, "__CONTEXT_MANAGEMENT_CTX-FLUSH_HANDLED__");

        expect(fake.calls.filter((call) => call.method === "session.flush")).toHaveLength(0);
        expect(liveSessionState.sessionDirectoryBySession.has(sessionId)).toBe(false);
        expect(liveSessionState.sessionMetadataReadStateBySession.has(sessionId)).toBe(false);
        expect(liveSessionState.subagentSessions.has(sessionId)).toBe(false);
        expect(liveSessionState.internalChildSessions.has(sessionId)).toBe(false);
    });
});
