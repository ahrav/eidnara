import { spawnSync } from "node:child_process";
import { createHash } from "node:crypto";
import { existsSync, mkdirSync, readFileSync, writeFileSync } from "node:fs";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";

export const SCHEMA = "eidnara.registry-gate/v1";
export const MAX_AGE_MS = 24 * 60 * 60 * 1000;
export const MIN_NPM_MAJOR = 11;

export const EIDNARA_PACKAGES = [
    "@eidnara/shm-native",
    "@eidnara/host-linux-x64-gnu",
    "@eidnara/cli",
    "@eidnara/opencode",
    "@eidnara/pi",
] as const;

export const NPM_SCOPE = "eidnara";

const PRERELEASE_RE = /^\d+\.\d+\.\d+-[0-9A-Za-z.-]+$/;

export interface CommandResult {
    status: number | null;
    stdout: string;
    stderr: string;
}

export type Runner = (args: string[]) => CommandResult;

export interface Probe {
    name: string | null;
    command: string;
    exit_status: number | null;
    response_sha256: string;
    response_bytes: number;
    summary: Record<string, unknown>;
}

export interface GateFile {
    schema: typeof SCHEMA;
    captured_at: string;
    npm_version: string;
    probes: Probe[];
}

export function npmRunner(args: string[]): CommandResult {
    const result = spawnSync("npm", args, { encoding: "utf8", maxBuffer: 64 * 1024 * 1024 });
    if (result.error) return { status: null, stdout: "", stderr: String(result.error) };
    return { status: result.status, stdout: result.stdout, stderr: result.stderr };
}

function digest(text: string): string {
    return createHash("sha256").update(text).digest("hex");
}

function summarizeView(result: CommandResult): Record<string, unknown> {
    if (result.status !== 0) {
        const code = /npm error code (\S+)/.exec(result.stderr)?.[1] ?? "error";
        return { state: code === "E404" ? "unpublished" : "error", code };
    }
    try {
        const parsed = JSON.parse(result.stdout) as Record<string, unknown>;
        // A successful view of a published name always carries its versions; a response
        // without them proves nothing about the name and is recorded as an error, since a
        // published summary with no versions would otherwise pass as prerelease-only.
        const versions = Array.isArray(parsed.versions) ? parsed.versions.filter((value): value is string => typeof value === "string") : [];
        if (versions.length === 0) return { state: "error", code: "no-versions" };
        return {
            state: "published",
            versions,
            dist_tags: parsed["dist-tags"] ?? {},
        };
    } catch {
        return { state: "error", code: "unparseable" };
    }
}

function probe(run: Runner, name: string | null, args: string[], summarize: (result: CommandResult) => Record<string, unknown>): Probe {
    const result = run(args);
    const raw = `${result.stdout}\n--- stderr ---\n${result.stderr}`;
    return {
        name,
        command: `npm ${args.join(" ")}`,
        exit_status: result.status,
        response_sha256: digest(raw),
        response_bytes: Buffer.byteLength(raw),
        summary: summarize(result),
    };
}

function summarizeExit(result: CommandResult): Record<string, unknown> {
    if (result.status === 0) return { state: "ok", lines: result.stdout.split("\n").filter((line) => line.trim() !== "").length };
    const code = /npm error code (\S+)/.exec(result.stderr)?.[1] ?? "error";
    return { state: code === "ENEEDAUTH" ? "needs-auth" : "error", code };
}

export function audit(run: Runner, now: Date): GateFile {
    const versionResult = run(["--version"]);
    const probes: Probe[] = [];
    for (const name of EIDNARA_PACKAGES) {
        probes.push(probe(run, name, ["view", name, "versions", "dist-tags", "--json"], summarizeView));
    }
    for (const name of EIDNARA_PACKAGES) {
        probes.push(probe(run, name, ["trust", "list", name], summarizeExit));
    }
    probes.push(probe(run, null, ["token", "list"], summarizeExit));
    probes.push(probe(run, null, ["org", "ls", NPM_SCOPE], summarizeExit));
    return {
        schema: SCHEMA,
        captured_at: now.toISOString(),
        npm_version: versionResult.stdout.trim(),
        probes,
    };
}

export interface CheckOptions {
    requireReservation: boolean;
}

// The shapes `summarizeView` records for a view probe. A recorded gate is checked against
// them so an empty object, an array, a published summary with no versions, or a probe whose
// versions are not strings cannot pass as evidence that a name is unpublished.
type ViewSummary =
    | { state: "published"; versions: string[] }
    | { state: "unpublished"; code: unknown }
    | { state: "error"; code: unknown };

const VIEW_STATES = new Set(["published", "unpublished", "error"]);

function viewSummaryProblem(summary: unknown): string | null {
    if (summary === null || typeof summary !== "object" || Array.isArray(summary)) return "is missing its summary";
    const record = summary as Record<string, unknown>;
    if (typeof record.state !== "string" || !VIEW_STATES.has(record.state)) {
        return `has summary state ${JSON.stringify(record.state ?? null)}; expected published, unpublished, or error`;
    }
    if (record.state === "published" && !(Array.isArray(record.versions) && record.versions.length > 0 && record.versions.every((value) => typeof value === "string"))) {
        return "is published without a non-empty string array of versions";
    }
    return null;
}

export function checkGate(value: unknown, now: Date, options: CheckOptions = { requireReservation: false }): string[] {
    const errors: string[] = [];
    if (value === null || typeof value !== "object" || Array.isArray(value)) return ["gate file must be a JSON object"];
    const gate = value as Record<string, unknown>;
    if (gate.schema !== SCHEMA) errors.push(`schema must be ${SCHEMA}`);
    const captured = typeof gate.captured_at === "string" ? Date.parse(gate.captured_at) : Number.NaN;
    if (Number.isNaN(captured)) {
        errors.push("captured_at must be an ISO-8601 timestamp");
    } else if (now.getTime() - captured > MAX_AGE_MS) {
        errors.push(`gate is stale: captured ${gate.captured_at}, older than 24 hours`);
    } else if (captured - now.getTime() > 5 * 60 * 1000) {
        errors.push(`gate captured_at ${gate.captured_at} is in the future`);
    }
    const npmVersion = typeof gate.npm_version === "string" ? gate.npm_version : "";
    const major = Number.parseInt(npmVersion.split(".")[0] ?? "", 10);
    if (Number.isNaN(major) || major < MIN_NPM_MAJOR) {
        errors.push(`npm_version ${npmVersion || "(missing)"} does not support npm trust; need >= ${MIN_NPM_MAJOR}`);
    }
    const probes = Array.isArray(gate.probes) ? (gate.probes as unknown[]) : undefined;
    if (probes === undefined) return [...errors, "probes must be an array"];

    const viewed = new Map<string, ViewSummary>();
    const commands = new Set<string>();
    probes.forEach((entry, index) => {
        if (entry === null || typeof entry !== "object") {
            errors.push(`probes[${index}] must be an object`);
            return;
        }
        const item = entry as Record<string, unknown>;
        const command = typeof item.command === "string" ? item.command : "";
        if (command === "") errors.push(`probes[${index}].command must be a non-empty string`);
        commands.add(command);
        if (typeof item.response_sha256 !== "string" || !/^[0-9a-f]{64}$/.test(item.response_sha256)) {
            errors.push(`probes[${index}] (${command}) is missing a response digest`);
        }
        if (!(typeof item.exit_status === "number" || item.exit_status === null)) {
            errors.push(`probes[${index}] (${command}) is missing exit_status`);
        }
        if (command.startsWith("npm view ")) {
            // A view probe without its package name or summary would satisfy the command
            // check while contributing no evidence, so it is an error rather than skipped.
            const expected = /^npm view (\S+) versions dist-tags --json$/.exec(command)?.[1];
            if (typeof item.name !== "string" || item.name !== expected) {
                errors.push(`probes[${index}] (${command}) must name the package it viewed`);
            } else {
                const problem = viewSummaryProblem(item.summary);
                if (problem !== null) {
                    errors.push(`probes[${index}] (${command}) ${problem}`);
                } else {
                    viewed.set(item.name, item.summary as ViewSummary);
                }
            }
        }
    });

    for (const name of EIDNARA_PACKAGES) {
        if (!commands.has(`npm view ${name} versions dist-tags --json`)) errors.push(`missing view probe for ${name}`);
    }
    for (const name of EIDNARA_PACKAGES) {
        if (!commands.has(`npm trust list ${name}`)) errors.push(`missing trust probe for ${name}`);
    }
    if (!commands.has("npm token list")) errors.push("missing token probe");
    if (!commands.has(`npm org ls ${NPM_SCOPE}`)) errors.push("missing org probe");

    for (const name of EIDNARA_PACKAGES) {
        const summary = viewed.get(name);
        if (summary === undefined) {
            errors.push(`${name} has no usable view summary`);
            continue;
        }
        const versions = summary.state === "published" ? summary.versions : [];
        const ga = versions.filter((version) => !PRERELEASE_RE.test(version));
        if (ga.length > 0) errors.push(`${name} has non-prerelease versions ${ga.join(", ")}; genesis has not been published`);
        if (summary.state === "error") errors.push(`${name} view probe errored (${String(summary.code)})`);
        if (options.requireReservation && !versions.some((version) => /-reserved\.\d+$/.test(version))) {
            errors.push(`${name} holds no inert reservation version (expected 1.0.0-reserved.N); observed state ${String(summary.state)}`);
        }
    }
    return errors;
}

export function run(argv: string[], root: string, runner: Runner = npmRunner, now: Date = new Date()): number {
    const check = argv.includes("--check");
    const options: CheckOptions = { requireReservation: argv.includes("--require-reservation") };
    const unknown = argv.filter((arg) => arg !== "--check" && arg !== "--require-reservation");
    if (unknown.length > 0) {
        console.error("usage: bun scripts/eidnara-migration/registry-audit.ts [--check] [--require-reservation]");
        return 2;
    }
    const target = join(root, "release", "registry-gate.json");
    if (check) {
        if (!existsSync(target)) {
            console.error(`${target} does not exist; run without --check to capture it`);
            return 1;
        }
        let parsed: unknown;
        try {
            parsed = JSON.parse(readFileSync(target, "utf8"));
        } catch (error) {
            console.error(`${target} is not valid JSON: ${String(error)}`);
            return 1;
        }
        const errors = checkGate(parsed, now, options);
        if (errors.length > 0) {
            errors.forEach((error) => console.error(error));
            return 1;
        }
        console.log(`registry-gate: OK (${target})`);
        return 0;
    }
    const gate = audit(runner, now);
    mkdirSync(dirname(target), { recursive: true });
    writeFileSync(target, `${JSON.stringify(gate, null, 2)}\n`);
    const errors = checkGate(gate, now, options);
    console.log(`registry-gate: wrote ${target} (${gate.probes.length} probes)`);
    if (errors.length > 0) {
        errors.forEach((error) => console.error(error));
        return 1;
    }
    return 0;
}

if (import.meta.main) {
    const root = resolve(dirname(fileURLToPath(import.meta.url)), "..", "..");
    process.exit(run(process.argv.slice(2), root));
}
