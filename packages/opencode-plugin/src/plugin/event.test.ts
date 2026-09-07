import { describe, expect, test } from "bun:test";
import { createEventHandler } from "./event";

describe("createEventHandler — instance dispose cleanup", () => {
    test("fires onInstanceDisposed with the directory on server.instance.disposed", async () => {
        const calls: string[] = [];
        const handler = createEventHandler({
            eidnara: null,
            onInstanceDisposed: (dir) => {
                calls.push(dir);
            },
        });

        await handler({
            event: {
                type: "server.instance.disposed",
                properties: { directory: "/proj/a" },
            } as any,
        });

        expect(calls).toEqual(["/proj/a"]);
    });

    test("does not fire onInstanceDisposed for unrelated events", async () => {
        let fired = false;
        const handler = createEventHandler({
            eidnara: null,
            onInstanceDisposed: () => {
                fired = true;
            },
        });

        await handler({ event: { type: "message.updated", properties: {} } as any });
        await handler({ event: { type: "session.deleted", properties: {} } as any });

        expect(fired).toBe(false);
    });

    test("swallows a throwing cleanup without rejecting", async () => {
        const handler = createEventHandler({
            eidnara: null,
            onInstanceDisposed: () => {
                throw new Error("cleanup boom");
            },
        });

        await expect(
            handler({
                event: {
                    type: "server.instance.disposed",
                    properties: { directory: "/proj/b" },
                } as any,
            }),
        ).resolves.toBeUndefined();
    });

    test("still runs the eidnara handler for every event", async () => {
        const seen: string[] = [];
        const handler = createEventHandler({
            eidnara: {
                event: async (input) => {
                    seen.push(`eidnara:${input.event.type}`);
                },
            },
        });

        await handler({ event: { type: "message.updated", properties: {} } as any });

        expect(seen).toEqual(["eidnara:message.updated"]);
    });
});
