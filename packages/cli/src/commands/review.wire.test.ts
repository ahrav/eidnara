import { afterEach, describe, expect, test } from "bun:test";
import { mkdtemp, rm } from "node:fs/promises";
import os from "node:os";
import path from "node:path";
import { HostClient } from "@eidnara/opencode/shared/host-client";
import {
    FakeDaemon,
    writeConnectionFile,
} from "@eidnara/opencode/shared/host-client/__tests__/fake-daemon";
import { hostClientOptions, type ReviewCommandDependencies, runReviewCommand } from "./review";

const HEX = "c".repeat(64);
let tmpDir: string | null = null;

afterEach(async () => {
    if (tmpDir) await rm(tmpDir, { recursive: true, force: true });
    tmpDir = null;
});

/** The command over the real transport: `HostClient.connect` against a scripted daemon, with the ambient module identity present in the environment. */
async function transport(): Promise<{
    deps: ReviewCommandDependencies;
    daemon: FakeDaemon;
    stdout: string[];
    stderr: string[];
    clients: HostClient[];
}> {
    tmpDir = await mkdtemp(path.join(os.tmpdir(), "eidnara-review-cli-"));
    const daemon = new FakeDaemon();
    const connectionFile = await writeConnectionFile(path.join(tmpDir, "connection.json"));
    const stdout: string[] = [];
    const stderr: string[] = [];
    const clients: HostClient[] = [];
    const deps: ReviewCommandDependencies = {
        connect: async () => {
            const client = await HostClient.connect({
                connectionFile,
                channelFactory: daemon.channelFactory,
                shutdownDeadlineMs: 500,
                requestTimeoutMs: 2_000,
            });
            clients.push(client);
            return client;
        },
        resolveProjectRoot: (p) => p,
        cwd: () => "/work/project",
        env: { HOME: tmpDir, EIDNARA_MODULE_ID: "context", EIDNARA_LAUNCH_NONCE: "nonce-1" },
        stdout: (line) => stdout.push(line),
        stderr: (line) => stderr.push(line),
    };
    return { deps, daemon, stdout, stderr, clients };
}

describe("review over the host transport", () => {
    test("list probes the catalog, opens one observational route without the ambient identity, sends one request, decodes exact integers, and closes", async () => {
        const saved = {
            module: process.env.EIDNARA_MODULE_ID,
            nonce: process.env.EIDNARA_LAUNCH_NONCE,
        };
        process.env.EIDNARA_MODULE_ID = "context";
        process.env.EIDNARA_LAUNCH_NONCE = "nonce-1";
        try {
            const { deps, daemon, stdout, clients } = await transport();
            const run = runReviewCommand(["list", "--project", "/work/project", "--json"], deps);
            const catalog = await daemon.nextRequest();
            expect(catalog.json.op).toBe("catalog.list");
            daemon.respond(catalog.header, {
                op: "catalog.list",
                generation: 1,
                host_ops: ["route.open", "catalog.list", "host.shutdown", "host.status"],
                modules: [
                    { module_id: "context", module_version: "1", roles: [], control_ops: [] },
                    {
                        module_id: "local_embeddings",
                        module_version: "1",
                        roles: [],
                        control_ops: [],
                    },
                    {
                        module_id: "model_execution",
                        module_version: "1",
                        roles: [],
                        control_ops: [],
                    },
                ],
            });
            const open = await daemon.nextRequest();
            expect(open.json.op).toBe("route.open");
            expect(open.json.target).toEqual({ kind: "tool_provider", module_id: "context" });
            const identity = open.json.identity as {
                project_root: string;
                harness: string;
                session: string;
            };
            expect(identity.project_root).toBe("/work/project");
            expect(identity.harness).toBe("cli");
            expect(identity.session).toMatch(/^eidnara-review:/);
            expect("consumer_identity" in open.json).toBe(false);
            daemon.respond(open.header, { op: "route.open", route_channel: 7, route_epoch: 1 });
            const routed = await daemon.nextRequest();
            expect(routed.header.channel).toBe(7);
            expect(routed.json).toEqual({
                v: 1,
                session_id: identity.session,
                project_root: "/work/project",
                method: "review.list",
                limit: 16,
                after: null,
            });
            daemon.respondText(
                routed.header,
                `{"kind":"page","items":[{"causal_identity":"${HEX}","generation":9007199254740993,"outcome":"complete","selected":true}],"next":null}`,
            );
            expect(await run).toBe(0);
            expect(stdout[0]).toContain('"generation":9007199254740993');
            expect(clients).toHaveLength(1);
            expect(clients[0].isClosed).toBe(true);
            expect(process.env.EIDNARA_MODULE_ID).toBe("context");
        } finally {
            if (saved.module === undefined) delete process.env.EIDNARA_MODULE_ID;
            else process.env.EIDNARA_MODULE_ID = saved.module;
            if (saved.nonce === undefined) delete process.env.EIDNARA_LAUNCH_NONCE;
            else process.env.EIDNARA_LAUNCH_NONCE = saved.nonce;
        }
    });

    test("a route refusal closes the connection and reports only a code, and status decodes through the real transport", async () => {
        const refused = await transport();
        const run = runReviewCommand(["show", HEX], refused.deps);
        const catalog = await refused.daemon.nextRequest();
        refused.daemon.respond(catalog.header, {
            op: "catalog.list",
            generation: 1,
            host_ops: ["route.open", "catalog.list", "host.shutdown", "host.status"],
            modules: [
                { module_id: "context", module_version: "1", roles: [], control_ops: [] },
                { module_id: "local_embeddings", module_version: "1", roles: [], control_ops: [] },
                { module_id: "model_execution", module_version: "1", roles: [], control_ops: [] },
            ],
        });
        const open = await refused.daemon.nextRequest();
        refused.daemon.respond(open.header, { op: "route.open", route_channel: 7, route_epoch: 1 });
        const routed = await refused.daemon.nextRequest();
        refused.daemon.fail(routed.header, {
            code: "session_mismatch",
            message: `peer text ${HEX}`,
        });
        expect(await run).toBe(1);
        expect(refused.stdout).toEqual([]);
        expect(refused.stderr).toEqual(["Review show failed: terminal (session_mismatch)."]);
        expect(refused.clients[0].isClosed).toBe(true);

        const timedOut = await transport();
        const timedOutRun = runReviewCommand(["status"], {
            ...timedOut.deps,
            connect: async () => {
                const client = await HostClient.connect({
                    connectionFile: path.join(tmpDir ?? "", "connection.json"),
                    channelFactory: timedOut.daemon.channelFactory,
                    shutdownDeadlineMs: 500,
                    requestTimeoutMs: 50,
                });
                timedOut.clients.push(client);
                return client;
            },
        });
        // The daemon reads the status request and never answers it.
        await timedOut.daemon.nextRequest();
        expect(await timedOutRun).toBe(1);
        expect(timedOut.stdout).toEqual([]);
        expect(timedOut.stderr).toHaveLength(1);
        expect(timedOut.stderr[0]).toMatch(
            /^Review status failed: (terminal|outcome_unknown|not_sent)/,
        );
        expect(timedOut.clients[0].isClosed).toBe(true);

        const status = await transport();
        const statusRun = runReviewCommand(["status"], status.deps);
        const control = await status.daemon.nextRequest();
        expect(control.json.op).toBe("host.status");
        status.daemon.respondText(
            control.header,
            '{"op":"host.status","health":"ok","metrics":{"components":{"context":{"status":"ok","metrics":{"storage_state":"ready","memory_reviewer":{"memory_reviewer_state":"ready","activation_state":"open","sampled_at_ms":5,"jobs_ready":9007199254740992,"jobs_reserved":9007199254740993}}}}}}',
        );
        expect(await statusRun).toBe(0);
        expect(status.stdout[0]).toContain("MemoryReviewer store: ready");
        expect(status.stdout[0]).toContain("jobs_ready: 9007199254740992");
        expect(status.stdout[0]).toContain("jobs_reserved: unavailable");
        expect(status.clients[0].isClosed).toBe(true);
    });

    test("a route.open the daemon never answers fails within the command's one request bound", async () => {
        const stalled = await transport();
        const run = runReviewCommand(["list", "--project", "/work/project"], {
            ...stalled.deps,
            connect: async () => {
                const client = await HostClient.connect({
                    connectionFile: path.join(tmpDir ?? "", "connection.json"),
                    channelFactory: stalled.daemon.channelFactory,
                    ...hostClientOptions(50),
                });
                stalled.clients.push(client);
                return client;
            },
        });
        const catalog = await stalled.daemon.nextRequest();
        stalled.daemon.respond(catalog.header, {
            op: "catalog.list",
            generation: 1,
            host_ops: ["route.open", "catalog.list", "host.shutdown", "host.status"],
            modules: [
                { module_id: "context", module_version: "1", roles: [], control_ops: [] },
                { module_id: "local_embeddings", module_version: "1", roles: [], control_ops: [] },
                { module_id: "model_execution", module_version: "1", roles: [], control_ops: [] },
            ],
        });
        const open = await stalled.daemon.nextRequest();
        expect(open.json.op).toBe("route.open");
        const started = Date.now();
        expect(await run).toBe(1);
        expect(Date.now() - started).toBeLessThan(2_000);
        expect(stalled.stdout).toEqual([]);
        expect(stalled.stderr[0]).toMatch(/^Review list failed: /);
        expect(stalled.clients[0].isClosed).toBe(true);
    }, 5_000);
});
