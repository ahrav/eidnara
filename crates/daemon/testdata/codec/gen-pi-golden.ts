#!/usr/bin/env bun
import { existsSync, readFileSync, readdirSync, statSync, writeFileSync } from "node:fs";
import { homedir } from "node:os";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const here = dirname(fileURLToPath(import.meta.url));
const outPath = resolve(here, "pi-golden.json");
const defaultSession = join(
  homedir(),
  ".pi/agent/sessions/--Users-ufukaltinok-Work-Projects-Eidnara-anthropic-auth--/2026-05-01T16-48-44-508Z_019de471-4fdc-762d-9286-624dfad0b5fe.jsonl",
);
const sessionsRoot = process.env.PI_SESSIONS_ROOT ?? join(homedir(), ".pi/agent/sessions");
const check = process.argv.includes("--check");

const requiredClasses = [
  "text_signature",
  "thinking_signature",
  "redacted_thinking",
  "image",
  "tool_call_split_pipe",
  "thought_signature",
  "tool_result",
  "tool_result_details",
  "custom_message",
  "compaction",
  "branch_summary",
  "bash_execution",
  "aborted_assistant",
] as const;

/** Prefix of the reserved ids the plugin gives rows built from non-message entries. */
const RESERVED_ID_PREFIX = "eidnara:";

type RequiredClass = (typeof requiredClasses)[number];

type CapturedEntry = { path: string; entry: any };

/** Path label of the authored entries below. */
const AUTHORED = "authored";

/**
 * Entries for classes the captured sessions may lack, each written in Pi 0.80.2's session-entry
 * shape. One is used only when no captured entry covers its class.
 */
const authoredEntries: CapturedEntry[] = [
  {
    path: AUTHORED,
    entry: {
      type: "message",
      id: "c3d4e5f6",
      parentId: "ad9f3999",
      timestamp: "2026-05-01T17:00:00.000Z",
      message: {
        role: "bashExecution",
        command: "git status",
        output: "clean",
        exitCode: 0,
        cancelled: false,
        truncated: false,
        timestamp: 1777654800000,
      },
    },
  },
  {
    path: AUTHORED,
    entry: {
      type: "branch_summary",
      id: "d4e5f607",
      parentId: "c3d4e5f6",
      timestamp: "2026-05-01T17:01:00.000Z",
      fromId: "781a4cd4",
      summary: "Explored an alternate fix and returned.",
    },
  },
];

if (check) {
  const golden = JSON.parse(readFileSync(outPath, "utf8"));
  assertInternalConsistency(golden);
  process.exit(0);
}

const files = sessionFiles();
const selected = selectEntries(files);
const rows = selected.entries.map(({ entry }) => rowFromEntry(sanitizeEntry(entry)));
const golden = {
  projection_oracle: {
    status: "todo",
    reason:
      "The Pi provider serializer entry points are not vendored in the Rust workspace test closure; these goldens assert round-trip identity of the {id, message} AgentMessage rows the plugin builds from session entries.",
  },
  generated_from: {
    session_files: [
      ...new Set(selected.entries.map((entry) => entry.path).filter((path) => path !== AUTHORED)),
    ].sort(),
    authored_entries: selected.entries
      .filter((entry) => entry.path === AUTHORED)
      .map((entry) => entry.entry.id),
    selection:
      "JSONL session-entry feature scan over captured Pi session files, then authored entries for classes no captured entry covers",
  },
  coverage: selected.coverage,
  missing_capture_classes: selected.missing,
  cases: [
    {
      name: "captured-pi-feature-rows",
      rows,
    },
  ],
};
assertInternalConsistency(golden);
writeFileSync(outPath, `${JSON.stringify(golden, null, 2)}\n`);

function sessionFiles(): string[] {
  if (existsSync(defaultSession)) return [defaultSession, ...walkJsonl(sessionsRoot).filter((p) => p !== defaultSession)];
  return walkJsonl(sessionsRoot);
}

function walkJsonl(root: string): string[] {
  if (!existsSync(root)) return [];
  const out: string[] = [];
  const stack = [root];
  while (stack.length > 0) {
    const dir = stack.pop()!;
    for (const name of readdirSync(dir)) {
      const path = join(dir, name);
      const stat = statSync(path);
      if (stat.isDirectory()) stack.push(path);
      else if (path.endsWith(".jsonl")) out.push(path);
    }
  }
  return out.sort();
}

function selectEntries(files: string[]): {
  entries: CapturedEntry[];
  coverage: RequiredClass[];
  missing: RequiredClass[];
} {
  const wanted = new Set<RequiredClass>(requiredClasses);
  const byClass = new Map<RequiredClass, CapturedEntry>();
  for (const path of files) {
    const lines = readFileSync(path, "utf8").split(/\r?\n/).filter(Boolean);
    for (const line of lines) {
      let entry: any;
      try {
        entry = JSON.parse(line);
      } catch {
        continue;
      }
      for (const klass of classify(entry)) {
        if (wanted.has(klass) && !byClass.has(klass)) byClass.set(klass, { path, entry });
      }
      if ([...wanted].every((klass) => byClass.has(klass) || klass === "redacted_thinking")) {
        // redacted_thinking is allowed to be absent when no scanned JSONL entry contains it.
        break;
      }
    }
  }

  for (const authored of authoredEntries) {
    for (const klass of classify(authored.entry)) {
      if (wanted.has(klass) && !byClass.has(klass)) byClass.set(klass, authored);
    }
  }

  const unique = new Map<string, CapturedEntry>();
  const coverage: RequiredClass[] = [];
  const missing: RequiredClass[] = [];
  for (const klass of requiredClasses) {
    const hit = byClass.get(klass);
    if (hit) {
      coverage.push(klass);
      unique.set(`${hit.path}:${hit.entry.id ?? JSON.stringify(hit.entry).slice(0, 80)}`, hit);
    } else {
      missing.push(klass);
    }
  }
  return { entries: [...unique.values()], coverage, missing };
}

/**
 * Builds the row the plugin sends for one session entry, as Pi 0.80.2's `buildSessionContext`
 * builds its `AgentMessage`: a message entry keeps its entry id; a compaction, branch summary, or
 * custom message entry takes a reserved id derived from its entry id.
 */
function rowFromEntry(entry: any): { id: string; message: unknown } {
  const timestamp = (value: string) => new Date(value).getTime();
  switch (entry.type) {
    case "message":
      return { id: entry.id, message: entry.message };
    case "compaction":
      return {
        id: `${RESERVED_ID_PREFIX}compactionSummary:${entry.id}`,
        message: {
          role: "compactionSummary",
          summary: entry.summary,
          tokensBefore: entry.tokensBefore,
          timestamp: timestamp(entry.timestamp),
        },
      };
    case "branch_summary":
      return {
        id: `${RESERVED_ID_PREFIX}branchSummary:${entry.id}`,
        message: {
          role: "branchSummary",
          summary: entry.summary,
          fromId: entry.fromId,
          timestamp: timestamp(entry.timestamp),
        },
      };
    case "custom_message":
      return {
        id: `${RESERVED_ID_PREFIX}custom:${entry.id}`,
        message: {
          role: "custom",
          customType: entry.customType,
          content: entry.content,
          display: entry.display,
          details: entry.details,
          timestamp: timestamp(entry.timestamp),
        },
      };
    default:
      throw new Error(`entry type ${entry.type} builds no AgentMessage`);
  }
}

function classify(entry: any): RequiredClass[] {
  const out: RequiredClass[] = [];
  if (entry?.type === "custom_message") out.push("custom_message");
  if (entry?.type === "compaction") out.push("compaction");
  if (entry?.type === "branch_summary" && entry.summary) out.push("branch_summary");
  const message = entry?.type === "message" ? entry.message : undefined;
  if (!message) return out;
  if (message.role === "bashExecution") out.push("bash_execution");
  if (message.role === "assistant" && message.stopReason === "aborted" && Array.isArray(message.content) && message.content.length === 0) {
    out.push("aborted_assistant");
  }
  if (message.role === "toolResult") {
    out.push("tool_result");
    if (message.details !== undefined) out.push("tool_result_details");
  }
  for (const part of Array.isArray(message.content) ? message.content : []) {
    if (!part || typeof part !== "object") continue;
    if (part.type === "text" && typeof part.textSignature === "string") out.push("text_signature");
    if (part.type === "thinking" && typeof part.thinkingSignature === "string") out.push("thinking_signature");
    if (part.type === "thinking" && part.redacted === true) out.push("redacted_thinking");
    if (part.type === "image") out.push("image");
    if (part.type === "toolCall" && typeof part.id === "string" && part.id.includes("|")) out.push("tool_call_split_pipe");
    if (part.type === "toolCall" && typeof part.thoughtSignature === "string") out.push("thought_signature");
  }
  return out;
}

function sanitizeEntry(entry: any): unknown {
  return sanitize(entry, []);
}

function sanitize(value: unknown, path: string[]): unknown {
  if (Array.isArray(value)) return value.map((item, index) => sanitize(item, [...path, String(index)]));
  if (value === null || typeof value !== "object") return value;
  const input = value as Record<string, unknown>;
  const output: Record<string, unknown> = {};
  for (const [key, child] of Object.entries(input)) {
    if (typeof child === "string") output[key] = sanitizeString(key, child);
    else output[key] = sanitize(child, [...path, key]);
  }
  return output;
}

function sanitizeString(key: string, text: string): string {
  if (text.length === 0) return text;
  if (new Set(["text", "thinking", "summary", "content", "errorMessage", "output"]).has(key)) {
    return sameLength(text, key);
  }
  if (key === "data") return sameLength(text, "base64");
  return text;
}

function sameLength(text: string, label: string): string {
  const seed = `[redacted:${label}]`;
  return seed.repeat(Math.ceil(text.length / seed.length)).slice(0, text.length);
}

function assertInternalConsistency(golden: any): void {
  const coverage = new Set<string>(golden.coverage ?? []);
  const missing = new Set<string>(golden.missing_capture_classes ?? []);
  const unresolved = requiredClasses.filter((klass) => !coverage.has(klass) && !missing.has(klass));
  if (unresolved.length > 0) {
    throw new Error(`Pi golden neither covers nor records missing classes: ${unresolved.join(", ")}`);
  }
  const cases = golden.cases ?? [];
  if (!Array.isArray(cases) || cases.length === 0) throw new Error("Pi golden has no cases");
  for (const testCase of cases) {
    if (!Array.isArray(testCase.rows) || testCase.rows.length === 0) {
      throw new Error(`Pi case ${testCase.name ?? "<unnamed>"} has no rows`);
    }
    for (const row of testCase.rows) {
      if (typeof row?.id !== "string" || typeof row.message?.role !== "string") {
        throw new Error("Pi fixture row lacks an id or an AgentMessage role");
      }
      const reserved = ["custom", "branchSummary", "compactionSummary"].includes(row.message.role);
      if (reserved !== row.id.startsWith(RESERVED_ID_PREFIX)) {
        throw new Error(`Pi fixture row ${row.id} is in the wrong id space for ${row.message.role}`);
      }
    }
  }
}
