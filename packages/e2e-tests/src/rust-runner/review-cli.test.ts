import { afterEach, describe, expect, it } from "bun:test";
import { chmodSync, mkdtempSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { HostClient } from "@eidnara/opencode/shared/host-client";
import { type ReviewCommandDependencies, runReviewCommand } from "../../../cli/src/commands/review";
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
        realpath: (path) => path,
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
});
