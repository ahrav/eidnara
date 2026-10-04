import { describe, expect, it } from "bun:test";
import { existsSync, readFileSync } from "node:fs";
import { dirname, relative, resolve } from "node:path";

import { TransformCaptureAdmission } from "./transform-capture";
import {
    createTransformSessionClient,
    type RustModeModuleClient,
    type TransformPassOutcome,
    type TransformPassSource,
} from "./transform-session-client";
import { HALF_CAP_BLOCKS, HALF_CAP_BYTES } from "./window-cap";

/** A plain-data host: values in order, each carrying its id. */
interface PlainHost {
    values: { id: string; text: string }[];
    published: unknown[][];
    windows: [number, number][];
}

function plainHost(count: number, from = 1): PlainHost {
    return {
        values: Array.from({ length: count }, (_, index) => ({
            id: `m${index + from}`,
            text: `message ${index + from}`,
        })),
        published: [],
        windows: [],
    };
}

function source(host: PlainHost): TransformPassSource {
    return {
        serializerProfile: "pi",
        invocationProfile: "pi-heuristic",
        host: {
            get length() {
                return host.values.length;
            },
            idAt: (index) => host.values[index]?.id,
        },
        preflight: async () => "/project",
        readWindow: (start, end) => {
            host.windows.push([start, end]);
            return host.values.slice(start, end);
        },
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
): Promise<TransformPassOutcome> {
    const admitted = admission.admit("ses");
    if (!("lease" in admitted)) throw new Error("admission declined");
    return client.run("ses", admitted.lease, source(host));
}

const ids = (values: unknown) => (values as { id: string }[]).map((value) => value.id);

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

        expect(await pass(client, admission, host)).toEqual({
            kind: "applied",
            boundary: { mid: "m7", sequence: 3 },
        });
        expect(host.windows).toEqual([[6, 10]]);
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
        expect(host.windows).toEqual([
            [6, 10],
            [6, 11],
        ]);
        const second = transport.calls.at(-1)?.body ?? {};
        expect(second.boundary).toEqual({ mid: "m7", sequence: 3 });
        expect(ids(second.native_messages)).toEqual(["m7", "m8", "m9", "m10", "m11"]);
        expect(second.previous_output_revision).toBe(`out-${outputCounter - 1}`);
        expect(host.published).toHaveLength(2);
        expect(client.state("ses")).toMatchObject({
            initialized: true,
            consecutiveFailures: 0,
            passCount: 2,
        });
        expect(admission.activePasses).toBe(0);
        expect(admission.chargedBytes).toBe(0);
    });

    it("walks to a later anchor page through the id filter", async () => {
        const host = plainHost(8);
        const transport = fakeTransport({
            "transform.boundary": [
                () => ({
                    anchors: [
                        { mid: "gone-1", sequence: 9 },
                        { mid: "gone-2", sequence: 8 },
                    ],
                }),
                (body) => {
                    expect(body.before_sequence).toBe(8);
                    return { anchors: [{ mid: "m5", sequence: 4 }] };
                },
            ],
            transform: [foldReply({ mid: "m5", sequence: 4 })],
        });
        const client = createTransformSessionClient({ moduleClient: transport.client });
        await pass(client, new TransformCaptureAdmission(), host);
        expect(host.windows).toEqual([[4, 8]]);
        expect(transport.calls.at(-1)?.body.boundary).toEqual({ mid: "m5", sequence: 4 });
    });

    it("keeps the boundary and fails open when the daemon answers a busy session", async () => {
        const host = plainHost(6);
        const transport = fakeTransport({
            "transform.boundary": [() => ({ anchors: [{ mid: "m3", sequence: 1 }] })],
            transform: [
                foldReply({ mid: "m3", sequence: 1 }),
                () => ({ status: "session_busy", action: "SESSION_BUSY" }),
            ],
        });
        const client = createTransformSessionClient({ moduleClient: transport.client });
        const admission = new TransformCaptureAdmission();
        await pass(client, admission, host);
        const applied = host.published[0] ?? [];
        host.values.push({ id: "m7", text: "message 7" });
        expect(await pass(client, admission, host)).toEqual({
            kind: "declined",
            servedLastApplied: true,
        });
        expect(host.published[1]).toEqual([...applied, host.values[6]]);
        expect(client.state("ses")).toMatchObject({
            boundary: { mid: "m3", sequence: 1 },
            failureCount: 0,
            consecutiveFailures: 0,
        });
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
        expect(await pass(client, admission, host)).toEqual({
            kind: "declined",
            servedLastApplied: true,
        });
        expect(host.published).toHaveLength(2);
        expect(host.published[1]).toEqual([...(applied ?? []), host.values[6]]);
        expect(client.state("ses")).toMatchObject({ consecutiveFailures: 1, failureCount: 1 });
    });

    async function passEditedDuringPrepare(
        host: PlainHost,
        privateWindow: boolean,
        edit: () => void,
    ): Promise<TransformPassOutcome> {
        const transport = fakeTransport({
            "transform.boundary": [() => ({ anchors: [{ mid: "m2", sequence: 1 }] })],
            transform: [foldReply({ mid: "m2", sequence: 1 })],
        });
        const client = createTransformSessionClient({ moduleClient: transport.client });
        const admitted = new TransformCaptureAdmission().admit("ses");
        if (!("lease" in admitted)) throw new Error("admission declined");
        return client.run("ses", admitted.lease, {
            ...source(host),
            privateWindow,
            prepare: async () => {
                edit();
                return { encodeInput: () => [], fields: { model_key: "test/model" } };
            },
        });
    }

    it("rechecks a shared window's contents and a private window's slots", async () => {
        for (const privateWindow of [false, true]) {
            const host = plainHost(4);
            const outcome = await passEditedDuringPrepare(host, privateWindow, () => {
                (host.values[3] as { text: string }).text = "edited in place";
            });
            expect(outcome.kind).toBe(privateWindow ? "applied" : "declined");
            expect(host.published).toHaveLength(privateWindow ? 1 : 0);
        }
    });

    it("declines a private window whose slot now holds another value", async () => {
        const host = plainHost(4);
        const outcome = await passEditedDuringPrepare(host, true, () => {
            host.values[3] = { id: "m4", text: "message 4" };
        });
        expect(outcome).toEqual({ kind: "declined", servedLastApplied: false });
        expect(host.published).toEqual([]);
    });

    it("skips the id scan for anchors a host reports it does not hold", async () => {
        const host = plainHost(1_000);
        const newer = { mid: "m901", sequence: 3 };
        const older = { mid: "m601", sequence: 2 };
        const transport = fakeTransport({
            "transform.boundary": [
                () => ({ anchors: [newer, older] }),
                () => ({ anchors: [newer, older] }),
            ],
            transform: [foldReply(newer), foldReply(older)],
        });
        const client = createTransformSessionClient({ moduleClient: transport.client });
        const admission = new TransformCaptureAdmission();
        let idReads = 0;
        const counted = (): TransformPassSource => {
            const plain = source(host);
            return {
                ...plain,
                host: {
                    get length() {
                        return host.values.length;
                    },
                    idAt: (index) => {
                        idReads += 1;
                        return host.values[index]?.id;
                    },
                    holds: (id) => host.values.some((value) => value.id === id),
                },
            };
        };
        const run = () => {
            const admitted = admission.admit("ses");
            if (!("lease" in admitted)) throw new Error("admission declined");
            return client.run("ses", admitted.lease, counted());
        };
        expect(await run()).toEqual({ kind: "applied", boundary: newer });
        host.values.length = 900;
        idReads = 0;
        expect(await run()).toEqual({ kind: "applied", boundary: older });
        expect(idReads).toBe(300);
        expect(host.windows.at(-1)).toEqual([600, 900]);
    });
});

describe("transform session client cold import", () => {
    /** A source whose every slot is one CK block; `lead` slots head every cold window. */
    function sizedSource(host: PlainHost, lead = 0): TransformPassSource {
        return { ...source(host), sizeOf: () => ({ blocks: 1, bytes: 10 }), coldLead: lead };
    }

    function coldReply(boundary: { mid: string; sequence: number } | null): Reply {
        return (body) => ({
            status: "ok",
            action: "SOFT",
            boundary,
            base_revision: body.base_revision,
            output_revision: `cold-${outputCounter++}`,
            operations: [
                {
                    op: "keep",
                    source: "input",
                    start: 0,
                    count: (body.native_messages as unknown[]).length,
                },
            ],
        });
    }

    async function coldPass(
        client: ReturnType<typeof createTransformSessionClient>,
        admission: TransformCaptureAdmission,
        source: TransformPassSource,
    ): Promise<TransformPassOutcome> {
        const admitted = admission.admit("ses");
        if (!("lease" in admitted)) throw new Error("admission declined");
        return client.run("ses", admitted.lease, source);
    }

    it("sends the newest half-cap suffix with no anchor and keeps its head pinned until one exists", async () => {
        const host = plainHost(HALF_CAP_BLOCKS * 3);
        const transport = fakeTransport({
            "transform.boundary": [() => ({ anchors: [] })],
            transform: [coldReply(null), coldReply(null), coldReply({ mid: "m1000", sequence: 1 })],
        });
        const client = createTransformSessionClient({ moduleClient: transport.client });
        const admission = new TransformCaptureAdmission();
        const head = HALF_CAP_BLOCKS * 3 - HALF_CAP_BLOCKS + 1;

        await coldPass(client, admission, sizedSource(host));
        const sends = () => transport.calls.filter((call) => call.method === "transform");
        const first = sends()[0]?.body ?? {};
        expect(first.boundary).toBeNull();
        expect(ids(first.native_messages)).toHaveLength(HALF_CAP_BLOCKS);
        expect(ids(first.native_messages)[0]).toBe(`m${head}`);
        expect(client.state("ses").pinnedHead).toBe(`m${head}`);

        host.values.push({ id: "m1201", text: "new" }, { id: "m1202", text: "newer" });
        await coldPass(client, admission, sizedSource(host));
        const second = sends()[1]?.body ?? {};
        expect(ids(second.native_messages)[0]).toBe(`m${head}`);
        expect(ids(second.native_messages)).toHaveLength(HALF_CAP_BLOCKS + 2);

        await coldPass(client, admission, sizedSource(host));
        expect(client.state("ses")).toMatchObject({
            boundary: { mid: "m1000", sequence: 1 },
            pinnedHead: undefined,
        });
    });

    it("sends the cold lead slots ahead of the suffix", async () => {
        const host = plainHost(HALF_CAP_BLOCKS * 2);
        const transport = fakeTransport({
            "transform.boundary": [() => ({ anchors: [] })],
            transform: [coldReply(null)],
        });
        const client = createTransformSessionClient({ moduleClient: transport.client });
        const outcome = await coldPass(
            client,
            new TransformCaptureAdmission(),
            sizedSource(host, 1),
        );
        expect(outcome.kind).toBe("applied");
        const sent = transport.calls.find((call) => call.method === "transform")?.body ?? {};
        const sentIds = ids(sent.native_messages);
        expect(sentIds[0]).toBe("m1");
        expect(sentIds[1]).toBe(`m${HALF_CAP_BLOCKS + 2}`);
        expect(sentIds).toHaveLength(HALF_CAP_BLOCKS);
        expect(host.published[0]?.map((value) => (value as { id: string }).id)).toEqual(sentIds);
    });

    it("stops the suffix before a message that would carry it past half the cap", async () => {
        const host = plainHost(HALF_CAP_BLOCKS);
        const transport = fakeTransport({
            "transform.boundary": [() => ({ anchors: [] })],
            transform: [coldReply(null)],
        });
        const client = createTransformSessionClient({ moduleClient: transport.client });
        // Three blocks a message: 133 messages carry 399 blocks and a 134th would carry 402.
        const threeBlocks = { ...source(host), sizeOf: () => ({ blocks: 3, bytes: 10 }) };
        await coldPass(client, new TransformCaptureAdmission(), threeBlocks);
        const sent = transport.calls.find((call) => call.method === "transform")?.body ?? {};
        const fit = Math.floor(HALF_CAP_BLOCKS / 3);
        expect(ids(sent.native_messages)).toHaveLength(fit);
        expect(ids(sent.native_messages)[0]).toBe(`m${HALF_CAP_BLOCKS - fit + 1}`);
    });

    it("bounds the suffix by bytes as well as blocks", async () => {
        const host = plainHost(10);
        const transport = fakeTransport({
            "transform.boundary": [() => ({ anchors: [] })],
            transform: [coldReply(null)],
        });
        const client = createTransformSessionClient({ moduleClient: transport.client });
        const heavy = {
            ...source(host),
            sizeOf: () => ({ blocks: 1, bytes: Math.floor(HALF_CAP_BYTES / 4) + 1 }),
        };
        await coldPass(client, new TransformCaptureAdmission(), heavy);
        const sent = transport.calls.find((call) => call.method === "transform")?.body ?? {};
        expect(ids(sent.native_messages)).toEqual(["m8", "m9", "m10"]);
    });

    it("sends a whole array at exactly half the cap", async () => {
        const host = plainHost(HALF_CAP_BLOCKS);
        const transport = fakeTransport({
            "transform.boundary": [() => ({ anchors: [] })],
            transform: [coldReply(null)],
        });
        const client = createTransformSessionClient({ moduleClient: transport.client });
        await coldPass(client, new TransformCaptureAdmission(), sizedSource(host));
        const sent = transport.calls.find((call) => call.method === "transform")?.body ?? {};
        expect(ids(sent.native_messages)).toHaveLength(HALF_CAP_BLOCKS);
        expect(client.state("ses").pinnedHead).toBeUndefined();
    });

    it("leaves a host with an unmeasured slot whole, for its capture to refuse", async () => {
        const host = plainHost(HALF_CAP_BLOCKS * 3);
        const transport = fakeTransport({
            "transform.boundary": [() => ({ anchors: [] })],
            transform: [coldReply(null)],
        });
        const client = createTransformSessionClient({ moduleClient: transport.client });
        const unmeasured = HALF_CAP_BLOCKS * 3 - 10;
        const gap = {
            ...source(host),
            sizeOf: (index: number) =>
                index === unmeasured ? undefined : { blocks: 1, bytes: 10 },
        };
        await coldPass(client, new TransformCaptureAdmission(), gap);
        const sent = transport.calls.find((call) => call.method === "transform")?.body ?? {};
        expect(ids(sent.native_messages)).toHaveLength(HALF_CAP_BLOCKS * 3);
        expect(client.state("ses").pinnedHead).toBeUndefined();
    });

    it("sends a newest slot past half the cap on its own", async () => {
        const host = plainHost(5);
        const transport = fakeTransport({
            "transform.boundary": [() => ({ anchors: [] })],
            transform: [coldReply(null)],
        });
        const client = createTransformSessionClient({ moduleClient: transport.client });
        const oversized = {
            ...source(host),
            sizeOf: () => ({ blocks: HALF_CAP_BLOCKS + 1, bytes: 1 }),
        };
        await coldPass(client, new TransformCaptureAdmission(), oversized);
        const sent = transport.calls.find((call) => call.method === "transform")?.body ?? {};
        expect(ids(sent.native_messages)).toEqual(["m5"]);
        expect(client.state("ses").pinnedHead).toBe("m5");
    });

    it("measures a new head once the pinned head leaves the host", async () => {
        const host = plainHost(HALF_CAP_BLOCKS * 2);
        const transport = fakeTransport({
            "transform.boundary": [() => ({ anchors: [] })],
            transform: [coldReply(null), coldReply(null)],
        });
        const client = createTransformSessionClient({ moduleClient: transport.client });
        const admission = new TransformCaptureAdmission();
        await coldPass(client, admission, sizedSource(host));
        expect(client.state("ses").pinnedHead).toBe(`m${HALF_CAP_BLOCKS + 1}`);
        // A navigation leaves a host that no longer holds the pinned message.
        host.values.splice(HALF_CAP_BLOCKS - 10);
        host.values.push(
            ...Array.from({ length: HALF_CAP_BLOCKS }, (_, n) => ({ id: `b${n}`, text: "b" })),
        );
        await coldPass(client, admission, sizedSource(host));
        const second = transport.calls.filter((call) => call.method === "transform")[1]?.body ?? {};
        expect(ids(second.native_messages)).toHaveLength(HALF_CAP_BLOCKS);
        expect(ids(second.native_messages)[0]).toBe("b0");
        expect(client.state("ses").pinnedHead).toBe("b0");
        expect(host.published[1]?.map((value) => (value as { id: string }).id)).toEqual(
            ids(second.native_messages),
        );
    });

    it("sends the whole array below half the cap", async () => {
        const host = plainHost(HALF_CAP_BLOCKS - 1);
        const transport = fakeTransport({
            "transform.boundary": [() => ({ anchors: [] })],
            transform: [coldReply(null)],
        });
        const client = createTransformSessionClient({ moduleClient: transport.client });
        await coldPass(client, new TransformCaptureAdmission(), sizedSource(host));
        const sent = transport.calls.find((call) => call.method === "transform")?.body ?? {};
        expect(ids(sent.native_messages)).toHaveLength(HALF_CAP_BLOCKS - 1);
        expect(client.state("ses").pinnedHead).toBeUndefined();
    });
});

/** What one module's source loads at runtime: static specifiers (value imports, re-exports, side-effect imports) and the argument text of each dynamic import. */
function runtimeSpecifiers(path: string): { static: string[]; dynamic: string[] } {
    const text = readFileSync(path, "utf8");
    const found = { static: [] as string[], dynamic: [] as string[] };
    for (const match of text.matchAll(/^(import|export)\s+(type\s+)?[^;]*?from\s+"([^"]+)";/gms))
        if (!match[2] && match[3]) found.static.push(match[3]);
    for (const match of text.matchAll(/^import\s+"([^"]+)";/gm))
        if (match[1]) found.static.push(match[1]);
    for (const match of text.matchAll(/\bimport\(\s*([^)]*?)\s*\)/g))
        found.dynamic.push(match[1] ?? "");
    return found;
}

/** Every module the client reaches at runtime, relative to `src/`; a new edge fails until reviewed. */
const CLIENT_MODULES = [
    "hooks/context/edit-recipe.ts",
    "hooks/context/invocation-budget.ts",
    "hooks/context/module-transport.ts",
    "hooks/context/module-wire.ts",
    "hooks/context/transform-capture.ts",
    "hooks/context/transform-session-client.ts",
    "hooks/context/transform-stage-logger.ts",
    "hooks/context/window-cap.ts",
    "shared/atomic-file.ts",
    "shared/data-path.ts",
    "shared/harness.ts",
    "shared/host-client/bytes.ts",
    "shared/host-client/client.ts",
    "shared/host-client/connection-file.ts",
    "shared/host-client/connection.ts",
    "shared/host-client/credential-fingerprint.ts",
    "shared/host-client/deadline.ts",
    "shared/host-client/errors.ts",
    "shared/host-client/exact-json.ts",
    "shared/host-client/frame-channel.ts",
    "shared/host-client/index.ts",
    "shared/host-client/owner.ts",
    "shared/host-client/protocol.ts",
    "shared/host-client/route-handle.ts",
    "shared/host-client/serialized-json-body.ts",
    "shared/host-client/shm-frame-channel.ts",
    "shared/host-client/types.ts",
    "shared/host-lifecycle/bootstrap.ts",
    "shared/host-lifecycle/compatibility.ts",
    "shared/host-lifecycle/contract.ts",
    "shared/host-lifecycle/index.ts",
    "shared/host-lifecycle/managed-policy.ts",
    "shared/host-lifecycle/native-launcher.ts",
    "shared/host-lifecycle/owner.ts",
    "shared/host-lifecycle/ownership.ts",
    "shared/host-lifecycle/paths.ts",
    "shared/host-lifecycle/policy.ts",
    "shared/host-release-layout.ts",
    "shared/logger.ts",
    "shared/record-type-guard.ts",
    "shared/stable-json.ts",
    "shared/write-all.ts",
];

/** The packages the client reaches; none reads a database. */
const CLIENT_PACKAGES = [
    "@eidnara/shm-native",
    "node:buffer",
    "node:child_process",
    "node:crypto",
    "node:fs",
    "node:fs/promises",
    "node:os",
    "node:path",
    "node:url",
    "node:util",
];

describe("transform session client import rule", () => {
    it("reaches only reviewed modules, none of them a database reader or host-array writer", () => {
        const src = resolve(import.meta.dir, "../..");
        const seen = new Set<string>();
        const pending = [resolve(import.meta.dir, "transform-session-client.ts")];
        const external = new Set<string>();
        const dynamic: string[] = [];
        while (pending.length > 0) {
            const path = pending.pop() as string;
            if (seen.has(path)) continue;
            seen.add(path);
            const specifiers = runtimeSpecifiers(path);
            for (const argument of specifiers.dynamic)
                dynamic.push(`${relative(src, path)}: import(${argument})`);
            for (const specifier of specifiers.static) {
                if (!specifier.startsWith(".")) {
                    external.add(specifier);
                    continue;
                }
                const resolved = resolve(dirname(path), specifier);
                if (/\.(json|js|mjs)$/.test(resolved)) continue;
                const file = resolved.endsWith(".ts") ? resolved : `${resolved}.ts`;
                pending.push(existsSync(file) ? file : resolve(resolved, "index.ts"));
            }
        }
        expect([...seen].map((path) => relative(src, path)).sort()).toEqual(CLIENT_MODULES);
        expect([...external].sort()).toEqual(CLIENT_PACKAGES);
        expect(dynamic).toEqual([]);
    });
});
