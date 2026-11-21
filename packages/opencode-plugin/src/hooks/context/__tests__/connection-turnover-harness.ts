import { mkdtemp, rm } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { setTimeout as delay } from "node:timers/promises";
import { HostClient } from "../../../shared/host-client";
import { FakeDaemon, writeConnectionFile } from "../../../shared/host-client/__tests__/fake-daemon";
import { type EnvelopeHeader, FrameType } from "../../../shared/host-client/protocol";
import { FakeKernel } from "../../../shared/kernel-client-testing/fake-kernel";
import { resetKernelClientsForTest } from "../kernel-transport";
import type { HostModuleTransport } from "../module-transport";

/** The `call` of a module transport, or of the module client a plugin builds over one. */
export type ModuleCall = HostModuleTransport["call"];

export interface RoutedRequest {
    header: EnvelopeHeader;
    json: Record<string, unknown>;
}

/** The fused ranking uses `MEMORY_ID` to reference the seeded decision. */
const MEMORY_ID = `mem_${"1".padStart(32, "0")}`;
export const MEMORY_SUMMARY = "Release branches are cut from main.";

/**
 * A fake daemon behind the plugin's real client and transports: every
 * transport that dials `connectionFile` reaches one `FakeDaemon`, and a
 * `FakeKernel` holding one memory answers the kernel routes. A test drives
 * the daemon side frame by frame, so it decides which requests stay pending.
 */
export class ConnectionTurnoverHarness {
    readonly daemon = new FakeDaemon();
    readonly kernel = new FakeKernel();
    /** Every client the plugin dialed, oldest first. */
    readonly clients: HostClient[] = [];
    private nextChannel = 7;

    private constructor(
        readonly root: string,
        readonly connectionFile: string,
        private readonly connect: typeof HostClient.connect,
    ) {}

    /** Routes every `HostClient.connect` to the fake daemon until `stop` restores the real dial. */
    static async start(): Promise<ConnectionTurnoverHarness> {
        const root = await mkdtemp(join(tmpdir(), "connection-turnover-"));
        const connectionFile = await writeConnectionFile(join(root, "connection.json"));
        const connect = HostClient.connect;
        const harness = new ConnectionTurnoverHarness(root, connectionFile, connect);
        HostClient.connect = async (options) => {
            const client = await connect.call(HostClient, {
                ...options,
                channelFactory: harness.daemon.channelFactory,
            });
            harness.clients.push(client);
            return client;
        };
        harness.kernel.seedDecision({
            object_id: MEMORY_ID,
            decision_kind: "PROJECT_RULES",
            summary: MEMORY_SUMMARY,
        });
        harness.kernel.routeReply = () => ({
            kind: "fused",
            degraded: false,
            truncated: false,
            lanes: {
                exact: { status: "complete" },
                lexical: { status: "complete" },
                dense: { status: "complete" },
            },
            entries: [
                {
                    occurrence_id: "occ-0",
                    position: 1,
                    score: 1,
                    lanes: { dense: { position: 1, raw: 0.5 } },
                    canonical: {
                        decision_object_id: MEMORY_ID,
                        source_revision: 1,
                        visibility: "visible",
                        causality: "unknown",
                    },
                },
            ],
        });
        return harness;
    }

    async stop(): Promise<void> {
        HostClient.connect = this.connect;
        resetKernelClientsForTest();
        for (const client of this.clients) await client.closeAsync().catch(() => {});
        await rm(this.root, { recursive: true, force: true });
    }

    /** How many connections the plugin has dialed. */
    get connections(): number {
        return this.daemon.links.length;
    }

    /** Reads frames of the newest connection until one is a `Request`. */
    async nextRequest(): Promise<RoutedRequest> {
        const startedAt = Date.now();
        for (;;) {
            const request = this.takeRequest();
            if (request) return request;
            if (Date.now() - startedAt > 3_000) throw new Error("no request frame arrived");
            await delay(1);
        }
    }

    private takeRequest(): RoutedRequest | null {
        for (;;) {
            const frame = this.daemon.drain();
            if (!frame) return null;
            if (frame.header.ty !== FrameType.Request) continue;
            return {
                header: frame.header,
                json: JSON.parse(Buffer.from(frame.body).toString("utf8")) as Record<
                    string,
                    unknown
                >,
            };
        }
    }

    /** Answers a `route.open` with a channel of its own, as the host allocates one per route. */
    openRoute(open: RoutedRequest): number {
        if (open.json.op !== "route.open") {
            throw new Error(`expected route.open, got ${JSON.stringify(open.json)}`);
        }
        const channel = this.nextChannel++;
        this.daemon.respond(open.header, {
            op: "route.open",
            route_channel: channel,
            route_epoch: 1,
        });
        return channel;
    }

    async acceptRoute(): Promise<number> {
        return this.openRoute(await this.nextRequest());
    }

    /** Opens the caller's route with one answered `session.status`, so the process connection exists before a kernel client dials. */
    async warm(call: ModuleCall, sessionId: string): Promise<void> {
        const warm = call({
            method: "session.status",
            sessionId,
            projectRoot: this.root,
            body: { method: "session.status" },
        });
        void warm.catch(() => {});
        await this.acceptRoute();
        this.daemon.respond((await this.nextRequest()).header, { ok: true });
        await warm;
    }

    /** Sends a transform the daemon leaves pending, and resolves once its deadline has expired, to the error that expiry raises. */
    async expiringTransform(call: ModuleCall, sessionId: string): Promise<unknown> {
        const expired = call({
            method: "transform",
            sessionId,
            projectRoot: this.root,
            body: { method: "transform", session_id: sessionId },
            timeoutMs: 500,
        }).then(
            () => new Error("the transform was answered"),
            (error: unknown) => error,
        );
        const pass = await this.nextRequest();
        if (pass.json.method !== "transform") {
            throw new Error(`expected the transform, got ${JSON.stringify(pass.json)}`);
        }
        return expired;
    }

    /** Answers one request as the daemon does: a route for `route.open`, the fake kernel's reply for a kernel method. */
    answer(request: RoutedRequest): string {
        if (request.header.channel === 0) {
            this.openRoute(request);
            return "route.open";
        }
        const method = String(request.json.method);
        this.daemon.respond(
            request.header,
            this.kernel.reply({
                sessionId: String(request.json.session_id),
                projectRoot: String(request.json.project_root),
                method,
                body: request.json,
            }),
        );
        return method;
    }

    /** Answers every request of the newest connection until `work` settles, and returns what it answered. */
    async serve<T>(work: Promise<T>): Promise<{ result: T; answered: string[] }> {
        const answered: string[] = [];
        let settled = false;
        const outcome = work.finally(() => {
            settled = true;
        });
        outcome.catch(() => {});
        const startedAt = Date.now();
        while (!settled) {
            const request = this.takeRequest();
            if (request) answered.push(this.answer(request));
            else if (Date.now() - startedAt > 5_000) throw new Error("the work did not settle");
            else await delay(1);
        }
        return { result: await outcome, answered };
    }
}
