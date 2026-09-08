import { afterEach, describe, expect, it, mock } from "bun:test";
import {
    __childSessionSpawnTest,
    childSessionMessagesFetcher,
    createChildSession,
    deleteChildSession,
} from "./child-session-spawn";

afterEach(() => {
    __childSessionSpawnTest.reset();
});

describe("createChildSession", () => {
    it("creates the child under its parent with the title and directory", async () => {
        const create = mock(async (_input: unknown) => ({ id: "ses_child" }));
        const result = await createChildSession({
            client: { session: { create } } as never,
            parentSessionId: "ses_parent",
            title: "eidnara-sidekick",
            directory: "/work/dir",
        });
        expect(result).toEqual({ id: "ses_child" });
        expect(create.mock.calls[0]?.[0]).toEqual({
            body: { parentID: "ses_parent", title: "eidnara-sidekick" },
            query: { directory: "/work/dir" },
        });
    });

    it("omits the parent when none is given", async () => {
        const create = mock(async (_input: unknown) => ({}));
        await createChildSession({ client: { session: { create } } as never, title: "t" });
        expect(create.mock.calls[0]?.[0]).toEqual({
            body: { title: "t" },
            query: { directory: undefined },
        });
    });

    it("rejects when the create endpoint never settles", async () => {
        const create = mock(() => new Promise<never>(() => {}));
        __childSessionSpawnTest.setLifecycleTimeoutMs(20);
        await expect(
            createChildSession({ client: { session: { create } } as never, title: "t" }),
        ).rejects.toThrow("child session create timed out");
    });
});

describe("deleteChildSession", () => {
    it("deletes by id and rejects when the endpoint never settles", async () => {
        const del = mock(async (_input: unknown) => ({}));
        await deleteChildSession({ session: { delete: del } } as never, "ses_child");
        expect(del.mock.calls[0]?.[0]).toEqual({ path: { id: "ses_child" } });

        const hung = mock(() => new Promise<never>(() => {}));
        __childSessionSpawnTest.setLifecycleTimeoutMs(20);
        await expect(
            deleteChildSession({ session: { delete: hung } } as never, "ses_child"),
        ).rejects.toThrow("child session delete timed out");
    });
});

describe("childSessionMessagesFetcher", () => {
    it("reads the child session messages with the bound id, directory, and limit", async () => {
        const messages = mock(async (_input: unknown) => [{ info: { role: "assistant" } }]);
        const fetcher = childSessionMessagesFetcher(
            { session: { messages } } as never,
            "ses_child",
            "/work/dir",
            50,
        );
        const output = await fetcher();
        expect(messages).toHaveBeenCalledTimes(1);
        expect(messages.mock.calls[0]?.[0]).toEqual({
            path: { id: "ses_child" },
            query: { directory: "/work/dir", limit: 50 },
        });
        expect(output).toEqual([{ info: { role: "assistant" } }]);
    });

    it("unwraps a data-envelope response and falls back to an empty array", async () => {
        const enveloped = childSessionMessagesFetcher(
            { session: { messages: async () => ({ data: [{ id: 1 }] }) } } as never,
            "ses_child",
            undefined,
            20,
        );
        expect(await enveloped()).toEqual([{ id: 1 }]);

        // preferResponseOnMissingData keeps the bare wrapper when `data` is null,
        // matching every prompt-run call site this helper replaced.
        const bareWrapper = childSessionMessagesFetcher(
            { session: { messages: async () => ({ data: null }) } } as never,
            "ses_child",
            undefined,
            20,
        );
        expect(await bareWrapper()).toEqual({ data: null } as never);

        const missing = childSessionMessagesFetcher(
            { session: { messages: async () => null } } as never,
            "ses_child",
            undefined,
            20,
        );
        expect(await missing()).toEqual([]);
    });
});
