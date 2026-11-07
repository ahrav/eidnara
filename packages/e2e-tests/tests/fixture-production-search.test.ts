import { afterAll, beforeAll, describe, expect, it } from "bun:test";
import { join } from "node:path";
import { disposeAll } from "../src/harness-primitives";
import { RustTestHarness } from "../src/rust-harness";
import { buildDirectHostFixture, HermeticHostStack } from "../src/rust-runner/hermetic-host";
import {
    missingPayloadFiles,
    PAYLOAD_DIR_ENV,
    waitForSearchAdmitted,
} from "../src/rust-runner/payload-host";
import { rustPrereqs } from "../src/rust-scenario-support";
import { runScriptedToolCall } from "../src/scripted-tool-call";

const PAYLOAD_DIR = process.env[PAYLOAD_DIR_ENV];
const SUITE_TIMEOUT_MS = 1_500_000;
const ADMISSION_TIMEOUT_MS = 300_000;
const CATCH_UP_TIMEOUT_MS = 300_000;
const FALLBACK = "results come from the legacy snapshot scan";
const ROUTE_LINE = "direct_host_fixture: search route production";

/**
 * Project memories a coding agent keeps for one web service. Some of them answer a question in
 * `QUESTIONS`, which a developer asks later in other words; the rest compete as distractors.
 */
const MEMORIES: readonly string[] = [
    "Integration tests need a running Postgres; start it with `docker compose up db` before `make test`.",
    "The API rate limiter allows 100 requests per minute per token and returns 429 with a Retry-After header.",
    "Feature branches are squash-merged into main after review.",
    "Logs are shipped as JSON lines; the request_id field correlates one request across services.",
    "The frontend build uses Vite; production bundles go to dist/ and nginx serves them.",
    "Database migrations live in migrations/ and run with `make migrate`; each migration must be reversible.",
    "Secrets are loaded from environment variables at startup; the .env file is for local development and is gitignored.",
    "The user prefers small focused commits with imperative commit subjects.",
    "Background jobs run on a Redis-backed queue; a failed job retries three times with exponential backoff.",
    "Timestamps are stored in UTC in the database and converted to the viewer's time zone in the UI.",
    "The CLI reads its configuration from ~/.config/tool/config.toml and falls back to built-in defaults.",
    "Images uploaded by users are resized to at most 2048 pixels on the long edge before storage.",
    "Public API changes require a version bump and an entry in CHANGELOG.md.",
    "The payment webhook handler is idempotent because the provider redelivers events.",
    "Code formatting is enforced by prettier in a pre-commit hook, and CI fails on unformatted files.",
    "Session cookies expire after 30 minutes of inactivity.",
    "A cron job rebuilds the search index every night at 02:00.",
    "Error responses use the problem+json format with type, title, and detail fields.",
    "The mobile app calls the same REST API under the /v2 prefix.",
    "Load tests run with k6 against the staging environment.",
    "The project uses pnpm; lockfile changes are committed with the change that caused them.",
    "Health checks hit /healthz, which reports database and cache connectivity.",
    "The user asked to discuss any new runtime dependency before adding it.",
    "Large CSV exports are generated asynchronously and emailed as a download link.",
    "Unit tests use an in-memory SQLite database; integration tests use Postgres.",
    "The deploy pipeline runs on merge to main and promotes to production after a manual approval.",
    "Translations live in locales/*.json with keys in alphabetical order.",
    "Passwords are hashed with argon2id; bcrypt hashes from the old system are upgraded at the next login.",
    "Feature flags are read from LaunchDarkly at startup and refreshed every 60 seconds.",
    "Shared types live in packages/types and import only from other packages.",
    "Profiling traced the slow dashboard to its account query, which needs a composite index on (account_id, created_at).",
    "The reverse proxy caps file uploads at 25 MB.",
    "The on-call runbook in docs/runbook.md lists the steps for restarting workers.",
    "TypeScript strict mode is on, and new code declares concrete types in place of `any`.",
    "The user wants explanations kept short and to the point.",
    "Email goes out through the notifications service, which alone talks to the SMTP provider.",
    "The GraphQL schema is generated from code; run `npm run codegen` after changing resolvers.",
    "Product listings are cached for 5 minutes and invalidated when a product is edited.",
    "CI runs on GitHub Actions; the end-to-end browser suite is the slowest job at about 12 minutes.",
    "Docker images build from a distroless base to keep the attack surface small.",
    "The staging database is refreshed from an anonymized production snapshot every Monday.",
    "Test fixtures live in tests/fixtures and are loaded by name.",
    "The admin panel is reachable only over the VPN.",
    "Request handlers finish within 10 seconds; longer work moves to a background job.",
    "Node is pinned to the version in .nvmrc.",
    "Analytics events are batched and flushed every 5 seconds.",
    "Pull requests need one approving review and green CI before merge.",
    "The image CDN serves files from the uploads bucket with a one day cache.",
    "Search results are paginated with a cursor, 20 items per page.",
    "Go code is formatted with gofmt and linted with golangci-lint.",
    "The database connection pool holds 20 connections per instance.",
    "API tokens are scoped per project and can be revoked from the settings page.",
    "Release notes are drafted from merged pull request titles.",
    "Third-party scripts load under a strict content security policy.",
    "The WORKERS environment variable sets the worker process count.",
    "Audit logs older than 90 days are archived to cold storage.",
    "The design system components live in packages/ui.",
    "Outbound HTTP calls retry with jittered backoff capped at 30 seconds.",
    "The local dev server runs on port 3000 and proxies API calls to port 8080.",
    "Snapshot tests are updated deliberately with `--update-snapshots`.",
];

/** Each question and the index in `MEMORIES` of the memory that answers it. */
const QUESTIONS: readonly (readonly [string, number])[] = [
    ["what do I need running before the test suite works", 0],
    ["how many calls can a client make before it gets throttled", 1],
    ["how do we get changes into the main branch", 2],
    ["how to trace a single request through the logs of several services", 3],
    ["where are schema changes kept and how are they applied", 5],
    ["how does the app get its credentials and API keys", 6],
    ["what happens when an async task fails", 8],
    ["which time zone are dates saved in", 9],
    ["do we shrink pictures people upload", 11],
    ["the billing provider sends the same notification twice, is that handled", 13],
    ["how long until an idle user gets logged out", 15],
    ["when does the full-text index get regenerated", 16],
    ["what shape should an error body have", 17],
    ["where do we run performance stress tests", 19],
    ["which package manager does the project use", 20],
    ["what endpoint does the liveness probe call", 21],
    ["how do users get big spreadsheet downloads", 23],
    ["how does code reach the live environment", 25],
    ["how are user credentials stored securely", 27],
    ["why is the dashboard slow and how do we fix it", 30],
    ["what is the biggest file someone can upload", 31],
    ["are loose types allowed in the TS code", 33],
    ["how long are product pages cached", 37],
    ["what base image do the containers use", 39],
];

const SEARCH_LIMIT = 10;

function createdId(resultText: string): string {
    const id = /"objectId":"(mem_[0-9a-f]{32})"/.exec(resultText)?.[1];
    if (!id) throw new Error(`memory create returned no object: ${resultText}`);
    return id;
}

/** The memory ids a rendered search result lists, in rank order. */
function rankedIds(result: string): string[] {
    return [...result.matchAll(/^\[\d+\] \[memory\] .* id=(mem_[0-9a-f]{32}) /gm)].map(
        (match) => match[1] as string,
    );
}

function headerFor(result: string, objectId: string): string | undefined {
    return result.split("\n").find((line) => line.includes(` id=${objectId} `));
}

interface Arm {
    harness: RustTestHarness<HermeticHostStack>;
    session: string;
    ids: string[];
}

async function search(arm: Arm, query: string): Promise<string> {
    return (
        await runScriptedToolCall(arm.harness, arm.session, {
            tool: "eidnara_search",
            input: { query, sources: ["memory"], limit: SEARCH_LIMIT },
            prompt: "Search project memory.",
        })
    ).resultText;
}

async function seedArm(harness: RustTestHarness<HermeticHostStack>): Promise<Arm> {
    const session = await harness.createSession();
    const ids: string[] = [];
    for (const content of MEMORIES) {
        const created = await runScriptedToolCall(harness, session, {
            tool: "eidnara_memory",
            input: { action: "create", content, category: "PROJECT_RULES" },
            prompt: "Record this project fact.",
        });
        ids.push(createdId(created.resultText));
    }
    return { harness, session, ids };
}

/** Searches the opening words of each memory until the route ranks it, so every memory competes in the recall run. */
async function waitForProjection(arm: Arm): Promise<void> {
    const deadline = Date.now() + CATCH_UP_TIMEOUT_MS;
    for (const [index, content] of MEMORIES.entries()) {
        const id = arm.ids[index] as string;
        const opening = content.split(/\s+/).slice(0, 8).join(" ");
        let result = await search(arm, opening);
        while (!headerFor(result, id)?.includes("match=fused") && Date.now() < deadline) {
            await Bun.sleep(1_000);
            result = await search(arm, opening);
        }
        if (!headerFor(result, id)?.includes("match=fused")) {
            throw new Error(
                `memory ${index} never ranked through the route:\n${result}\n${arm.harness.host.hostLog()}`,
            );
        }
    }
}

/** The share of questions whose target ranks within the first `k` results. */
function recallAt(ranks: readonly (number | null)[], k: number): number {
    return ranks.filter((rank) => rank !== null && rank <= k).length / ranks.length;
}

describe.skipIf(!PAYLOAD_DIR)("direct-host fixture with the production search route", () => {
    let legacy: Arm | undefined;
    let production: Arm | undefined;
    // Each harness is held from the moment it starts, so a later setup failure still disposes it.
    const started: RustTestHarness<HermeticHostStack>[] = [];

    beforeAll(async () => {
        const payload = PAYLOAD_DIR as string;
        expect(missingPayloadFiles(payload), `${PAYLOAD_DIR_ENV}=${payload}`).toEqual([]);
        expect(rustPrereqs.skipReason ?? "present").toBe("present");
        const fixtureBin = await buildDirectHostFixture();
        const productionHarness = await RustTestHarness.createWithHost({}, async (env) =>
            HermeticHostStack.start({
                dataDir: env.dataDir,
                fixtureBin,
                productionSearchManifest: join(payload, "payload-manifest.json"),
            }),
        );
        started.push(productionHarness);
        const legacyHarness = await RustTestHarness.create();
        started.push(legacyHarness);
        production = await seedArm(productionHarness);
        legacy = await seedArm(legacyHarness);
        await waitForSearchAdmitted(
            productionHarness.host,
            productionHarness.env.workdir,
            production.session,
            ADMISSION_TIMEOUT_MS,
        );
        await waitForProjection(production);
    }, SUITE_TIMEOUT_MS);

    afterAll(async () => {
        await disposeAll(started);
    }, 120_000);

    it(
        "serves fused-route results and names the route at startup, while the default fixture falls back",
        async () => {
            const routedArm = production as Arm;
            const legacyArm = legacy as Arm;
            const productionLog = routedArm.harness.host.logFile();
            const legacyLog = legacyArm.harness.host.logFile();
            expect(productionLog).toContain(
                `${ROUTE_LINE}, embedding model gte-modernbert-base-f32, payload `,
            );
            expect(legacyLog).toContain("daemon:");
            expect(legacyLog).not.toContain(ROUTE_LINE);

            const [question] = QUESTIONS[0] as readonly [string, number];
            const routed = await search(routedArm, question);
            expect(routed).not.toContain(FALLBACK);
            const ranked = rankedIds(routed);
            expect(ranked.length).toBeGreaterThan(0);
            for (const id of ranked) {
                expect(headerFor(routed, id)).toMatch(/ match=fused lanes=[a-z,]+ /);
            }

            const fallback = await search(legacyArm, question);
            expect(fallback).toContain(
                "Memory: the fused search route is unavailable (disabled); results come from the legacy snapshot scan.",
            );
        },
        SUITE_TIMEOUT_MS,
    );

    it(
        "reports paraphrase recall@5 and recall@10 for the legacy scan and the production route",
        async () => {
            const arms = { legacy: legacy as Arm, production: production as Arm };
            const ranks = { legacy: [] as (number | null)[], production: [] as (number | null)[] };
            // The embedding lane ranks rows for paraphrases; a route serving lexical and exact lanes alone would leave this at zero.
            let denseRanked = 0;
            for (const [question, target] of QUESTIONS) {
                for (const name of ["legacy", "production"] as const) {
                    const arm = arms[name];
                    const result = await search(arm, question);
                    expect(result.includes(FALLBACK)).toBe(name === "legacy");
                    const rank = rankedIds(result).indexOf(arm.ids[target] as string);
                    ranks[name].push(rank < 0 ? null : rank + 1);
                    if (name === "production" && / match=fused lanes=[a-z,]*dense/.test(result)) {
                        denseRanked += 1;
                    }
                }
            }
            const report = {
                memories: MEMORIES.length,
                questions: QUESTIONS.length,
                legacy: {
                    recall_at_5: recallAt(ranks.legacy, 5),
                    recall_at_10: recallAt(ranks.legacy, 10),
                },
                production: {
                    recall_at_5: recallAt(ranks.production, 5),
                    recall_at_10: recallAt(ranks.production, 10),
                },
                ranks,
            };
            console.log(`[paraphrase-recall] ${JSON.stringify(report)}`);
            expect(report.production.recall_at_10).toBeGreaterThan(report.legacy.recall_at_10);
            expect(report.production.recall_at_5).toBeGreaterThan(report.legacy.recall_at_5);
            expect(denseRanked).toBeGreaterThan(0);
        },
        SUITE_TIMEOUT_MS,
    );
});

describe.skipIf(Boolean(PAYLOAD_DIR))(
    "direct-host fixture with the production search route skip visibility",
    () => {
        it("prints why it skips", () => {
            console.log(
                `[fixture-production-search] SKIPPED: ${PAYLOAD_DIR_ENV} names no built payload package`,
            );
        });
    },
);
