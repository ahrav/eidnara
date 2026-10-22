import { afterEach, describe, expect, it } from "bun:test";
import { chmodSync, mkdirSync, mkdtempSync, realpathSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { HostClient } from "@eidnara/opencode/shared/host-client";
import { type ReviewCommandDependencies, runReviewCommand } from "../../../cli/src/commands/review";
import { raiseMemoriesAuthority } from "../bedrock-peer/classify";
import { waitFor } from "../harness-primitives";
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
            try {
                // The host opens its Memory Store after it publishes its connection file; the authority write waits for it.
                await waitFor(
                    "the memories authority reaching MODULE",
                    () =>
                        raiseMemoriesAuthority(stack, {
                            project_root: projectRoot,
                            harness: "opencode",
                            session: "review-witness",
                        }).catch(() => undefined),
                    30_000,
                    () => stack.hostLog(),
                );
                const witness = await stack.reviewWitness(projectRoot);
                // One provider exchange per fixture job; the production jobs abstain before any attempt or connection.
                expect(witness.fixture).toHaveLength(2);
                expect(witness.production).toHaveLength(3);
                expect(witness.providerRequests).toBe(2);
                expect(witness.providerConnections).toBe(2);
                expect(witness.productionAttempts).toBe(0);
                const fixtureIdentities = witness.fixture.map(
                    (read) => read.causal_identity as string,
                );

                const list = commandDependencies(root);
                expect(
                    await runReviewCommand(
                        ["list", "--project", projectRoot, "--limit", "64", "--json"],
                        list.deps,
                    ),
                ).toBe(0);
                expect(list.err).toEqual([]);
                const expectedItems = [
                    ...fixtureIdentities.map((causal_identity) => ({
                        causal_identity,
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
                ].sort((left, right) => left.causal_identity.localeCompare(right.causal_identity));
                expect(JSON.parse(list.out[0])).toEqual({
                    kind: "page",
                    project_root: projectRoot,
                    items: expectedItems,
                    next: null,
                });
                expect(list.clients[0]?.isClosed).toBe(true);

                for (const read of witness.fixture) {
                    const show = commandDependencies(root);
                    const identity = read.causal_identity as string;
                    expect(
                        await runReviewCommand(
                            ["show", identity, "--project", projectRoot, "--json"],
                            show.deps,
                        ),
                    ).toBe(0);
                    expect(show.err).toEqual([]);
                    expect(show.clients[0]?.isClosed).toBe(true);
                    // The command renders every field the Kernel serializes except the policy dependencies, which stay out of operator output.
                    const { policy_dependencies, ...rendered } = read.proposal as Record<
                        string,
                        unknown
                    >;
                    expect(policy_dependencies).toBeDefined();
                    const shown = JSON.parse(show.out[0]) as Record<string, unknown>;
                    expect(shown).toEqual({
                        ...read,
                        project_root: projectRoot,
                        proposal: rendered,
                    });
                    // The target is the originating decision at its exact tokens, and every citation is a nonempty span of a fixture form.
                    const proposal = shown.proposal as {
                        action: string;
                        target: Record<string, unknown>;
                        support: { evidence_id: string; span?: { start: number; end: number } }[];
                    };
                    expect(proposal.action).toBe("retain");
                    expect(proposal.target).toEqual({ kind: "memory", ...witness.target });
                    expect(proposal.support.length).toBeGreaterThan(0);
                    for (const reference of proposal.support) {
                        expect(witness.evidence).toContain(reference.evidence_id);
                        expect(reference.span).toBeDefined();
                        expect(reference.span?.end ?? 0).toBeGreaterThan(
                            reference.span?.start ?? 0,
                        );
                    }
                }

                const text = commandDependencies(root);
                expect(
                    await runReviewCommand(
                        ["show", fixtureIdentities[0], "--project", projectRoot],
                        text.deps,
                    ),
                ).toBe(0);
                expect(text.out[0]).toContain("Action: retain");
                expect(text.out[0]).toContain("Reference only");

                for (const identity of witness.production) {
                    const show = commandDependencies(root);
                    expect(
                        await runReviewCommand(
                            ["show", identity, "--project", projectRoot, "--json"],
                            show.deps,
                        ),
                    ).toBe(1);
                    expect(show.err).toEqual([]);
                    expect(JSON.parse(show.out[0])).toEqual({
                        kind: "terminal",
                        terminal: "not_selected",
                    });
                    expect(show.clients[0]?.isClosed).toBe(true);
                }
            } finally {
                await stack.stop();
            }
        },
        180_000,
    );
});
