import { describe, expect, it, mock } from "bun:test";

import { knownSessionDirectory, resolveSessionDirectory } from "./session-directory";

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

    it("uses the launch directory when no client is available", async () => {
        expect(await resolveSessionDirectory({ directory: "/launch" }, "ses-none")).toBe("/launch");
        expect(knownSessionDirectory({}, "ses-none")).toBe(process.cwd());
    });
});
