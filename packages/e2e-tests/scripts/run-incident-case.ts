#!/usr/bin/env bun

/**
 * The child appends exactly one schema-versioned envelope line to `EIDNARA_INCIDENT_ENVELOPE_PATH`;
 * stdout and stderr carry diagnostics only.
 */

import { appendFileSync, readFileSync } from "node:fs";
import { resolve } from "node:path";
import { type IncidentVariant, parseIncidentCatalog } from "../src/incident-pool/contract";
import { rowDigest, splitLedgerLines } from "../src/incident-pool/history";
import {
    builtinIncidentCaseRegistry,
    implementationBundleDigest,
    ledgerFingerprint,
    semanticFingerprint,
} from "../src/incident-pool/registry";
import { CASE_ENVELOPE_SCHEMA, type CaseEnvelope } from "../src/incident-pool/runner";

const E2E_ROOT = resolve(import.meta.dir, "..");
const REPO_ROOT = resolve(E2E_ROOT, "../..");
const INCIDENTS_DIR = resolve(E2E_ROOT, "incidents");

function requireEnv(name: string): string {
    const value = process.env[name];
    if (typeof value !== "string" || value.length === 0) {
        throw new Error(`missing required case environment variable ${name}`);
    }
    return value;
}

function findVariant(variantId: string): IncidentVariant {
    const catalogText = readFileSync(resolve(INCIDENTS_DIR, "catalog.json"), "utf8");
    let rawCatalog: unknown;
    try {
        rawCatalog = JSON.parse(catalogText) as unknown;
    } catch (error) {
        throw new Error(`committed catalog.json is not valid JSON: ${String(error)}`);
    }
    const catalog = parseIncidentCatalog(rawCatalog);
    for (const family of catalog.families) {
        for (const variant of family.variants) {
            if (variant.id === variantId) return variant;
        }
    }
    throw new Error(`variant ${variantId} is not in the committed catalog`);
}

async function main(envelopePath: string): Promise<void> {
    const variantId = requireEnv("EIDNARA_INCIDENT_VARIANT_ID");
    const registry = builtinIncidentCaseRegistry();
    const registered = registry.get(variantId);
    if (!registered) {
        throw new Error(
            `variant ${variantId} has no registered case; scenario units register cases in builtinIncidentCaseRegistry`,
        );
    }
    const variant = findVariant(variantId);
    const adjudicationLines = splitLedgerLines(
        readFileSync(resolve(INCIDENTS_DIR, "adjudications.jsonl"), "utf8"),
    );

    const raw = await registered.driver({
        workspaceRoot: requireEnv("EIDNARA_INCIDENT_WORKSPACE_ROOT"),
        storeDir: requireEnv("EIDNARA_INCIDENT_STORE_DIR"),
        storeNamespace: requireEnv("EIDNARA_INCIDENT_STORE_NAMESPACE"),
    });
    const observation = registered.normalizer(raw);
    const precondition = registered.precondition(observation);

    const envelope: CaseEnvelope = {
        schema: CASE_ENVELOPE_SCHEMA,
        run_nonce: requireEnv("EIDNARA_INCIDENT_RUN_NONCE"),
        variant_id: variantId,
        semantic_fingerprint: semanticFingerprint(variant, registered.fixtures),
        implementation_digest: implementationBundleDigest(
            REPO_ROOT,
            registered.implementationFiles,
        ),
        ledger_fingerprint: ledgerFingerprint(adjudicationLines),
        baseline_event_id: requireEnv("EIDNARA_INCIDENT_BASELINE_EVENT_ID"),
        preconditions: "satisfied",
        precondition_reason: null,
        blocked_by: [],
        verdict: null,
        failed_checks: [],
        observation_signature: null,
    };

    if (precondition.satisfied) {
        const checks = registered.verifier(observation);
        // A verifier must emit every normative check before the case can pass.
        const emitted = new Set(checks.map((entry) => entry.id));
        for (const required of variant.normative_checks) {
            if (!emitted.has(required)) {
                throw new Error(
                    `verifier for ${variantId} omitted declared normative check ${required}`,
                );
            }
        }
        const failed = checks.filter((check) => !check.passed).map((check) => check.id);
        envelope.verdict = failed.length === 0 ? "pass" : "assertion_fail";
        envelope.failed_checks = failed;
        envelope.observation_signature = failed.length === 0 ? null : rowDigest(observation);
    } else {
        envelope.precondition_reason = precondition.reason;
        envelope.preconditions = "failed";
        envelope.blocked_by =
            precondition.reason === "blocked_by_dependency" ? precondition.blockedBy : [];
    }

    // Appending keeps a second write visible to the parent as a duplicate envelope instead of silently replacing the first.
    appendFileSync(envelopePath, `${JSON.stringify(envelope)}\n`, { mode: 0o600 });
}

// Without a destination the parent can never score this case, so the child stops before running the driver.
const envelopePath = process.env.EIDNARA_INCIDENT_ENVELOPE_PATH;
if (typeof envelopePath !== "string" || envelopePath.length === 0) {
    console.error("incident case failed: EIDNARA_INCIDENT_ENVELOPE_PATH is not set");
    process.exit(2);
}

main(envelopePath)
    .then(() => process.exit(0))
    .catch((error: unknown) => {
        console.error(
            `incident case failed: ${error instanceof Error ? error.message : String(error)}`,
        );
        process.exit(1);
    });
