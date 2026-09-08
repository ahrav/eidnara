/**
 * `RustTestHarness.create` allocates under `os.tmpdir()`, so `createCaseHarness` redirects the
 * sandbox-directory variables during creation and rejects a resolved `dataDir` outside
 * `context.workspaceRoot`.
 */

import { mkdirSync, realpathSync } from "node:fs";
import { join } from "node:path";
import { RustTestHarness, type RustTestHarnessOptions } from "../../rust-harness";
import type { CaseDriverContext } from "../registry";

export async function createCaseHarness(
    context: CaseDriverContext,
    options: RustTestHarnessOptions,
): Promise<RustTestHarness> {
    const harnessTmp = join(context.workspaceRoot, "case-harness");
    mkdirSync(harnessTmp, { recursive: true });
    const saved = {
        TMPDIR: process.env.TMPDIR,
        TMP: process.env.TMP,
        TEMP: process.env.TEMP,
    };
    process.env.TMPDIR = harnessTmp;
    process.env.TMP = harnessTmp;
    process.env.TEMP = harnessTmp;
    let harness: RustTestHarness;
    try {
        harness = await RustTestHarness.create(options);
    } finally {
        for (const [key, value] of Object.entries(saved)) {
            if (value === undefined) delete process.env[key];
            else process.env[key] = value;
        }
    }
    if (!caseHarnessIsWorkspaceScoped(harness, context)) {
        await harness.dispose();
        throw new Error(
            "case harness escaped the case-owned workspace (canonical-path check failed)",
        );
    }
    return harness;
}

export function caseHarnessIsWorkspaceScoped(
    h: RustTestHarness,
    context: CaseDriverContext,
): boolean {
    try {
        const dataDir = realpathSync(h.env.dataDir);
        const root = realpathSync(context.workspaceRoot);
        return dataDir === root || dataDir.startsWith(`${root}/`);
    } catch {
        return false;
    }
}

export function caseNamespaceIsUnique(context: CaseDriverContext): boolean {
    return (
        context.storeNamespace.startsWith("incident-") &&
        context.storeNamespace.length > "incident-".length
    );
}
