import { describe, expect, test } from "bun:test";
import { mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { validateShape } from "./check";
import { buildIndex, parseCatalog, run } from "./generate-property-index";

const catalog = `# Part 1 catalog

## Provenance

Some prose with a colon: not a field.

## Group A

### quarantine-authority-survives-peer-writes

Type: safety
Reachability: default-production — the ring transport is built unconditionally
(\`crates/mc-host/src/runtime.rs:741\`), so this code is on the shipped path.
Status: active
Exercised: not yet — needs a peer that writes the lifecycle page directly.
Guarantee: Once a direction is quarantined locally, no action by the peer can
make it accept a reserve again.
Check: \`always\` — after \`enter_quarantine()\`, every operation still returns
\`Quarantined\`.
Fault/timing angle: the peer writes 0 to the flag between two gate reads.
Required faults and enabling state: a quarantine trigger and a peer write.
Confidence: high — [evidence](evidence/quarantine.md). Verified by reading the
gate at \`ring.rs:12\`.
Existing check: none
Impact: a quarantined ring accepts traffic again.
Open questions:
- Does the peer ever legitimately write the page? (needs human input)
- Second question that wraps onto
  the next line.

### custody-terminal-transition-exactly-once

Type: safety — revised from liveness after review.
Reachability: test-only. Invalidated rather than live: the backend is deleted.
Reaches production: no
Status: invalidated
Exercised: yes — the former test drove both orders.
Guarantee: Each candidate's charges are released exactly once.
Check: \`always\` — \`release(); assert!(!release())\`.
Fault/timing angle: two terminal transitions racing.
Required faults and enabling state: none constructible now.
Confidence: high on the mechanism — [evidence](evidence/custody.md).
Existing check: none
Impact: none; mechanism deleted.
Open questions: None.
`;

describe("property index generator", () => {
    test("parses METHOD records, tolerating wrapped fields, extra fields, and enum suffixes", () => {
        const errors: string[] = [];
        const records = parseCatalog(catalog, errors);
        expect(errors).toEqual([]);
        expect(records).toHaveLength(2);
        const first = records[0]!;
        expect(first.slug).toBe("quarantine-authority-survives-peer-writes");
        expect(first.type).toBe("safety");
        expect(first.reachability).toBe("default-production");
        expect(first.reachability_note).toContain("built unconditionally (`crates/mc-host/src/runtime.rs:741`)");
        expect(first.exercised).toEqual({ state: "not-yet", note: "needs a peer that writes the lifecycle page directly." });
        expect(first.check.semantics).toBe("always");
        expect(first.check.condition).toContain("every operation still returns `Quarantined`.");
        expect(first.confidence.level).toBe("high");
        expect(first.confidence.evidence).toContain("[evidence](evidence/quarantine.md)");
        expect(first.open_questions).toEqual([
            "Does the peer ever legitimately write the page? (needs human input)",
            "Second question that wraps onto the next line.",
        ]);
        const second = records[1]!;
        expect(second.type).toBe("safety");
        expect(second.reachability).toBe("test-only");
        expect(second.status).toBe("invalidated");
        expect(second.unreachability_evidence).toContain("Invalidated rather than live");
        expect(second.extra["Reaches production"]).toBe("no");
        expect(second.confidence.level).toBe("high");
        expect(second.open_questions).toEqual([]);
    });

    test("reports missing fields, duplicate slugs, and unquoted check semantics", () => {
        const broken = `### dup

Type: safety
Status: active
Check: always

### dup

Type: safety
Status: active
Check: \`always\` — ok
`;
        const errors: string[] = [];
        parseCatalog(broken, errors);
        expect(errors).toContain("record dup: duplicate slug");
        expect(errors).toContain("record dup: missing field Guarantee");
        expect(errors).toContain("record dup: Check must start with a backquoted semantics term");
    });

    test("generated index validates against the property-catalog checker", () => {
        const root = mkdtempSync(join(tmpdir(), "eidnara-property-index-"));
        try {
            const part = join(root, "part-1-shm-transport");
            mkdirSync(part);
            writeFileSync(join(part, "catalog.md"), catalog);
            const errors: string[] = [];
            const index = buildIndex(part, errors);
            expect(errors).toEqual([]);
            expect(index?.part).toBe("part-1-shm-transport");
            expect(validateShape("property-catalog", index)).toEqual([]);
        } finally {
            rmSync(root, { recursive: true, force: true });
        }
    });

    test("--check detects drift between catalog.md and index.json", () => {
        const root = mkdtempSync(join(tmpdir(), "eidnara-property-index-drift-"));
        try {
            const part = join(root, "part-x");
            mkdirSync(part);
            writeFileSync(join(part, "catalog.md"), catalog);
            expect(run([part, "--check"])).toBe(1);
            expect(run([part])).toBe(0);
            expect(run([part, "--check"])).toBe(0);
            const written = readFileSync(join(part, "index.json"), "utf8");
            expect(written.endsWith("\n")).toBe(true);
            writeFileSync(join(part, "catalog.md"), `${catalog}\n### new-record\n\nType: safety\nStatus: active\n`);
            expect(run([part, "--check"])).toBe(1);
            expect(run([])).toBe(2);
        } finally {
            rmSync(root, { recursive: true, force: true });
        }
    });
});
