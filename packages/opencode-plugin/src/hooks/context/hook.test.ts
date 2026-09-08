/// <reference types="bun-types" />

import { afterEach, describe, expect, it, mock } from "bun:test";
import { mkdtempSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import {
    clearHookInitFailure,
    getLastHookInitFailure,
} from "../../features/context/fail-closed-block";
import { __resetProjectIdentityForTests } from "../../features/context/project-identity";
import { createEidnaraHook, type EidnaraDeps } from "./hook";
import { createLiveSessionState } from "./live-session-state";
import type { RustModeModuleClient } from "./rust-mode-transform";

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
const originalXdgDataHome = process.env.XDG_DATA_HOME;

/** An empty data home has no `opencode.db`, so tool verdicts freeze fail-open and prompt hashes persist. */
function useTempDataHome(prefix: string): string {
    const dir = mkdtempSync(join(tmpdir(), prefix));
    tempDirs.push(dir);
    process.env.XDG_DATA_HOME = dir;
    return dir;
}

afterEach(() => {
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
            const call = { sessionId, projectRoot, method, body };
            calls.push(call);
            return respond(call);
        },
        deleteSession,
        closeSession,
    };
    return { client, calls, deleteSession, closeSession };
}

function createClientMock(promptMock = mock(() => undefined)) {
    return {
        session: {
            prompt: promptMock,
            promptAsync: mock(async () => undefined),
            get: mock(async () => ({ data: {} })),
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
        expect(Object.keys(hook.rustToolBackends).sort()).toEqual([
            "note",
            "noteEvaluationAvailable",
            "reduce",
        ]);
        expect(hook.rustToolBackends.noteEvaluationAvailable?.("any-project")).toBe(true);
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
        expect(Object.keys(hook.rustToolBackends).sort()).toEqual([
            "note",
            "noteEvaluationAvailable",
            "reduce",
        ]);

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

    it("sends agent_drops.append through rustToolBackends.reduce", async () => {
        useTempDataHome("hook-reduce-");
        const fake = createFakeModuleClient();
        const hook = requireHook(
            createEidnaraHook(createDeps({ rustModeModuleClient: fake.client })),
        );

        await hook.rustToolBackends?.reduce?.({
            sessionId: "ses-reduce",
            projectRoot: "/repo",
            drop: "tool output summary",
            commandId: "cmd-1",
        });

        expect(fake.calls).toEqual([
            {
                sessionId: "ses-reduce",
                projectRoot: "/repo",
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

    it("sends ctx_note facade arguments through rustToolBackends.note", async () => {
        useTempDataHome("hook-note-");
        const fake = createFakeModuleClient(() => ({ result: { note_id: 7 } }));
        const hook = requireHook(
            createEidnaraHook(createDeps({ rustModeModuleClient: fake.client })),
        );

        const response = await hook.rustToolBackends?.note?.({
            commandId: "cmd-note",
            sessionId: "ses-note",
            projectRoot: "/repo",
            projectPath: "git:abc",
            memoryProject: "git:abc",
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
                projectRoot: "/repo",
                method: "ctx_note",
                body: {
                    name: "ctx_note",
                    arguments: {
                        command_id: "cmd-note",
                        action: "write",
                        content: "Remember the build flag",
                        memory_project: "git:abc",
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
            projectRoot: "/repo",
            projectPath: "git:abc",
            memoryProject: "git:abc",
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

    it("clears the transform session and prompt state on session.deleted", async () => {
        useTempDataHome("hook-session-deleted-");
        const fake = createFakeModuleClient();
        const liveSessionState = createLiveSessionState();
        const hook = requireHook(
            createEidnaraHook(createDeps({ rustModeModuleClient: fake.client, liveSessionState })),
        );
        const sessionId = "ses-deleted";
        const selectModel = () =>
            hook["chat.message"]({
                sessionID: sessionId,
                model: { providerID: "provider", modelID: "model" },
            });

        await selectModel();
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

        expect(fake.deleteSession).toHaveBeenCalledWith(sessionId, "/tmp");
        expect(fake.closeSession).toHaveBeenCalledWith(sessionId);
        expect(liveSessionState.liveModelBySession.has(sessionId)).toBe(false);
        expect(liveSessionState.historyRefreshSessions.has(sessionId)).toBe(false);

        // A prompt change after deletion finds no persisted hash, so it initializes instead of flagging a change.
        await selectModel();
        await hook["experimental.chat.system.transform"](
            { sessionID: sessionId },
            { system: ["third prompt"] },
        );
        expect(liveSessionState.historyRefreshSessions.has(sessionId)).toBe(false);
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
});
