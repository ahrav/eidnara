import { describe, expect, test } from "bun:test";
import { mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { audit, checkGate, EIDNARA_PACKAGES, run, SCHEMA, type CommandResult, type Runner } from "./registry-audit";

const now = new Date("2026-09-02T12:00:00Z");

function ok(stdout: string): CommandResult {
    return { status: 0, stdout, stderr: "" };
}

function e404(): CommandResult {
    return { status: 1, stdout: "", stderr: "npm error code E404\nnpm error 404 Not Found" };
}

function fakeRunner(overrides: Record<string, CommandResult> = {}): Runner {
    return (args) => {
        const command = args.join(" ");
        if (command in overrides) return overrides[command]!;
        if (command === "--version") return ok("11.16.0\n");
        if (args[0] === "view") {
            const name = args[1] ?? "";
            if (name.startsWith("@eidnara/")) {
                return ok(JSON.stringify({ versions: ["1.0.0-reserved.1"], "dist-tags": { reserved: "1.0.0-reserved.1" } }));
            }
            return ok(JSON.stringify({ versions: ["0.38.0"], "dist-tags": { latest: "0.38.0" } }));
        }
        if (args[0] === "trust") return ok("(no trusted publishers)\n");
        if (args[0] === "token") return ok("(no tokens)\n");
        if (args[0] === "org") return ok("ahrav - owner\n");
        return { status: 1, stdout: "", stderr: "npm error code EUNKNOWN" };
    };
}

describe("registry audit", () => {
    test("captures one probe per name plus trust, token, and org probes with digests", () => {
        const gate = audit(fakeRunner(), now);
        expect(gate.schema).toBe(SCHEMA);
        expect(gate.npm_version).toBe("11.16.0");
        expect(gate.probes).toHaveLength(EIDNARA_PACKAGES.length * 2 + 2);
        for (const probe of gate.probes) {
            expect(probe.response_sha256).toMatch(/^[0-9a-f]{64}$/);
            expect(typeof probe.exit_status).toBe("number");
        }
        expect(checkGate(gate, now)).toEqual([]);
        expect(checkGate(gate, now, { requireReservation: true })).toEqual([]);
    });

    test("rejects a gate older than 24 hours", () => {
        const gate = audit(fakeRunner(), now);
        const later = new Date(now.getTime() + 25 * 60 * 60 * 1000);
        expect(checkGate(gate, later)).toContain(`gate is stale: captured ${gate.captured_at}, older than 24 hours`);
        expect(checkGate(gate, new Date(now.getTime() + 23 * 60 * 60 * 1000))).toEqual([]);
    });

    test("rejects missing digests, missing probes, and unsupported npm", () => {
        const gate = audit(fakeRunner(), now);
        const probe = gate.probes[0]!;
        const broken = { ...gate, probes: [{ ...probe, response_sha256: "" }] };
        const errors = checkGate(broken, now);
        expect(errors).toContain(`probes[0] (${probe.command}) is missing a response digest`);
        expect(errors).toContain("missing token probe");
        expect(errors).toContain("missing org probe");
        expect(checkGate({ ...gate, npm_version: "10.9.0" }, now)).toContain("npm_version 10.9.0 does not support npm trust; need >= 11");
    });

    test("a view probe that lost its package name or summary is an error, not a skipped package", () => {
        const gate = audit(fakeRunner({}), now);
        const probes = gate.probes as unknown as Record<string, unknown>[];
        const view = probes.findIndex((probe) => probe.command === "npm view @eidnara/cli versions dist-tags --json");
        expect(view).toBeGreaterThanOrEqual(0);
        const nameless = { ...gate, probes: probes.map((probe, index) => (index === view ? { ...probe, name: null } : probe)) };
        expect(checkGate(nameless, now, { requireReservation: true })).toContain(
            "probes[" + view + "] (npm view @eidnara/cli versions dist-tags --json) must name the package it viewed",
        );
        expect(checkGate(nameless, now, { requireReservation: true })).toContain("@eidnara/cli has no usable view summary");
        const summaryless = { ...gate, probes: probes.map((probe, index) => (index === view ? { ...probe, summary: null } : probe)) };
        expect(checkGate(summaryless, now)).toContain("probes[" + view + "] (npm view @eidnara/cli versions dist-tags --json) is missing its summary");
    });

    test("a malformed view summary is an error rather than an unpublished name", () => {
        const gate = audit(fakeRunner({}), now);
        const probes = gate.probes as unknown as Record<string, unknown>[];
        const view = probes.findIndex((probe) => probe.command === "npm view @eidnara/cli versions dist-tags --json");
        const command = "npm view @eidnara/cli versions dist-tags --json";
        const withSummary = (summary: unknown) => ({ ...gate, probes: probes.map((probe, index) => (index === view ? { ...probe, summary } : probe)) });
        expect(checkGate(withSummary({}), now)).toContain(`probes[${view}] (${command}) has summary state null; expected published, unpublished, or error`);
        expect(checkGate(withSummary({}), now)).toContain("@eidnara/cli has no usable view summary");
        expect(checkGate(withSummary([]), now)).toContain(`probes[${view}] (${command}) is missing its summary`);
        expect(checkGate(withSummary({ state: "reserved" }), now)).toContain(
            `probes[${view}] (${command}) has summary state "reserved"; expected published, unpublished, or error`,
        );
        expect(checkGate(withSummary({ state: "published" }), now)).toContain(`probes[${view}] (${command}) is published without a string array of versions`);
        expect(checkGate(withSummary({ state: "published", versions: ["1.0.0-reserved.1", 7] }), now)).toContain(
            `probes[${view}] (${command}) is published without a string array of versions`,
        );
        expect(checkGate(withSummary({ state: "published", versions: ["1.0.0-reserved.1"], dist_tags: {} }), now)).toEqual([]);
        expect(checkGate(withSummary({ state: "unpublished", code: "E404" }), now)).toEqual([]);
    });

    test("rejects a non-prerelease @eidnara version", () => {
        const runner = fakeRunner({
            "view @eidnara/cli versions dist-tags --json": ok(JSON.stringify({ versions: ["1.0.0-reserved.1", "1.0.0"], "dist-tags": { latest: "1.0.0" } })),
        });
        const errors = checkGate(audit(runner, now), now);
        expect(errors).toContain("@eidnara/cli has non-prerelease versions 1.0.0; genesis has not been published");
    });

    test("--require-reservation demands an inert reservation on every @eidnara name", () => {
        const runner = fakeRunner({ "view @eidnara/pi versions dist-tags --json": e404() });
        const gate = audit(runner, now);
        expect(checkGate(gate, now)).toEqual([]);
        expect(checkGate(gate, now, { requireReservation: true })).toContain(
            "@eidnara/pi holds no inert reservation version (expected 1.0.0-reserved.N); observed state unpublished",
        );
    });

    test("run writes the gate file and --check reads it back", () => {
        const root = mkdtempSync(join(tmpdir(), "eidnara-registry-audit-"));
        try {
            mkdirSync(join(root, "release"));
            expect(run([], root, fakeRunner(), now)).toBe(0);
            const written = JSON.parse(readFileSync(join(root, "release", "registry-gate.json"), "utf8")) as { schema: string };
            expect(written.schema).toBe(SCHEMA);
            expect(run(["--check"], root, fakeRunner(), now)).toBe(0);
            expect(run(["--check", "--require-reservation"], root, fakeRunner(), now)).toBe(0);
            expect(run(["--check"], root, fakeRunner(), new Date(now.getTime() + 48 * 60 * 60 * 1000))).toBe(1);
            writeFileSync(join(root, "release", "registry-gate.json"), "{not json");
            expect(run(["--check"], root, fakeRunner(), now)).toBe(1);
            expect(run(["--bogus"], root, fakeRunner(), now)).toBe(2);
        } finally {
            rmSync(root, { recursive: true, force: true });
        }
    });
});
