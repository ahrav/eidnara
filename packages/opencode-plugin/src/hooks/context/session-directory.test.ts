import { afterEach, describe, expect, it, mock } from "bun:test";
import type { SessionMetadataReadState } from "./live-session-state";
import {
    __sessionDirectoryTest,
    knownSessionDirectory,
    resolveSessionDirectory,
} from "./session-directory";

afterEach(() => __sessionDirectoryTest.reset());

describe("resolveSessionDirectory", () => {
    it("returns the cached host directory without an SDK read", async () => {
        const get = mock(async () => ({ data: { directory: "/from/sdk" } }));
        const deps = {
            client: { session: { get } } as never,
            directory: "/launch",
            sessionDirectoryBySession: new Map([["ses-cached", "/cached"]]),
        };
        expect(await resolveSessionDirectory(deps, "ses-cached")).toBe("/cached");
        expect(get).not.toHaveBeenCalled();
    });

    it("reads the host directory once and caches it", async () => {
        const get = mock(async () => ({ data: { directory: "/from/sdk" } }));
        const deps = {
            client: { session: { get } } as never,
            directory: "/launch",
            sessionDirectoryBySession: new Map<string, string>(),
        };
        expect(await resolveSessionDirectory(deps, "ses-sdk")).toBe("/from/sdk");
        expect(await resolveSessionDirectory(deps, "ses-sdk")).toBe("/from/sdk");
        expect(get).toHaveBeenCalledTimes(1);
        expect(knownSessionDirectory(deps, "ses-sdk")).toBe("/from/sdk");
    });

    it("falls back to the launch directory when the read fails, is empty, or never settles", async () => {
        const failing = {
            client: {
                session: { get: mock(async () => Promise.reject(new Error("boom"))) },
            } as never,
            directory: "/launch",
        };
        expect(await resolveSessionDirectory(failing, "ses-fail")).toBe("/launch");

        const empty = {
            client: { session: { get: mock(async () => ({ data: {} })) } } as never,
            directory: "/launch",
        };
        expect(await resolveSessionDirectory(empty, "ses-empty")).toBe("/launch");

        const hung = {
            client: { session: { get: mock(() => new Promise<never>(() => {})) } } as never,
            directory: "/launch",
        };
        const startedAt = performance.now();
        expect(await resolveSessionDirectory(hung, "ses-hung")).toBe("/launch");
        expect(performance.now() - startedAt).toBeGreaterThanOrEqual(1_500);
    });

    it("pins the fallback so a later successful read cannot move the session's route", async () => {
        let fail = true;
        const get = mock(async () => {
            if (fail) throw new Error("boom");
            return { data: { directory: "/from/sdk" } };
        });
        const deps = {
            client: { session: { get } } as never,
            directory: "/launch",
            sessionDirectoryBySession: new Map<string, string>(),
        };
        expect(await resolveSessionDirectory(deps, "ses-pinned")).toBe("/launch");
        fail = false;
        expect(await resolveSessionDirectory(deps, "ses-pinned")).toBe("/launch");
        expect(get).toHaveBeenCalledTimes(1);
        expect(deps.sessionDirectoryBySession.get("ses-pinned")).toBe("/launch");
    });

    it("retries child classification after a failed read without moving the fallback route", async () => {
        __sessionDirectoryTest.setRetryDelayMs(0);
        let fail = true;
        const get = mock(async () => {
            if (fail) throw new Error("boom");
            return {
                data: { directory: "/from/sdk", parentID: "ses-parent", title: "eidnara-sidekick" },
            };
        });
        const subagentSessions = new Set<string>();
        const internalChildSessions = new Set<string>();
        const sessionMetadataReadStateBySession = new Map<string, SessionMetadataReadState>();
        const deps = {
            client: { session: { get } } as never,
            directory: "/launch",
            sessionDirectoryBySession: new Map<string, string>(),
            sessionMetadataReadStateBySession,
            subagentSessions,
            internalChildSessions,
        };

        expect(await resolveSessionDirectory(deps, "ses-restored-child")).toBe("/launch");
        fail = false;
        expect(await resolveSessionDirectory(deps, "ses-restored-child")).toBe("/launch");
        expect(await resolveSessionDirectory(deps, "ses-restored-child")).toBe("/launch");

        expect(get).toHaveBeenCalledTimes(2);
        expect(sessionMetadataReadStateBySession.get("ses-restored-child")?.attempts).toBe(2);
        expect(subagentSessions.has("ses-restored-child")).toBe(true);
        expect(internalChildSessions.has("ses-restored-child")).toBe(true);
    });

    it("catches a synchronous SDK throw and preserves the bounded metadata retry", async () => {
        __sessionDirectoryTest.setRetryDelayMs(0);
        let calls = 0;
        const get = mock(() => {
            calls++;
            if (calls === 1) throw new Error("synchronous SDK failure");
            return Promise.resolve({
                data: { directory: "/from/sdk", parentID: "ses-parent" },
            });
        });
        const subagentSessions = new Set<string>();
        const deps = {
            client: { session: { get } } as never,
            directory: "/launch",
            sessionDirectoryBySession: new Map<string, string>(),
            sessionMetadataReadStateBySession: new Map<string, SessionMetadataReadState>(),
            subagentSessions,
        };

        expect(await resolveSessionDirectory(deps, "ses-sync-throw")).toBe("/launch");
        expect(await resolveSessionDirectory(deps, "ses-sync-throw")).toBe("/launch");

        expect(get).toHaveBeenCalledTimes(2);
        expect(subagentSessions.has("ses-sync-throw")).toBe(true);
    });

    it("stops retrying metadata after the bounded second failure", async () => {
        __sessionDirectoryTest.setRetryDelayMs(0);
        const get = mock(async () => Promise.reject(new Error("still unavailable")));
        const deps = {
            client: { session: { get } } as never,
            directory: "/launch",
            sessionDirectoryBySession: new Map<string, string>(),
            sessionMetadataReadStateBySession: new Map(),
        };

        expect(await resolveSessionDirectory(deps, "ses-unavailable")).toBe("/launch");
        expect(await resolveSessionDirectory(deps, "ses-unavailable")).toBe("/launch");
        expect(await resolveSessionDirectory(deps, "ses-unavailable")).toBe("/launch");

        expect(get).toHaveBeenCalledTimes(2);
    });

    it("does not spend the retry on a duplicate read in the same failure window", async () => {
        __sessionDirectoryTest.setRetryDelayMs(20);
        let fail = true;
        const get = mock(async () => {
            if (fail) throw new Error("temporarily unavailable");
            return { data: { directory: "/from/sdk", parentID: "ses-parent" } };
        });
        const subagentSessions = new Set<string>();
        const deps = {
            client: { session: { get } } as never,
            directory: "/launch",
            sessionDirectoryBySession: new Map<string, string>(),
            sessionMetadataReadStateBySession: new Map(),
            subagentSessions,
        };

        expect(await resolveSessionDirectory(deps, "ses-retry-next-turn")).toBe("/launch");
        expect(await resolveSessionDirectory(deps, "ses-retry-next-turn")).toBe("/launch");
        fail = false;
        await Bun.sleep(25);
        expect(await resolveSessionDirectory(deps, "ses-retry-next-turn")).toBe("/launch");

        expect(get).toHaveBeenCalledTimes(2);
        expect(subagentSessions.has("ses-retry-next-turn")).toBe(true);
    });

    it("shares a concurrent first metadata read and preserves the later retry", async () => {
        __sessionDirectoryTest.setRetryDelayMs(20);
        let fail = true;
        const get = mock(async () => {
            if (fail) throw new Error("temporarily unavailable");
            return { data: { directory: "/from/sdk", parentID: "ses-parent" } };
        });
        const subagentSessions = new Set<string>();
        const deps = {
            client: { session: { get } } as never,
            directory: "/launch",
            sessionDirectoryBySession: new Map<string, string>(),
            sessionMetadataReadStateBySession: new Map<string, SessionMetadataReadState>(),
            subagentSessions,
        };

        expect(
            await Promise.all([
                resolveSessionDirectory(deps, "ses-concurrent-retry"),
                resolveSessionDirectory(deps, "ses-concurrent-retry"),
            ]),
        ).toEqual(["/launch", "/launch"]);
        expect(get).toHaveBeenCalledTimes(1);

        fail = false;
        await Bun.sleep(25);
        expect(await resolveSessionDirectory(deps, "ses-concurrent-retry")).toBe("/launch");
        expect(get).toHaveBeenCalledTimes(2);
        expect(subagentSessions.has("ses-concurrent-retry")).toBe(true);
    });

    it("records a session with a parentID as a subagent from the same read", async () => {
        const get = mock(async ({ path }: { path: { id: string } }) => ({
            data: {
                directory: "/from/sdk",
                parentID: path.id === "ses-child" ? "ses-parent" : undefined,
            },
        }));
        const subagentSessions = new Set<string>();
        const deps = {
            client: { session: { get } } as never,
            directory: "/launch",
            sessionDirectoryBySession: new Map<string, string>(),
            subagentSessions,
        };
        await resolveSessionDirectory(deps, "ses-child");
        await resolveSessionDirectory(deps, "ses-parent");
        expect([...subagentSessions]).toEqual(["ses-child"]);
    });

    it("keeps the first pin when concurrent first-time callers finish in the other order", async () => {
        let settleSlow: ((value: { data: { directory: string } }) => void) | undefined;
        let calls = 0;
        const get = mock(() => {
            calls += 1;
            if (calls === 1) {
                return new Promise<{ data: { directory: string } }>((resolve) => {
                    settleSlow = resolve;
                });
            }
            return Promise.resolve({ data: { directory: "/from/sdk" } });
        });
        const deps = {
            client: { session: { get } } as never,
            directory: "/launch",
            sessionDirectoryBySession: new Map<string, string>(),
        };
        const slow = resolveSessionDirectory(deps, "ses-race");
        const fast = await resolveSessionDirectory(deps, "ses-race");
        expect(fast).toBe("/from/sdk");
        settleSlow?.({ data: { directory: "/other/root" } });
        expect(await slow).toBe("/from/sdk");
        expect(deps.sessionDirectoryBySession.get("ses-race")).toBe("/from/sdk");
    });

    it("uses the launch directory when no client is available", async () => {
        expect(await resolveSessionDirectory({ directory: "/launch" }, "ses-none")).toBe("/launch");
        expect(knownSessionDirectory({}, "ses-none")).toBe(process.cwd());
    });
});
