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
import {
    decodeStringLiteral,
    maskSourceSpans,
    type SourceSpanKind,
    scanSourceSpans,
} from "./source-spans";
import {
    type SmartNoteCapabilityName,
    type SmartNoteCheckManifest,
    type SmartNoteCheckResult,
    SmartNoteNetworkError,
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
    /** Null when a declared URL is unreachable during compilation. */
    dryRun: SmartNoteCheckResult | null;
    dryRunNetworkError?: string;
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
/** Cleanup runs after the deadline may have passed, so it carries its own short bound. */
const SESSION_CLEANUP_TIMEOUT_MS = 3_000;
const DEADLINE_EXPIRED_ERROR = "smart-note compile deadline expired";
const NON_CODE_SPANS: ReadonlySet<SourceSpanKind> = new Set(["comment", "string", "template"]);
const CHECK_SIGNATURE = /\bfunction\s+check\s*\(\s*cap\s*\)/;
const DIRECT_CAPABILITY_CALL = /^\s*\.\s*(?:readFile|httpGet|gitHeadSha|gitTag|gitLog)\s*\(/;

type LiteralCapabilityMethod = "readFile" | "httpGet";

interface CapabilityCallSite {
    method: LiteralCapabilityMethod;
    /** Decoded string value, or null when the argument is anything other than one string literal. */
    literal: string | null;
}

interface ValidatedCompilerOutput {
    compiledCheck: string;
    manifest: SmartNoteCheckManifest;
    checkCron: string;
    dryRun: SmartNoteCheckResult | null;
    dryRunNetworkError?: string;
}

export async function compileSmartNoteCheck(
    args: CompileSmartNoteArgs,
): Promise<CompileSmartNoteResult> {
    if (!args.note.surfaceCondition) {
        return { ok: false, cancelled: false, error: "note has no surface condition" };
    }
    if (args.signal.aborted) {
        return { ok: false, cancelled: true, error: "smart-note compile cancelled" };
    }
    const remainingMs = args.deadline - Date.now();
    if (remainingMs <= 0) {
        return { ok: false, cancelled: false, error: DEADLINE_EXPIRED_ERROR };
    }
    // The retry helper budgets `timeoutMs` per attempt, so a fallback that starts late would
    // otherwise receive a fresh full budget. Aborting the signal at the absolute deadline
    // bounds session creation, every prompt attempt, and every dry run together.
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
            signal,
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
        const { compiledCheck, manifest, checkCron, dryRun, dryRunNetworkError } = run.validated;
        return {
            ok: true,
            compiledCheck,
            manifest,
            checkCron,
            checkHash: hashCheck(args.note.surfaceCondition, compiledCheck, manifest, checkCron),
            dryRun,
            ...(dryRunNetworkError === undefined ? {} : { dryRunNetworkError }),
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
            await args.client.session
                .delete({
                    path: { id: childSessionId },
                    signal: AbortSignal.timeout(SESSION_CLEANUP_TIMEOUT_MS),
                })
                .catch(() => {});
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
    const bound = await bindDeclaredRequests(compiledCheck, capabilityFactory, signal);
    const dryRun = await runCompiledSmartNoteCheck({
        compiledCheck,
        capabilityFactory: bound.factory,
        signal,
        timeoutMs: DRY_RUN_TIMEOUT_MS,
    });
    if (dryRun.ok) {
        return { compiledCheck, manifest, checkCron, dryRun: dryRun.result };
    }
    // A dry run remains pending only when the check propagates its served network failure
    // unchanged; a check that catches it and then fails for another reason is not waived.
    const servedNetworkFailure = bound.servedNetworkFailure();
    if (
        !dryRun.cancelled &&
        servedNetworkFailure !== null &&
        dryRun.error === `${SmartNoteNetworkError.name}: ${servedNetworkFailure}`
    ) {
        return {
            compiledCheck,
            manifest,
            checkCron,
            dryRun: null,
            dryRunNetworkError: boundedError(servedNetworkFailure),
        };
    }
    throw new Error(`dry-run failed: ${dryRun.error}`);
}

type BoundResponse =
    | { ok: true; value: { status: number; body: string } }
    | { ok: false; error: SmartNoteNetworkError };

export interface BoundRequests {
    factory: SmartNoteCapabilityFactory;
    /** Message of the first prefetch network failure the guest's `httpGet` received, or null. */
    servedNetworkFailure(): string | null;
}

/**
 * Host requests depend only on the literal URLs in the check source, never on file contents
 * or control flow inside the sandbox, so a check cannot encode repository data in its choice
 * of which literal URL to request.
 *
 * A non-network prefetch failure aborts binding even if the guest does not request its URL. A
 * network failure is transient, so it is stored and rethrown from the guest's call instead.
 */
export async function bindDeclaredRequests(
    compiledCheck: string,
    factory: SmartNoteCapabilityFactory,
    signal: AbortSignal,
): Promise<BoundRequests> {
    const readFiles = new Set(literalCalls(compiledCheck, "readFile"));
    const urls = [...new Set(literalCalls(compiledCheck, "httpGet"))];
    if (urls.length > MAX_MANIFEST_ENTRIES) {
        throw new Error(`compiled_check declares more than ${MAX_MANIFEST_ENTRIES} URLs`);
    }
    const fetcher = factory(signal);
    const responses = new Map<string, BoundResponse>();
    await Promise.all(
        urls.map(async (url) => {
            try {
                responses.set(url, { ok: true, value: await fetcher.httpGet(url) });
            } catch (error) {
                if (!(error instanceof SmartNoteNetworkError)) throw error;
                responses.set(url, { ok: false, error });
            }
        }),
    );
    let served: string | null = null;
    return {
        servedNetworkFailure: () => served,
        factory: (runSignal) => {
            const cap: SmartNoteCapabilityApi = factory(runSignal);
            return {
                readFile: (repoRelativePath) =>
                    readFiles.has(repoRelativePath)
                        ? cap.readFile(repoRelativePath)
                        : Promise.reject(nonLiteralArgumentError("readFile", repoRelativePath)),
                httpGet: (url) => {
                    const bound = responses.get(url);
                    if (!bound) return Promise.reject(nonLiteralArgumentError("httpGet", url));
                    if (bound.ok) return Promise.resolve(bound.value);
                    served ??= bound.error.message;
                    return Promise.reject(bound.error);
                },
                gitHeadSha: () => cap.gitHeadSha(),
                gitTag: () => cap.gitTag(),
                gitLog: (opts) => cap.gitLog(opts),
            };
        },
    };
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
    // Mask comments and literals so textual `require` or `export function check(` remains valid.
    const exported = /\bexport\s+(?=function\s+check\s*\()/.exec(
        maskSourceSpans(code, NON_CODE_SPANS),
    );
    if (exported) {
        code = code.slice(0, exported.index) + code.slice(exported.index + exported[0].length);
    }
    const codeOnly = maskSourceSpans(code, NON_CODE_SPANS);
    if (/\basync\s+function\s+check\s*\(/.test(codeOnly)) {
        throw new Error("compiled_check must be synchronous");
    }
    const signature = CHECK_SIGNATURE.exec(codeOnly);
    if (!signature) {
        throw new Error("compiled_check must define function check(cap)");
    }
    if (/\b(?:import|require)\b/.test(codeOnly)) {
        throw new Error("compiled_check must not import modules");
    }
    if (/\barguments\b/.test(codeOnly)) {
        throw new Error("compiled_check must not use arguments");
    }
    const misuse = capabilityMisuse(code, codeOnly, signature.index + signature[0].indexOf("cap"));
    if (misuse !== null) {
        throw new Error(`cap may only be called directly as cap.<capability>(...): ${misuse}`);
    }
    const computed = capabilityCallSites(code).find((site) => site.literal === null);
    if (computed) {
        throw new Error(`cap.${computed.method} argument must be a single string literal`);
    }
    if (Buffer.byteLength(code, "utf8") > MAX_COMPILED_CHECK_BYTES) {
        throw new Error("compiled_check exceeds 64 KiB");
    }
    return code;
}

function capabilityMisuse(code: string, codeOnly: string, parameterIndex: number): string | null {
    const identifier = /(?<![\w$.])cap(?![\w$])/g;
    for (const match of codeOnly.matchAll(identifier)) {
        const index = match.index ?? 0;
        if (index === parameterIndex) continue;
        if (DIRECT_CAPABILITY_CALL.test(codeOnly.slice(index + 3))) continue;
        return JSON.stringify(
            code.slice(index, index + MAX_REJECTED_ARGUMENT_CHARS).split("\n")[0],
        );
    }
    return null;
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
    const codeOnly = maskSourceSpans(code, NON_CODE_SPANS);
    for (const match of codeOnly.matchAll(regex)) uses.add(match[1] as SmartNoteCapabilityName);
    return uses;
}

/**
 * Call sites are located in code with comments and literal interiors blanked, so text inside a
 * comment or another string cannot add a call site or a literal; the argument value is then
 * read from the original source at the literal span that starts at the argument position.
 */
function capabilityCallSites(code: string): CapabilityCallSite[] {
    const spans = scanSourceSpans(code);
    const codeOnly = maskSourceSpans(code, NON_CODE_SPANS);
    const sites: CapabilityCallSite[] = [];
    const regex = /\bcap\s*\.\s*(readFile|httpGet)\s*\(\s*/g;
    for (const match of codeOnly.matchAll(regex)) {
        const method = match[1] as LiteralCapabilityMethod;
        const argumentStart = (match.index ?? 0) + match[0].length;
        const span = spans.find(
            (candidate) =>
                candidate.start === argumentStart &&
                (candidate.kind === "string" || candidate.kind === "template"),
        );
        if (!span) {
            sites.push({ method, literal: null });
            continue;
        }
        const quote = code[span.start];
        const body = code.slice(span.start + 1, span.end - 1);
        // A template segment ending at `${` has no closing backtick.
        const terminated = span.end - span.start >= 2 && code[span.end - 1] === quote;
        const isRegexLiteral = quote === "/";
        const closesCall = /^\s*\)/.test(codeOnly.slice(span.end));
        sites.push({
            method,
            literal: terminated && !isRegexLiteral && closesCall ? decodeStringLiteral(body) : null,
        });
    }
    return sites;
}

function literalCalls(code: string, method: LiteralCapabilityMethod): string[] {
    const values: string[] = [];
    for (const site of capabilityCallSites(code)) {
        if (site.method === method && site.literal !== null) values.push(site.literal);
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
