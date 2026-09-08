import { afterAll, beforeAll, describe, expect, it } from "bun:test";
import { mkdtempSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";
import { detectRustPrerequisites } from "../scripts/check-rust-prerequisites";
import { E2E_ROOT, INCIDENTS_DIR, loadHistorySnapshot } from "../scripts/validate-incident-history";
import { validateIncidentHistory } from "../src/incident-pool/history";
import {
    builtinIncidentCaseRegistry,
    implementationBundleDigest,
    validateRegistryCatalogCorrespondence,
} from "../src/incident-pool/registry";
import type { IncidentCaseResult } from "../src/incident-pool/report";
import { buildRunSnapshot, runCaseInIsolation } from "../src/incident-pool/runner";
import { rustPrereqs } from "../src/rust-scenario-support";

// The case timeout stays below bun:test's timeout so the runner classifies a case failure
// before bun:test times out the test.
const GREEN_TEST_TIMEOUT_MS = 300_000;
const GREEN_CASE_TIMEOUT_MS = GREEN_TEST_TIMEOUT_MS - 60_000;

const RUST_GREEN_VARIANT_IDS = [
    "var-parity-a1-pure-defer-stability",
    "var-parity-a3-ctx-reduce-survival",
] as const;

// History, registry, and digests validate at import so a catalog defect fails even where the
// runtime cannot run a case.
const historyFiles = loadHistorySnapshot(INCIDENTS_DIR, "working");
const history = validateIncidentHistory(historyFiles);
const registry = builtinIncidentCaseRegistry();
validateRegistryCatalogCorrespondence(registry, history.catalog);
const implementationDigests = new Map(
    [...registry].map(([variantId, registered]) => [
        variantId,
        implementationBundleDigest(resolve(E2E_ROOT, "../.."), registered.implementationFiles),
    ]),
);

describe.skipIf(!rustPrereqs.ok)("incident pool baseline-green wrappers (rust)", () => {
    let workspaceParentDir: string;

    beforeAll(() => {
        if (!process.env.EIDNARA_E2E_DIRECT_HOST_FIXTURE_BIN) {
            const prereqs = detectRustPrerequisites({ allowBuild: true });
            if (prereqs.fixtureBin) {
                process.env.EIDNARA_E2E_DIRECT_HOST_FIXTURE_BIN = prereqs.fixtureBin;
            }
        }
        workspaceParentDir = mkdtempSync(join(tmpdir(), "incident-green-"));
    });

    afterAll(() => {
        rmSync(workspaceParentDir, { recursive: true, force: true });
    });

    async function runGreenVariant(variantId: string): Promise<IncidentCaseResult> {
        const snapshot = buildRunSnapshot({
            catalog: history.catalog,
            ledger: history.ledger,
            adjudicationLines: historyFiles.adjudicationLines,
            harness: "rust",
            lanes: ["green"],
            variantIds: [variantId],
            implementationDigests,
        });
        if (snapshot.selected.length !== 1) {
            throw new Error(
                `green wrapper ${variantId} selected ${snapshot.selected.length} cases`,
            );
        }
        const selected = snapshot.selected[0]!;
        const registered = registry.get(variantId);
        if (!registered) throw new Error(`green variant ${variantId} is not registered`);
        const prerequisite = registered.prerequisite?.() ?? { ok: true as const };
        if (!prerequisite.ok) {
            throw new Error(
                `green variant ${variantId} prerequisite unmet: ${prerequisite.reason}`,
            );
        }
        const execution = await runCaseInIsolation(snapshot, selected, {
            argv: [process.execPath, resolve(E2E_ROOT, "scripts", "run-incident-case.ts")],
            timeoutMs: GREEN_CASE_TIMEOUT_MS,
            workspaceParentDir,
            extraEnv: { EIDNARA_E2E_MODE: "rust" },
        });
        return execution.result;
    }

    for (const variantId of RUST_GREEN_VARIANT_IDS) {
        it(
            variantId,
            async () => {
                const result = await runGreenVariant(variantId);
                expect(result.variant_id).toBe(variantId);
                expect(result.run_health).toBe("completed");
                expect(result.behavioral_verdict).toBe("pass");
                expect(result.baseline_comparison).toBe("expected_green");
                expect(result.reason_code).toBeNull();
            },
            GREEN_TEST_TIMEOUT_MS,
        );
    }
});
