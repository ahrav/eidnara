import { afterEach, beforeEach, expect, test } from "bun:test";
import { isHostCallError } from "../../shared/host-client";
import { executeEidnaraSearch } from "../../tools/eidnara-search/execute";
import { ConnectionTurnoverHarness, MEMORY_SUMMARY } from "./__tests__/connection-turnover-harness";
import { createKernelClient, kernelClientResolver } from "./kernel-transport";
import { HostModuleTransport } from "./module-transport";

const SESSION = "session-shared-connection";

let harness: ConnectionTurnoverHarness;
let transform: HostModuleTransport;

beforeEach(async () => {
    harness = await ConnectionTurnoverHarness.start();
    transform = new HostModuleTransport(harness.connectionFile);
});

afterEach(async () => {
    transform.disconnect();
    await harness.stop();
});

/** Opens the transform transport's route with one answered call, so the process connection exists before the kernel client dials. */
async function warmTransform(): Promise<void> {
    const warm = transform.call({
        method: "session.status",
        sessionId: SESSION,
        projectRoot: harness.root,
        body: { method: "session.status" },
    });
    void warm.catch(() => {});
    await harness.acceptRoute();
    harness.daemon.respond((await harness.nextRequest()).header, { ok: true });
    await warm;
}

/** A transform the daemon leaves pending; resolves to the error its expired deadline raises. */
function expiringTransform(): Promise<unknown> {
    return transform
        .call({
            method: "transform",
            sessionId: SESSION,
            projectRoot: harness.root,
            body: { method: "transform", session_id: SESSION },
            timeoutMs: 80,
        })
        .then(
            () => new Error("the transform was answered"),
            (error: unknown) => error,
        );
}

function search() {
    return executeEidnaraSearch(
        {
            kernelClient: kernelClientResolver({
                host: { connection_file: harness.connectionFile },
            }),
            resolveProjectPath: () => "git:shared-connection",
        },
        { query: "release branch" },
        { sessionID: SESSION, directory: harness.root },
    );
}

test("a search whose ranking is in flight when a transform deadline expires returns its results", async () => {
    await warmTransform();
    const expired = expiringTransform();
    expect((await harness.nextRequest()).json.method).toBe("transform");

    const searching = search();
    await harness.acceptRoute();
    expect((await harness.nextRequest()).json.method).toBe("retrieval.query");
    expect(harness.connections).toBe(1);

    // Both requests stay pending until the transform's deadline expires and its transport closes the connection.
    const failure = await expired;
    expect(isHostCallError(failure) && failure.kind).toBe("outcome_unknown");
    expect(harness.clients[0]?.isClosed).toBe(true);

    const { result, answered } = await harness.serve(searching);
    expect(answered).toEqual(["route.open", "retrieval.query", "kernel.read"]);
    expect(harness.connections).toBe(2);
    expect(result.status).toBe("complete");
    expect(result.text).toContain(MEMORY_SUMMARY);
    expect(result.text).not.toContain("Error");
});

test("the first search after a transform deadline closed the shared connection returns its results", async () => {
    await warmTransform();
    const warm = await harness.serve(search());
    expect(warm.result.status).toBe("complete");
    expect(harness.connections).toBe(1);

    const expired = expiringTransform();
    expect((await harness.nextRequest()).json.method).toBe("transform");
    const failure = await expired;
    expect(isHostCallError(failure) && failure.kind).toBe("outcome_unknown");
    expect(harness.clients[0]?.isClosed).toBe(true);

    const { result, answered } = await harness.serve(search());
    expect(answered).toEqual(["route.open", "retrieval.query", "kernel.read"]);
    expect(harness.connections).toBe(2);
    expect(result.status).toBe("complete");
    expect(result.text).toContain(MEMORY_SUMMARY);
    expect(result.text).not.toContain("Error");
});

test("a kernel call that begins after another transport closed the shared connection is answered on the next one", async () => {
    await warmTransform();
    const kernel = createKernelClient({
        sessionId: SESSION,
        projectRoot: harness.root,
        config: { host: { connection_file: harness.connectionFile } },
    });
    const ranked = () => kernel.query({ query: "release branch", destination: "local" });
    expect((await harness.serve(ranked())).result.state).toEqual({ kind: "available" });
    const before = kernel.connectionIdentity();

    const expired = expiringTransform();
    expect((await harness.nextRequest()).json.method).toBe("transform");
    await expired;
    expect(harness.clients[0]?.isClosed).toBe(true);
    expect(kernel.connectionIdentity()).not.toBe(before);

    const { result, answered } = await harness.serve(ranked());
    expect(answered).toEqual(["route.open", "retrieval.query"]);
    expect(result.state).toEqual({ kind: "available" });
    expect(harness.connections).toBe(2);
});
