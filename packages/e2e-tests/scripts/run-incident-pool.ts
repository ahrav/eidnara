#!/usr/bin/env bun

import { mkdtempSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";
import { EXECUTABLE_LANES, type Harness, type Lane } from "../src/incident-pool/contract";
import { validateEvidenceAndSources } from "../src/incident-pool/evidence";
import { type IncidentHistoryState, validateIncidentHistory } from "../src/incident-pool/history";
import {
    builtinIncidentCaseRegistry,
    type IncidentCaseRegistry,
    implementationBundleDigest,
    validateRegistryCatalogCorrespondence,
} from "../src/incident-pool/registry";
import {
    buildScheduledIncidentReport,
    type IncidentMode,
    type IncidentPoolReport,
    incidentPoolExitCode,
    publishIncidentReport,
    publishScheduledIncidentReport,
    scheduledIncidentExitCode,
    scoredBaselineMismatches,
    unexpectedIncompleteResults,
} from "../src/incident-pool/report";
import {
    buildRunSnapshot,
    DEFAULT_CASE_TIMEOUT_MS,
    runCaseInIsolation,
    runIncidentPool,
    unavailableCaseResult,
} from "../src/incident-pool/runner";
import { detectRustModePrereqs } from "../src/rust-runner/hermetic-host";
import { detectRustPrerequisites } from "./check-rust-prerequisites";
import { E2E_ROOT, INCIDENTS_DIR, loadHistorySnapshot } from "./validate-incident-history";

const REPO_ROOT = resolve(E2E_ROOT, "../..");

interface CliArgs {
    mode: IncidentMode | null;
    variants: string[];
    reportPath: string;
    timeoutMs: number;
}

function parseArgs(args: string[]): CliArgs {
    let mode: IncidentMode | null = null;
    const variants: string[] = [];
    let reportPath: string | null = null;
    let timeoutMs = DEFAULT_CASE_TIMEOUT_MS;
    for (let index = 0; index < args.length; index += 1) {
        const arg = args[index];
        if (arg === "--mode") {
            const value = args[++index];
            if (value !== "rust") throw new Error("--mode requires rust");
            mode = value;
        } else if (arg === "--variant") {
            const value = args[++index];
            if (!value) throw new Error("--variant requires a variant id");
            variants.push(value);
        } else if (arg === "--report") {
            const value = args[++index];
            if (!value) throw new Error("--report requires a file path");
            reportPath = resolve(value);
        } else if (arg === "--timeout") {
            const value = Number(args[++index]);
            if (!Number.isInteger(value) || value <= 0) {
                throw new Error("--timeout requires positive milliseconds");
            }
            timeoutMs = value;
        } else if (arg === "--help" || arg === "-h") {
            console.log(
                "Usage: run-incident-pool.ts [--mode rust] [--variant <id>]... [--report <path>] [--timeout <ms>]",
            );
            process.exit(0);
        } else {
            throw new Error(`unknown argument: ${arg}`);
        }
    }
    if (mode !== null && variants.length > 0) {
        throw new Error("--variant requires a single-harness run without --mode");
    }
    const defaultPath = mode
        ? resolve(E2E_ROOT, "artifacts", `incident-pool-${mode}-report.json`)
        : resolve(E2E_ROOT, "incident-report.json");
    return {
        mode,
        variants: [...new Set(variants)],
        reportPath: reportPath ?? defaultPath,
        timeoutMs,
    };
}

async function runHarness(
    state: IncidentHistoryState,
    adjudicationLines: readonly string[],
    registry: IncidentCaseRegistry,
    implementationDigests: ReadonlyMap<string, string>,
    harness: Harness,
    lanes: Lane[],
    variants: string[],
    timeoutMs: number,
    workspaceParentDir: string,
): Promise<IncidentPoolReport> {
    const snapshot = buildRunSnapshot({
        catalog: state.catalog,
        ledger: state.ledger,
        adjudicationLines,
        harness,
        lanes,
        variantIds: variants.length > 0 ? variants : undefined,
        implementationDigests,
    });
    for (const excluded of snapshot.excluded) {
        console.error(
            `[incident-pool:${harness}] not scheduled ${excluded.variantId}: ${excluded.reason}`,
        );
    }

    const childScript = resolve(import.meta.dir, "run-incident-case.ts");
    const report = await runIncidentPool(snapshot, async (selected) => {
        const registered = registry.get(selected.variantId);
        if (!registered) {
            throw new Error(`selected variant ${selected.variantId} has no registered case`);
        }
        const prerequisite = registered.prerequisite?.() ?? {
            ok: true as const,
        };
        if (!prerequisite.ok) {
            console.error(
                `[incident-pool:${harness}] ${selected.variantId} unavailable: ${prerequisite.reason}`,
            );
            return unavailableCaseResult(selected);
        }
        const execution = await runCaseInIsolation(snapshot, selected, {
            argv: [process.execPath, childScript],
            timeoutMs,
            workspaceParentDir,
            extraEnv: {
                EIDNARA_E2E_MODE: "rust",
            },
        });
        console.error(
            `[incident-pool:${harness}] ${selected.variantId}: ${execution.result.run_health} / ` +
                `${execution.result.behavioral_verdict} / ${execution.result.baseline_comparison}`,
        );
        return execution.result;
    });
    for (const incomplete of unexpectedIncompleteResults(report)) {
        console.error(
            `[incident-pool:${harness}] unexpected incomplete result ${incomplete.variant_id}: ` +
                `${incomplete.run_health} (${incomplete.reason_code ?? "no reason"})`,
        );
    }
    for (const mismatch of scoredBaselineMismatches(report)) {
        console.error(
            `[incident-pool:${harness}] baseline mismatch ${mismatch.variant_id}: ` +
                `${mismatch.baseline_comparison} (failed: ${mismatch.failed_checks.join(", ") || "none"})`,
        );
    }
    return report;
}

async function main(): Promise<number> {
    const args = parseArgs(Bun.argv.slice(2));
    // The runner deletes the previous report before validation so validation failures cannot leave a stale report.
    rmSync(args.reportPath, { force: true });
    const files = loadHistorySnapshot(INCIDENTS_DIR, "working");
    const state = validateIncidentHistory(files);
    const registry = builtinIncidentCaseRegistry();
    validateRegistryCatalogCorrespondence(registry, state.catalog);
    validateEvidenceAndSources(state.inventory, state.catalog);

    const implementationDigests = new Map<string, string>();
    for (const [variantId, registered] of registry) {
        implementationDigests.set(
            variantId,
            implementationBundleDigest(REPO_ROOT, registered.implementationFiles),
        );
    }

    // The runtime probe runs before any fixture build: without the shared-memory channel no
    // case can reach the daemon, so the pool prints the skip and exits clean instead of
    // publishing a report whose every result is `unavailable`.
    const runtime = detectRustModePrereqs();
    if (!runtime.ok) {
        console.log(`[incident-pool] SKIPPED: ${runtime.skipReason ?? "unknown reason"}`);
        return 0;
    }
    const harnesses: Harness[] = ["rust"];
    const lanes: Lane[] = [...EXECUTABLE_LANES];
    const workspaceParentDir = mkdtempSync(join(tmpdir(), "incident-pool-"));
    const prereqs = detectRustPrerequisites({ allowBuild: true });
    if (!prereqs.ok) {
        console.error(
            `[incident-pool] rust prerequisites unresolved: ${prereqs.missing.join("; ")}`,
        );
    } else if (prereqs.fixtureBin) {
        process.env.EIDNARA_E2E_DIRECT_HOST_FIXTURE_BIN = prereqs.fixtureBin;
    }
    try {
        const reports: IncidentPoolReport[] = [];
        for (const harness of harnesses) {
            reports.push(
                await runHarness(
                    state,
                    files.adjudicationLines,
                    registry,
                    implementationDigests,
                    harness,
                    lanes,
                    args.variants,
                    args.timeoutMs,
                    workspaceParentDir,
                ),
            );
        }
        if (args.mode) {
            const scheduled = buildScheduledIncidentReport(args.mode, reports);
            publishScheduledIncidentReport(scheduled, args.reportPath);
            console.log(
                `published ${args.reportPath}: ${scheduled.variant_count} variants in ${scheduled.family_count} families, ` +
                    `evaluation_complete=${scheduled.evaluation_complete}`,
            );
            return scheduledIncidentExitCode(scheduled);
        }
        const report = reports[0]!;
        publishIncidentReport(report, args.reportPath);
        console.log(
            `published ${args.reportPath}: ${report.variant_count} variants in ${report.family_count} families, ` +
                `evaluation_complete=${report.evaluation_complete}`,
        );
        return incidentPoolExitCode(report);
    } finally {
        rmSync(workspaceParentDir, { recursive: true, force: true });
    }
}

main()
    .then((code) => process.exit(code))
    .catch((error: unknown) => {
        console.error(
            `incident pool run failed: ${error instanceof Error ? error.message : String(error)}`,
        );
        process.exit(1);
    });
