import { readFileSync, writeFileSync } from "node:fs";
import { resolve } from "node:path";
import { E2E_ROOT, REPO_ROOT, scanSources } from "../src/incident-pool/evidence";

const MUTATION_RATIONALE =
    "committed mutation record: crafted invalid state observed red and reverted rerun observed green against its bound verifier";

interface InventoryClaim {
    id: string;
    content_digest: string;
    disposition: string;
    rationale: string;
    family_links: string[];
}

interface InventoryItem {
    id: string;
    source_path: string;
    content_digest: string;
    claims: InventoryClaim[];
}

export interface MutationInventorySync {
    added: string[];
    updated: string[];
    removed: string[];
    artifacts: number;
    records: number;
}

/**
 * Matching claims retain their existing rationale and family links; non-mutation items remain unchanged.
 * Existing mutation rows keep their positions and new rows are appended, because the accepted-snapshot comparison rejects an insertion before accepted history.
 */
export function syncMutationInventory(
    e2eRoot: string = E2E_ROOT,
    repoRoot: string = REPO_ROOT,
): MutationInventorySync {
    const path = resolve(e2eRoot, "incidents", "source-inventory.json");
    const inventory = JSON.parse(readFileSync(path, "utf8")) as { items: InventoryItem[] };
    const scanned = scanSources(repoRoot, e2eRoot).filter((item) =>
        item.id.startsWith("src-mutation-"),
    );
    const existing = new Map(
        inventory.items
            .filter((item) => item.id.startsWith("src-mutation-"))
            .map((item) => [item.id, item] as const),
    );
    const result: MutationInventorySync = {
        added: [],
        updated: [],
        removed: [],
        artifacts: scanned.length,
        records: scanned.reduce((total, item) => total + item.claims.length, 0),
    };

    const nextById = new Map<string, InventoryItem>();
    for (const item of scanned) {
        const previous = existing.get(item.id);
        const previousClaims = new Map(
            (previous?.claims ?? []).map((claim) => [claim.id, claim] as const),
        );
        const next: InventoryItem = {
            id: item.id,
            source_path: item.sourcePath,
            content_digest: item.digest,
            claims: item.claims.map((claim) => ({
                id: claim.id,
                content_digest: claim.digest,
                disposition: "verifier_evidence",
                rationale: previousClaims.get(claim.id)?.rationale ?? MUTATION_RATIONALE,
                family_links: previousClaims.get(claim.id)?.family_links ?? [],
            })),
        };
        if (!previous) result.added.push(item.id);
        else if (JSON.stringify(previous) !== JSON.stringify(next)) result.updated.push(item.id);
        nextById.set(item.id, next);
    }
    for (const id of existing.keys()) {
        if (!nextById.has(id)) result.removed.push(id);
    }

    const retained: InventoryItem[] = [];
    for (const item of inventory.items) {
        if (!item.id.startsWith("src-mutation-")) {
            retained.push(item);
            continue;
        }
        const next = nextById.get(item.id);
        if (next) {
            retained.push(next);
            nextById.delete(item.id);
        }
    }
    inventory.items = [...retained, ...nextById.values()];
    writeFileSync(path, `${JSON.stringify(inventory, null, 4)}\n`);
    return result;
}

export function reportMutationInventorySync(sync: MutationInventorySync): void {
    for (const id of sync.added) console.log(`inventory: added ${id}`);
    for (const id of sync.updated) console.log(`inventory: updated ${id}`);
    for (const id of sync.removed) console.log(`inventory: removed ${id}`);
    console.log(
        `inventory: ${sync.artifacts} mutation artifacts, ${sync.records} records; ` +
            "EXPECTED_MUTATION_ARTIFACTS and EXPECTED_MUTATION_RECORDS in src/incident-pool/evidence.ts must match",
    );
}
