import { describe, expect, it } from "bun:test";
import { cpSync, mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";
import { parseSourceInventory } from "../src/incident-pool/contract";
import {
    E2E_ROOT,
    REPO_ROOT,
    scanSources,
    verifySourceCompleteness,
} from "../src/incident-pool/evidence";
import { syncMutationInventory } from "./mutation-inventory";

function executedRecord(name: string) {
    return {
        name,
        applied_diff: { path: "x", before: "a", after: "b", changed: true },
        observed_failure: { exit_status: 1, output: "FAILED" },
        reverted_rerun: { exit_status: 0, output: "ok", status: "pass" },
        adequacy_finding: null,
    };
}

function copyCommittedTree(): string {
    const temp = mkdtempSync(join(tmpdir(), "mutation-inventory-"));
    cpSync(resolve(E2E_ROOT, "mutations"), join(temp, "mutations"), { recursive: true });
    mkdirSync(join(temp, "incidents"));
    cpSync(
        resolve(E2E_ROOT, "incidents", "source-inventory.json"),
        join(temp, "incidents", "source-inventory.json"),
    );
    return temp;
}

describe("mutation inventory sync", () => {
    it("registers a new artifact, refreshes digests, drops vanished rows, and leaves other rows alone", () => {
        const temp = copyCommittedTree();
        try {
            const inventoryPath = join(temp, "incidents", "source-inventory.json");
            const before = JSON.parse(readFileSync(inventoryPath, "utf8")) as {
                items: Array<{ id: string; claims: Array<{ rationale: string }> }>;
            };
            const parityBefore = JSON.stringify(
                before.items.find((i) => i.id === "src-parity-findings-s2"),
            );
            before.items.find((i) => i.id === "src-mutation-goldens-dg-2")!.claims[0]!.rationale =
                "reviewed wording that regeneration must keep";
            writeFileSync(inventoryPath, `${JSON.stringify(before, null, 4)}\n`);

            writeFileSync(
                join(temp, "mutations", "fm-oc-2.json"),
                JSON.stringify({
                    command: "bun test tests/rust-fm-oc-2.test.ts",
                    mutations: [executedRecord("FM_OC_2_RUNG_DELETION")],
                }),
            );
            const dg1 = JSON.parse(
                readFileSync(join(temp, "mutations", "goldens-dg-1.json"), "utf8"),
            );
            dg1.mutations[0].observed_failure.output += "\n(rerun)";
            writeFileSync(
                join(temp, "mutations", "goldens-dg-1.json"),
                `${JSON.stringify(dg1, null, 2)}\n`,
            );
            rmSync(join(temp, "mutations", "goldens-dg-3.json"));

            const sync = syncMutationInventory(temp, REPO_ROOT);
            expect(sync.added).toEqual(["src-mutation-fm-oc-2"]);
            expect(sync.updated).toEqual(["src-mutation-goldens-dg-1"]);
            expect(sync.removed).toEqual(["src-mutation-goldens-dg-3"]);
            expect(sync.artifacts).toBe(3);
            expect(sync.records).toBe(3);

            const after = JSON.parse(readFileSync(inventoryPath, "utf8")) as {
                items: Array<{
                    id: string;
                    claims: Array<{ id: string; rationale: string; disposition: string }>;
                }>;
            };
            expect(JSON.stringify(after.items.find((i) => i.id === "src-parity-findings-s2"))).toBe(
                parityBefore,
            );
            expect(
                after.items.find((i) => i.id === "src-mutation-goldens-dg-2")!.claims[0]!.rationale,
            ).toBe("reviewed wording that regeneration must keep");
            const added = after.items.find((i) => i.id === "src-mutation-fm-oc-2")!;
            expect(added.claims.map((c) => [c.id, c.disposition])).toEqual([
                ["claim-mutation-fm-oc-2-rung-deletion", "verifier_evidence"],
            ]);
            // Accepted rows keep their order and the new row is appended, although "fm-oc-2" sorts before "goldens-*".
            expect(after.items.map((i) => i.id)).toEqual([
                "src-parity-findings-s2",
                "src-mutation-goldens-dg-1",
                "src-mutation-goldens-dg-2",
                "src-mutation-fm-oc-2",
            ]);
            verifySourceCompleteness(
                parseSourceInventory(JSON.parse(readFileSync(inventoryPath, "utf8"))),
                scanSources(REPO_ROOT, temp),
            );
        } finally {
            rmSync(temp, { recursive: true, force: true });
        }
    });

    it("is a no-op on the committed tree", () => {
        const temp = copyCommittedTree();
        try {
            const inventoryPath = join(temp, "incidents", "source-inventory.json");
            const original = readFileSync(inventoryPath, "utf8");
            const sync = syncMutationInventory(temp, REPO_ROOT);
            expect([sync.added, sync.updated, sync.removed]).toEqual([[], [], []]);
            expect(readFileSync(inventoryPath, "utf8")).toBe(original);
        } finally {
            rmSync(temp, { recursive: true, force: true });
        }
    });
});
