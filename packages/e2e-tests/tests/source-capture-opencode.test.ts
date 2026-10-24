import { Database } from "bun:sqlite";
import { describe, expect, it } from "bun:test";
import { join } from "node:path";

import { offered, openCodeWorkspace, startOpenCode } from "../src/corpus-sessions/harnesses";
import { runOpenCodeCommonMix, runOpenCodeMix } from "../src/corpus-sessions/opencode-mix";
import { RustTestHarness } from "../src/rust-harness";
import { readOpenCodeSession } from "../src/source-corpus/opencode-db-reader";
import { reconcile } from "../src/source-corpus/reconcile";

const prereqs = RustTestHarness.detectPrereqs();

function storeRecords(h: RustTestHarness, sessionId: string) {
    const db = new Database(join(h.env.dataDir, "opencode", "opencode.db"), { readonly: true });
    try {
        return readOpenCodeSession(db, sessionId);
    } finally {
        db.close();
    }
}

describe.skipIf(!prereqs.ok)("OpenCode source capture reconciles against the SQL reader", () => {
    it("accounts every expected unit of the R1 mix as admitted or one named loss", async () => {
        const h = await startOpenCode(true);
        try {
            const { root, sessionId } = await openCodeWorkspace(h);
            const run = await runOpenCodeMix(h, sessionId, root);
            const counters = await h.host.settledSourceCapture();
            const descriptors = await h.host.liveSourceDescriptors();
            const records = storeRecords(h, sessionId);
            const result = reconcile(offered(records), descriptors, sessionId, (_, unit) =>
                run.canaryCalls.includes(unit.tool?.callId ?? ""),
            );
            expect(result.unclassified).toEqual([]);
            expect(result.phantom).toEqual([]);
            expect(
                result.admitted + result.losses.unit_too_large + result.losses.secret_bearing,
            ).toBe(result.expected);
            expect(result.losses.secret_bearing).toBe(1);
            const documents = descriptors.filter((d) => d.class === "repository_documents");
            expect(documents.map((d) => d.identity.relative_path)).toEqual(["ARCHITECTURE.md"]);
            expect(counters.units_published).toBe(result.admitted + documents.length);
            expect(counters.units_replayed).toBe(0);
            expect(counters.units_unit_too_large > 0).toBe(result.losses.unit_too_large > 0);
            expect(counters.units_refused_terminal).toBe(result.losses.secret_bearing);
        } finally {
            await h.dispose();
        }
    }, 600_000);

    it("captures nothing when consent is off", async () => {
        const h = await startOpenCode(false);
        try {
            const { root, sessionId } = await openCodeWorkspace(h);
            await runOpenCodeCommonMix(h, sessionId, root);
            const counters = await h.host.settledSourceCapture();
            const records = storeRecords(h, sessionId);
            expect(offered(records).flatMap((record) => record.units).length).toBeGreaterThan(0);
            expect(await h.host.liveSourceDescriptors()).toEqual([]);
            expect(counters.units_published + counters.units_memo_skipped).toBe(0);
            expect(counters.admitted).toBe(0);
        } finally {
            await h.dispose();
        }
    }, 600_000);
});
