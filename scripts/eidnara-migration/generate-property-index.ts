import { createHash } from "node:crypto";
import { existsSync, readFileSync, writeFileSync } from "node:fs";
import { basename, join, resolve } from "node:path";

// Field order follows docs/properties/METHOD.md "Record schema".
const KNOWN_FIELDS: Record<string, string> = {
    Type: "type",
    Reachability: "reachability",
    Status: "status",
    Exercised: "exercised",
    Guarantee: "guarantee",
    Check: "check",
    "Fault/timing angle": "fault_timing",
    "Required faults and enabling state": "required_faults",
    Confidence: "confidence",
    "Existing check": "existing_check",
    Impact: "impact",
    "Open questions": "open_questions",
    Invalidated: "invalidated",
};

const FIELD_LINE_RE = /^([A-Z][A-Za-z/ ]{1,60}):(?:\s+(.*))?$/;
const HEADING_RE = /^### (\S+)\s*$/;
const DASH = /\s+[—-]\s+/;

export interface IndexRecord {
    slug: string;
    type: string;
    reachability: string;
    reachability_note: string;
    status: string;
    exercised: { state: string; note: string };
    guarantee: string;
    check: { semantics: string; condition: string };
    fault_timing: string;
    required_faults: string;
    confidence: { level: string; evidence: string };
    existing_check: string;
    impact: string;
    open_questions: string[];
    unreachability_evidence?: string;
    extra: Record<string, string>;
}

export interface PropertyIndex {
    schema_version: 1;
    part: string;
    source: string;
    source_sha256: string;
    records: IndexRecord[];
}

interface RawRecord {
    slug: string;
    fields: Map<string, string[]>;
    order: string[];
}

function splitRecords(markdown: string): RawRecord[] {
    const records: RawRecord[] = [];
    let current: RawRecord | undefined;
    let field: string | undefined;
    for (const rawLine of markdown.split("\n")) {
        const line = rawLine.replace(/\s+$/, "");
        const heading = HEADING_RE.exec(line);
        if (heading?.[1] !== undefined) {
            current = { slug: heading[1], fields: new Map(), order: [] };
            records.push(current);
            field = undefined;
            continue;
        }
        if (current === undefined) continue;
        if (line.startsWith("## ") || line.startsWith("# ")) {
            current = undefined;
            field = undefined;
            continue;
        }
        const match = FIELD_LINE_RE.exec(line);
        if (match?.[1] !== undefined) {
            field = match[1];
            current.fields.set(field, [match[2] ?? ""]);
            current.order.push(field);
            continue;
        }
        if (field === undefined) continue;
        if (line === "") {
            field = undefined;
            continue;
        }
        current.fields.get(field)?.push(line);
    }
    return records.filter((record) => record.fields.has("Type") && record.fields.has("Status"));
}

function joinLines(lines: string[] | undefined): string {
    if (lines === undefined) return "";
    return lines
        .map((line) => line.trim())
        .filter((line) => line !== "")
        .join(" ")
        .trim();
}

function splitDash(text: string): [string, string] {
    const match = DASH.exec(text);
    if (match === null || match.index === undefined) return [text.trim(), ""];
    return [text.slice(0, match.index).trim(), text.slice(match.index + match[0].length).trim()];
}

// Leading enum token plus the remainder ("default-production. rest" -> ["default-production", "rest"]).
function leadToken(text: string): [string, string] {
    const match = /^([A-Za-z][A-Za-z-]*)\.?(?:\s+[—-]\s+|\s+|$)(.*)$/s.exec(text.trim());
    if (match === null) return [text.trim(), ""];
    return [match[1] ?? "", (match[2] ?? "").trim()];
}

function normalizeExercised(head: string): string {
    const lower = head.toLowerCase();
    if (lower.startsWith("not yet")) return "not-yet";
    if (lower.startsWith("partial")) return "partial";
    if (lower.startsWith("yes")) return "yes";
    return head;
}

function parseOpenQuestions(lines: string[] | undefined): string[] {
    if (lines === undefined) return [];
    const items: string[] = [];
    for (const raw of lines) {
        const line = raw.trim();
        if (line === "" || /^none\.?$/i.test(line)) continue;
        if (line.startsWith("- ")) items.push(line.slice(2).trim());
        else if (items.length > 0) items[items.length - 1] = `${items[items.length - 1]} ${line}`;
        else items.push(line);
    }
    return items;
}

export function parseCatalog(markdown: string, errors: string[]): IndexRecord[] {
    const out: IndexRecord[] = [];
    const seen = new Set<string>();
    for (const raw of splitRecords(markdown)) {
        const where = `record ${raw.slug}`;
        if (seen.has(raw.slug)) errors.push(`${where}: duplicate slug`);
        seen.add(raw.slug);
        for (const required of Object.keys(KNOWN_FIELDS)) {
            if (required === "Invalidated") continue;
            if (!raw.fields.has(required)) errors.push(`${where}: missing field ${required}`);
        }
        const text = (name: string): string => joinLines(raw.fields.get(name));
        const [type] = leadToken(text("Type"));
        const [reachability, reachabilityNote] = leadToken(text("Reachability"));
        const [status] = leadToken(text("Status"));
        const [exercisedHead, exercisedNote] = splitDash(text("Exercised"));
        const checkText = text("Check");
        const checkMatch = /^`([^`]+)`\s*(?:[—-]\s*)?(.*)$/.exec(checkText);
        const [confidenceLevel, confidenceEvidence] = leadToken(text("Confidence"));
        const record: IndexRecord = {
            slug: raw.slug,
            type,
            reachability,
            reachability_note: reachabilityNote,
            status,
            exercised: { state: normalizeExercised(exercisedHead), note: exercisedNote || exercisedHead },
            guarantee: text("Guarantee"),
            check: {
                semantics: checkMatch?.[1] ?? checkText,
                condition: checkMatch?.[2] ?? checkText,
            },
            fault_timing: text("Fault/timing angle"),
            required_faults: text("Required faults and enabling state"),
            confidence: { level: confidenceLevel, evidence: confidenceEvidence },
            existing_check: text("Existing check"),
            impact: text("Impact"),
            open_questions: parseOpenQuestions(raw.fields.get("Open questions")),
            extra: {},
        };
        if (status === "invalidated") {
            const explicit = text("Invalidated");
            record.unreachability_evidence = explicit !== "" ? explicit : reachabilityNote;
            if (record.unreachability_evidence === "") {
                errors.push(`${where}: invalidated record has no Invalidated: field and no reachability note`);
            }
        }
        for (const name of raw.order) {
            if (Object.hasOwn(KNOWN_FIELDS, name)) continue;
            record.extra[name] = text(name);
        }
        if (checkMatch === null) errors.push(`${where}: Check must start with a backquoted semantics term`);
        out.push(record);
    }
    return out;
}

export function buildIndex(partDir: string, errors: string[]): PropertyIndex | undefined {
    const catalogPath = join(partDir, "catalog.md");
    if (!existsSync(catalogPath)) {
        errors.push(`${catalogPath} does not exist`);
        return undefined;
    }
    const markdown = readFileSync(catalogPath, "utf8");
    const records = parseCatalog(markdown, errors);
    if (records.length === 0) errors.push(`${catalogPath} has no records`);
    return {
        schema_version: 1,
        part: basename(resolve(partDir)),
        source: "catalog.md",
        source_sha256: createHash("sha256").update(markdown).digest("hex"),
        records,
    };
}

export function renderIndex(index: PropertyIndex): string {
    return `${JSON.stringify(index, null, 2)}\n`;
}

export function run(argv: string[]): number {
    const check = argv.includes("--check");
    const parts = argv.filter((arg) => arg !== "--check");
    if (parts.length === 0) {
        console.error("usage: bun scripts/eidnara-migration/generate-property-index.ts <docs/properties/<part>>... [--check]");
        return 2;
    }
    let failed = false;
    for (const part of parts) {
        const errors: string[] = [];
        const index = buildIndex(part, errors);
        if (index === undefined || errors.length > 0) {
            errors.forEach((error) => console.error(error));
            failed = true;
            continue;
        }
        const rendered = renderIndex(index);
        const target = join(part, "index.json");
        if (check) {
            const existing = existsSync(target) ? readFileSync(target, "utf8") : undefined;
            if (existing !== rendered) {
                console.error(`${target} drifts from catalog.md; rerun without --check`);
                failed = true;
            } else {
                console.log(`property-index: OK (${target}, ${index.records.length} records)`);
            }
        } else {
            writeFileSync(target, rendered);
            console.log(`property-index: wrote ${target} (${index.records.length} records)`);
        }
    }
    return failed ? 1 : 0;
}

if (import.meta.main) process.exit(run(process.argv.slice(2)));
