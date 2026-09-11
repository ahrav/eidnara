import { describe, expect, it } from "bun:test";
import { readFileSync } from "node:fs";
import { resolve } from "node:path";
import { parseIncidentCatalog } from "../src/incident-pool/contract";
import { boundVerifierFiles, E2E_ROOT, loadMutationEvidence } from "../src/incident-pool/evidence";
import { builtinIncidentCaseRegistry } from "../src/incident-pool/registry";
import {
    assertBoundVerifierBytesUnchanged,
    assertCatalogBindingsUnchanged,
    assertCatalogBoundVerifierBytesUnchanged,
    assertMutationBindingsUnchanged,
    catalogBindings,
    mutationBindings,
} from "./validate-incident-verifiers";

function committedCatalog() {
    return parseIncidentCatalog(
        JSON.parse(readFileSync(resolve(E2E_ROOT, "incidents", "catalog.json"), "utf8")) as unknown,
    );
}

describe("incident verifier contributor gate", () => {
    it("accepts unchanged or newly bound verifier bytes and blocks changed or dropped bindings", () => {
        const bound = { "tests/verifier.test.ts": "a".repeat(64) };
        expect(() => assertBoundVerifierBytesUnchanged(bound, { ...bound })).not.toThrow();
        // An unbound base has no recorded bytes to compare.
        expect(() =>
            assertBoundVerifierBytesUnchanged({}, { "tests/verifier.test.ts": "b".repeat(64) }),
        ).not.toThrow();
        expect(() =>
            assertBoundVerifierBytesUnchanged(bound, { "tests/verifier.test.ts": "b".repeat(64) }),
        ).toThrow(/changed without recorded mutation replay support/);
        // Otherwise deleting the record exempts the verifier from replay.
        expect(() => assertBoundVerifierBytesUnchanged(bound, {})).toThrow(
            /no longer bind accepted verifiers/,
        );
    });
});

describe("catalog-bound executable verifier gate", () => {
    const scenario = "src/incident-pool/scenarios/audit-memory-search.ts";
    const key = `packages/e2e-tests/${scenario}`;

    it("covers every executable module the committed catalog binds", () => {
        const files = boundVerifierFiles(committedCatalog());
        expect(files.length).toBeGreaterThan(0);
        for (const file of files) {
            expect(file.startsWith("src/")).toBe(true);
        }
        expect(files.some((file) => file.startsWith("src/incident-pool/scenarios/"))).toBe(true);
        // The A1/A3 verdicts come from `analyzePasses`, so the gate freezes the oracle module with the bound scenario module.
        expect(files).toContain("src/cache-analysis.ts");
        expect(builtinIncidentCaseRegistry().size).toBe(2);
    });

    it("accepts unchanged or newly bound module bytes and blocks changed or dropped bindings", () => {
        const bound = { [key]: "a".repeat(64) };
        expect(() => assertCatalogBoundVerifierBytesUnchanged(bound, { ...bound })).not.toThrow();
        // The accepted base has no bytes for a newly bound module to drift from.
        expect(() =>
            assertCatalogBoundVerifierBytesUnchanged({}, { [key]: "b".repeat(64) }),
        ).not.toThrow();
        expect(() =>
            assertCatalogBoundVerifierBytesUnchanged(bound, { [key]: "b".repeat(64) }),
        ).toThrow(/changed without recorded replay support/);
        // Otherwise removing the binding exempts the module from the gate.
        expect(() => assertCatalogBoundVerifierBytesUnchanged(bound, {})).toThrow(
            /no longer binds accepted executable verifiers/,
        );
    });
});

describe("per-variant binding gate", () => {
    const module = "src/incident-pool/scenarios/source-linked-regressions.ts";

    it("derives one binding per committed executable variant, including oracle dependencies", () => {
        const bindings = catalogBindings(committedCatalog());
        expect(Object.keys(bindings).sort()).toEqual([
            "var-parity-a1-pure-defer-stability",
            "var-parity-a3-ctx-reduce-survival",
        ]);
        for (const binding of Object.values(bindings)) {
            expect(binding).toContain(`${module}#drive`);
            expect(binding).toContain(`${module}#verify`);
            expect(binding).toContain("src/cache-analysis.ts");
        }
    });

    it("blocks rebinding an accepted variant even when every previously bound path keeps its bytes", () => {
        // Rebinding A1 to a new module leaves A3 holding the accepted paths, so the path-set gates see nothing.
        const accepted = {
            "var-a1": `${module}#driveA1\n${module}#verifyA1\nsrc/cache-analysis.ts`,
            "var-a3": `${module}#driveA3\n${module}#verifyA3\nsrc/cache-analysis.ts`,
        };
        const rebound = {
            ...accepted,
            "var-a1":
                "src/incident-pool/scenarios/other.ts#driveA1\nsrc/incident-pool/scenarios/other.ts#verifyA1\nsrc/cache-analysis.ts",
        };
        expect(() => assertCatalogBindingsUnchanged(accepted, rebound)).toThrow(
            /rebound their verifier without recorded replay support: var-a1/,
        );
        const resymboled = {
            ...accepted,
            "var-a1": accepted["var-a1"].replace("verifyA1", "verifyA1Loose"),
        };
        expect(() => assertCatalogBindingsUnchanged(accepted, resymboled)).toThrow(/var-a1/);
        const droppedDependency = {
            ...accepted,
            "var-a1": `${module}#driveA1\n${module}#verifyA1`,
        };
        expect(() => assertCatalogBindingsUnchanged(accepted, droppedDependency)).toThrow(/var-a1/);
    });

    it("blocks dropping an accepted executable variant's binding and accepts a new variant", () => {
        const accepted = { "var-a1": "a" };
        expect(() => assertCatalogBindingsUnchanged(accepted, {})).toThrow(
            /accepted executable variants no longer bind a verifier: var-a1/,
        );
        expect(() =>
            assertCatalogBindingsUnchanged(accepted, { ...accepted, "var-new": "b" }),
        ).not.toThrow();
        expect(() => assertCatalogBindingsUnchanged({}, { "var-new": "b" })).not.toThrow();
    });
});

describe("per-record mutation binding gate", () => {
    it("derives one binding per committed evidence record with its verifier, fixture, and command", () => {
        const bindings = mutationBindings(loadMutationEvidence());
        expect(Object.keys(bindings).sort()).toEqual([
            "ev-dg-1-one-byte-input",
            "ev-dg-2-one-byte-input",
            "ev-dg-3-one-byte-input",
        ]);
        for (const binding of Object.values(bindings)) {
            expect(binding).toContain("crates/daemon/src/differential_goldens.rs");
            expect(binding).toContain("crates/daemon/testdata/differential-golden.json");
            expect(binding).toContain("cargo test -p daemon --lib dg_goldens_");
        }
    });

    it("blocks rebinding one record while its siblings keep the accepted path", () => {
        const golden =
            "crates/daemon/src/differential_goldens.rs\ncrates/daemon/testdata/differential-golden.json\ncargo test -p daemon --lib dg_goldens_match_ts_wire_surface_and_gate_labels --locked";
        const accepted = { "ev-dg-1": golden, "ev-dg-2": golden, "ev-dg-3": golden };
        const rebound = {
            ...accepted,
            "ev-dg-1":
                "crates/daemon/src/weaker.rs\ncargo test -p daemon --lib weaker::tests::always_green --locked",
        };
        expect(() => assertMutationBindingsUnchanged(accepted, rebound)).toThrow(
            /rebound their verifier without recorded replay support: ev-dg-1/,
        );
        const recommanded = { ...accepted, "ev-dg-2": golden.replace("--locked", "") };
        expect(() => assertMutationBindingsUnchanged(accepted, recommanded)).toThrow(/ev-dg-2/);
        expect(() =>
            assertMutationBindingsUnchanged(accepted, { "ev-dg-2": golden, "ev-dg-3": golden }),
        ).toThrow(/accepted mutation records vanished: ev-dg-1/);
        expect(() =>
            assertMutationBindingsUnchanged(accepted, { ...accepted, "ev-new": "x" }),
        ).not.toThrow();
        expect(() => assertMutationBindingsUnchanged({}, { "ev-new": "x" })).not.toThrow();
    });
});
