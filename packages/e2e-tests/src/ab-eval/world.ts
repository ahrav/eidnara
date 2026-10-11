import { spawnSync } from "node:child_process";
import { mkdirSync, writeFileSync } from "node:fs";
import { dirname, join } from "node:path";

export type FactKind =
    | "decision"
    | "update"
    | "rationale"
    | "constraint"
    | "tool_detail"
    | "multi_hop"
    | "recent_control"
    | "repo_fact"
    | "abstain";

export interface Fact {
    id: string;
    kind: FactKind;
    subject: string;
    answer: string;
    stale?: string;
    statedSession: number;
    statedTurn: number;
    firstSession?: number;
    firstTurn?: number;
}

export type ScriptedStep =
    | { kind: "tool"; tool: "read"; path: string }
    | { kind: "tool"; tool: "bash"; command: string; description: string }
    | { kind: "text"; text: string };

export interface Turn {
    index: number;
    kind: "filler" | "fact" | "probe";
    user: string;
    steps: ScriptedStep[];
    factIds: string[];
    probe?: Probe;
}

export interface Probe {
    factId: string;
    scope: "in_session" | "cross_session" | "control";
}

export interface Session {
    index: number;
    title: string;
    turns: Turn[];
}

export interface World {
    seed: number;
    tier: string;
    files: Record<string, string>;
    facts: Fact[];
    sessions: Session[];
}

export interface TierSpec {
    name: string;
    sessions: number;
    turnsPerSession: number;
    factsPerSession: number;
}

export const TIERS: Record<string, TierSpec> = {
    xs: { name: "xs", sessions: 2, turnsPerSession: 24, factsPerSession: 3 },
    c: { name: "c", sessions: 1, turnsPerSession: 170, factsPerSession: 10 },
    s: { name: "s", sessions: 3, turnsPerSession: 110, factsPerSession: 9 },
    m: { name: "m", sessions: 12, turnsPerSession: 260, factsPerSession: 9 },
    l: { name: "l", sessions: 40, turnsPerSession: 600, factsPerSession: 8 },
};

function rng(seed: number): () => number {
    let a = seed >>> 0;
    return () => {
        a = (a + 0x6d2b79f5) >>> 0;
        let t = a;
        t = Math.imul(t ^ (t >>> 15), t | 1);
        t ^= t + Math.imul(t ^ (t >>> 7), t | 61);
        return ((t ^ (t >>> 14)) >>> 0) / 4294967296;
    };
}

const COMPONENTS = [
    "billing",
    "ledger",
    "auth-gateway",
    "notifier",
    "search-indexer",
    "payouts",
    "reconciler",
    "risk-engine",
    "webhook-relay",
    "export-worker",
    "scheduler",
    "audit-log",
    "rate-limiter",
    "session-store",
    "invoice-renderer",
    "fraud-scorer",
];

const NUMERIC_ATTRS = [
    "HTTP port",
    "retry limit",
    "batch size",
    "request timeout in milliseconds",
    "queue shard count",
    "max connection pool size",
    "cache TTL in seconds",
    "worker concurrency",
    "read replica count",
    "max payload size in kilobytes",
    "circuit breaker error threshold",
    "log retention in days",
    "lease duration in seconds",
    "rebalance interval in seconds",
];

const NAME_ATTRS = [
    "feature flag",
    "release codename",
    "Kafka topic",
    "on-call rotation alias",
    "dead-letter queue name",
    "canary cohort label",
    "primary region alias",
    "metrics namespace",
    "deploy pipeline name",
    "secrets path prefix",
    "alarm channel",
];

const ADJ = [
    "amber",
    "brisk",
    "cobalt",
    "dusky",
    "ember",
    "fallow",
    "gilded",
    "hollow",
    "ivory",
    "jaunty",
    "kelp",
    "lunar",
    "mossy",
    "nimble",
    "ochre",
    "pewter",
    "quiet",
    "russet",
    "sable",
    "tawny",
    "umber",
    "velvet",
    "wistful",
    "xeric",
    "yonder",
    "zephyr",
    "briny",
    "cinder",
    "drowsy",
    "fable",
];
const NOUN = [
    "otter",
    "heron",
    "falcon",
    "marmot",
    "lynx",
    "badger",
    "egret",
    "ibis",
    "jackal",
    "kestrel",
    "lemur",
    "magpie",
    "newt",
    "oriole",
    "puffin",
    "quokka",
    "raven",
    "stoat",
    "tapir",
    "urchin",
    "vole",
    "walrus",
    "yak",
    "zebu",
    "bison",
    "condor",
    "dingo",
    "gecko",
    "hare",
    "koala",
];

const LIBS = [
    "tokio-retry",
    "bullmq",
    "pgbouncer",
    "envoy",
    "nats",
    "redis-streams",
    "zstd",
    "grpc-web",
];
const TESTS = [
    "settles_partial_refund",
    "rejects_expired_token",
    "replays_dead_letters",
    "rounds_fx_amounts",
    "throttles_burst_writes",
    "rotates_signing_keys",
    "dedupes_webhook_retries",
    "paginates_large_exports",
    "reorders_late_events",
    "caps_fanout_batches",
];

const CONSUMERS = [
    "settlement consumer",
    "replay consumer",
    "audit consumer",
    "backfill consumer",
    "fanout consumer",
    "retry consumer",
    "export consumer",
    "alerting consumer",
];

/** Rejection sampling stops after this many draws, so an undersized value space fails loudly. */
const MAX_DRAWS = 100_000;

class Tokens {
    private used = new Set<string>();
    private serials = 0;
    constructor(private readonly r: () => number) {}

    pick<T>(items: readonly T[]): T {
        return items[Math.floor(this.r() * items.length)] as T;
    }

    /**
     * Draws until `keyOf` yields an unclaimed key. Answer tokens and fact subjects share one
     * claim set, so no two facts share an answer or a question.
     */
    unique<T>(make: () => T, keyOf: (value: T) => string): T {
        for (let draw = 0; draw < MAX_DRAWS; draw++) {
            const value = make();
            const key = keyOf(value);
            if (!this.used.has(key)) {
                this.used.add(key);
                return value;
            }
        }
        throw new Error(
            `no unclaimed value after ${MAX_DRAWS} draws; the value space is too small`,
        );
    }

    private fresh(make: () => string): string {
        return this.unique(make, (value) => value);
    }

    /**
     * Filler identifiers are letter-only serials: they never repeat, draw nothing from the
     * generator, and contain no digits, so no answer token appears inside one.
     */
    serial(prefix: string): string {
        let n = ++this.serials;
        let letters = "";
        while (n > 0) {
            n--;
            letters = String.fromCharCode(97 + (n % 26)) + letters;
            n = Math.floor(n / 26);
        }
        return `${prefix}-${letters}`;
    }

    number(): string {
        return this.fresh(() => String(10000 + Math.floor(this.r() * 89999)));
    }

    codename(): string {
        return this.fresh(
            () => `${this.pick(ADJ)}-${this.pick(NOUN)}-${10 + Math.floor(this.r() * 89)}`,
        );
    }

    incident(): string {
        return this.fresh(() => `INC-${10000 + Math.floor(this.r() * 89999)}`);
    }

    errorCode(): string {
        return this.fresh(() => `E-${10000 + Math.floor(this.r() * 89999)}`);
    }
}

const ack = (t: Tokens): string =>
    t.pick(["Noted.", "Got it, noted.", "Understood.", "Okay, recorded.", "Sounds good, noted."]);

function prose(r: () => number, t: Tokens, n: number): string {
    const subjects = [
        "the retry loop",
        "the batch flusher",
        "the cursor scan",
        "the lease renewal",
        "the quota check",
        "the idempotency guard",
        "the backoff helper",
        "the consumer offset commit",
        "the snapshot writer",
        "the checksum pass",
        "the fanout step",
        "the deadline propagation",
        "the connection pool",
    ];
    const verbs = [
        "wraps",
        "short-circuits",
        "retries",
        "batches",
        "defers",
        "logs",
        "rejects",
        "caps",
        "replays",
        "throttles",
        "re-reads",
        "acknowledges",
    ];
    const objects = [
        "stale cursors",
        "duplicate deliveries",
        "partial writes",
        "late events",
        "oversized payloads",
        "expired leases",
        "slow shards",
        "timed-out requests",
        "unacked offsets",
        "burst traffic",
    ];
    const tails = [
        "before it touches the store",
        "on every flush",
        "when the window closes",
        "inside the handler",
        "after the first attempt fails",
        "only on the hot path",
        "behind the feature check",
        "with a fixed jitter",
    ];
    const out: string[] = [];
    for (let i = 0; i < n; i++) {
        const sentence = `${t.pick(subjects)} ${t.pick(verbs)} ${t.pick(objects)} ${t.pick(tails)}`;
        const extra = r() < 0.3 ? `, roughly ${10 + Math.floor(r() * 900)} per batch` : "";
        out.push(`${sentence.replace(/^./, (c) => c.toUpperCase())}${extra}.`);
    }
    return out.join(" ");
}

function codeFile(r: () => number, t: Tokens, comp: string, role: string): string {
    const lines: string[] = [
        `// ${comp}/${role}.ts`,
        `import { Context } from "../shared/context";`,
        "",
    ];
    const fns = 6 + Math.floor(r() * 6);
    for (let i = 0; i < fns; i++) {
        const name = `${t.pick(["handle", "build", "apply", "resolve", "flush", "load", "score"])}${t.pick(["Batch", "Event", "Window", "Lease", "Cursor", "Quota", "Retry"])}${i}`;
        lines.push(`/** ${prose(r, t, 1)} */`);
        lines.push(`export async function ${name}(ctx: Context, input: Record<string, unknown>) {`);
        const body = 4 + Math.floor(r() * 10);
        for (let j = 0; j < body; j++) {
            const lit = 100 + Math.floor(r() * 8000);
            lines.push(
                t.pick([
                    `    const limit${j} = ctx.config.get("${t.pick(["batch", "retry", "window", "quota"])}") ?? ${lit};`,
                    `    if (input.attempt && Number(input.attempt) > ${lit % 17}) return ctx.fail("retry_exhausted");`,
                    `    await ctx.metrics.observe("${comp}.${name}.latency_ms", ${lit});`,
                    `    const cursor${j} = await ctx.store.scan({ from: input.cursor, limit: ${lit % 500} });`,
                    `    ctx.log.debug("${name} step ${j}", { shard: ${lit % 64} });`,
                ]),
            );
        }
        lines.push("    return ctx.ok();", "}", "");
    }
    return lines.join("\n");
}

function configFile(r: () => number, comp: string, defaults: Record<string, string>): string {
    const entries = Object.entries(defaults)
        .map(([k, v]) => `    ${k}: ${v},`)
        .join("\n");
    return `// ${comp}/config.ts\nexport const defaults = {\n${entries}\n    log_level: "${r() < 0.5 ? "info" : "warn"}",\n};\n`;
}

function testLog(
    r: () => number,
    t: Tokens,
    comp: string,
    failure?: { test: string; code: string },
): { log: string; failed: number; passed: number } {
    const lines: string[] = [
        `$ ./scripts/test.sh ${comp}`,
        `running ${comp} suite (seed ${t.serial("s")})`,
    ];
    const n = 40 + Math.floor(r() * 50);
    for (let i = 0; i < n; i++) {
        const name = `${comp.replace(/-/g, "_")}::${t.pick(TESTS)}_${i}`;
        lines.push(`test ${name} ... ok (${1 + Math.floor(r() * 900)}ms)`);
    }
    let failed = 0;
    if (failure) {
        failed = 1;
        lines.push(`test ${comp.replace(/-/g, "_")}::${failure.test} ... FAILED`);
        lines.push("", "failures:", `---- ${failure.test} stdout ----`);
        lines.push(`thread 'main' panicked: assertion failed with error code ${failure.code}`);
        lines.push(
            `  left: ${100 + Math.floor(r() * 900)}`,
            `  right: ${100 + Math.floor(r() * 900)}`,
        );
    }
    lines.push("", `test result: ${failed ? "FAILED" : "ok"}. ${n} passed; ${failed} failed`);
    return { log: lines.join("\n"), failed, passed: n };
}

function serviceLog(r: () => number, t: Tokens, comp: string): string {
    const lines: string[] = [`$ ./scripts/logs.sh ${comp} --tail 60`];
    for (let i = 0; i < 60; i++) {
        const lvl = r() < 0.08 ? "WARN" : r() < 0.03 ? "ERROR" : "INFO";
        lines.push(
            `2026-09-${String(1 + Math.floor(r() * 28)).padStart(2, "0")}T${String(Math.floor(r() * 24)).padStart(2, "0")}:${String(Math.floor(r() * 60)).padStart(2, "0")}:11Z ${lvl} ${comp} req=${t.serial("r")} ${prose(r, t, 1)}`,
        );
    }
    return lines.join("\n");
}

interface PlannedFact {
    fact: Fact;
    statement?: string;
    question: string;
}

/** Component counts for `--project-size`; each component contributes four source files. */
export const PROJECT_SIZES: Record<string, number> = { small: 16, medium: 64, large: 256 };

/** `count` component names: the base list, then suffixed variants of it. */
function componentNames(count: number): string[] {
    const names: string[] = [];
    for (let round = 0; names.length < count; round++) {
        for (const base of COMPONENTS) {
            if (names.length >= count) break;
            names.push(round === 0 ? base : `${base}-${round + 1}`);
        }
    }
    return names;
}

export function buildWorld(seed: number, tierName: string, componentCount = 16): World {
    // Constraint facts pair two distinct components, so the list needs at least two.
    if (!Number.isInteger(componentCount) || componentCount < 2) {
        throw new Error(`componentCount must be an integer of at least 2, got ${componentCount}`);
    }
    const components = componentNames(componentCount);
    const tier = TIERS[tierName];
    if (!tier) throw new Error(`unknown tier ${tierName}`);
    const r = rng(seed);
    const t = new Tokens(r);
    const files: Record<string, string> = {};
    const repoFacts: { comp: string; key: string; value: string }[] = [];

    for (const comp of components) {
        for (const role of ["handler", "client", "store"]) {
            files[`src/${comp}/${role}.ts`] = codeFile(r, t, comp, role);
        }
        const key = t.pick(["drain_interval_ms", "max_inflight", "heartbeat_ms", "page_size"]);
        const value = t.number();
        repoFacts.push({ comp, key, value });
        files[`src/${comp}/config.ts`] = configFile(r, comp, {
            [key]: value,
            jitter_ms: String(10 + Math.floor(r() * 400)),
        });
    }
    files["src/shared/context.ts"] =
        "export interface Context { config: Map<string, number>; store: any; metrics: any; log: any; ok(): unknown; fail(code: string): unknown }\n";
    files["README.md"] = `# Halcyon payments platform\n\n${prose(r, t, 12)}\n`;
    files["scripts/test.sh"] = '#!/bin/sh\nf=".runs/test-$1-$3.log"\ncat "$f" && rm -f "$f"\n';
    files["scripts/logs.sh"] = '#!/bin/sh\nf=".runs/logs-$1-$5.log"\ncat "$f" && rm -f "$f"\n';

    const facts: Fact[] = [];
    const sessions: Session[] = [];
    const subject = (attrs: readonly string[]): { comp: string; attr: string } =>
        t.unique(
            () => ({ comp: t.pick(components), attr: t.pick(attrs) }),
            ({ comp, attr }) => `subject:${comp} ${attr}`,
        );
    let factSeq = 0;
    const nextId = (kind: FactKind) => `${kind}-${String(++factSeq).padStart(3, "0")}`;
    let runSeq = 0;

    const owedCross: PlannedFact[] = [];
    const pendingUpdates: { fact: Fact; comp: string; attr: string; v1: string }[] = [];
    const pendingHops: { comp: string; queue: string; firstSession: number }[] = [];

    const totalSessions = tier.sessions + 1;
    for (let s = 0; s < totalSessions; s++) {
        const finalProbeSession = s === tier.sessions;
        const turns: Turn[] = [];
        const turnCount = finalProbeSession ? 0 : tier.turnsPerSession;
        const planned: { at: number; make: (index: number) => Turn }[] = [];
        const inSessionProbes: PlannedFact[] = [];

        if (!finalProbeSession) {
            const span = Math.floor(turnCount * 0.7);
            const slot = () => 2 + Math.floor(r() * Math.max(1, span - 2));
            const ownFacts: PlannedFact[] = [];
            for (let f = 0; f < tier.factsPerSession; f++) {
                const kind = t.pick([
                    "decision",
                    "decision",
                    "rationale",
                    "constraint",
                    "tool_detail",
                ] as const);
                ownFacts.push(makeFact(kind, s));
            }
            if (s < tier.sessions - 1) {
                const { comp, attr } = subject(NUMERIC_ATTRS);
                const v1 = t.number();
                const fact: Fact = {
                    id: nextId("update"),
                    kind: "update",
                    subject: `${comp} ${attr}`,
                    answer: "",
                    stale: v1,
                    statedSession: -1,
                    statedTurn: -1,
                    firstSession: s,
                    firstTurn: -1,
                };
                planned.push({
                    at: slot(),
                    make: (index) => ({
                        index,
                        kind: "fact",
                        user: `Decision for the record: the ${comp} ${attr} is ${v1}.`,
                        steps: [{ kind: "text", text: ack(t) }],
                        factIds: [fact.id],
                    }),
                });
                pendingUpdates.push({ fact, comp, attr, v1 });
            }
            for (const pending of pendingUpdates.filter((u) => u.fact.firstSession === s - 1)) {
                const v2 = t.number();
                pending.fact.answer = v2;
                pending.fact.statedSession = s;
                facts.push(pending.fact);
                const signalled = r() < 0.5;
                const pf: PlannedFact = {
                    fact: pending.fact,
                    statement: signalled
                        ? `Change of plan: set the ${pending.comp} ${pending.attr} to ${v2}; ${pending.v1} turned out to be wrong.`
                        : `Set the ${pending.comp} ${pending.attr} to ${v2}.`,
                    question: `what's the ${pending.comp} ${pending.attr} now?`,
                };
                planned.push({ at: slot(), make: (index) => factTurn(index, pf) });
                (r() < 0.5 ? inSessionProbes : owedCross).push(pf);
            }
            if (s < tier.sessions - 1) {
                const comp = t.unique(
                    () => `${t.pick(components)} ${t.pick(CONSUMERS)}`,
                    (consumer) => `hop:${consumer}`,
                );
                const queue = t.codename();
                pendingHops.push({ comp, queue, firstSession: s });
                planned.push({
                    at: slot(),
                    make: (index) => ({
                        index,
                        kind: "fact",
                        user: `FYI the ${comp} now consumes from the ${queue} queue.`,
                        steps: [{ kind: "text", text: ack(t) }],
                        factIds: [`hop:${queue}`],
                    }),
                });
            }
            for (const hop of pendingHops.filter((h) => h.firstSession === s - 1)) {
                const shards = t.number();
                const fact: Fact = {
                    id: nextId("multi_hop"),
                    kind: "multi_hop",
                    subject: `${hop.comp} via ${hop.queue}`,
                    answer: shards,
                    statedSession: s,
                    statedTurn: -1,
                    firstSession: hop.firstSession,
                    firstTurn: -1,
                };
                facts.push(fact);
                const pf: PlannedFact = {
                    fact,
                    statement: `The ${hop.queue} queue is getting ${shards} shards starting this week.`,
                    question: `how many shards does the queue that the ${hop.comp} consumes from have?`,
                };
                planned.push({ at: slot(), make: (index) => factTurn(index, pf) });
                owedCross.push(pf);
            }
            for (const pf of ownFacts) {
                facts.push(pf.fact);
                planned.push({
                    at: slot(),
                    make: (index) =>
                        pf.fact.kind === "tool_detail"
                            ? toolDetailTurn(index, pf)
                            : factTurn(index, pf),
                });
                (r() < 0.55 ? inSessionProbes : owedCross).push(pf);
            }
            const control = makeFact("recent_control", s);
            facts.push(control.fact);

            planned.sort((a, b) => a.at - b.at);
            const workTurns = turnCount - inSessionProbes.length - 1;
            const slots = workTurns - 4;
            let p = 0;
            // Each slot takes the next planned turn once its time has come, or once the slots
            // left equal the plans left, so every plan lands inside the session's budget.
            for (let i = 0; turns.length < slots; i++) {
                const next = planned[p];
                if (next && (next.at <= i || planned.length - p >= slots - turns.length)) {
                    turns.push(next.make(turns.length));
                    p++;
                } else {
                    turns.push(fillerTurn(turns.length));
                }
            }
            turns.push(factTurn(turns.length, control));
            for (let k = 0; k < 3; k++) turns.push(fillerTurn(turns.length));
            for (const pf of shuffle(r, [...inSessionProbes])) {
                turns.push(probeTurn(turns.length, pf, "in_session"));
            }
            turns.push(probeTurn(turns.length, control, "control"));
        }
        if (s > 0) {
            const due = owedCross.filter((pf) => pf.fact.statedSession < s);
            const take = finalProbeSession ? due : due.filter(() => r() < 0.35);
            for (const pf of take) owedCross.splice(owedCross.indexOf(pf), 1);
            const opening = take.map((pf, i) => probeTurn(i, pf, "cross_session"));
            if (finalProbeSession) {
                for (let a = 0; a < Math.max(3, Math.round(tier.sessions * 1.5)); a++) {
                    const abstain = makeFact("abstain", s);
                    facts.push(abstain.fact);
                    opening.push(probeTurn(opening.length, abstain, "cross_session"));
                }
                for (const rf of repoFacts.slice(0, 3)) {
                    const fact: Fact = {
                        id: nextId("repo_fact"),
                        kind: "repo_fact",
                        subject: `${rf.comp} ${rf.key}`,
                        answer: rf.value,
                        statedSession: -1,
                        statedTurn: -1,
                    };
                    facts.push(fact);
                    opening.push(
                        probeTurn(
                            opening.length,
                            {
                                fact,
                                question: `what is the default ${rf.key} in src/${rf.comp}/config.ts?`,
                            },
                            "cross_session",
                        ),
                    );
                }
            }
            turns.unshift(...shuffle(r, opening));
            turns.forEach((turn, i) => {
                turn.index = i;
            });
        }
        sessions.push({
            index: s,
            title: finalProbeSession ? "probe session" : `work session ${s + 1}`,
            turns,
        });
    }
    restateTurnIndexes(sessions, facts);
    return { seed, tier: tierName, files, facts, sessions };

    function fillerTurn(index: number): Turn {
        const comp = t.pick(components);
        const roll = r();
        if (roll < 0.4) {
            const role = t.pick(["handler", "client", "store", "config"]);
            const path = `src/${comp}/${role}.ts`;
            return {
                index,
                kind: "filler",
                user: t.pick([
                    `Open ${path} and walk me through what it does.`,
                    `Can you read ${path} and check whether the retry handling looks right?`,
                    `Take a look at ${path}; I want to know where the batching happens.`,
                    `Read ${path} and summarize the main functions for me.`,
                ]),
                steps: [
                    { kind: "tool", tool: "read", path },
                    { kind: "text", text: `Here's the gist of ${path}: ${prose(r, t, 4)}` },
                ],
                factIds: [],
            };
        }
        if (roll < 0.62) {
            const id = ++runSeq;
            const run = testLog(r, t, comp);
            files[`.runs/test-${comp}-${id}.log`] = run.log;
            return {
                index,
                kind: "filler",
                user: t.pick([
                    `Run the ${comp} tests.`,
                    `Kick off the ${comp} test suite and tell me how it looks.`,
                    `Let's make sure ${comp} is still green, run its tests.`,
                ]),
                steps: [
                    {
                        kind: "tool",
                        tool: "bash",
                        command: `./scripts/test.sh ${comp} --run ${id}`,
                        description: `Run the ${comp} test suite`,
                    },
                    {
                        kind: "text",
                        text: `All ${run.passed} ${comp} tests passed. ${prose(r, t, 1)}`,
                    },
                ],
                factIds: [],
            };
        }
        if (roll < 0.8) {
            const id = ++runSeq;
            files[`.runs/logs-${comp}-${id}.log`] = serviceLog(r, t, comp);
            return {
                index,
                kind: "filler",
                user: t.pick([
                    `Check the recent ${comp} logs for anything odd.`,
                    `Pull the last few minutes of ${comp} logs.`,
                    `Anything weird in the ${comp} logs?`,
                ]),
                steps: [
                    {
                        kind: "tool",
                        tool: "bash",
                        command: `./scripts/logs.sh ${comp} --tail 60 --run ${id}`,
                        description: `Tail the ${comp} logs`,
                    },
                    {
                        kind: "text",
                        text: `The ${comp} logs look mostly healthy. ${prose(r, t, 2)}`,
                    },
                ],
                factIds: [],
            };
        }
        return {
            index,
            kind: "filler",
            user: `Thinking about ${comp} next: ${prose(r, t, 3)} What do you think?`,
            steps: [{ kind: "text", text: prose(r, t, 5) }],
            factIds: [],
        };
    }

    function factTurn(index: number, pf: PlannedFact): Turn {
        return {
            index,
            kind: "fact",
            user: pf.statement as string,
            steps: [{ kind: "text", text: ack(t) }],
            factIds: [pf.fact.id],
        };
    }

    function makeFact(kind: FactKind, s: number): PlannedFact {
        const id = nextId(kind);
        if (kind === "decision" || kind === "recent_control") {
            const numeric = r() < 0.5;
            const { comp, attr } = subject(numeric ? NUMERIC_ATTRS : NAME_ATTRS);
            const value = numeric ? t.number() : t.codename();
            const fact: Fact = {
                id,
                kind,
                subject: `${comp} ${attr}`,
                answer: value,
                statedSession: s,
                statedTurn: -1,
            };
            return {
                fact,
                statement: t.pick([
                    `Decision for the record: the ${comp} ${attr} is ${value}.`,
                    `Let's lock it in: ${comp} ${attr} = ${value}.`,
                    `After the review we agreed the ${comp} ${attr} will be ${value}.`,
                    `Note this down, the ${comp} ${attr} should be ${value} going forward.`,
                ]),
                question: `what did we settle on for the ${comp} ${attr}?`,
            };
        }
        if (kind === "rationale") {
            const { comp, lib } = t.unique(
                () => ({ comp: t.pick(components), lib: t.pick(LIBS) }),
                ({ comp, lib }) => `rationale:${comp} ${lib}`,
            );
            const incident = t.incident();
            const fact: Fact = {
                id,
                kind,
                subject: `${comp} uses ${lib}`,
                answer: incident,
                statedSession: s,
                statedTurn: -1,
            };
            return {
                fact,
                statement: `We're moving ${comp} onto ${lib}, mainly because of the outage in ${incident}; I don't want a repeat of that.`,
                question: `which incident was the reason we moved ${comp} onto ${lib}? Give the incident ID.`,
            };
        }
        if (kind === "constraint") {
            const { comp, other } = t.unique(
                () => {
                    const comp = t.pick(components);
                    return { comp, other: t.pick(components.filter((c) => c !== comp)) };
                },
                ({ comp, other }) => `constraint:${comp} ${other}`,
            );
            const proxy = t.codename();
            const fact: Fact = {
                id,
                kind,
                subject: `${comp} -> ${other}`,
                answer: proxy,
                statedSession: s,
                statedTurn: -1,
            };
            return {
                fact,
                statement: `Hard rule for this repo: ${comp} must never call ${other} directly; it always goes through the ${proxy} proxy.`,
                question: `if ${comp} needs data from ${other}, what is it required to go through?`,
            };
        }
        if (kind === "tool_detail") {
            const { comp, test } = t.unique(
                () => ({
                    comp: t.pick(components),
                    test: `${t.pick(TESTS)}_${Math.floor(r() * 900)}`,
                }),
                ({ comp, test }) => `tool:${comp} ${test}`,
            );
            const code = t.errorCode();
            const fact: Fact = {
                id,
                kind,
                subject: `${comp} ${test}`,
                answer: code,
                statedSession: s,
                statedTurn: -1,
            };
            return {
                fact,
                question: `what error code did the ${test} test fail with when we ran the ${comp} tests?`,
            };
        }
        const { comp, attr } = subject(r() < 0.5 ? NUMERIC_ATTRS : NAME_ATTRS);
        const fact: Fact = {
            id,
            kind: "abstain",
            subject: `${comp} ${attr}`,
            answer: "UNKNOWN",
            statedSession: -1,
            statedTurn: -1,
        };
        return { fact, question: `what did we settle on for the ${comp} ${attr}?` };
    }

    function toolDetailTurn(index: number, pf: PlannedFact): Turn {
        const [comp, test] = pf.fact.subject.split(" ") as [string, string];
        const id = ++runSeq;
        const run = testLog(r, t, comp, { test, code: pf.fact.answer });
        files[`.runs/test-${comp}-${id}.log`] = run.log;
        return {
            index,
            kind: "fact",
            user: `Run the ${comp} tests.`,
            steps: [
                {
                    kind: "tool",
                    tool: "bash",
                    command: `./scripts/test.sh ${comp} --run ${id}`,
                    description: `Run the ${comp} test suite`,
                },
                {
                    kind: "text",
                    text: `${run.passed} passed, 1 failed: ${test} panicked on an assertion. It looks flaky; I'd re-run it before digging in.`,
                },
            ],
            factIds: [pf.fact.id],
        };
    }
}

function restateTurnIndexes(sessions: Session[], facts: Fact[]): void {
    const byId = new Map(facts.map((f) => [f.id, f]));
    const hopFirst = new Map<string, { session: number; turn: number }>();
    for (const session of sessions) {
        for (const turn of session.turns) {
            if (turn.kind !== "fact") continue;
            for (const id of turn.factIds) {
                if (id.startsWith("hop:")) {
                    hopFirst.set(id.slice(4), { session: session.index, turn: turn.index });
                    continue;
                }
                const fact = byId.get(id);
                if (!fact) continue;
                if (fact.kind === "update" && session.index === fact.firstSession) {
                    fact.firstTurn = turn.index;
                } else {
                    fact.statedTurn = turn.index;
                }
            }
        }
    }
    for (const fact of facts) {
        if (fact.kind !== "multi_hop") continue;
        const first = hopFirst.get(fact.subject.split(" via ")[1] as string);
        if (first) {
            fact.firstSession = first.session;
            fact.firstTurn = first.turn;
        }
    }
}

function probeTurn(index: number, pf: PlannedFact, scope: Probe["scope"]): Turn {
    return {
        index,
        kind: "probe",
        user: `Quick question about this project: ${pf.question} Answer with just the value (or UNKNOWN if we never settled it).`,
        steps: [],
        factIds: [pf.fact.id],
        probe: { factId: pf.fact.id, scope },
    };
}

function shuffle<T>(r: () => number, items: T[]): T[] {
    for (let i = items.length - 1; i > 0; i--) {
        const j = Math.floor(r() * (i + 1));
        [items[i], items[j]] = [items[j] as T, items[i] as T];
    }
    return items;
}

/**
 * Turns the written repository into a Git repository with one commit, as the arms' agents expect.
 * The commit runs with signing off and hooks disabled, so the evaluator's own Git configuration
 * leaves the harness-owned repository alone.
 */
export function initRepo(dir: string): void {
    const init = spawnSync(
        "sh",
        [
            "-c",
            [
                "git init -q .",
                "git add -A",
                "git -c user.email=ab@eval -c user.name=ab -c commit.gpgsign=false -c core.hooksPath=/dev/null commit --no-gpg-sign -qm init",
            ].join(" && "),
        ],
        { cwd: dir, encoding: "utf8" },
    );
    if (init.status !== 0) {
        throw new Error(
            `git setup of ${dir} failed (${init.status ?? init.error?.message}): ${init.stderr.trim()}`,
        );
    }
}

export function writeRepo(world: World, dir: string): void {
    for (const [path, content] of Object.entries(world.files)) {
        const full = join(dir, path);
        mkdirSync(dirname(full), { recursive: true });
        writeFileSync(full, content, { mode: path.startsWith("scripts/") ? 0o755 : 0o644 });
    }
}

export type Grade = "correct" | "stale" | "abstained" | "wrong" | "failed";

/** A probe whose turn ended in an error has no answer to grade, whatever text arrived first. */
export function gradeOutcome(fact: Fact, result: { answer: string; error?: string }): Grade {
    return result.error ? "failed" : grade(fact, result.answer);
}

const ABSTAIN =
    /\bUNKNOWN\b|\b(don'?t|do not) (know|have|see)\b|\bnever (settled|decided|discussed|set|mentioned)\b|\bnot (sure|aware)\b|\bno record\b|\bcan(no|')t find\b/i;

/** `token` as a whole word: letters, digits, and hyphens on either side would make it part of a larger token. */
function standalone(token: string): RegExp {
    const escaped = token.replace(/[.*+?^${}()|[\]\\]/g, "\\$&");
    return new RegExp(`(^|(?<=[^A-Za-z0-9-]))${escaped}(?=$|[^A-Za-z0-9-])`, "gi");
}

function carries(text: string, token: string): boolean {
    return standalone(token).test(text);
}

/** The shapes the world's answers take: five-digit numbers and `adjective-noun-NN` codenames. */
const CANDIDATE_VALUE = /\b\d{5}\b|\b[a-z]+-[a-z]+-\d{2}\b/i;

export function grade(fact: Fact, answer: string): Grade {
    // An abstention beside a guessed value is a guess; the question's own subject words, which
    // a larger project's component names can shape like codenames, are set aside first.
    if (fact.kind === "abstain") {
        const rest = fact.subject
            .split(/\s+/)
            .filter((word) => word.length > 0)
            .reduce((text, word) => text.replace(standalone(word), " "), answer);
        return ABSTAIN.test(answer) && !CANDIDATE_VALUE.test(rest) ? "correct" : "wrong";
    }
    if (carries(answer, fact.answer)) return "correct";
    if (fact.stale && carries(answer, fact.stale)) return "stale";
    if (ABSTAIN.test(answer)) return "abstained";
    return "wrong";
}
