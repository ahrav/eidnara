import { afterEach, describe, expect, it } from "bun:test";
import { spawn } from "node:child_process";
import {
    chmodSync,
    existsSync,
    mkdirSync,
    mkdtempSync,
    readdirSync,
    realpathSync,
    rmSync,
} from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { HostClient, isHostCallError } from "@eidnara/opencode/shared/host-client";
import { managedSubtreePath } from "@eidnara/opencode/shared/host-lifecycle/paths";
import { type ReviewCommandDependencies, runReviewCommand } from "../../../cli/src/commands/review";
import { raiseMemoriesAuthority } from "../bedrock-peer/classify";
import { waitFor } from "../harness-primitives";
import { waitForChildExit } from "../process-exit";
import { buildDirectHostFixture, detectRustModePrereqs, HermeticHostStack } from "./hermetic-host";

const fixturePrereqs = detectRustModePrereqs();
const temporaryRoots: string[] = [];

afterEach(() => {
    for (const root of temporaryRoots.splice(0)) rmSync(root, { recursive: true, force: true });
});

/** The command against the real host, connecting through the data root the fixture published into. */
function commandDependencies(dataDir: string): {
    deps: ReviewCommandDependencies;
    out: string[];
    err: string[];
    clients: HostClient[];
} {
    const out: string[] = [];
    const err: string[] = [];
    const clients: HostClient[] = [];
    const deps: ReviewCommandDependencies = {
        connect: async (connectionFile) => {
            const client = await HostClient.connect({ connectionFile, requestTimeoutMs: 5_000 });
            clients.push(client);
            return client;
        },
        resolveProjectRoot: (path) => path,
        cwd: () => dataDir,
        env: { XDG_DATA_HOME: dataDir },
        stdout: (line) => out.push(line),
        stderr: (line) => err.push(line),
    };
    return { deps, out, err, clients };
}

function kernelLeasePath(dataDir: string): string | undefined {
    const leases = join(managedSubtreePath(dataDir), "context", "kernel", "leases");
    if (!existsSync(leases)) return undefined;
    const files = readdirSync(leases).filter((name) => name.endsWith(".lease"));
    return files.length === 1 ? join(leases, files[0]) : undefined;
}

/** The returned promise resolves after the child confirms acquisition of the shared lock; `release` closes the child's stdin to end the lock holder. */
async function holdSharedFlock(path: string): Promise<{ release(): Promise<void> }> {
    const child = spawn(
        "flock",
        ["--shared", "--nonblock", path, "--command", "echo held; exec cat"],
        {
            stdio: ["pipe", "pipe", "inherit"],
        },
    );
    await new Promise<void>((resolveHeld, rejectHeld) => {
        child.once("error", rejectHeld);
        child.once("exit", (code) =>
            rejectHeld(new Error(`flock exited ${code} before holding ${path}`)),
        );
        child.stdout?.once("data", () => resolveHeld());
    });
    return {
        async release() {
            child.stdin?.end();
            if (!(await waitForChildExit(child, 5_000))) child.kill("SIGKILL");
        },
    };
}

describe("review command against the direct host", () => {
    it.skipIf(!fixturePrereqs.ok)(
        "status, list, and show speak flat envelopes over the real handshake and close their connection",
        async () => {
            const fixtureBin = await buildDirectHostFixture();
            const root = mkdtempSync(join(tmpdir(), "eidnara-review-cli-e2e-"));
            temporaryRoots.push(root);
            chmodSync(root, 0o700);
            const stack = await HermeticHostStack.start({ dataDir: root, fixtureBin });
            try {
                const before = await stack.backendCounters();
                const status = commandDependencies(root);
                expect(await runReviewCommand(["status", "--json"], status.deps)).toBe(0);
                const snapshot = JSON.parse(status.out[0]) as {
                    kind: string;
                    memory_reviewer_state: string | null;
                    counters: Record<string, unknown>;
                };
                expect(snapshot.kind).toBe("status");
                expect(["ready", "starting", "unavailable"]).toContain(
                    snapshot.memory_reviewer_state ?? "unreported",
                );
                expect(Object.keys(snapshot.counters)).toHaveLength(36);
                if (snapshot.memory_reviewer_state === "ready") {
                    expect(
                        Object.values(snapshot.counters).some((value) => typeof value === "number"),
                    ).toBe(true);
                }
                expect(status.clients[0]?.isClosed).toBe(true);

                const project = join(root, "project");
                const list = commandDependencies(root);
                const listExit = await runReviewCommand(
                    ["list", "--project", project, "--json"],
                    list.deps,
                );
                expect(list.err).toEqual([]);
                const listAnswer = JSON.parse(list.out[0]) as { kind: string };
                // A root with no MODULE memories authority answers the closed terminal or the Kernel's own state; either is a decoded shared refusal, never raw text.
                expect(["terminal", "state"]).toContain(listAnswer.kind);
                expect(listExit).toBe(1);
                expect(list.clients[0]?.isClosed).toBe(true);

                const show = commandDependencies(root);
                expect(
                    await runReviewCommand(
                        ["show", "d".repeat(64), "--project", project],
                        show.deps,
                    ),
                ).toBe(1);
                expect(show.err).toEqual([]);
                expect(show.out[0]).not.toContain("Action:");
                expect(show.clients[0]?.isClosed).toBe(true);
                // Two observational routes opened and closed and one status read: no background work started on the host.
                expect(await stack.backendCounters()).toEqual(before);
            } finally {
                await stack.stop();
            }
        },
        120_000,
    );
    it.skipIf(!fixturePrereqs.ok)(
        "list and show read a fixture-class proposal field for field and a production-class owner_sensitive abstention",
        async () => {
            const fixtureBin = await buildDirectHostFixture();
            const root = mkdtempSync(join(tmpdir(), "eidnara-review-cli-e2e-"));
            temporaryRoots.push(root);
            chmodSync(root, 0o700);
            const project = join(root, "project");
            mkdirSync(project);
            const projectRoot = realpathSync.native(project);
            const stack = await HermeticHostStack.start({ dataDir: root, fixtureBin });
            let kernelLease: { release(): Promise<void> } | undefined;
            const review = async (args: string[], exit: number): Promise<string> => {
                const command = commandDependencies(root);
                expect(await runReviewCommand(args, command.deps)).toBe(exit);
                expect(command.err).toEqual([]);
                expect(command.clients).toHaveLength(1);
                expect(command.clients[0]?.isClosed).toBe(true);
                return command.out.join("\n");
            };
            try {
                // The shared flock blocks the restarted Kernel from acquiring its store lease.
                const leasePath = await waitFor(
                    "the Kernel lease file",
                    () => kernelLeasePath(root),
                    30_000,
                    () => stack.hostLog(),
                );
                await stack.crashHost();
                kernelLease = await holdSharedFlock(leasePath);
                await stack.restartHost();
                await waitFor(
                    "the restarted Kernel waiting on its lease",
                    () => (stack.hostLog().includes("kernel store lease held") ? true : undefined),
                    30_000,
                    () => stack.hostLog(),
                );
                // The host opens its Memory Store after it publishes its connection file; only that refusal is retried.
                await waitFor(
                    "the memories authority reaching MODULE",
                    () =>
                        raiseMemoriesAuthority(stack, {
                            project_root: projectRoot,
                            harness: "opencode",
                            session: "review-witness",
                        }).catch((error: unknown) => {
                            if (isHostCallError(error) && error.code === "store_unavailable") {
                                return undefined;
                            }
                            throw error;
                        }),
                    30_000,
                    () => stack.hostLog(),
                );
                const heldLease = kernelLease;
                const leaseReleased = Bun.sleep(250).then(() => heldLease.release());
                const witness = await waitFor(
                    "the review witness over an open Kernel",
                    () =>
                        stack.reviewWitness(projectRoot).catch((error: unknown) => {
                            if (String(error).endsWith("rejected: kernel_unavailable")) {
                                return undefined;
                            }
                            throw new Error(`${String(error)}\n${stack.hostLog()}`);
                        }),
                    30_000,
                    () => stack.hostLog(),
                );
                await leaseReleased;
                // One provider exchange per fixture job; the production jobs abstain before any attempt or connection.
                expect(witness.fixture).toHaveLength(2);
                expect(witness.production).toHaveLength(3);
                expect(witness.providerRequests).toBe(2);
                expect(witness.providerConnections).toBe(2);
                expect(witness.productionAttempts).toBe(0);
                expect(witness.fixture.flatMap((job) => job.requiredEvidence).sort()).toEqual(
                    [...witness.evidence].sort(),
                );

                // The ledger's own list, and independently, each fixture job complete and selected and each production job abstained owner_sensitive.
                const byteOrder = (left: string, right: string) =>
                    left < right ? -1 : left > right ? 1 : 0;
                const expectedItems = [
                    ...witness.fixture.map(({ read }) => ({
                        causal_identity: read.causal_identity,
                        generation: 1,
                        outcome: "complete",
                        selected: true,
                    })),
                    ...witness.production.map((causal_identity) => ({
                        causal_identity,
                        generation: 1,
                        outcome: "abstained",
                        reason: "owner_sensitive",
                        selected: false,
                    })),
                ].sort((left, right) => byteOrder(left.causal_identity, right.causal_identity));
                expect(witness.outcomes).toEqual(expectedItems);
                const listed = await review(
                    ["list", "--project", projectRoot, "--limit", "64", "--json"],
                    0,
                );
                expect(JSON.parse(listed)).toEqual({
                    kind: "page",
                    project_root: projectRoot,
                    items: witness.outcomes,
                    next: null,
                });

                for (const { read, requiredEvidence } of witness.fixture) {
                    const shown = JSON.parse(
                        await review(
                            ["show", read.causal_identity, "--project", projectRoot, "--json"],
                            0,
                        ),
                    ) as Record<string, unknown>;
                    // The command renders every field the Kernel serializes except the policy dependencies, which stay out of operator output.
                    const { policy_dependencies, ...rendered } = read.proposal;
                    expect(policy_dependencies).toBeDefined();
                    expect(shown).toEqual({
                        ...read,
                        project_root: projectRoot,
                        proposal: rendered,
                    });
                    // Independently of the Kernel's read: the scripted retain step, the originating decision at its exact tokens, and one nonempty span of the job's own form.
                    expect(rendered).toMatchObject({
                        action: "retain",
                        target: { kind: "memory", ...witness.target },
                        contradictions: [],
                        limitations: ["only the form itself was read"],
                        uncertainty: "medium",
                    });
                    expect("new_text" in rendered).toBe(false);
                    const support = rendered.support as {
                        evidence_id: string;
                        span: { alias: string; start: number; end: number };
                    }[];
                    expect(support.map((reference) => reference.evidence_id)).toEqual(
                        requiredEvidence,
                    );
                    for (const { span } of support) {
                        expect(span.alias).toBe("ref-1");
                        expect(span.end).toBeGreaterThan(span.start);
                    }
                }

                const text = await review(
                    ["show", witness.fixture[0].read.causal_identity, "--project", projectRoot],
                    0,
                );
                expect(text).toContain("Action: retain");
                expect(text).toContain("Reference only");

                for (const identity of witness.production) {
                    expect(
                        JSON.parse(
                            await review(["show", identity, "--project", projectRoot, "--json"], 1),
                        ),
                    ).toEqual({ kind: "terminal", terminal: "not_selected" });
                }
            } finally {
                await kernelLease?.release();
                await stack.stop();
            }
        },
        180_000,
    );
});
