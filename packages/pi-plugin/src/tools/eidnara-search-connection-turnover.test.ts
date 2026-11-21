import { afterEach, beforeEach, expect, it } from "bun:test";
import {
    ConnectionTurnoverHarness,
    MEMORY_SUMMARY,
    type ModuleCall,
} from "@eidnara/opencode/hooks/context/__tests__/connection-turnover-harness";
import {
    createHostModuleClient,
    type HostModuleClient,
} from "@eidnara/opencode/hooks/context/module-transport";
import { fakeContext } from "../__tests__/test-utils";
import { createPiKernelClientResolver, resetPiKernelClientsForTest } from "../kernel-client-pi";
import { createEidnaraSearchTool } from "./eidnara-search";

const SESSION = "ses-shared-connection";

let harness: ConnectionTurnoverHarness;
let moduleClient: HostModuleClient;

beforeEach(async () => {
    harness = await ConnectionTurnoverHarness.start();
    moduleClient = createHostModuleClient(harness.connectionFile);
});

afterEach(async () => {
    moduleClient.disconnect();
    resetPiKernelClientsForTest();
    await harness.stop();
});

function search() {
    const tool = createEidnaraSearchTool({
        kernelClient: createPiKernelClientResolver(() => ({
            host: { connection_file: harness.connectionFile },
        })),
        resolveProjectPath: () => "git:shared-connection",
    });
    return tool.execute(
        "call-search",
        { query: "release branch" } as never,
        new AbortController().signal,
        undefined,
        fakeContext(SESSION, harness.root) as never,
    );
}

it("eidnara_search returns its results when a transform deadline closes the shared connection under it", async () => {
    const call: ModuleCall = (args) => moduleClient.call(args);
    await harness.warm(call, SESSION);
    const searching = search();
    await harness.acceptRoute();
    expect((await harness.nextRequest()).json.method).toBe("retrieval.query");
    expect(harness.connections).toBe(1);

    // The ranking and the transform both stay pending until the transform's deadline expires and its transport closes the connection.
    const expired = await harness.expiringTransform(call, SESSION);
    expect(expired).toMatchObject({ kind: "outcome_unknown" });
    expect(harness.clients[0]?.isClosed).toBe(true);

    const { result, answered } = await harness.serve(searching);
    expect(answered).toEqual(["route.open", "retrieval.query", "kernel.read"]);
    expect(harness.connections).toBe(2);
    expect(result.isError).toBeUndefined();
    const text = result.content[0]?.type === "text" ? result.content[0].text : "";
    expect(text).toContain(MEMORY_SUMMARY);
    expect(text).not.toContain("Error");
});
