import { Database } from "bun:sqlite";
import { describe, expect, it } from "bun:test";
import { join } from "node:path";

import {
    offered,
    openCodeWorkspace,
    startOpenCode,
    startPi,
} from "../src/corpus-sessions/harnesses";
import { runOpenCodeCommonMix } from "../src/corpus-sessions/opencode-mix";
import { runPiCommonMix, runPiMix } from "../src/corpus-sessions/pi-mix";
import { detectPiPrereqs } from "../src/pi-runner/spawn";
import { detectRustModePrereqs } from "../src/rust-runner/hermetic-host";
import { readOpenCodeSession } from "../src/source-corpus/opencode-db-reader";
import { readPiSession } from "../src/source-corpus/pi-jsonl-reader";
import { reconcile } from "../src/source-corpus/reconcile";

const { readFileSync } = process.getBuiltinModule("node:fs");

const rust = detectRustModePrereqs();
const pi = detectPiPrereqs();
const PI_REQUIRED = process.env.EIDNARA_E2E_REQUIRE_PI === "1";
const active = rust.ok && pi.ok;

describe.skipIf(!active)("Pi source capture reconciles against the JSONL reader", () => {
    it("accounts every expected unit and refuses the image-bearing tool result", async () => {
        const run = await startPi(true);
        try {
            const mix = await runPiMix(run.harness);
            const counters = await run.stack.settledSourceCapture();
            const descriptors = await run.stack.liveSourceDescriptors();
            const records = readPiSession(readFileSync(run.file, "utf8"));
            const sessionId = records[0]?.id ?? "";
            const result = reconcile(offered(records), descriptors, sessionId, (_, unit) =>
                mix.canaryCalls.includes(unit.tool?.callId ?? ""),
            );
            expect(result.unclassified).toEqual([]);
            expect(result.phantom).toEqual([]);
            expect(
                result.admitted + result.losses.unit_too_large + result.losses.secret_bearing,
            ).toBe(result.expected);
            expect(result.losses).toEqual({ unit_too_large: 1, secret_bearing: 1 });
            const refused = records.filter((record) =>
                record.nonUnits.some((nonUnit) => nonUnit.reason === "non_text_item"),
            );
            expect(refused.map((record) => record.nonUnits[0]?.kind)).toEqual(["image"]);
            // The adapter refuses the one image-bearing record on every pass that offers it.
            expect(counters.adapter_refused).toBeGreaterThan(0);
            const documents = descriptors.filter((d) => d.class === "repository_documents");
            expect(documents.map((d) => d.identity.relative_path)).toEqual(["ARCHITECTURE.md"]);
            expect(counters.units_published).toBe(result.admitted + documents.length);
            expect(counters.units_replayed).toBe(0);
            expect(counters.units_unit_too_large).toBeGreaterThan(0);
            expect(counters.units_refused_terminal).toBe(1);
        } finally {
            await run.close();
        }
    }, 600_000);

    it("captures nothing when consent is off", async () => {
        const run = await startPi(false);
        try {
            await runPiCommonMix(run.harness);
            const counters = await run.stack.settledSourceCapture();
            const records = readPiSession(readFileSync(run.file, "utf8"));
            expect(offered(records).flatMap((record) => record.units).length).toBeGreaterThan(0);
            expect(await run.stack.liveSourceDescriptors()).toEqual([]);
            expect(counters.units_published + counters.units_memo_skipped).toBe(0);
            expect(counters.admitted).toBe(0);
        } finally {
            await run.close();
        }
    }, 600_000);

    it("captures equal unit counts and document sets on both harnesses for one mix", async () => {
        const run = await startPi(true);
        let piUnits: number;
        let piDocuments: string[];
        try {
            await runPiCommonMix(run.harness);
            await run.stack.settledSourceCapture();
            const descriptors = await run.stack.liveSourceDescriptors();
            const records = readPiSession(readFileSync(run.file, "utf8"));
            const result = reconcile(
                offered(records),
                descriptors,
                records[0]?.id ?? "",
                () => false,
            );
            expect(result.unclassified).toEqual([]);
            expect(result.expected).toBeGreaterThan(0);
            piUnits = result.admitted;
            piDocuments = descriptors
                .filter((d) => d.class === "repository_documents")
                .map((d) => d.identity.relative_path ?? "");
        } finally {
            await run.close();
        }
        const h = await startOpenCode(true);
        try {
            const { root, sessionId } = await openCodeWorkspace(h);
            await runOpenCodeCommonMix(h, sessionId, root);
            await h.host.settledSourceCapture();
            const descriptors = await h.host.liveSourceDescriptors();
            const db = new Database(join(h.env.dataDir, "opencode", "opencode.db"), {
                readonly: true,
            });
            const records = readOpenCodeSession(db, sessionId);
            db.close();
            const result = reconcile(offered(records), descriptors, sessionId, () => false);
            expect(result.unclassified).toEqual([]);
            expect(result.expected).toBeGreaterThan(0);
            expect(result.admitted).toBe(piUnits);
            expect(piDocuments).toEqual(["ARCHITECTURE.md"]);
            expect(
                descriptors
                    .filter((d) => d.class === "repository_documents")
                    .map((d) => d.identity.relative_path ?? ""),
            ).toEqual(piDocuments);
        } finally {
            await h.dispose();
        }
    }, 600_000);
});

describe.skipIf(active || !PI_REQUIRED)("Pi source capture prerequisites", () => {
    it("are present when EIDNARA_E2E_REQUIRE_PI=1", () => {
        expect(rust.skipReason ?? pi.skipReason ?? "present").toBe("present");
    });
});
