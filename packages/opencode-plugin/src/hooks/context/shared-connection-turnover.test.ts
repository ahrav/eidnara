import { afterEach, beforeEach, expect, test } from "bun:test";
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

const transformCall: HostModuleTransport["call"] = (args) => transform.call(args);

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
    await harness.warm(transformCall, SESSION);
    const searching = search();
    await harness.acceptRoute();
    expect((await harness.nextRequest()).json.method).toBe("retrieval.query");
    expect(harness.connections).toBe(1);

    // The ranking and the transform both stay pending until the transform's deadline expires and its transport closes the connection.
    const expired = await harness.expiringTransform(transformCall, SESSION);
    expect(await expired).toMatchObject({ kind: "outcome_unknown" });
    expect(harness.clients[0]?.isClosed).toBe(true);

    const { result, answered } = await harness.serve(searching);
    expect(answered).toEqual(["route.open", "retrieval.query", "kernel.read"]);
    expect(harness.connections).toBe(2);
    expect(result.status).toBe("complete");
    expect(result.text).toContain(MEMORY_SUMMARY);
    expect(result.text).not.toContain("Error");
});

test("the first search after a transform deadline closed the shared connection returns its results", async () => {
    await harness.warm(transformCall, SESSION);
    const warm = await harness.serve(search());
    expect(warm.result.status).toBe("complete");
    expect(harness.connections).toBe(1);

    const expired = await harness.expiringTransform(transformCall, SESSION);
    expect(await expired).toMatchObject({ kind: "outcome_unknown" });
    expect(harness.clients[0]?.isClosed).toBe(true);

    const { result, answered } = await harness.serve(search());
    expect(answered).toEqual(["route.open", "retrieval.query", "kernel.read"]);
    expect(harness.connections).toBe(2);
    expect(result.status).toBe("complete");
    expect(result.text).toContain(MEMORY_SUMMARY);
    expect(result.text).not.toContain("Error");
});

test("a kernel call that begins after another transport closed the shared connection is answered on the next one", async () => {
    await harness.warm(transformCall, SESSION);
    const kernel = createKernelClient({
        sessionId: SESSION,
        projectRoot: harness.root,
        config: { host: { connection_file: harness.connectionFile } },
    });
    const ranked = () => kernel.query({ query: "release branch", destination: "local" });
    expect((await harness.serve(ranked())).result.state).toEqual({ kind: "available" });
    const before = kernel.connectionIdentity();

    const expired = await harness.expiringTransform(transformCall, SESSION);
    await expired;
    expect(harness.clients[0]?.isClosed).toBe(true);
    expect(kernel.connectionIdentity()).not.toBe(before);

    const { result, answered } = await harness.serve(ranked());
    expect(answered).toEqual(["route.open", "retrieval.query"]);
    expect(result.state).toEqual({ kind: "available" });
    expect(harness.connections).toBe(2);
});
