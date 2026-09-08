import { afterEach, beforeEach, describe, expect, it, mock, spyOn } from "bun:test";
import { mkdirSync, mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { platform, tmpdir } from "node:os";
import { join } from "node:path";
import { closeReadOnlySessionDb } from "../hooks/context/read-session-db";
import { __ignoredNotificationTest } from "../hooks/context/send-session-notification";
import type { ConflictResult } from "../shared/conflict-detector";
import { formatConflictShort } from "../shared/conflict-detector";
import {
    __resetNotificationStateForTests,
    registerNotificationSink,
} from "../shared/rpc-notifications";
import { Database } from "../shared/sqlite";
import { closeQuietly } from "../shared/sqlite-helpers";
import { cleanupConflictWarnings, sendConflictWarning } from "./conflict-warning-hook";

const SESSION_ID = "ses_conflict_hook_test";
const REAL_TITLE = "Investigating the flaky cache";

let configHome: string;
let originalXdgConfigHome: string | undefined;
let directorySerial = 0;

/** Registers `SESSION_ID` as the Desktop session for a fresh project
 *  directory. The hook caches desktop state per directory, so each test gets
 *  its own directory key. */
function seedDesktopSession(options: { sidecarUrl?: string } = {}): string {
    directorySerial += 1;
    const directory = `/project/conflict-hook-${directorySerial}`;
    const stateDir = join(configHome, "ai.opencode.desktop");
    mkdirSync(stateDir, { recursive: true });
    const state: Record<string, string> = {
        "layout.page": JSON.stringify({
            lastProjectSession: { [directory]: { id: SESSION_ID } },
        }),
    };
    if (options.sidecarUrl) {
        state.server = JSON.stringify({ currentSidecarUrl: options.sidecarUrl });
    }
    writeFileSync(join(stateDir, "opencode.global.dat"), JSON.stringify(state));
    return directory;
}

function titledClient() {
    const prompt = mock(async () => ({}));
    const get = mock(async () => ({ title: REAL_TITLE }));
    const messages = mock(async () => [
        {
            info: {
                role: "assistant",
                agent: "builder",
                model: { providerID: "anthropic", modelID: "claude-fable" },
                variant: "max",
            },
        },
    ]);
    return { client: { session: { prompt, get, messages } }, prompt };
}

const CONFLICT: ConflictResult = {
    hasConflict: true,
    reasons: ["another eidnara install is active"],
} as ConflictResult;

// The Desktop state file location is platform-specific; only the Linux
// location is env-relocatable for an isolated test.
describe.if(platform() === "linux")(
    "conflict-warning senders route through sendIgnoredMessage",
    () => {
        beforeEach(() => {
            originalXdgConfigHome = process.env.XDG_CONFIG_HOME;
            configHome = join(tmpdir(), `conflict-hook-config-${Date.now()}-${directorySerial}`);
            process.env.XDG_CONFIG_HOME = configHome;
        });

        afterEach(() => {
            if (originalXdgConfigHome === undefined) delete process.env.XDG_CONFIG_HOME;
            else process.env.XDG_CONFIG_HOME = originalXdgConfigHome;
            rmSync(configHome, { recursive: true, force: true });
            __ignoredNotificationTest.reset();
            __resetNotificationStateForTests();
        });

        it("defers the conflict warning while the assistant is mid-turn", async () => {
            const directory = seedDesktopSession();
            __ignoredNotificationTest.setMidTurnDetector(() => true);
            const { client, prompt } = titledClient();

            await sendConflictWarning(client, directory, CONFLICT);

            expect(prompt).not.toHaveBeenCalled();
            expect(__ignoredNotificationTest.pendingTexts(SESSION_ID)).toHaveLength(1);
        });

        it("pins the session's agent, model, and variant onto the conflict warning", async () => {
            const directory = seedDesktopSession();
            __ignoredNotificationTest.setMidTurnDetector(() => false);
            const { client, prompt } = titledClient();

            await sendConflictWarning(client, directory, CONFLICT);

            expect(prompt).toHaveBeenCalledTimes(1);
            const input = prompt.mock.calls[0]?.[0] as {
                body: { agent?: string; model?: unknown; variant?: string };
            };
            expect(input.body.agent).toBe("builder");
            expect(input.body.model).toEqual({ providerID: "anthropic", modelID: "claude-fable" });
            expect(input.body.variant).toBe("max");
        });

        it("does not persist a second warning while one is already in the session", async () => {
            const directory = seedDesktopSession();
            __ignoredNotificationTest.setMidTurnDetector(() => false);
            const prompt = mock(async () => ({}));
            const client = {
                session: {
                    prompt,
                    get: mock(async () => ({ title: REAL_TITLE })),
                    messages: mock(async () => [
                        {
                            info: { id: "msg_warning", role: "user" },
                            parts: [
                                {
                                    type: "text",
                                    text: formatConflictShort(CONFLICT),
                                    ignored: true,
                                },
                            ],
                        },
                    ]),
                },
            };

            await sendConflictWarning(client, directory, CONFLICT);

            expect(prompt).not.toHaveBeenCalled();
        });

        it("persists the conflict warning even when a TUI is connected", async () => {
            // cleanupConflictWarnings deletes the persisted warning row when
            // the conflict is resolved; a toast would leave nothing to clean
            // and vanish after five seconds despite the blocking state.
            const directory = seedDesktopSession();
            __ignoredNotificationTest.setMidTurnDetector(() => false);
            const toasts: unknown[] = [];
            const unregister = registerNotificationSink({
                sessionId: SESSION_ID,
                protocol: 2,
                send: (notification) => toasts.push(notification),
            });
            try {
                const { client, prompt } = titledClient();
                await sendConflictWarning(client, directory, CONFLICT);
                expect(prompt).toHaveBeenCalledTimes(1);
                expect(toasts).toHaveLength(0);
            } finally {
                unregister();
            }
        });

        it("routes the enabled confirmation to the TUI toast when a TUI is connected", async () => {
            const directory = seedDesktopSession();
            __ignoredNotificationTest.setMidTurnDetector(() => false);
            const warningText = formatConflictShort(CONFLICT);
            const toasts: unknown[] = [];
            const unregister = registerNotificationSink({
                sessionId: SESSION_ID,
                protocol: 2,
                send: (notification) => toasts.push(notification),
            });
            const fetchSpy = spyOn(globalThis, "fetch").mockImplementation(
                (async () => new Response("{}", { status: 200 })) as unknown as typeof fetch,
            );
            try {
                const prompt = mock(async () => ({}));
                const messages = mock(async () => ({
                    data: [
                        {
                            info: { id: "msg_warning", role: "user" },
                            parts: [{ type: "text", text: warningText, ignored: true }],
                        },
                    ],
                }));
                const client = {
                    session: {
                        prompt,
                        get: mock(async () => ({ title: REAL_TITLE })),
                        messages,
                    },
                };

                await cleanupConflictWarnings(client, directory, "http://127.0.0.1:1");

                expect(prompt).not.toHaveBeenCalled();
                expect(toasts).toHaveLength(1);
            } finally {
                unregister();
                fetchSpy.mockRestore();
            }
        });

        it("deletes a warning that later user and assistant turns have buried", async () => {
            const directory = seedDesktopSession();
            __ignoredNotificationTest.setMidTurnDetector(() => false);
            const warningText = formatConflictShort(CONFLICT);
            const deletedUrls: string[] = [];
            const fetchSpy = spyOn(globalThis, "fetch").mockImplementation((async (
                input: string | URL | Request,
            ) => {
                deletedUrls.push(String(input));
                return new Response("{}", { status: 200 });
            }) as unknown as typeof fetch);
            try {
                const prompt = mock(async () => ({}));
                // The SDK returns the bare array when the client is built with
                // `responseStyle: "data"`; the hook must read that shape too.
                const messages = mock(async () => [
                    {
                        info: { id: "msg_warning", role: "user" },
                        parts: [{ type: "text", text: warningText, ignored: true }],
                    },
                    {
                        info: { id: "msg_user", role: "user" },
                        parts: [{ type: "text", text: "carry on without eidnara" }],
                    },
                    {
                        info: { id: "msg_assistant", role: "assistant" },
                        parts: [{ type: "text", text: "sure" }],
                    },
                ]);
                const client = {
                    session: {
                        prompt,
                        get: mock(async () => ({ title: REAL_TITLE })),
                        messages,
                    },
                };

                await cleanupConflictWarnings(client, directory, "http://127.0.0.1:1");

                expect(deletedUrls).toEqual([
                    `http://127.0.0.1:1/session/${SESSION_ID}/message/msg_warning`,
                ]);
                // The enabled confirmation persists when no TUI is connected.
                expect(prompt).toHaveBeenCalledTimes(1);
            } finally {
                fetchSpy.mockRestore();
            }
        });

        it("finds a warning buried under more than the SDK window through OpenCode's database", async () => {
            const directory = seedDesktopSession();
            __ignoredNotificationTest.setMidTurnDetector(() => false);
            const warningText = formatConflictShort(CONFLICT);
            const dataHome = mkdtempSync(join(tmpdir(), "conflict-hook-data-"));
            const originalXdgDataHome = process.env.XDG_DATA_HOME;
            const dbPath = join(dataHome, "opencode", "opencode.db");
            mkdirSync(join(dataHome, "opencode"), { recursive: true });
            const db = new Database(dbPath);
            db.exec(`
                CREATE TABLE message (id TEXT PRIMARY KEY, session_id TEXT NOT NULL, time_created INTEGER NOT NULL, data TEXT NOT NULL);
                CREATE TABLE part (id TEXT PRIMARY KEY, session_id TEXT NOT NULL, message_id TEXT NOT NULL, data TEXT NOT NULL);
            `);
            const insertMessage = db.prepare(
                "INSERT INTO message (id, session_id, time_created, data) VALUES (?, ?, ?, ?)",
            );
            const insertPart = db.prepare(
                "INSERT INTO part (id, session_id, message_id, data) VALUES (?, ?, ?, ?)",
            );
            insertMessage.run("msg_warning", SESSION_ID, 1, JSON.stringify({ role: "user" }));
            insertPart.run(
                "prt_warning",
                SESSION_ID,
                "msg_warning",
                JSON.stringify({ type: "text", text: warningText, ignored: true }),
            );
            // A real user message that merely mentions the marker text keeps a non-ignored part and must survive.
            insertMessage.run("msg_quote", SESSION_ID, 2, JSON.stringify({ role: "user" }));
            insertPart.run(
                "prt_quote",
                SESSION_ID,
                "msg_quote",
                JSON.stringify({ type: "text", text: warningText }),
            );
            for (let index = 0; index < 60; index += 1) {
                insertMessage.run(
                    `msg_later_${index}`,
                    SESSION_ID,
                    10 + index,
                    JSON.stringify({ role: index % 2 === 0 ? "user" : "assistant" }),
                );
                insertPart.run(
                    `prt_later_${index}`,
                    SESSION_ID,
                    `msg_later_${index}`,
                    JSON.stringify({ type: "text", text: `turn ${index}` }),
                );
            }
            closeQuietly(db);
            process.env.XDG_DATA_HOME = dataHome;

            const deletedUrls: string[] = [];
            const fetchSpy = spyOn(globalThis, "fetch").mockImplementation((async (
                input: string | URL | Request,
            ) => {
                deletedUrls.push(String(input));
                return new Response("{}", { status: 200 });
            }) as unknown as typeof fetch);
            const messages = mock(async () => []);
            try {
                const client = {
                    session: {
                        prompt: mock(async () => ({})),
                        get: mock(async () => ({ title: REAL_TITLE })),
                        messages,
                    },
                };

                await cleanupConflictWarnings(client, directory, "http://127.0.0.1:1");

                // The SDK mock returns no messages, so only the database scan can have found the warning.
                expect(deletedUrls).toEqual([
                    `http://127.0.0.1:1/session/${SESSION_ID}/message/msg_warning`,
                ]);
            } finally {
                fetchSpy.mockRestore();
                closeReadOnlySessionDb();
                if (originalXdgDataHome === undefined) delete process.env.XDG_DATA_HOME;
                else process.env.XDG_DATA_HOME = originalXdgDataHome;
                rmSync(dataHome, { recursive: true, force: true });
            }
        });

        it("skips the enabled confirmation while a warning row survives a failed DELETE", async () => {
            const directory = seedDesktopSession();
            __ignoredNotificationTest.setMidTurnDetector(() => false);
            const warningText = formatConflictShort(CONFLICT);
            const fetchSpy = spyOn(globalThis, "fetch").mockImplementation(
                (async () => new Response("nope", { status: 500 })) as unknown as typeof fetch,
            );
            try {
                const prompt = mock(async () => ({}));
                const messages = mock(async () => ({
                    data: [
                        {
                            info: { id: "msg_warning", role: "user" },
                            parts: [{ type: "text", text: warningText, ignored: true }],
                        },
                    ],
                }));
                const client = {
                    session: {
                        prompt,
                        get: mock(async () => ({ title: REAL_TITLE })),
                        messages,
                    },
                };

                await cleanupConflictWarnings(client, directory, "http://127.0.0.1:1");

                expect(fetchSpy).toHaveBeenCalledTimes(1);
                expect(prompt).not.toHaveBeenCalled();
            } finally {
                fetchSpy.mockRestore();
            }
        });

        it("issues every warning DELETE concurrently so one stalled endpoint costs one timeout", async () => {
            const directory = seedDesktopSession();
            __ignoredNotificationTest.setMidTurnDetector(() => false);
            const warningText = formatConflictShort(CONFLICT);
            const warningCount = 5;
            let inFlight = 0;
            let peakInFlight = 0;
            let release: () => void = () => {};
            const gate = new Promise<void>((resolve) => {
                release = resolve;
            });
            const fetchSpy = spyOn(globalThis, "fetch").mockImplementation((async () => {
                inFlight += 1;
                peakInFlight = Math.max(peakInFlight, inFlight);
                await gate;
                inFlight -= 1;
                return new Response("{}", { status: 200 });
            }) as unknown as typeof fetch);
            try {
                const prompt = mock(async () => ({}));
                const messages = mock(async () => ({
                    data: Array.from({ length: warningCount }, (_, i) => ({
                        info: { id: `msg_warning_${i}`, role: "user" },
                        parts: [{ type: "text", text: warningText, ignored: true }],
                    })),
                }));
                const client = {
                    session: {
                        prompt,
                        get: mock(async () => ({ title: REAL_TITLE })),
                        messages,
                    },
                };

                const cleanup = cleanupConflictWarnings(client, directory, "http://127.0.0.1:1");
                for (let i = 0; i < 10 && inFlight < warningCount; i++) {
                    await new Promise((resolve) => setTimeout(resolve, 0));
                }
                expect(peakInFlight).toBe(warningCount);
                release();
                await cleanup;

                expect(fetchSpy).toHaveBeenCalledTimes(warningCount);
                expect(prompt).toHaveBeenCalledTimes(1);
            } finally {
                fetchSpy.mockRestore();
            }
        });

        it("falls back to the Desktop sidecar URL when no serverUrl is supplied", async () => {
            const directory = seedDesktopSession({ sidecarUrl: "http://127.0.0.1:4096" });
            __ignoredNotificationTest.setMidTurnDetector(() => false);
            const warningText = formatConflictShort(CONFLICT);
            const deletedUrls: string[] = [];
            const fetchSpy = spyOn(globalThis, "fetch").mockImplementation((async (
                input: string | URL | Request,
            ) => {
                deletedUrls.push(String(input));
                return new Response("{}", { status: 200 });
            }) as unknown as typeof fetch);
            try {
                const prompt = mock(async () => ({}));
                const messages = mock(async () => ({
                    data: [
                        {
                            info: { id: "msg_warning", role: "user" },
                            parts: [{ type: "text", text: warningText, ignored: true }],
                        },
                    ],
                }));
                const client = {
                    session: {
                        prompt,
                        get: mock(async () => ({ title: REAL_TITLE })),
                        messages,
                    },
                };

                await cleanupConflictWarnings(client, directory);

                expect(deletedUrls).toEqual([
                    `http://127.0.0.1:4096/session/${SESSION_ID}/message/msg_warning`,
                ]);
            } finally {
                fetchSpy.mockRestore();
            }
        });
    },
);
