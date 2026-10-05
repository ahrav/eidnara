import { createHash } from "node:crypto";
import type { BindIdentity } from "@eidnara/opencode/shared/host-client";
import type { HermeticHostStack } from "../rust-runner/hermetic-host";

/** `memory_classifier.run_task` runs only on a root whose memories authority is MODULE. */
async function raiseMemoriesAuthority(
    host: HermeticHostStack,
    identity: BindIdentity,
): Promise<number> {
    const key = {
        v: 1,
        session_id: identity.session,
        context_store_uuid: "bedrock-only-e2e",
        project: `bedrock-only-${identity.harness}`,
        domain: "memories",
    };
    const begun = await host.contextRequest(identity, {
        ...key,
        method: "authority.prepare",
        phase: "begin",
    });
    const generation = Number(
        (begun.authority as { generation?: unknown } | undefined)?.generation,
    );
    await host.contextRequest(identity, {
        ...key,
        method: "authority.prepare",
        phase: "complete",
        generation,
        checksum_expected: createHash("sha256").update("[]").digest("hex"),
    });
    const acked = await host.contextRequest(identity, {
        ...key,
        method: "authority.prepare",
        phase: "ack",
        generation,
    });
    const authority = acked.authority as { state?: unknown; generation?: unknown } | undefined;
    if (authority?.state !== "MODULE" || typeof authority.generation !== "number") {
        throw new Error(`memories authority did not reach MODULE: ${JSON.stringify(acked)}`);
    }
    return authority.generation;
}

async function memoryObjectIds(host: HermeticHostStack, identity: BindIdentity): Promise<string[]> {
    const read = await host.contextRequest(identity, {
        method: "kernel.read",
        v: 1,
        session_id: identity.session,
        project_root: identity.project_root,
        surface: "explicit_search",
        gated: false,
    });
    const rows = (read.rows ?? []) as Array<{ token?: { object_id?: unknown } }>;
    return rows.flatMap((row) =>
        typeof row.token?.object_id === "string" ? [row.token.object_id] : [],
    );
}

/** Classifies every memory the bound project's explicit-search surface shows, as a Memory Classifier client does. */
export async function classifyMemories(
    host: HermeticHostStack,
    identity: BindIdentity,
    modelRef: string,
): Promise<{ objectIds: string[]; response: Record<string, unknown> }> {
    const generation = await raiseMemoriesAuthority(host, identity);
    const objectIds = await memoryObjectIds(host, identity);
    const response = await host.contextRequest(identity, {
        method: "memory_classifier.run_task",
        v: 1,
        session_id: identity.session,
        task: "classify",
        command_id: `bedrock-only-${identity.harness}-classify`,
        authority_generation: generation,
        payload: { object_ids: objectIds, model_chain: [modelRef], timeout_ms: 120_000 },
    });
    return { objectIds, response };
}
