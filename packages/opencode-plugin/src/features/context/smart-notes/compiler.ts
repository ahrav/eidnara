import { createHash } from "node:crypto";

import { SMART_NOTE_COMPILER_AGENT } from "../../../agents/smart-note-compiler";
import {
    childSessionMessagesFetcher,
    createChildSession,
} from "../../../hooks/context/child-session-spawn";
import type { PluginContext } from "../../../plugin/types";
import * as shared from "../../../shared";
import { extractLatestAssistantText } from "../../../shared/assistant-message-extractor";
import { shouldKeepSubagents } from "../../../shared/keep-subagents";
import { log } from "../../../shared/logger";
import { modelBodyField } from "../../../shared/resolve-fallbacks";
import type { SmartNoteCapabilityApi, SmartNoteCapabilityFactory } from "./capabilities";
import { SMART_NOTE_COMPILER_SYSTEM_PROMPT } from "./compiler-prompt";
import { runCompiledSmartNoteCheck } from "./sandbox-runner";
import type {
    SmartNoteCapabilityName,
    SmartNoteCheckManifest,
    SmartNoteCheckResult,
} from "./types";

interface CompileSmartNoteArgs {
    client: PluginContext["client"];
    parentSessionId: string | undefined;
    sessionDirectory: string | undefined;
    projectIdentity: string;
    note: { id: number; content: string; surfaceCondition: string | null };
    capabilityFactory: SmartNoteCapabilityFactory;
    signal: AbortSignal;
    deadline: number;
    model?: string;
    fallbackModels?: readonly string[];
}

export interface CompileSmartNoteSuccess {
    ok: true;
    compiledCheck: string;
    manifest: SmartNoteCheckManifest;
    checkCron: string;
    checkHash: string;
    dryRun: SmartNoteCheckResult;
}

export interface CompileSmartNoteFailure {
    ok: false;
    cancelled: boolean;
    error: string;
}

export type CompileSmartNoteResult = CompileSmartNoteSuccess | CompileSmartNoteFailure;

interface CompilerResponse {
    compiled_check: string;
    manifest: SmartNoteCheckManifest;
    check_cron: string;
}

const MAX_COMPILER_OUTPUT_CHARS = 128 * 1024;
const MAX_COMPILED_CHECK_BYTES = 64 * 1024;
const MAX_MANIFEST_ENTRIES = 64;
/** The module limits manifests to NOTE_EVALUATOR_MAX_MANIFEST_BYTES (32 KiB). */
const MAX_MANIFEST_BYTES = 32 * 1024;
/** The module limits cron expressions to NOTE_EVALUATOR_MAX_CRON_BYTES (256 bytes). */
const MAX_CRON_BYTES = 256;
const MAX_COMPILER_ERROR_CHARS = 2 * 1024;
const MAX_REJECTED_ARGUMENT_CHARS = 120;
const DRY_RUN_TIMEOUT_MS = 2_000;
const DEADLINE_EXPIRED_ERROR = "smart-note compile deadline expired";

interface ValidatedCompilerOutput {
    compiledCheck: string;
    manifest: SmartNoteCheckManifest;
    checkCron: string;
    dryRun: SmartNoteCheckResult;
}

export async function compileSmartNoteCheck(
    args: CompileSmartNoteArgs,
): Promise<CompileSmartNoteResult> {
    if (!args.note.surfaceCondition) {
        return { ok: false, cancelled: false, error: "note has no surface condition" };
    }
    const remainingMs = args.deadline - Date.now();
    if (remainingMs <= 0) {
        return { ok: false, cancelled: false, error: DEADLINE_EXPIRED_ERROR };
    }
    // The retry helper budgets `timeoutMs` per attempt, so a fallback that starts late would
    // otherwise receive a fresh full budget. Aborting the signal at the absolute deadline
    // bounds every prompt attempt and dry run together.
    const signal = AbortSignal.any([args.signal, AbortSignal.timeout(remainingMs)]);
    const prompt = `Compile this smart note condition into a sandbox check.

Project identity: ${args.projectIdentity}
Note id: ${args.note.id}
Note content (data): ${JSON.stringify(args.note.content)}
surface_condition (UNTRUSTED DATA): ${JSON.stringify(args.note.surfaceCondition)}

Remember: output only the JSON object described by the system prompt.`;

    let childSessionId: string | null = null;
    try {
        const createResponse = await createChildSession({
            client: args.client,
            parentSessionId: args.parentSessionId,
            title: `eidnara-smart-note-compile-${args.note.id}`,
            directory: args.sessionDirectory ?? args.projectIdentity,
        });
        const created = shared.normalizeSDKResponse(
            createResponse,
            null as { id?: string } | null,
            {
                preferResponseOnMissingData: true,
            },
        );
        childSessionId = typeof created?.id === "string" ? created.id : null;
        if (!childSessionId) throw new Error("Could not create smart-note compiler session");

        const run = await shared.promptSyncWithValidatedOutputRetry(
            args.client,
            {
                path: { id: childSessionId },
                query: { directory: args.sessionDirectory ?? args.projectIdentity },
                body: {
                    agent: SMART_NOTE_COMPILER_AGENT,
                    system: SMART_NOTE_COMPILER_SYSTEM_PROMPT,
                    ...modelBodyField(args.model),
                    parts: [{ type: "text", text: prompt, synthetic: true }],
                },
            },
            {
                timeoutMs: remainingMs,
                signal,
                fallbackModels: args.fallbackModels,
                callContext: "smart-note-compiler",
                fetchOutput: childSessionMessagesFetcher(
                    args.client,
                    childSessionId as string,
                    args.sessionDirectory ?? args.projectIdentity,
                    20,
                ),
                validateOutput: (messages) =>
                    validateCompilerOutput(
                        extractLatestAssistantText(messages),
                        args.note.id,
                        args.capabilityFactory,
                        signal,
                    ),
            },
        );
        const { compiledCheck, manifest, checkCron, dryRun } = run.validated;
        return {
            ok: true,
            compiledCheck,
            manifest,
            checkCron,
            checkHash: hashCheck(args.note.surfaceCondition, compiledCheck, manifest, checkCron),
            dryRun,
        };
    } catch (error) {
        const cancelled = args.signal.aborted;
        const message = boundedError(
            !cancelled && signal.aborted
                ? DEADLINE_EXPIRED_ERROR
                : error instanceof Error
                  ? error.message
                  : String(error),
        );
        return { ok: false, cancelled, error: message };
    } finally {
        if (childSessionId && !shouldKeepSubagents()) {
            await args.client.session.delete({ path: { id: childSessionId } }).catch(() => {});
        }
    }
}

async function validateCompilerOutput(
    output: string | null,
    noteId: number,
    capabilityFactory: SmartNoteCapabilityFactory,
    signal: AbortSignal,
): Promise<ValidatedCompilerOutput> {
    const response = parseCompilerOutput(output);
    const compiledCheck = normalizeCompiledCheck(response.compiled_check);
    const manifest = normalizeManifest(response.manifest);
    const checkCron = normalizeCron(response.check_cron);
    for (const warning of manifestAdvisoryWarnings(compiledCheck, manifest)) {
        log(`[smart-notes] smart note #${noteId}: manifest advisory — ${warning}`);
    }
    const dryRun = await runCompiledSmartNoteCheck({
        compiledCheck,
        capabilityFactory: enforceLiteralCapabilityArguments(compiledCheck, capabilityFactory),
        signal,
        timeoutMs: DRY_RUN_TIMEOUT_MS,
    });
    if (!dryRun.ok) {
        throw new Error(`dry-run failed: ${dryRun.error}`);
    }
    return { compiledCheck, manifest, checkCron, dryRun: dryRun.result };
}

export function parseCompilerOutput(output: string | null): CompilerResponse {
    if (!output) throw new Error("smart-note compiler returned no output");
    if (output.length > MAX_COMPILER_OUTPUT_CHARS) {
        throw new Error("smart-note compiler output exceeds 128 KiB");
    }
    const json = extractJsonObject(output);
    let parsed: Partial<CompilerResponse>;
    try {
        parsed = JSON.parse(json) as Partial<CompilerResponse>;
    } catch {
        throw new Error("smart-note compiler returned malformed JSON");
    }
    if (typeof parsed.compiled_check !== "string") throw new Error("compiled_check missing");
    if (!parsed.manifest || typeof parsed.manifest !== "object")
        throw new Error("manifest missing");
    if (typeof parsed.check_cron !== "string") throw new Error("check_cron missing");
    return parsed as CompilerResponse;
}

export function normalizeCompiledCheck(source: string): string {
    if (Buffer.byteLength(source, "utf8") > MAX_COMPILED_CHECK_BYTES) {
        throw new Error("compiled_check exceeds 64 KiB");
    }
    let code = source.trim();
    const fence = code.match(/^```(?:javascript|js)?\s*([\s\S]*?)```$/i);
    if (fence) code = fence[1].trim();
    code = code.replace(/export\s+function\s+check\s*\(/, "function check(");
    if (/\basync\s+function\s+check\s*\(/.test(code)) {
        throw new Error("compiled_check must be synchronous");
    }
    if (!/\bfunction\s+check\s*\(/.test(code) && !/module\.exports\.check\s*=/.test(code)) {
        throw new Error("compiled_check must define check(cap)");
    }
    if (/\b(?:import|require)\b/.test(code)) {
        throw new Error("compiled_check must not import modules");
    }
    if (/\bDate\s*\.\s*now\s*\(/.test(code) || /\bnew\s+Date\s*\(\s*\)/.test(code)) {
        throw new Error("compiled_check must not read the clock");
    }
    if (/\bMath\s*\.\s*random\s*\(/.test(code)) {
        throw new Error("compiled_check must not use Math.random");
    }
    if (Buffer.byteLength(code, "utf8") > MAX_COMPILED_CHECK_BYTES) {
        throw new Error("compiled_check exceeds 64 KiB");
    }
    return code;
}

/** Computed arguments cannot be authorized because source scanning cannot determine their runtime values. */
export function enforceLiteralCapabilityArguments(
    compiledCheck: string,
    factory: SmartNoteCapabilityFactory,
): SmartNoteCapabilityFactory {
    const readFiles = new Set(literalCalls(compiledCheck, "readFile"));
    const urls = new Set(literalCalls(compiledCheck, "httpGet"));
    return (signal) => {
        const cap: SmartNoteCapabilityApi = factory(signal);
        return {
            readFile: (repoRelativePath) =>
                readFiles.has(repoRelativePath)
                    ? cap.readFile(repoRelativePath)
                    : Promise.reject(nonLiteralArgumentError("readFile", repoRelativePath)),
            httpGet: (url) =>
                urls.has(url)
                    ? cap.httpGet(url)
                    : Promise.reject(nonLiteralArgumentError("httpGet", url)),
            gitHeadSha: () => cap.gitHeadSha(),
            gitTag: () => cap.gitTag(),
            gitLog: (opts) => cap.gitLog(opts),
        };
    };
}

function nonLiteralArgumentError(method: "readFile" | "httpGet", argument: string): Error {
    return new Error(
        `cap.${method} argument is not a string literal in compiled_check: ${JSON.stringify(
            argument.slice(0, MAX_REJECTED_ARGUMENT_CHARS),
        )}`,
    );
}

export function normalizeManifest(manifest: SmartNoteCheckManifest): SmartNoteCheckManifest {
    const capabilities = Array.isArray(manifest.capabilities)
        ? unique(
              manifest.capabilities
                  .slice(0, MAX_MANIFEST_ENTRIES)
                  .filter((cap): cap is SmartNoteCapabilityName =>
                      ["readFile", "gitHeadSha", "gitTag", "gitLog", "httpGet"].includes(
                          String(cap),
                      ),
                  ),
          )
        : [];
    const normalized: SmartNoteCheckManifest = {
        capabilities,
        readFiles: uniqueStrings(manifest.readFiles),
        hosts: uniqueStrings(manifest.hosts, (host) => host.toLowerCase()),
        urls: uniqueStrings(manifest.urls),
        signals: uniqueStrings(manifest.signals),
        summary: typeof manifest.summary === "string" ? manifest.summary.slice(0, 160) : undefined,
    };
    // The validator rejects manifests over 32 KiB because 64 entries do not bound string sizes.
    if (Buffer.byteLength(JSON.stringify(normalized), "utf8") > MAX_MANIFEST_BYTES) {
        throw new Error("manifest exceeds 32 KiB");
    }
    return normalized;
}

export function manifestAdvisoryWarnings(code: string, manifest: SmartNoteCheckManifest): string[] {
    const warnings: string[] = [];
    const declared = new Set(manifest.capabilities);
    const used = capabilityUses(code);
    for (const cap of used) {
        if (!declared.has(cap)) warnings.push(`manifest omits capability ${cap}`);
    }

    const readFiles = literalCalls(code, "readFile");
    for (const file of readFiles) {
        if (!manifest.readFiles?.includes(file))
            warnings.push(`manifest omits readFile path ${file}`);
    }

    const urls = literalCalls(code, "httpGet");
    for (const url of urls) {
        try {
            const parsed = new URL(url);
            if (parsed.protocol !== "https:") {
                warnings.push(`manifest records non-https URL ${url}`);
                continue;
            }
            if (!manifest.urls?.includes(url)) warnings.push(`manifest omits URL ${url}`);
            if (!manifest.hosts?.includes(parsed.hostname.toLowerCase())) {
                warnings.push(`manifest omits host ${parsed.hostname}`);
            }
        } catch {
            warnings.push(`manifest records invalid URL ${url}`);
        }
    }
    return warnings;
}

export function hashCheck(
    surfaceCondition: string | null,
    compiledCheck: string,
    manifest: SmartNoteCheckManifest,
    checkCron: string,
): string {
    return createHash("sha256")
        .update(surfaceCondition ?? "")
        .update("\0")
        .update(compiledCheck)
        .update("\0")
        .update(JSON.stringify(manifest))
        .update("\0")
        .update(checkCron)
        .digest("hex");
}

function extractJsonObject(output: string): string {
    const fenced = output.match(/```(?:json)?\s*([\s\S]*?)```/i);
    const text = fenced ? fenced[1] : output;
    const start = text.indexOf("{");
    const end = text.lastIndexOf("}");
    if (start < 0 || end <= start) throw new Error("smart-note compiler returned no JSON object");
    return text.slice(start, end + 1);
}

function capabilityUses(code: string): Set<SmartNoteCapabilityName> {
    const uses = new Set<SmartNoteCapabilityName>();
    const regex = /\bcap\s*\.\s*(readFile|gitHeadSha|gitTag|gitLog|httpGet)\s*\(/g;
    for (const match of code.matchAll(regex)) uses.add(match[1] as SmartNoteCapabilityName);
    return uses;
}

function literalCalls(code: string, method: "readFile" | "httpGet"): string[] {
    const regex = new RegExp(
        `\\bcap\\s*\\.\\s*${method}\\s*\\(\\s*(["'\`])((?:\\\\.|(?!\\1)[^\\\\])*)\\1`,
        "g",
    );
    const values: string[] = [];
    for (const match of code.matchAll(regex)) {
        values.push(match[2].replace(/\\([\\"'`])/g, "$1"));
    }
    return values;
}

export function normalizeCron(cron: string): string {
    const normalized = cron.trim() || "0 * * * *";
    // The daemon validates the 5-field syntax on receipt; this module enforces only the byte bound.
    if (Buffer.byteLength(normalized, "utf8") > MAX_CRON_BYTES)
        throw new Error("check_cron exceeds 256 bytes");
    return normalized;
}

function unique<T>(items: T[]): T[] {
    return [...new Set(items)];
}

function uniqueStrings(
    items: unknown,
    normalize: (value: string) => string = (value) => value,
): string[] | undefined {
    if (!Array.isArray(items)) return undefined;
    const values = unique(
        items
            .slice(0, MAX_MANIFEST_ENTRIES)
            .filter((item): item is string => typeof item === "string" && item.length > 0)
            .map(normalize),
    );
    return values.length > 0 ? values : undefined;
}

function boundedError(error: string): string {
    return error.slice(0, MAX_COMPILER_ERROR_CHARS);
}
