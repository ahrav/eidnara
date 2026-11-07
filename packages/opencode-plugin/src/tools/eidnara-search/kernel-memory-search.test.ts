import { describe, expect, test } from "bun:test";
import { Deadline } from "../../shared/host-client";
import { KernelClient, MAX_READ_OBJECT_IDS, type ReadRow } from "../../shared/kernel-client";
import {
    ANTI_MEMORY_CATEGORY,
    renderAntiMemoryContent,
} from "../../shared/kernel-client/anti-memory";
import { FakeKernel, FakeKernelTransport } from "../../shared/kernel-client-testing/fake-kernel";
import {
    memoryResultFromRow,
    parseObjectIdQuery,
    readObjectRowsChunked,
    scanSnapshotForLexicalSearch,
    searchKernelMemoryRows,
} from "./kernel-memory-search";

const OBJECT_A = `mem_${"a".repeat(32)}`;
const OBJECT_B = `mem_${"b".repeat(32)}`;

function readRow(input: {
    objectId: string;
    decisionKind: string;
    summary: string;
    rationale?: string;
    seq?: number;
    labeled?: boolean;
}): ReadRow {
    const seq = input.seq ?? 1;
    return {
        object: {
            object_id: input.objectId,
            object_kind: "decision",
            domain_id: "memory",
            source_kind: "assistant",
            source_id: "eidnara_memory",
            source_revision: 1,
            created_commit_seq: seq,
            invalidated_commit_seq: null,
            superseded_by: null,
            sensitivity: "normal",
        },
        visibility: "labeled",
        labeled: input.labeled ?? true,
        scope_id: null,
        token: { object_id: input.objectId, known_as_of: seq },
        decision: {
            decision_kind: input.decisionKind,
            payload: { summary: input.summary, rationale: input.rationale ?? "" },
        },
    };
}

describe("memoryResultFromRow", () => {
    test("a rejected-approach row surfaces as an anti-memory warning with the parsed fields", () => {
        const summary = renderAntiMemoryContent({
            trigger: "session caching",
            rejectedStrategy: "Redis",
            rejectionReason: "it creates split ownership",
            saferAlternative: "use SQLite",
        });
        const row = readRow({
            objectId: OBJECT_A,
            decisionKind: ANTI_MEMORY_CATEGORY,
            summary,
            seq: 7,
        });
        const result = memoryResultFromRow(row, 0.9, "exact");
        expect(result.source).toBe("anti_memory");
        if (result.source !== "anti_memory") return;
        expect(result.trigger).toBe("session caching");
        expect(result.rejectedStrategy).toBe("Redis");
        expect(result.rejectionReason).toBe("it creates split ownership");
        expect(result.saferAlternative).toBe("use SQLite");
        expect(result.score).toBe(0.9);
        expect(result.objectId).toBe(OBJECT_A);
        expect(result.matchType).toBe("exact");
        expect(result.policyLabel).toBe("labeled");
        expect(result.contentDigest).toMatch(/^[0-9a-f]{64}$/);
        expect(result.normalizedHash).toBe(result.contentDigest);
    });

    test("a rejected-approach row with an unparseable summary stays a conservative anti-memory warning", () => {
        const row = readRow({
            objectId: OBJECT_A,
            decisionKind: ANTI_MEMORY_CATEGORY,
            summary: "free-form text without labeled anti-memory fields",
        });
        const result = memoryResultFromRow(row, 0.5, "lexical");
        expect(result.source).toBe("anti_memory");
        if (result.source !== "anti_memory") return;
        expect(result.rejectedStrategy).toBe("free-form text without labeled anti-memory fields");
        expect(result.trigger).toBe("");
        expect(result.rejectionReason).toContain("unparseable");
        expect(result.saferAlternative).toBeNull();
        expect(result.matchType).toBe("lexical");
    });

    test("an anti-memory row carries its decision rationale; an empty rationale stays absent", () => {
        const summary = renderAntiMemoryContent({
            trigger: "session caching",
            rejectedStrategy: "Redis",
            rejectionReason: "it creates split ownership",
        });
        const withRationale = memoryResultFromRow(
            readRow({
                objectId: OBJECT_A,
                decisionKind: ANTI_MEMORY_CATEGORY,
                summary,
                rationale: "the nonce handshake stalls under load",
            }),
            0.5,
            "lexical",
        );
        expect(withRationale.source).toBe("anti_memory");
        if (withRationale.source !== "anti_memory") return;
        expect(withRationale.rationale).toBe("the nonce handshake stalls under load");

        const withoutRationale = memoryResultFromRow(
            readRow({ objectId: OBJECT_B, decisionKind: ANTI_MEMORY_CATEGORY, summary }),
            0.5,
            "lexical",
        );
        expect(withoutRationale.source).toBe("anti_memory");
        if (withoutRationale.source !== "anti_memory") return;
        expect(withoutRationale.rationale).toBeUndefined();
    });
});

describe("searchKernelMemoryRows match labeling and domain fence", () => {
    test("an object-id query labels hits exact; a text query labels hits lexical", () => {
        const rows = [
            readRow({
                objectId: OBJECT_A,
                decisionKind: "PROJECT_RULES",
                summary: "the history_summarizer runs on a lease",
            }),
        ];
        const byId = searchKernelMemoryRows({ rows, query: OBJECT_A, limit: 5 });
        expect(byId?.map((hit) => hit.matchType)).toEqual(["exact"]);
        const byText = searchKernelMemoryRows({
            rows,
            query: "history_summarizer lease",
            limit: 5,
        });
        expect(byText?.map((hit) => hit.matchType)).toEqual(["lexical"]);
    });

    test("an anti-memory matched only through its rationale surfaces with that rationale", () => {
        const rows = [
            readRow({
                objectId: OBJECT_A,
                decisionKind: ANTI_MEMORY_CATEGORY,
                summary: renderAntiMemoryContent({
                    trigger: "session caching",
                    rejectedStrategy: "Redis",
                    rejectionReason: "it creates split ownership",
                }),
                rationale: "the nonce handshake stalls under load",
            }),
        ];
        const hits = searchKernelMemoryRows({ rows, query: "nonce handshake", limit: 5 });
        expect(hits?.map((hit) => hit.source)).toEqual(["anti_memory"]);
        const hit = hits?.[0];
        if (hit?.source !== "anti_memory") return;
        expect(hit.rationale).toBe("the nonce handshake stalls under load");
    });

    test("a decision row outside the memory domain never matches", () => {
        const foreign = readRow({
            objectId: OBJECT_A,
            decisionKind: "PROJECT_RULES",
            summary: "alpha beta",
        });
        foreign.object.domain_id = "notes";
        expect(searchKernelMemoryRows({ rows: [foreign], query: "alpha beta", limit: 5 })).toBe(
            null,
        );
        expect(searchKernelMemoryRows({ rows: [foreign], query: OBJECT_A, limit: 5 })).toBe(null);
    });
});

describe("searchKernelMemoryRows anti-memory expiry", () => {
    const NOW = 1_700_000_000_000;
    const antiRow = (objectId: string, expiresAt: number | null) =>
        readRow({
            objectId,
            decisionKind: ANTI_MEMORY_CATEGORY,
            summary: renderAntiMemoryContent({
                trigger: "session caching",
                rejectedStrategy: "Redis",
                rejectionReason: "it creates split ownership",
                expiresAt,
            }),
        });

    test("an expired anti-memory surfaces on neither the exact nor the lexical path", () => {
        const rows = [antiRow(OBJECT_A, NOW - 1)];
        expect(
            searchKernelMemoryRows({ rows, query: "session caching", limit: 5, nowMs: NOW }),
        ).toBeNull();
        expect(searchKernelMemoryRows({ rows, query: OBJECT_A, limit: 5, nowMs: NOW })).toBeNull();
    });

    test("an unexpired anti-memory still surfaces as a lexical anti-memory hit", () => {
        const rows = [antiRow(OBJECT_A, NOW + 1)];
        const hits = searchKernelMemoryRows({
            rows,
            query: "session caching",
            limit: 5,
            nowMs: NOW,
        });
        expect(hits?.map((hit) => [hit.source, hit.matchType])).toEqual([
            ["anti_memory", "lexical"],
        ]);
    });
});

describe("searchKernelMemoryRows baseline exclusion", () => {
    const rows = [
        readRow({
            objectId: OBJECT_A,
            decisionKind: "PROJECT_RULES",
            summary: "the history_summarizer runs on a lease",
        }),
        readRow({
            objectId: OBJECT_B,
            decisionKind: "PROJECT_RULES",
            summary: "the history_summarizer writes claims",
        }),
    ];
    const excludeObjectIds = new Set([OBJECT_A]);

    test("an explicit object-id query resolves a baseline-visible object", () => {
        const hits = searchKernelMemoryRows({ rows, query: OBJECT_A, limit: 5, excludeObjectIds });
        expect(hits?.map((hit) => [hit.objectId, hit.matchType])).toEqual([[OBJECT_A, "exact"]]);
    });

    test("lexical ranking still excludes baseline-visible objects", () => {
        const hits = searchKernelMemoryRows({
            rows,
            query: "history_summarizer",
            limit: 5,
            excludeObjectIds,
        });
        expect(hits?.map((hit) => hit.objectId)).toEqual([OBJECT_B]);
    });
});

describe("parseObjectIdQuery id@commit tolerance", () => {
    test("an id@commit token rendered by an earlier release resolves through the exact object-id path", () => {
        const row = readRow({
            objectId: OBJECT_A,
            decisionKind: "PROJECT_RULES",
            summary: "the history_summarizer runs on a lease",
            seq: 7,
        });
        const result = memoryResultFromRow(row, 1, "exact");
        expect(result).not.toHaveProperty("commitSeq");
        const locator = `${result.objectId}@7`;
        const hits = searchKernelMemoryRows({ rows: [row], query: locator, limit: 5 });
        expect(hits?.map((hit) => [hit.objectId, hit.matchType])).toEqual([[OBJECT_A, "exact"]]);
    });

    test("bare ids and locators mix in one query and deduplicate to the same object", () => {
        expect(parseObjectIdQuery(`${OBJECT_A}@3 ${OBJECT_B}`)).toEqual([OBJECT_A, OBJECT_B]);
        expect(parseObjectIdQuery(`${OBJECT_A} ${OBJECT_A}@3`)).toEqual([OBJECT_A]);
    });

    test("a malformed locator keeps the query ordinary text", () => {
        expect(parseObjectIdQuery(`${OBJECT_A}@`)).toBeNull();
        expect(parseObjectIdQuery(`${OBJECT_A}@r1`)).toBeNull();
        expect(parseObjectIdQuery(`mcm_${"a".repeat(32)}/r1/${"0".repeat(64)}`)).toBeNull();
    });
});

describe("searchKernelMemoryRows tie-breaking", () => {
    test("rows with equal score, seq, and object id keep their input order", () => {
        const rows = [
            readRow({
                objectId: OBJECT_A,
                decisionKind: "PROJECT_RULES",
                summary: "alpha beta one",
                seq: 3,
            }),
            readRow({
                objectId: OBJECT_A,
                decisionKind: "PROJECT_RULES",
                summary: "alpha beta two",
                seq: 3,
            }),
        ];
        const hits = searchKernelMemoryRows({ rows, query: "alpha beta", limit: 5 });
        expect(hits).not.toBeNull();
        expect(hits?.map((hit) => (hit.source === "memory" ? hit.content : hit.trigger))).toEqual([
            "alpha beta one",
            "alpha beta two",
        ]);
    });

    test("rows with equal score and seq sort ascending by object id", () => {
        const rows = [
            readRow({
                objectId: OBJECT_B,
                decisionKind: "PROJECT_RULES",
                summary: "alpha beta",
                seq: 3,
            }),
            readRow({
                objectId: OBJECT_A,
                decisionKind: "PROJECT_RULES",
                summary: "alpha beta",
                seq: 3,
            }),
        ];
        const hits = searchKernelMemoryRows({ rows, query: "alpha beta", limit: 5 });
        expect(hits?.map((hit) => hit.objectId)).toEqual([OBJECT_A, OBJECT_B]);
    });
});

describe("readObjectRowsChunked snapshot pin", () => {
    const IDS = ["a", "b", "c", "d"].map((letter) => `mem_${letter.repeat(32)}`);

    /** A two-row cap makes the four-id read split into two complete reads after truncation. */
    function harness(afterFirstReply: (kernel: FakeKernel) => void) {
        const kernel = new FakeKernel();
        for (const id of IDS) {
            kernel.seedDecision({ object_id: id, decision_kind: "ARCHITECTURE", summary: id });
        }
        kernel.filteredReadRowCap = 2;
        const transport = new FakeKernelTransport(kernel);
        const inner = transport.call.bind(transport);
        let replies = 0;
        transport.call = async (args) => {
            const reply = await inner(args);
            replies += 1;
            if (replies === 1) afterFirstReply(kernel);
            return reply;
        };
        const client = new KernelClient({
            transport,
            enabled: true,
            sessionId: "ses-chunked",
            projectRoot: "/tmp/chunked",
        });
        return { kernel, transport, client };
    }

    function asOfBodies(transport: FakeKernelTransport): unknown[] {
        return transport.calls.map((call) => (call.body as { as_of: unknown }).as_of);
    }

    test("a retire landing between chunk reads leaves the assembled rows at the first snapshot", async () => {
        const { transport, client } = harness((kernel) => {
            kernel.tip += 1;
            const oldest = kernel.objects.get(IDS[0] as string);
            if (oldest) oldest.invalidated_commit_seq = kernel.tip;
        });
        const read = await readObjectRowsChunked({
            client,
            surface: "explicit_search",
            gated: false,
            objectIds: IDS,
        });
        expect(read.ok).toBe(true);
        if (!read.ok) return;
        expect(asOfBodies(transport)).toEqual([null, 4, 4]);
        expect(read.knownAsOf).toBe(4);
        expect(read.rows.map((row) => row.object.object_id).sort()).toEqual([...IDS].sort());
        expect(read.unresolvedObjectIds).toEqual([]);
    });

    test("a pinned chunk answered from another snapshot fails closed as snapshot_diverged", async () => {
        const { transport, client } = harness((kernel) => {
            kernel.tip = 1;
        });
        const read = await readObjectRowsChunked({
            client,
            surface: "explicit_search",
            gated: false,
            objectIds: IDS,
        });
        expect(read).toEqual({
            ok: false,
            state: { kind: "unavailable", reason: "snapshot_diverged" },
        });
        // The client re-reads the tip once after the daemon answers the pin as diverged; the chunked read rejects that tip reply instead of mixing it in.
        expect(asOfBodies(transport)).toEqual([null, 4, null]);
    });
});

describe("readObjectRowsChunked filter bound", () => {
    test("an id list over MAX_READ_OBJECT_IDS is read as filtered chunks pinned to one snapshot", async () => {
        const kernel = new FakeKernel();
        const count = MAX_READ_OBJECT_IDS + 1;
        const ids = Array.from(
            { length: count },
            (_, index) => `mem_${String(index).padStart(32, "0")}`,
        );
        for (const id of ids) {
            kernel.seedDecision({ object_id: id, decision_kind: "ARCHITECTURE", summary: id });
        }
        // The unfiltered snapshot would drop the oldest row; the filtered chunks must still serve it.
        kernel.readRowCap = MAX_READ_OBJECT_IDS;
        const transport = new FakeKernelTransport(kernel);
        const client = new KernelClient({
            transport,
            enabled: true,
            sessionId: "ses-chunked",
            projectRoot: "/tmp/chunked",
        });
        const read = await readObjectRowsChunked({
            client,
            surface: "explicit_search",
            gated: false,
            objectIds: ids,
        });
        expect(read.ok).toBe(true);
        if (!read.ok) return;
        expect(read.rows.map((row) => row.object.object_id).sort()).toEqual([...ids].sort());
        expect(read.unresolvedObjectIds).toEqual([]);
        expect(read.knownAsOf).toBe(count);
        const bodies = transport.calls.map(
            (call) => call.body as { object_ids?: string[]; as_of: unknown },
        );
        expect(bodies.map((body) => body.object_ids?.length ?? 0).sort((a, b) => a - b)).toEqual([
            1,
            MAX_READ_OBJECT_IDS,
        ]);
        expect(bodies.flatMap((body) => body.object_ids ?? []).sort()).toEqual([...ids].sort());
        expect(bodies.map((body) => body.as_of)).toEqual([null, count]);
    });
});

/** A store whose oldest memory answers the query, under `newer` later memories that share only one query word with it. */
function storeLargerThanOnePage(newer: number): { kernel: FakeKernel; target: string } {
    const kernel = new FakeKernel();
    const target = `mem_${"f".repeat(32)}`;
    kernel.seedDecision({
        object_id: target,
        decision_kind: "PROJECT_RULES",
        summary: "The nightly backup job writes database dumps to the cold storage volume.",
    });
    for (let index = 0; index < newer; index += 1) {
        kernel.seedDecision({
            object_id: `mem_${String(index).padStart(32, "0")}`,
            decision_kind: "ARCHITECTURE",
            summary: `Module ${index} reads its settings from the database at startup.`,
        });
    }
    return { kernel, target };
}

function clientOver(transport: FakeKernelTransport): KernelClient {
    return new KernelClient({
        transport,
        enabled: true,
        sessionId: "ses-paged",
        projectRoot: "/tmp/paged",
    });
}

const BACKUP_QUERY = "nightly backup database dumps";

describe("scanSnapshotForLexicalSearch", () => {
    test("finds the oldest memory of a store larger than one read page, which a single read misses", async () => {
        const { kernel, target } = storeLargerThanOnePage(450);
        kernel.readRowCap = 100;
        const transport = new FakeKernelTransport(kernel);
        const client = clientOver(transport);
        const scan = (maxPages?: number) =>
            scanSnapshotForLexicalSearch({
                client,
                surface: "explicit_search",
                gated: false,
                query: BACKUP_QUERY,
                limit: 10,
                nowMs: 0,
                ...(maxPages === undefined ? {} : { maxPages }),
            });

        // One page is the single capped read the scan replaced.
        const single = await scan(1);
        expect(single.ok && single.truncated).toBe(true);
        if (!single.ok) return;
        expect(single.rows.map((row) => row.object.object_id)).not.toContain(target);

        transport.calls.length = 0;
        const paged = await scan();
        expect(paged.ok).toBe(true);
        if (!paged.ok) return;
        expect(paged.truncated).toBe(false);
        const hits = searchKernelMemoryRows({
            rows: paged.rows,
            query: BACKUP_QUERY,
            limit: 10,
            nowMs: 0,
        });
        expect(hits?.[0]?.objectId).toBe(target);
        const bodies = transport.calls.map(
            (call) => call.body as { as_of: number | null; before?: { object_id: string } },
        );
        // 451 rows at 100 per page: five pages, every page after the first pinned to the first page's snapshot.
        expect(bodies).toHaveLength(5);
        expect(bodies[0]?.before).toBeUndefined();
        expect(new Set(bodies.slice(1).map((body) => body.as_of))).toEqual(new Set([kernel.tip]));
        expect(paged.rows.length).toBe(10);
    });

    test("reports truncation when the page bound stops the scan before the oldest rows", async () => {
        const { kernel, target } = storeLargerThanOnePage(450);
        kernel.readRowCap = 100;
        const scan = await scanSnapshotForLexicalSearch({
            client: clientOver(new FakeKernelTransport(kernel)),
            surface: "explicit_search",
            gated: false,
            query: BACKUP_QUERY,
            limit: 10,
            nowMs: 0,
            maxPages: 4,
        });
        expect(scan.ok && scan.truncated).toBe(true);
        if (!scan.ok) return;
        expect(scan.rows.map((row) => row.object.object_id)).not.toContain(target);
    });

    /** A client whose first page is truncated and whose second read answers `second`; `afterFirst` names the connection identity after page 1. */
    function twoPageClient(
        second: unknown,
        afterFirst = "connection",
    ): { client: KernelClient; reads: unknown[] } {
        const connection = { current: "connection" };
        const first = readRow({
            objectId: OBJECT_A,
            decisionKind: "ARCHITECTURE",
            summary: "backup volume",
            seq: 5,
        });
        const reads: unknown[] = [];
        const client = {
            connectionIdentity: () => connection.current,
            read: async (args: unknown) => {
                reads.push(args);
                if (reads.length === 1) {
                    connection.current = afterFirst;
                    return {
                        state: { kind: "available" },
                        known_as_of: 9,
                        tip: 9,
                        gated: false,
                        truncated: true,
                        rows: [first],
                    };
                }
                return second;
            },
        } as unknown as KernelClient;
        return { client, reads };
    }

    const scanWith = (client: KernelClient) =>
        scanSnapshotForLexicalSearch({
            client,
            surface: "explicit_search",
            gated: true,
            query: "backup",
            limit: 10,
            nowMs: 0,
        });

    test("keeps the first page's rows as partial when a later page is refused", async () => {
        const { client, reads } = twoPageClient({
            state: { kind: "unavailable", reason: "daemon_absent" },
        });
        const scan = await scanWith(client);
        expect(scan).toEqual({
            ok: true,
            rows: [
                expect.objectContaining({
                    object: expect.objectContaining({ object_id: OBJECT_A }),
                }),
            ],
            truncated: true,
        });
        // Only the first page is gated.
        expect(reads.map((args) => (args as { gated: boolean }).gated)).toEqual([true, false]);
    });

    test("answers the cancelled state when the caller's signal aborts after the first page", async () => {
        const controller = new AbortController();
        const first = readRow({
            objectId: OBJECT_A,
            decisionKind: "ARCHITECTURE",
            summary: "backup volume",
            seq: 5,
        });
        let reads = 0;
        const client = {
            connectionIdentity: () => "connection",
            read: async () => {
                reads += 1;
                if (reads === 1) {
                    controller.abort();
                    return {
                        state: { kind: "available" },
                        known_as_of: 9,
                        tip: 9,
                        gated: false,
                        truncated: true,
                        rows: [first],
                    };
                }
                return { state: { kind: "cancelled" } };
            },
        } as unknown as KernelClient;
        const scan = await scanSnapshotForLexicalSearch({
            client,
            surface: "explicit_search",
            gated: false,
            query: "backup",
            limit: 10,
            nowMs: 0,
            signal: controller.signal,
        });
        expect(scan).toEqual({ ok: false, state: { kind: "cancelled" } });
    });

    test("passes one deadline's remainder to every page", async () => {
        const { client, reads } = twoPageClient({
            state: { kind: "available" },
            known_as_of: 9,
            tip: 9,
            gated: false,
            truncated: false,
            rows: [],
        });
        let now = 0;
        const clock = () => {
            const at = now;
            now += 1_000;
            return at;
        };
        await scanSnapshotForLexicalSearch({
            client,
            surface: "explicit_search",
            gated: false,
            query: "backup",
            limit: 10,
            nowMs: 0,
            deadline: Deadline.start(60_000, clock),
        });
        // The clock reads 0 at the start, 1,000 for page 1's budget, then 2,000 for page 2's expiry check and 3,000 for its budget.
        expect(reads.map((args) => (args as { deadlineMs?: number }).deadlineMs)).toEqual([
            59_000, 57_000,
        ]);
    });

    test("keeps the rows already ranked when the shared deadline expires during a later page", async () => {
        const { kernel, target } = storeLargerThanOnePage(450);
        kernel.readRowCap = 100;
        let now = 0;
        const clock = () => {
            const at = now;
            now += 400;
            return at;
        };
        const scan = await scanSnapshotForLexicalSearch({
            client: clientOver(new FakeKernelTransport(kernel)),
            surface: "explicit_search",
            gated: false,
            query: BACKUP_QUERY,
            limit: 10,
            nowMs: 0,
            // Page 1 receives 600 ms; page 2 passes its expiry check at 800 and receives no budget at 1,200, which the client reports as `cancelled`.
            deadline: Deadline.start(1_000, clock),
        });
        expect(scan).toMatchObject({ ok: true, truncated: true });
        if (!scan.ok) return;
        expect(scan.rows).toHaveLength(10);
        expect(scan.rows.map((row) => row.object.object_id)).not.toContain(target);
    });

    test("continues inside one commit: a later page may start with that commit's greater ids", async () => {
        const sibling = readRow({
            objectId: OBJECT_B,
            decisionKind: "ARCHITECTURE",
            summary: "backup",
            seq: 5,
        });
        const { client } = twoPageClient({
            state: { kind: "available" },
            known_as_of: 9,
            tip: 9,
            gated: false,
            truncated: false,
            rows: [sibling],
        });
        const scan = await scanWith(client);
        expect(scan).toMatchObject({ ok: true, truncated: false });
        if (!scan.ok) return;
        expect(scan.rows.map((row) => row.object.object_id).sort()).toEqual([OBJECT_A, OBJECT_B]);
    });

    test("stops as partial when a later page arrives over another daemon connection", async () => {
        const same = readRow({
            objectId: OBJECT_B,
            decisionKind: "ARCHITECTURE",
            summary: "backup",
            seq: 4,
        });
        const { client } = twoPageClient(
            {
                state: { kind: "available" },
                known_as_of: 9,
                tip: 9,
                gated: false,
                truncated: false,
                rows: [same],
            },
            "replacement",
        );
        const scan = await scanWith(client);
        expect(scan).toMatchObject({ ok: true, truncated: true });
        if (!scan.ok) return;
        expect(scan.rows.map((row) => row.object.object_id)).toEqual([OBJECT_A]);
    });

    test("orders a same-commit tie by UTF-8 bytes, as the daemon does", async () => {
        // U+E000 sorts before U+10000 in UTF-8 and after it in UTF-16 code units.
        const low = readRow({
            objectId: "mem_\uE000",
            decisionKind: "ARCHITECTURE",
            summary: "backup",
            seq: 5,
        });
        const high = readRow({
            objectId: "mem_\u{10000}",
            decisionKind: "ARCHITECTURE",
            summary: "backup",
            seq: 5,
        });
        const reads: unknown[] = [];
        const client = {
            connectionIdentity: () => "connection",
            read: async (args: unknown) => {
                reads.push(args);
                return reads.length === 1
                    ? {
                          state: { kind: "available" },
                          known_as_of: 9,
                          tip: 9,
                          gated: false,
                          truncated: true,
                          rows: [low],
                      }
                    : {
                          state: { kind: "available" },
                          known_as_of: 9,
                          tip: 9,
                          gated: false,
                          truncated: false,
                          rows: [high],
                      };
            },
        } as unknown as KernelClient;
        const scan = await scanWith(client);
        expect(scan).toMatchObject({ ok: true, truncated: false });
        if (!scan.ok) return;
        expect(scan.rows).toHaveLength(2);
    });

    test("stops at a same-commit row that UTF-8 orders before the cursor, though UTF-16 orders it after", async () => {
        const high = readRow({
            objectId: "mem_\u{10000}",
            decisionKind: "ARCHITECTURE",
            summary: "backup",
            seq: 5,
        });
        const low = readRow({
            objectId: "mem_\uE000",
            decisionKind: "ARCHITECTURE",
            summary: "backup",
            seq: 5,
        });
        const page = (rows: ReadRow[], truncated: boolean) => ({
            state: { kind: "available" },
            known_as_of: 9,
            tip: 9,
            gated: false,
            truncated,
            rows,
        });
        let reads = 0;
        const client = {
            connectionIdentity: () => "connection",
            read: async () => {
                reads += 1;
                return reads === 1 ? page([high], true) : page([low], false);
            },
        } as unknown as KernelClient;
        const scan = await scanWith(client);
        expect(scan).toMatchObject({ ok: true, truncated: true });
        if (!scan.ok) return;
        expect(scan.rows).toHaveLength(1);
    });

    test("caches mutation tokens for the first page only", async () => {
        const { kernel } = storeLargerThanOnePage(450);
        kernel.readRowCap = 100;
        const client = clientOver(new FakeKernelTransport(kernel));
        const scan = await scanSnapshotForLexicalSearch({
            client,
            surface: "explicit_search",
            gated: false,
            query: BACKUP_QUERY,
            limit: 10,
            nowMs: 0,
        });
        expect(scan).toMatchObject({ ok: true, truncated: false });
        expect(client.tokens.size("/tmp/paged")).toBe(100);
    });

    test("stops as partial when a later page answers another snapshot", async () => {
        const moved = readRow({
            objectId: OBJECT_B,
            decisionKind: "ARCHITECTURE",
            summary: "backup",
            seq: 4,
        });
        const { client } = twoPageClient({
            state: { kind: "available" },
            known_as_of: 10,
            tip: 10,
            gated: false,
            truncated: false,
            rows: [moved],
        });
        const scan = await scanWith(client);
        expect(scan.ok && scan.truncated).toBe(true);
        if (!scan.ok) return;
        expect(scan.rows.map((row) => row.object.object_id)).toEqual([OBJECT_A]);
    });

    test("stops as partial when a later page serves a row at or before the cursor", async () => {
        const repeated = readRow({
            objectId: OBJECT_A,
            decisionKind: "ARCHITECTURE",
            summary: "backup volume",
            seq: 5,
        });
        const { client } = twoPageClient({
            state: { kind: "available" },
            known_as_of: 9,
            tip: 9,
            gated: false,
            truncated: true,
            rows: [repeated],
        });
        const scan = await scanWith(client);
        expect(scan.ok && scan.truncated).toBe(true);
        if (!scan.ok) return;
        expect(scan.rows.map((row) => row.object.object_id)).toEqual([OBJECT_A]);
    });
});

describe("scanSnapshotForLexicalSearch over several pages", () => {
    const NOW = 1_700_000_000_000;

    test("ranks as one search over every row: strong hits on the newest and oldest pages both stay", async () => {
        const { kernel, target } = storeLargerThanOnePage(450);
        const newest = `mem_${"e".repeat(32)}`;
        kernel.seedDecision({
            object_id: newest,
            decision_kind: "PROJECT_RULES",
            summary: "The nightly backup job keeps database dumps for thirty days.",
        });
        kernel.readRowCap = 100;
        const paged = await scanSnapshotForLexicalSearch({
            client: clientOver(new FakeKernelTransport(kernel)),
            surface: "explicit_search",
            gated: false,
            query: BACKUP_QUERY,
            limit: 10,
            nowMs: 0,
        });
        expect(paged.ok && !paged.truncated).toBe(true);
        if (!paged.ok) return;
        const pagedHits = searchKernelMemoryRows({
            rows: paged.rows,
            query: BACKUP_QUERY,
            limit: 10,
            nowMs: 0,
        });
        kernel.readRowCap = null;
        const whole = await clientOver(new FakeKernelTransport(kernel)).read({
            surface: "explicit_search",
        });
        if (whole.state.kind !== "available") throw new Error("uncapped read unavailable");
        const wholeHits = searchKernelMemoryRows({
            rows: (whole as { rows: ReadRow[] }).rows,
            query: BACKUP_QUERY,
            limit: 10,
            nowMs: 0,
        });
        expect(pagedHits?.map((hit) => hit.objectId)).toEqual(
            wholeHits?.map((hit) => hit.objectId),
        );
        expect(
            pagedHits
                ?.slice(0, 2)
                .map((hit) => hit.objectId)
                .sort(),
        ).toEqual([newest, target].sort());
    });

    test("an expired anti-memory that matches better takes no slot from an eligible memory", async () => {
        const kernel = new FakeKernel();
        const target = `mem_${"f".repeat(32)}`;
        kernel.seedDecision({
            object_id: target,
            decision_kind: "PROJECT_RULES",
            summary: "Session caching uses the embedded store.",
        });
        kernel.seedDecision({
            object_id: `mem_${"d".repeat(32)}`,
            decision_kind: ANTI_MEMORY_CATEGORY,
            summary: renderAntiMemoryContent({
                trigger: "session caching store",
                rejectedStrategy: "Redis",
                rejectionReason: "it creates split ownership",
                expiresAt: NOW - 1,
            }),
        });
        kernel.readRowCap = 1;
        const scan = await scanSnapshotForLexicalSearch({
            client: clientOver(new FakeKernelTransport(kernel)),
            surface: "explicit_search",
            gated: false,
            query: "session caching store",
            limit: 1,
            nowMs: NOW,
        });
        expect(scan.ok && !scan.truncated).toBe(true);
        if (!scan.ok) return;
        expect(scan.rows.map((row) => row.object.object_id)).toEqual([target]);
    });
});
