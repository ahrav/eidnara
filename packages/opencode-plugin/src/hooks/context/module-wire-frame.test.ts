import { afterEach, beforeEach, expect, spyOn, test } from "bun:test";
import { mkdtemp, rm } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import {
    FakeDaemon,
    type PeerFrame,
    writeConnectionFile,
} from "../../shared/host-client/__tests__/fake-daemon";
import { HostClient } from "../../shared/host-client/client";
import { serializedJsonText } from "../../shared/host-client/serialized-json-body";
import {
    SERIALIZED_TRANSFORM_SESSION,
    serializedTransformCorpus,
} from "./__tests__/serialized-transform-corpus";
import { HostModuleTransport } from "./module-transport";
import { buildPagedModuleTransformPayloads } from "./module-wire";
import type { RustModeModuleClient } from "./rust-mode-transform";

let root: string;
let daemon: FakeDaemon;
let transport: HostModuleTransport;
let client: HostClient | undefined;
let connectSpy: ReturnType<typeof spyOn<typeof HostClient, "connect">>;

beforeEach(async () => {
    root = await mkdtemp(join(tmpdir(), "serialized-module-transport-"));
    daemon = new FakeDaemon();
    client = undefined;
    const connect = HostClient.connect;
    connectSpy = spyOn(HostClient, "connect").mockImplementation(async (options) => {
        client = await connect({ ...options, channelFactory: daemon.channelFactory });
        return client;
    });
    transport = new HostModuleTransport(await writeConnectionFile(join(root, "connection.json")));
    const warm = transport.call({
        method: "session.status",
        sessionId: SERIALIZED_TRANSFORM_SESSION,
        projectRoot: root,
        body: { method: "session.status" },
    });
    void warm.catch(() => {});
    await daemon.acceptRouteOpen();
    await daemon.answerRouted({ ok: true });
    await warm;
});

afterEach(async () => {
    connectSpy?.mockRestore();
    transport?.disconnect();
    await client?.closeAsync();
    if (root) await rm(root, { recursive: true, force: true });
});

test("pager corpus reaches the native-writer fake with exact header and body bytes", async () => {
    for (const fixture of serializedTransformCorpus()) {
        if (fixture.pagerRefuses) {
            expect(() => buildPagedModuleTransformPayloads(fixture.body)).toThrow(
                "module transform scalar tail exceeds the 512 KiB page limit",
            );
            continue;
        }
        const pages = buildPagedModuleTransformPayloads(fixture.body);
        if (fixture.firstPageBytes !== undefined)
            expect(pages[0]?.bytes).toBe(fixture.firstPageBytes);
        if (fixture.lastPageBytes !== undefined)
            expect(pages.at(-1)?.bytes).toBe(fixture.lastPageBytes);
        if (fixture.pageCount !== undefined) expect(pages).toHaveLength(fixture.pageCount);
        for (const { page, bytes } of pages) {
            const expected = new TextEncoder().encode(serializedJsonText(page));
            const stringify = spyOn(JSON, "stringify");
            const waiting = transport.call({
                method: "transform",
                sessionId: SERIALIZED_TRANSFORM_SESSION,
                projectRoot: root,
                body: page,
            });
            void waiting.catch(() => {});
            let frame: PeerFrame;
            try {
                frame = await daemon.next();
                expect(stringify).not.toHaveBeenCalled();
            } finally {
                stringify.mockRestore();
            }
            const header = frame.headerBytes!;
            expect(new DataView(header.buffer, header.byteOffset).getUint32(0, true)).toBe(bytes);
            expect(frame.body.byteLength).toBe(bytes);
            expect(frame.body).toEqual(expected);
            expect(bytes).toBeLessThanOrEqual(524_288);
            daemon.respond(frame.header, { ok: true });
            await waiting;
        }
    }
});

test("module transport sends the pager snapshot without another body stringify", async () => {
    const moduleClient: RustModeModuleClient = transport;
    let reads = 0;
    const body = {
        method: "transform",
        get messages() {
            reads += 1;
            return [{ text: reads === 1 ? "é😀\ud800" : "changed on second read" }];
        },
    };
    const stringify = spyOn(JSON, "stringify");
    try {
        const [{ page, bytes }] = buildPagedModuleTransformPayloads(body);
        body.method = "mutated";
        (page.messages as Array<{ text: string }>)[0]!.text = "inspection edit";
        expect(Reflect.set(page, "method", "wrong")).toBe(false);
        const callsBeforeSend = stringify.mock.calls.length;
        const waiting = moduleClient.call({
            method: "transform",
            sessionId: SERIALIZED_TRANSFORM_SESSION,
            projectRoot: root,
            body: page,
        });
        void waiting.catch(() => {});
        const frame = await daemon.next();
        expect(stringify.mock.calls.length).toBe(callsBeforeSend);
        expect(reads).toBe(1);
        const header = frame.headerBytes!;
        expect(new DataView(header.buffer, header.byteOffset).getUint32(0, true)).toBe(bytes);
        expect(frame.body).toEqual(
            new TextEncoder().encode('{"method":"transform","messages":[{"text":"é😀\\ud800"}]}'),
        );
        expect(frame.body.byteLength).toBe(bytes);
        daemon.respond(frame.header, { ok: true });
        await waiting;
    } finally {
        stringify.mockRestore();
    }
});
