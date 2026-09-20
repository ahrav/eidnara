import { describe, expect, test } from "bun:test";
import { HostCallError } from "@eidnara/opencode/shared/host-client";
import {
    parseReviewArgs,
    type ReviewCommandDependencies,
    type ReviewConnection,
    runReviewCommand,
} from "./review";
import { decodeReviewStatus } from "./review-wire";

const HEX = "a".repeat(64);
const HEX_B = "b".repeat(64);
const ENV = { HOME: "/home/reviewer" };

interface Recorded {
    connected: string[];
    routeOpens: unknown[][];
    requests: unknown[];
    requestOptions: unknown[];
    closes: number;
    stdout: string[];
    stderr: string[];
}

function harness(
    answers: {
        catalog?: unknown[];
        status?: { health: "ok" | "degraded" | "failing"; metrics: Record<string, unknown> };
        respond?: (body: Record<string, unknown>) => unknown;
        connect?: () => Promise<ReviewConnection>;
    } = {},
): { deps: ReviewCommandDependencies; recorded: Recorded } {
    const recorded: Recorded = {
        connected: [],
        routeOpens: [],
        requests: [],
        requestOptions: [],
        closes: 0,
        stdout: [],
        stderr: [],
    };
    const connection: ReviewConnection = {
        catalogList: async () =>
            (answers.catalog ?? [
                { module_id: "context", module_version: "1", roles: [], control_ops: [] },
            ]) as never,
        hostStatus: async () => answers.status ?? { health: "ok", metrics: { components: {} } },
        routeOpen: async (...args) => {
            recorded.routeOpens.push(args);
            return { channel: 7, epoch: 1 } as never;
        },
        request: async (_handle, body, options) => {
            recorded.requests.push(body);
            recorded.requestOptions.push(options);
            const respond = answers.respond ?? (() => ({ kind: "page", items: [], next: null }));
            return respond(body as Record<string, unknown>);
        },
        closeAsync: async () => {
            recorded.closes += 1;
        },
    };
    const deps: ReviewCommandDependencies = {
        connect:
            answers.connect ??
            (async (file) => {
                recorded.connected.push(file);
                return connection;
            }),
        realpath: (path) => `${path}/real`,
        cwd: () => "/work/project",
        env: ENV,
        stdout: (line) => recorded.stdout.push(line),
        stderr: (line) => recorded.stderr.push(line),
    };
    return { deps, recorded };
}

function statusMetrics(memoryReviewer?: Record<string, unknown>): Record<string, unknown> {
    return {
        components: {
            context: {
                status: "ok",
                metrics: {
                    storage_state: "ready",
                    ...(memoryReviewer === undefined ? {} : { memory_reviewer: memoryReviewer }),
                },
            },
        },
    };
}

function selectedBody(): Record<string, unknown> {
    return {
        kind: "proposal",
        causal_identity: HEX,
        reference: {
            database_incarnation_id: "inc-1",
            candidate_id: "cand-1",
            payload_digest: HEX_B,
        },
        proposal: {
            action: "revise",
            target: {
                kind: "memory",
                object_id: "mem-1",
                source_revision: 3,
                known_as_of: 9007199254740993n,
                commit_token: -5,
            },
            new_text: "the workspace builds with bun\u001b[2K\nsecond line",
            support: [{ evidence_id: "ev-1", span: { alias: "s1", start: 0, end: 12 } }],
            contradictions: [],
            limitations: ["only the subject was read"],
            uncertainty: "medium",
            manifest: { manifest_id: "manifest-1", digest: HEX_B },
            policy_dependencies: { question_template: "extracted_facts", disclosed_inputs: [] },
        },
        review_expires_at: 1_700_000_000_000,
    };
}

describe("parseReviewArgs", () => {
    test("defaults and bounds", () => {
        expect(parseReviewArgs(["list"])).toEqual({
            command: "list",
            project: null,
            limit: 16,
            after: null,
            json: false,
        });
        expect(
            parseReviewArgs(["list", "--project", "/p", "--limit", "64", "--after", HEX, "--json"]),
        ).toEqual({
            command: "list",
            project: "/p",
            limit: 64,
            after: HEX,
            json: true,
        });
        expect(parseReviewArgs(["show", HEX, "--json"])).toEqual({
            command: "show",
            project: null,
            causalIdentity: HEX,
            json: true,
        });
        expect(parseReviewArgs(["status", "--json"])).toEqual({ command: "status", json: true });
        for (const bad of [
            ["list", "--limit", "0"],
            ["list", "--limit", "65"],
            ["list", "--limit", "1.5"],
            ["list", "--after", "not-hex"],
            ["list", "--bogus"],
            ["list", "extra"],
            ["show"],
            ["show", "nothex"],
            ["show", HEX, HEX],
            ["status", "--project", "/p"],
            ["status", "--limit", "1"],
            ["show", HEX, "--after", HEX],
            ["nonsense"],
        ]) {
            expect(typeof parseReviewArgs(bad)).toBe("string");
        }
    });
});

describe("review list", () => {
    test("binds the realpath of --project or cwd under the cli harness with a fresh session and no ambient identity, sends one explicit page request, and closes the connection", async () => {
        const { deps, recorded } = harness({
            respond: () => ({
                kind: "page",
                items: [
                    { causal_identity: HEX, generation: 1, outcome: "complete", selected: true },
                    {
                        causal_identity: HEX_B,
                        generation: 9007199254740993n,
                        outcome: "abstained",
                        reason: "owner_sensitive",
                        selected: false,
                    },
                ],
                next: HEX_B,
            }),
        });
        expect(await runReviewCommand(["list", "--project", "/p"], deps)).toBe(0);
        expect(recorded.connected).toEqual([
            "/home/reviewer/.local/share/eidnara/run/connection.json",
        ]);
        const [target, identity, options] = recorded.routeOpens[0] as [
            { kind: string; module_id: string },
            { project_root: string; harness: string; session: string },
            { consumerIdentity: null },
        ];
        expect(target).toEqual({ kind: "tool_provider", module_id: "context" });
        expect(identity.project_root).toBe("/p/real");
        expect(identity.harness).toBe("cli");
        expect(identity.session).toMatch(/^eidnara-review:[0-9a-f-]{36}$/);
        expect(options).toEqual({ consumerIdentity: null });
        expect(recorded.requests).toEqual([
            {
                v: 1,
                session_id: identity.session,
                project_root: "/p/real",
                method: "review.list",
                limit: 16,
                after: null,
            },
        ]);
        expect(recorded.requestOptions).toEqual([{ exactIntegers: true }]);
        expect(recorded.closes).toBe(1);
        const text = recorded.stdout.join("\n");
        expect(text).toContain("Project: /p/real");
        expect(text).toContain(`${HEX} gen 1 complete selected`);
        expect(text).toContain(`${HEX_B} gen 9007199254740993 abstained (owner_sensitive)`);
        expect(text).toContain(`Next page: eidnara review list --project /p/real --after ${HEX_B}`);

        const cwd = harness();
        expect(await runReviewCommand(["list"], cwd.deps)).toBe(0);
        expect(
            (cwd.recorded.routeOpens[0] as [unknown, { project_root: string }])[1].project_root,
        ).toBe("/work/project/real");
        expect(cwd.recorded.stdout.join("\n")).toContain("No completed outcomes on this page.");
        expect(cwd.recorded.stdout.join("\n")).toContain("End of walk.");
    });

    test("the next-page command carries the bound root and a non-default limit so it reruns from any directory", async () => {
        const page = { kind: "page", items: [], next: HEX_B };
        const quoted = harness({ respond: () => page });
        expect(
            await runReviewCommand(
                ["list", "--project", "/space d/it's", "--limit", "5"],
                quoted.deps,
            ),
        ).toBe(0);
        expect(quoted.recorded.stdout[0]).toContain(
            `Next page: eidnara review list --project '/space d/it'\\''s/real' --limit 5 --after ${HEX_B}`,
        );
        const cwd = harness({ respond: () => page });
        expect(await runReviewCommand(["list"], cwd.deps)).toBe(0);
        expect(cwd.recorded.stdout[0]).toContain(
            `Next page: eidnara review list --project /work/project/real --after ${HEX_B}`,
        );
        expect(cwd.recorded.stdout[0]).not.toContain("--limit");
        const long = harness({ respond: () => page });
        const deep = `/${"segment/".repeat(40)}leaf`;
        expect(await runReviewCommand(["list", "--project", deep], long.deps)).toBe(0);
        expect(long.recorded.stdout[0]).toContain(`--project ${deep}/real --after`);
    });

    test("an exact-full page yields a cursor whose follow-up may be empty, and nothing walks it automatically", async () => {
        const pages = new Map<string | null, unknown>([
            [
                null,
                {
                    kind: "page",
                    items: [
                        {
                            causal_identity: HEX,
                            generation: 1,
                            outcome: "complete",
                            selected: true,
                        },
                        {
                            causal_identity: HEX_B,
                            generation: 2,
                            outcome: "failed",
                            selected: false,
                        },
                    ],
                    next: HEX_B,
                },
            ],
            [HEX_B, { kind: "page", items: [], next: null }],
        ]);
        const first = harness({ respond: (body) => pages.get(body.after as string | null) });
        expect(await runReviewCommand(["list", "--limit", "2"], first.deps)).toBe(0);
        expect(first.recorded.requests).toHaveLength(1);
        expect(first.recorded.stdout[0]).toContain(
            `Next page: eidnara review list --project /work/project/real --limit 2 --after ${HEX_B}`,
        );
        const second = harness({ respond: (body) => pages.get(body.after as string | null) });
        expect(
            await runReviewCommand(["list", "--limit", "2", "--after", HEX_B], second.deps),
        ).toBe(0);
        expect(second.recorded.requests).toEqual([
            expect.objectContaining({ after: HEX_B, limit: 2 }),
        ]);
        expect(second.recorded.stdout[0]).toContain("No completed outcomes on this page.");
        expect(second.recorded.stdout[0]).toContain("End of walk.");
    });

    test("--json carries exact integers as number tokens and an explicit continuation", async () => {
        const { deps, recorded } = harness({
            respond: (body) => ({
                kind: "page",
                items: [
                    {
                        causal_identity: HEX,
                        generation: 18446744073709551615n,
                        outcome: "expired",
                        selected: false,
                    },
                ],
                next: body.after === HEX ? null : HEX,
            }),
        });
        expect(
            await runReviewCommand(["list", "--json", "--limit", "1", "--after", HEX], deps),
        ).toBe(0);
        expect(recorded.requests[0]).toMatchObject({ limit: 1, after: HEX });
        expect(recorded.stdout[0]).toBe(
            `{"kind":"page","project_root":"/work/project/real","items":[{"causal_identity":"${HEX}","generation":18446744073709551615,"outcome":"expired","selected":false}],"next":null}`,
        );
    });

    test("terminals, kernel states, and malformed pages refuse without success output and still close", async () => {
        const cases: [unknown, string, string][] = [
            [
                { kind: "terminal", terminal: "disabled" },
                "Review access is disabled: no review store is installed, or this bound root has no active MODULE memories authority, including no authority-route binding. The daemon does not identify which cause applies.",
                '{"kind":"terminal","terminal":"disabled"}',
            ],
            [
                { kind: "terminal", terminal: "store_unavailable" },
                "The review store could not be read.",
                '{"kind":"terminal","terminal":"store_unavailable"}',
            ],
            [
                { state: { kind: "invalid", reason: "project_mismatch" } },
                "Kernel state: invalid:project_mismatch",
                '{"kind":"state","state":{"kind":"invalid","reason":"project_mismatch"}}',
            ],
            [
                { state: { kind: "unavailable", reason: "store_starting" } },
                "Kernel state: unavailable:store_starting",
                '{"kind":"state","state":{"kind":"unavailable","reason":"store_starting"}}',
            ],
            [
                { state: { kind: "weird" } },
                "Kernel state: invalid:unrecognized_state",
                '{"kind":"state","state":{"kind":"invalid","reason":"unrecognized_state"}}',
            ],
            [
                { kind: "terminal", terminal: "not_selected" },
                "The response could not be validated: terminal is not in the protocol vocabulary.",
                '{"kind":"malformed","detail":"terminal is not in the protocol vocabulary"}',
            ],
            [
                {
                    kind: "page",
                    items: [
                        {
                            causal_identity: HEX,
                            generation: 1,
                            outcome: "complete",
                            selected: true,
                            reason: "secret",
                        },
                    ],
                    next: null,
                },
                "The response could not be validated: reason is present on a non-abstained item.",
                '{"kind":"malformed","detail":"reason is present on a non-abstained item"}',
            ],
            [
                {
                    kind: "page",
                    items: [
                        {
                            causal_identity: HEX,
                            generation: -1,
                            outcome: "complete",
                            selected: true,
                        },
                    ],
                    next: null,
                },
                "The response could not be validated: item generation is not a u64.",
                '{"kind":"malformed","detail":"item generation is not a u64"}',
            ],
            [
                {
                    kind: "page",
                    items: Array.from({ length: 65 }, () => ({
                        causal_identity: HEX,
                        generation: 1,
                        outcome: "complete",
                        selected: true,
                    })),
                    next: null,
                },
                "The response could not be validated: items is not a bounded array.",
                '{"kind":"malformed","detail":"items is not a bounded array"}',
            ],
            [
                "nope",
                "The response could not be validated: response is not an object.",
                '{"kind":"malformed","detail":"response is not an object"}',
            ],
        ];
        for (const [raw, text, json] of cases) {
            const plain = harness({ respond: () => raw });
            expect(await runReviewCommand(["list"], plain.deps)).toBe(1);
            expect(plain.recorded.stdout).toEqual([text]);
            expect(plain.recorded.closes).toBe(1);
            const machine = harness({ respond: () => raw });
            expect(await runReviewCommand(["list", "--json"], machine.deps)).toBe(1);
            expect(machine.recorded.stdout).toEqual([json]);
        }
    });
});

describe("review list vocabulary", () => {
    test("every outcome and every abstention reason renders as itself", async () => {
        const outcomes = ["complete", "abstained", "failed", "cancelled", "unknown", "expired"];
        const reasons = [
            "owner_sensitive",
            "wrong_scope",
            "secret",
            "expectation_changed",
            "undisclosed_citation",
            "partial_disclosure",
            "model_declined",
            "budget_exhausted",
            "invalid_proposal",
        ];
        const items = [
            ...outcomes
                .filter((o) => o !== "abstained")
                .map((outcome, i) => ({
                    causal_identity: HEX,
                    generation: i,
                    outcome,
                    selected: outcome === "complete",
                })),
            ...reasons.map((reason, i) => ({
                causal_identity: HEX_B,
                generation: 100 + i,
                outcome: "abstained",
                reason,
                selected: false,
            })),
        ];
        const { deps, recorded } = harness({
            respond: () => ({ kind: "page", items, next: null }),
        });
        expect(await runReviewCommand(["list"], deps)).toBe(0);
        const text = recorded.stdout[0];
        for (const outcome of outcomes.filter((o) => o !== "abstained"))
            expect(text).toContain(` ${outcome}`);
        for (const reason of reasons) expect(text).toContain(`abstained (${reason})`);
        const unknownOutcome = harness({
            respond: () => ({
                kind: "page",
                items: [
                    { causal_identity: HEX, generation: 1, outcome: "accepted", selected: true },
                ],
                next: null,
            }),
        });
        expect(await runReviewCommand(["list"], unknownOutcome.deps)).toBe(1);
        expect(unknownOutcome.recorded.stdout[0]).toBe(
            "The response could not be validated: item outcome is not in the protocol vocabulary.",
        );
        const unknownReason = harness({
            respond: () => ({
                kind: "page",
                items: [
                    {
                        causal_identity: HEX,
                        generation: 1,
                        outcome: "abstained",
                        reason: "tired",
                        selected: false,
                    },
                ],
                next: null,
            }),
        });
        expect(await runReviewCommand(["list"], unknownReason.deps)).toBe(1);
        expect(unknownReason.recorded.stdout[0]).toBe(
            "The response could not be validated: abstained item reason is not recognized.",
        );
    });
});

describe("review show", () => {
    test("issues exactly one read and renders every field with inert text and exact integers", async () => {
        const { deps, recorded } = harness({ respond: selectedBody });
        expect(await runReviewCommand(["show", HEX], deps)).toBe(0);
        expect(recorded.requests).toHaveLength(1);
        expect(recorded.requests[0]).toMatchObject({ method: "review.read", causal_identity: HEX });
        expect(recorded.requestOptions).toEqual([{ exactIntegers: true }]);
        const text = recorded.stdout[0];
        expect(text).toContain("Action: revise");
        expect(text).toContain(
            "Target: memory mem-1 revision 3 known as of 9007199254740993 commit token -5",
        );
        expect(text).toContain("the workspace builds with bun \nsecond line");
        expect(text).not.toContain("\u001b");
        expect(text).toContain("  ev-1 [s1 0..12]");
        expect(text).toContain("Contradictions: none");
        expect(text).toContain("  only the subject was read");
        expect(text).toContain("Uncertainty: medium");
        expect(text).toContain(`Manifest: manifest-1 ${HEX_B}`);
        expect(text).toContain("Review expires at: 1700000000000 ms");
        expect(text).toContain("Reference only");
        expect(recorded.closes).toBe(1);

        const machine = harness({ respond: selectedBody });
        expect(await runReviewCommand(["show", HEX, "--json"], machine.deps)).toBe(0);
        const json = JSON.parse(machine.recorded.stdout[0], (_key, value, context) =>
            typeof value === "number" ? context.source : value,
        ) as Record<string, unknown>;
        const proposal = json.proposal as Record<string, unknown>;
        const target = proposal.target as Record<string, unknown>;
        expect(target.known_as_of).toBe("9007199254740993");
        expect(target.commit_token).toBe("-5");
        expect(json.review_expires_at).toBe("1700000000000");
        expect((proposal.support as Record<string, unknown>[])[0]).toEqual({
            evidence_id: "ev-1",
            span: { alias: "s1", start: "0", end: "12" },
        });
        expect("policy_dependencies" in proposal).toBe(false);
    });

    test("every read terminal maps and malformed proposals refuse without a hidden list walk", async () => {
        for (const terminal of [
            "not_selected",
            "incarnation_mismatch",
            "selection_mismatch",
            "kernel_refused",
            "review_expired",
            "dependency_refused",
            "disabled",
            "store_unavailable",
        ]) {
            const { deps, recorded } = harness({ respond: () => ({ kind: "terminal", terminal }) });
            expect(await runReviewCommand(["show", HEX], deps)).toBe(1);
            expect(recorded.requests).toHaveLength(1);
            expect(recorded.stdout).toHaveLength(1);
            expect(recorded.stdout[0]).not.toContain("Action:");
            expect(recorded.closes).toBe(1);
        }
        // Byte caps are UTF-8 byte counts: 32768 ASCII bytes pass, 16385 two-byte characters do not.
        const ascii = harness({
            respond: () => {
                const body = selectedBody();
                const proposal = body.proposal as Record<string, unknown>;
                proposal.new_text = "x".repeat(32 * 1024);
                proposal.target = { kind: "staged_candidate", candidate_id: "cand-1" };
                proposal.limitations = [];
                proposal.support = [{ evidence_id: "ev-1" }];
                return body;
            },
        });
        expect(await runReviewCommand(["show", HEX], ascii.deps)).toBe(0);
        expect(ascii.recorded.stdout[0]).toContain("Target: staged candidate cand-1");
        expect(ascii.recorded.stdout[0]).toContain("Limitations: none");
        expect(ascii.recorded.stdout[0]).toContain("  ev-1\n");
        const malformed: [(body: Record<string, unknown>) => void, string][] = [
            [
                (b) => {
                    (b.proposal as Record<string, unknown>).new_text = "\u00e9".repeat(
                        16 * 1024 + 1,
                    );
                },
                "new_text is not bounded text",
            ],
            [
                (b) => {
                    (b.reference as Record<string, unknown>).candidate_id = "c".repeat(513);
                },
                "reference carries a field outside its domain",
            ],
            [
                (b) => {
                    (b.proposal as Record<string, unknown>).support = Array.from(
                        { length: 129 },
                        (_, i) => ({ evidence_id: `s-${i}` }),
                    );
                    (b.proposal as Record<string, unknown>).contradictions = Array.from(
                        { length: 128 },
                        (_, i) => ({ evidence_id: `c-${i}` }),
                    );
                },
                "support and contradictions exceed the reference bound together",
            ],
            [
                (b) => {
                    (b.proposal as Record<string, unknown>).action = "accept";
                },
                "action is not recognized",
            ],
            [
                (b) => {
                    (
                        (b.proposal as Record<string, unknown>).support as Record<string, unknown>[]
                    )[0].span = { alias: "s1", start: 12, end: 12 };
                },
                "support span is not a bounded alias with end after start",
            ],
            [
                (b) => {
                    (b.proposal as Record<string, unknown>).new_text = "x".repeat(32 * 1024 + 1);
                },
                "new_text is not bounded text",
            ],
            [
                (b) => {
                    b.review_expires_at = 9223372036854775808n;
                },
                "review_expires_at is not an i64",
            ],
            [
                (b) => {
                    (b.reference as Record<string, unknown>).payload_digest = "short";
                },
                "reference carries a field outside its domain",
            ],
            [
                (b) => {
                    b.kind = "page";
                },
                "kind is not recognized",
            ],
        ];
        for (const [mutate, detail] of malformed) {
            const { deps, recorded } = harness({
                respond: () => {
                    const body = selectedBody();
                    mutate(body);
                    return body;
                },
            });
            expect(await runReviewCommand(["show", HEX], deps)).toBe(1);
            expect(recorded.stdout).toEqual([`The response could not be validated: ${detail}.`]);
        }
    });
});

describe("review status", () => {
    test("reads host.status only, keeps overlapping counters, and reports unavailable rather than zero", async () => {
        const { deps, recorded } = harness({
            status: {
                health: "degraded",
                metrics: statusMetrics({
                    memory_reviewer_state: "ready",
                    activation_state: "open",
                    sampled_at_ms: 1_700_000_000_000,
                    jobs_ready: 9007199254740992n,
                    jobs_reserved: 9007199254740993n,
                    jobs_abstained: -1,
                    jobs_completed: 2.5,
                    jobs_failed: null,
                    receipts_complete: 4,
                    swept_jobs: 0,
                    future_counter: 7,
                }),
            },
        });
        expect(await runReviewCommand(["status"], deps)).toBe(0);
        expect(recorded.routeOpens).toEqual([]);
        expect(recorded.requests).toEqual([]);
        expect(recorded.closes).toBe(1);
        const text = recorded.stdout[0];
        expect(text).toContain("Host health: degraded");
        expect(text).toContain("MemoryReviewer store: ready");
        expect(text).toContain("Activation (disclosure admission, not application): open");
        expect(text).toContain("jobs_ready: 9007199254740992");
        expect(text).toContain("jobs_reserved: unavailable");
        expect(text).toContain("jobs_abstained: unavailable");
        expect(text).toContain("jobs_completed: unavailable");
        expect(text).toContain("jobs_failed: unavailable");
        expect(text).toContain("jobs_unknown: unavailable");
        expect(text).toContain("receipts_complete: 4");
        expect(text).toContain("swept_jobs: 0");
        expect(text).not.toContain("future_counter");
        expect(text).not.toMatch(/^(total|ratio|success)/im);
    });

    test("the block is read only under the context component, never from the top of metrics", () => {
        const block = { memory_reviewer_state: "ready", jobs_ready: 1 };
        expect(decodeReviewStatus(statusMetrics(block)).counters.jobs_ready).toBe(1);
        const flat = decodeReviewStatus({ components: {}, memory_reviewer: block });
        expect(flat.memory_reviewer_state).toBeNull();
        expect(flat.counters.jobs_ready).toBeNull();
        const otherComponent = decodeReviewStatus({
            components: { local_embeddings: { status: "ok", metrics: { memory_reviewer: block } } },
        });
        expect(otherComponent.memory_reviewer_state).toBeNull();
    });

    test("a store that is not ready reports every counter unavailable, even present zeros, and an absent block is unavailable", async () => {
        const starting = harness({
            status: {
                health: "ok",
                metrics: statusMetrics({
                    memory_reviewer_state: "starting",
                    activation_state: "stale",
                    sampled_at_ms: null,
                    swept_jobs: 0,
                    swept_selections: 0,
                    jobs_ready: 3,
                }),
            },
        });
        expect(await runReviewCommand(["status", "--json"], starting.deps)).toBe(0);
        const json = JSON.parse(starting.recorded.stdout[0]) as {
            kind: string;
            memory_reviewer_state: string;
            activation_state: string;
            sampled_at_ms: unknown;
            counters: Record<string, unknown>;
        };
        expect(json.kind).toBe("status");
        expect(json.memory_reviewer_state).toBe("starting");
        expect(json.activation_state).toBe("stale");
        expect(json.sampled_at_ms).toBeNull();
        expect(Object.keys(json.counters)).toHaveLength(36);
        expect(Object.values(json.counters).every((value) => value === null)).toBe(true);

        const noActivation = harness({
            status: {
                health: "ok",
                metrics: statusMetrics({
                    memory_reviewer_state: "ready",
                    sampled_at_ms: 42,
                    jobs_ready: 1,
                }),
            },
        });
        expect(await runReviewCommand(["status"], noActivation.deps)).toBe(0);
        expect(noActivation.recorded.stdout[0]).toContain(
            "Activation (disclosure admission, not application): unreported",
        );
        expect(noActivation.recorded.stdout[0]).toContain("Sampled at: 42 ms");
        expect(noActivation.recorded.stdout[0]).toContain("jobs_ready: 1");

        const absent = harness({ status: { health: "ok", metrics: statusMetrics() } });
        expect(await runReviewCommand(["status"], absent.deps)).toBe(0);
        expect(absent.recorded.stdout[0]).toContain("MemoryReviewer store: unreported");
        expect(absent.recorded.stdout[0]).toContain(
            "Activation (disclosure admission, not application): unreported",
        );

        // The wire's own `unknown` and `unavailable` render as themselves, distinct from an absent field.
        const literal = harness({
            status: {
                health: "ok",
                metrics: statusMetrics({
                    memory_reviewer_state: "unavailable",
                    activation_state: "unknown",
                }),
            },
        });
        expect(await runReviewCommand(["status"], literal.deps)).toBe(0);
        expect(literal.recorded.stdout[0]).toContain("MemoryReviewer store: unavailable");
        expect(literal.recorded.stdout[0]).toContain(
            "Activation (disclosure admission, not application): unknown",
        );

        expect(
            decodeReviewStatus(statusMetrics({ memory_reviewer_state: "later", jobs_ready: 1 }))
                .memory_reviewer_state,
        ).toBeNull();
        expect(
            decodeReviewStatus(
                statusMetrics({ memory_reviewer_state: "ready", activation_state: "later" }),
            ).activation_state,
        ).toBeNull();
    });
});

describe("connection lifecycle", () => {
    test("transport and route refusals report their code without a payload, and the connection closes on every path", async () => {
        const hostile = harness({
            respond: () => {
                throw new HostCallError("terminal", "x", "bad\u001b[2Krequest");
            },
        });
        expect(await runReviewCommand(["list"], hostile.deps)).toBe(1);
        expect(hostile.recorded.stderr).toEqual(["Review list failed: terminal (bad request)."]);
        for (const code of [
            "route_unbound",
            "session_mismatch",
            "bad_request",
            "unrecognized_request_shape",
            "invalid_params",
            "invalid_response_body",
            "daemon_generation_changed",
        ]) {
            const { deps, recorded } = harness({
                respond: () => {
                    throw new HostCallError("terminal", `secret detail ${HEX}`, code);
                },
            });
            expect(await runReviewCommand(["list"], deps)).toBe(1);
            expect(recorded.stdout).toEqual([]);
            expect(recorded.stderr).toEqual([`Review list failed: terminal (${code}).`]);
            expect(recorded.closes).toBe(1);
        }
        const unknown = harness({
            respond: () => {
                throw new HostCallError("outcome_unknown", "lost", undefined);
            },
        });
        expect(await runReviewCommand(["show", HEX], unknown.deps)).toBe(1);
        expect(unknown.recorded.stderr).toEqual(["Review show failed: outcome_unknown."]);
        expect(unknown.recorded.requests).toHaveLength(1);

        const aborted = harness({
            respond: () => {
                const error = new Error("aborted");
                error.name = "AbortError";
                throw error;
            },
        });
        expect(await runReviewCommand(["list"], aborted.deps)).toBe(1);
        expect(aborted.recorded.stderr).toEqual([
            "Review list failed before a response could be formed (AbortError).",
        ]);
        expect(aborted.recorded.closes).toBe(1);

        const unrenderable = harness();
        unrenderable.deps.stdout = () => {
            throw new Error("EPIPE");
        };
        expect(await runReviewCommand(["list"], unrenderable.deps)).toBe(1);
        expect(unrenderable.recorded.stderr).toEqual([
            "Review list failed before a response could be formed (Error).",
        ]);
        expect(unrenderable.recorded.closes).toBe(1);
    });

    test("the default realpath resolves relative, subdirectory, and symlinked project paths to one real root", async () => {
        const { mkdtempSync, mkdirSync, symlinkSync, realpathSync } = await import("node:fs");
        const { tmpdir } = await import("node:os");
        const { join, relative } = await import("node:path");
        const base = mkdtempSync(join(tmpdir(), "eidnara-review-paths-"));
        const project = join(base, "project");
        mkdirSync(join(project, "src"), { recursive: true });
        symlinkSync(project, join(base, "link"));
        const real = realpathSync.native(project);
        const roots: string[] = [];
        const { deps } = harness();
        deps.realpath = (path) => realpathSync.native(path);
        deps.cwd = () => join(project, "src");
        const connection = await deps.connect("unused");
        const open = connection.routeOpen;
        connection.routeOpen = async (target, identity, options) => {
            roots.push(identity.project_root);
            return open(target, identity, options);
        };
        expect(
            await runReviewCommand(["list", "--project", relative(process.cwd(), project)], deps),
        ).toBe(0);
        expect(await runReviewCommand(["list", "--project", join(base, "link")], deps)).toBe(0);
        expect(await runReviewCommand(["list", "--project", join(base, "link", "src")], deps)).toBe(
            0,
        );
        expect(await runReviewCommand(["list"], deps)).toBe(0);
        expect(roots).toEqual([real, real, join(real, "src"), join(real, "src")]);
    });

    test("an absent connection file, a missing data directory, and a daemon without the context module refuse before any route opens", async () => {
        const absent = harness({
            connect: async () => {
                const error = new Error(`connection file /home/reviewer/${HEX} does not exist`);
                error.name = "ConnectionFileError";
                throw Object.assign(error, { code: "not_found" });
            },
        });
        expect(await runReviewCommand(["status"], absent.deps)).toBe(1);
        expect(absent.recorded.stderr).toEqual([
            "Review status failed: the daemon connection file is absent; start the daemon first.",
        ]);
        const other = harness({
            connect: async () => {
                const error = new Error(`peer text ${HEX}`);
                error.name = "ConnectionFileError";
                throw Object.assign(error, { code: "stat_failed" });
            },
        });
        expect(await runReviewCommand(["status"], other.deps)).toBe(1);
        expect(other.recorded.stderr).toEqual([
            "Review status failed before a response could be formed (ConnectionFileError).",
        ]);

        const missingProject = harness();
        missingProject.deps.realpath = () => {
            throw Object.assign(new Error("ENOENT"), { code: "ENOENT" });
        };
        expect(await runReviewCommand(["list", "--project", "/nope"], missingProject.deps)).toBe(2);
        expect(missingProject.recorded.connected).toEqual([]);
        expect(missingProject.recorded.stderr).toEqual([
            "Review list failed: the project path does not exist.",
        ]);

        const noHome = harness();
        noHome.deps.env = {};
        expect(await runReviewCommand(["list"], noHome.deps)).toBe(1);
        expect(noHome.recorded.connected).toEqual([]);

        const noContext = harness({
            catalog: [
                { module_id: "local_embeddings", module_version: "1", roles: [], control_ops: [] },
            ],
        });
        expect(await runReviewCommand(["list"], noContext.deps)).toBe(1);
        expect(noContext.recorded.routeOpens).toEqual([]);
        expect(noContext.recorded.stdout).toEqual([
            "Review access is unavailable: the daemon serves no context module.",
        ]);
        expect(noContext.recorded.closes).toBe(1);

        const usage = harness();
        expect(await runReviewCommand(["status", "--project", "/p"], usage.deps)).toBe(2);
        expect(usage.recorded.connected).toEqual([]);
    });
});
