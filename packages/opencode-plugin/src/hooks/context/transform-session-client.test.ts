import { describe, expect, it } from "bun:test";
import { existsSync, readFileSync } from "node:fs";
import { dirname, resolve } from "node:path";

import { TransformCaptureAdmission } from "./transform-capture";
import {
    createTransformSessionClient,
    type RustModeModuleClient,
    type TransformPassSource,
} from "./transform-session-client";

/** A plain-data host: values in order, each carrying its id. */
interface PlainHost {
    values: { id: string; text: string }[];
    published: unknown[][];
}

function plainHost(count: number, from = 1): PlainHost {
    return {
        values: Array.from({ length: count }, (_, index) => ({
            id: `m${index + from}`,
            text: `message ${index + from}`,
        })),
        published: [],
    };
}

function source(host: PlainHost): TransformPassSource {
    return {
        serializerProfile: "pi",
        host: {
            get length() {
                return host.values.length;
            },
            idAt: (index) => host.values[index]?.id,
        },
        preflight: async () => "/project",
        readWindow: (start, end) => host.values.slice(start, end),
        idOf: (value) => (value as { id?: string }).id,
        liveWindow: (start, end) => host.values.slice(start, end),
        contextLimit: () => undefined,
        prepare: async () => ({
            encodeInput: () => [],
            fields: { model_key: "test/model" },
        }),
        publicationRejection: () => null,
        publish(values) {
            host.published.push([...values]);
            return undefined;
        },
    };
}

type Reply = (body: Record<string, unknown>) => unknown;

/** A transport that answers each method from a queue of scripted replies and records every body. */
function fakeTransport(replies: Record<string, Reply[]>): {
    client: RustModeModuleClient;
    calls: { method: string; body: Record<string, unknown> }[];
} {
    const calls: { method: string; body: Record<string, unknown> }[] = [];
    return {
        calls,
        client: {
            async call({ method, body }) {
                const record = body as Record<string, unknown>;
                calls.push({ method, body: record });
                const reply = replies[method]?.shift();
                if (!reply) throw new Error(`no scripted reply for ${method}`);
                return reply(record);
            },
        },
    };
}

let outputCounter = 0;

/** An `ok` response that keeps the submitted window after one synthetic summary value. */
function foldReply(boundary: { mid: string; sequence: number } | null): Reply {
    return (body) => {
        outputCounter += 1;
        const window = body.native_messages as unknown[];
        return {
            status: "ok",
            action: "HARD",
            boundary,
            base_revision: body.base_revision,
            output_revision: `out-${outputCounter}`,
            operations: [
                { op: "insert", values: [{ id: "eidnara:synthetic:m0", text: "summary" }] },
                { op: "keep", source: "input", start: 0, count: window.length },
            ],
        };
    };
}

async function pass(
    client: ReturnType<typeof createTransformSessionClient>,
    admission: TransformCaptureAdmission,
    host: PlainHost,
): Promise<void> {
    const admitted = admission.admit("ses");
    if (!("lease" in admitted)) throw new Error("admission declined");
    await client.run("ses", admitted.lease, source(host));
}

describe("transform session client over plain data", () => {
    it("discovers an anchor through the id reader and reaches a steady state", async () => {
        const host = plainHost(10);
        const transport = fakeTransport({
            "transform.boundary": [() => ({ anchors: [{ mid: "m7", sequence: 3 }] })],
            transform: [
                foldReply({ mid: "m7", sequence: 3 }),
                foldReply({ mid: "m7", sequence: 3 }),
            ],
        });
        const client = createTransformSessionClient({ moduleClient: transport.client });
        const admission = new TransformCaptureAdmission();

        await pass(client, admission, host);
        const first = transport.calls.find((call) => call.method === "transform")?.body ?? {};
        expect(first.boundary).toEqual({ mid: "m7", sequence: 3 });
        expect(first.serializer_profile).toBe("pi");
        expect(first.model_key).toBe("test/model");
        expect((first.native_messages as { id: string }[]).map((value) => value.id)).toEqual([
            "m7",
            "m8",
            "m9",
            "m10",
        ]);
        expect(host.published[0]?.map((value) => (value as { id: string }).id)).toEqual([
            "eidnara:synthetic:m0",
            "m7",
            "m8",
            "m9",
            "m10",
        ]);
        expect(client.state("ses").boundary).toEqual({ mid: "m7", sequence: 3 });

        host.values.push({ id: "m11", text: "message 11" });
        await pass(client, admission, host);
        const calls = transport.calls.map((call) => call.method);
        expect(calls).toEqual(["transform.boundary", "transform", "transform"]);
        const second = transport.calls.at(-1)?.body;
        expect(second?.boundary).toEqual({ mid: "m7", sequence: 3 });
        expect(second?.previous_output_revision).toBe(`out-${outputCounter - 1}`);
        expect(host.published).toHaveLength(2);
        expect(client.state("ses")).toMatchObject({
            initialized: true,
            consecutiveFailures: 0,
            passCount: 2,
        });
        expect(admission.activePasses).toBe(0);
        expect(admission.chargedBytes).toBe(0);
    });

    it("declines a busy session without publishing or moving the boundary", async () => {
        const host = plainHost(4);
        const transport = fakeTransport({
            "transform.boundary": [() => ({ anchors: [] })],
            transform: [() => ({ status: "session_busy", action: "SESSION_BUSY" })],
        });
        const client = createTransformSessionClient({ moduleClient: transport.client });
        await pass(client, new TransformCaptureAdmission(), host);
        expect(transport.calls.at(-1)?.body.boundary).toBeNull();
        expect(host.published).toEqual([]);
        expect(client.state("ses")).toMatchObject({
            initialized: false,
            boundary: undefined,
            failureCount: 0,
        });
    });

    it("fails open to the applied output with the messages appended since", async () => {
        const host = plainHost(6);
        const transport = fakeTransport({
            "transform.boundary": [() => ({ anchors: [{ mid: "m3", sequence: 1 }] })],
            transform: [
                foldReply({ mid: "m3", sequence: 1 }),
                () => {
                    throw new Error("daemon unavailable");
                },
            ],
        });
        const client = createTransformSessionClient({ moduleClient: transport.client });
        const admission = new TransformCaptureAdmission();
        await pass(client, admission, host);
        const applied = host.published[0];
        expect(applied).toBeDefined();

        host.values.push({ id: "m7", text: "message 7" });
        await pass(client, admission, host);
        expect(host.published).toHaveLength(2);
        expect(host.published[1]).toEqual([...(applied ?? []), host.values[6]]);
        expect(client.state("ses")).toMatchObject({ consecutiveFailures: 1, failureCount: 1 });
    });
});

/** Relative imports of one module's source, value imports only. */
function valueImports(path: string): string[] {
    const text = readFileSync(path, "utf8");
    const imports: string[] = [];
    for (const match of text.matchAll(/^import\s+(type\s+)?[^;]*?from\s+"([^"]+)";/gms)) {
        if (match[1] || !match[2]) continue;
        imports.push(match[2]);
    }
    return imports;
}

describe("transform session client import rule", () => {
    it("reaches no OpenCode database reader and no host-array writer", () => {
        const root = resolve(import.meta.dir, "transform-session-client.ts");
        const forbidden = new Set(
            [
                "read-session-db.ts",
                "read-session-raw.ts",
                "eidnara-reduce-availability.ts",
                "transform-publication.ts",
                "opencode-transform-adapter.ts",
            ].map((name) => resolve(import.meta.dir, name)),
        );
        const seen = new Set<string>();
        const pending = [root];
        const external: string[] = [];
        while (pending.length > 0) {
            const path = pending.pop() as string;
            if (seen.has(path)) continue;
            seen.add(path);
            expect(forbidden.has(path), `${path} is reachable from the client`).toBe(false);
            for (const specifier of valueImports(path)) {
                if (!specifier.startsWith(".")) {
                    external.push(specifier);
                    continue;
                }
                const resolved = resolve(dirname(path), specifier);
                if (/\.(json|js|mjs)$/.test(resolved)) continue;
                const file = resolved.endsWith(".ts") ? resolved : `${resolved}.ts`;
                pending.push(existsSync(file) ? file : resolve(resolved, "index.ts"));
            }
        }
        expect(seen.size).toBeGreaterThan(1);
        expect(external.filter((specifier) => /sqlite/.test(specifier))).toEqual([]);
    });
});
