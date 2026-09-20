import { randomUUID } from "node:crypto";
import { realpathSync } from "node:fs";
import { resolveProjectRootDirectory } from "@eidnara/opencode/features/context/project-identity";
import type { HostClient, HostClientOptions } from "@eidnara/opencode/shared/host-client";
import { isHostCallError, rawJsonInteger } from "@eidnara/opencode/shared/host-client";
import {
    connectionFilePath,
    resolveLifecycleDataRoot,
} from "@eidnara/opencode/shared/host-lifecycle";
import { stateKey } from "@eidnara/opencode/shared/kernel-client/state";
import { shellQuote } from "@eidnara/opencode/shared/shell-quote";
import { printableBlock, printableLine } from "../lib/terminal-text";
import {
    decodePage,
    decodeReviewStatus,
    decodeSelected,
    integerText,
    isHex64,
    MAX_IDENTITY_BYTES,
    MAX_PAGE_ITEMS,
    MAX_TEXT_BYTES,
    type Proposal,
    type ReadTerminal,
    type Reference,
    type ReviewAnswer,
    type ReviewStatus,
    STATUS_COUNTERS,
} from "./review-wire";

export const REVIEW_HARNESS = "cli";
export const DEFAULT_LIMIT = 16;
const MAX_LINE = 200;
const REQUEST_TIMEOUT_MS = 10_000;

/** One bound covers every daemon wait: the control calls, `route.open`, and the routed request. */
export function hostClientOptions(
    timeoutMs: number = REQUEST_TIMEOUT_MS,
): Pick<HostClientOptions, "requestTimeoutMs" | "routeOpenDeadlineMs"> {
    return { requestTimeoutMs: timeoutMs, routeOpenDeadlineMs: timeoutMs };
}

/** The connection-scoped operations the command uses. */
export type ReviewConnection = Pick<
    HostClient,
    "catalogList" | "hostStatus" | "routeOpen" | "request" | "closeAsync"
>;

export interface ReviewCommandDependencies {
    connect: (connectionFile: string) => Promise<ReviewConnection>;
    /** Resolves a project path to the root the harness routes bind; throws when the path does not exist. */
    resolveProjectRoot: (path: string) => string;
    cwd: () => string;
    env: Record<string, string | undefined>;
    stdout: (line: string) => void;
    stderr: (line: string) => void;
}

/** The OpenCode and Pi routes bind the Git worktree root, so a subdirectory reaches the same project digest; `realpathSync.native` first so a missing path still throws. */
export function defaultResolveProjectRoot(path: string): string {
    return resolveProjectRootDirectory(realpathSync.native(path));
}

const defaultDependencies: ReviewCommandDependencies = {
    connect: async (connectionFile) => {
        const { HostClient } = await import("@eidnara/opencode/shared/host-client");
        return HostClient.connect({ connectionFile, ...hostClientOptions() });
    },
    resolveProjectRoot: defaultResolveProjectRoot,
    cwd: () => process.cwd(),
    env: process.env,
    stdout: (line) => console.log(line),
    stderr: (line) => console.error(line),
};

export type ReviewArgs =
    | {
          command: "list";
          project: string | null;
          limit: number;
          after: string | null;
          json: boolean;
      }
    | { command: "show"; project: string | null; causalIdentity: string; json: boolean }
    | { command: "status"; json: boolean };

export function usage(): string {
    return [
        "Usage:",
        "  eidnara review list [--project PATH] [--limit 1..64] [--after CURSOR] [--json]",
        "  eidnara review show <causal-identity> [--project PATH] [--json]",
        "  eidnara review status [--json]",
        "",
        "Reads completed MemoryReviewer outcomes for the project bound to PATH or the current",
        "directory, one page or one selected proposal per invocation. Nothing here changes",
        "canonical memory, accepts a proposal, or starts review work; retain and no_change",
        "proposals do not change, extend, or corroborate the memory they name.",
        "Status is host-wide and names no project. An open activation state means the",
        "deployment owner admits model disclosure; it is not compaction status and applies",
        "nothing. The daemon connection is a bearer key, not an OS sandbox.",
    ].join("\n");
}

export function parseReviewArgs(args: string[]): ReviewArgs | string {
    const [command, ...rest] = args;
    if (command !== "list" && command !== "show" && command !== "status") return usage();
    let project: string | null = null;
    let limit = DEFAULT_LIMIT;
    let after: string | null = null;
    let json = false;
    const positional: string[] = [];
    for (let index = 0; index < rest.length; index++) {
        const token = rest[index];
        const value = (): string | null => (index + 1 < rest.length ? rest[++index] : null);
        switch (token) {
            case "--json":
                json = true;
                break;
            case "--project": {
                if (command === "status") return "review status takes no --project";
                const path = value();
                if (path === null || path.length === 0) return "--project requires a path";
                project = path;
                break;
            }
            case "--limit": {
                if (command !== "list") return `review ${command} takes no --limit`;
                const text = value();
                const parsed =
                    text !== null && /^[1-9][0-9]?$/.test(text) ? Number(text) : Number.NaN;
                if (!(parsed >= 1 && parsed <= MAX_PAGE_ITEMS))
                    return `--limit must be 1..${MAX_PAGE_ITEMS}`;
                limit = parsed;
                break;
            }
            case "--after": {
                if (command !== "list") return `review ${command} takes no --after`;
                const cursor = value();
                if (!isHex64(cursor)) return "--after must be a lower-hex sha256 causal identity";
                after = cursor;
                break;
            }
            default:
                if (token.startsWith("-"))
                    return `Unknown review argument: ${printableLine(token, 64)}`;
                positional.push(token);
        }
    }
    if (command === "show") {
        if (positional.length !== 1 || !isHex64(positional[0])) {
            return "review show requires one lower-hex sha256 causal identity";
        }
        return { command, project, causalIdentity: positional[0], json };
    }
    if (positional.length > 0) {
        return `Unexpected review ${command} argument: ${printableLine(positional[0], 64)}`;
    }
    return command === "list" ? { command, project, limit, after, json } : { command, json };
}

function terminalText(terminal: ReadTerminal): string {
    switch (terminal) {
        case "disabled":
            return "Review access is disabled: no review store is installed, or this bound root has no active MODULE memories authority, including no authority-route binding. The daemon does not identify which cause applies.";
        case "store_unavailable":
            return "The review store could not be read.";
        case "not_selected":
            return "No selected proposal: there is no completed receipt for this identity, or its outcome selected nothing.";
        case "incarnation_mismatch":
            return "The receipt was recorded under another Kernel or Memory Store incarnation.";
        case "selection_mismatch":
            return "The receipt's selection does not match the staged proposal.";
        case "kernel_refused":
            return "The Kernel refused the read.";
        case "review_expired":
            return "The review hold has expired; the proposal is no longer readable.";
        default:
            return "A disclosed input no longer passes the Kernel for a local reader.";
    }
}

/** JSON output with every `bigint` as an exact number token; safe integers already serialize exactly. */
function toJson(value: unknown): string {
    return JSON.stringify(value, (_key, entry) =>
        typeof entry === "bigint" ? rawJsonInteger(entry) : entry,
    );
}

function refusalLines<T extends ReadTerminal, B>(
    answer: Exclude<ReviewAnswer<T, B>, { kind: "body" }>,
    json: boolean,
): string {
    if (json) return toJson(answer);
    if (answer.kind === "terminal") return terminalText(answer.terminal);
    if (answer.kind === "state") return `Kernel state: ${stateKey(answer.state)}`;
    return `The response could not be validated: ${answer.detail}.`;
}

function referenceLines(label: string, references: Reference[]): string[] {
    if (references.length === 0) return [`${label}: none`];
    return [
        `${label}:`,
        ...references.map((reference) => {
            const span = reference.span
                ? ` [${printableLine(reference.span.alias, MAX_IDENTITY_BYTES)} ${integerText(reference.span.start)}..${integerText(reference.span.end)}]`
                : "";
            return `  ${printableLine(reference.evidence_id, MAX_IDENTITY_BYTES)}${span}`;
        }),
    ];
}

function proposalLines(proposal: Proposal): string[] {
    const target =
        proposal.target.kind === "staged_candidate"
            ? `staged candidate ${printableLine(proposal.target.candidate_id, MAX_IDENTITY_BYTES)}`
            : `memory ${printableLine(proposal.target.object_id, MAX_IDENTITY_BYTES)} revision ${integerText(proposal.target.source_revision)} known as of ${integerText(proposal.target.known_as_of)} commit token ${integerText(proposal.target.commit_token)}`;
    const lines = [`Action: ${proposal.action}`, `Target: ${target}`];
    if (proposal.new_text !== undefined) {
        // Every text line is indented so a model-authored line such as `Support:` never reads as the command's own field.
        lines.push(
            "Text:",
            ...printableBlock(proposal.new_text)
                .split("\n")
                .map((line) => `  ${line}`),
        );
    }
    lines.push(...referenceLines("Support", proposal.support));
    lines.push(...referenceLines("Contradictions", proposal.contradictions));
    lines.push(
        proposal.limitations.length === 0
            ? "Limitations: none"
            : `Limitations:\n${proposal.limitations.map((text) => `  ${printableLine(text, MAX_TEXT_BYTES)}`).join("\n")}`,
    );
    lines.push(`Uncertainty: ${proposal.uncertainty}`);
    lines.push(
        `Manifest: ${printableLine(proposal.manifest.manifest_id, MAX_IDENTITY_BYTES)} ${proposal.manifest.digest}`,
    );
    return lines;
}

const UNREPORTED = "unreported";

/** Characters a POSIX shell passes through unquoted. */
const SHELL_PLAIN = /^[A-Za-z0-9_./-]+$/;
/** Linux `PATH_MAX`. */
const MAX_PATH_LINE = 4096;

/** The command reruns the same walk from any directory: the root is explicit and a non-default page size is repeated. */
function nextPageCommand(projectRoot: string, limit: number, next: string): string {
    // A root that printable rendering would alter cannot be quoted back into a command that reaches the same directory.
    if (printableLine(projectRoot, MAX_PATH_LINE) !== projectRoot) {
        return `rerun this command with --after ${next}`;
    }
    const project = SHELL_PLAIN.test(projectRoot) ? projectRoot : shellQuote(projectRoot);
    const size = limit === DEFAULT_LIMIT ? "" : ` --limit ${limit}`;
    return `eidnara review list --project ${project}${size} --after ${next}`;
}

function statusLines(status: ReviewStatus): string[] {
    const lines = [
        `MemoryReviewer store: ${status.memory_reviewer_state ?? UNREPORTED}`,
        `Activation (disclosure admission, not application): ${status.activation_state ?? UNREPORTED}`,
        `Sampled at: ${status.sampled_at_ms === null ? "unavailable" : `${status.sampled_at_ms} ms`}`,
    ];
    for (const name of STATUS_COUNTERS) {
        const value = status.counters[name];
        lines.push(`${name}: ${value === null ? "unavailable" : String(value)}`);
    }
    lines.push(
        "Counters are overlapping populations over the whole data home, not a total, a ratio, or a per-project view.",
    );
    return lines;
}

/** Only closed codes and error names reach the terminal; a message may carry peer text. */
function failureText(command: string, error: unknown): string {
    if (isHostCallError(error)) {
        const code = error.code === undefined ? "" : ` (${printableLine(error.code, 64)})`;
        return `Review ${command} failed: ${error.kind}${code}.`;
    }
    const { name, code } = (error ?? {}) as { name?: unknown; code?: unknown };
    if (name === "ConnectionFileError" && code === "not_found") {
        return `Review ${command} failed: the daemon connection file is absent; start the daemon first.`;
    }
    return `Review ${command} failed before a response could be formed (${printableLine(typeof name === "string" ? name : "error", 64)}).`;
}

export async function runReviewCommand(
    args: string[],
    dependencies: ReviewCommandDependencies = defaultDependencies,
): Promise<number> {
    const parsed = parseReviewArgs(args);
    if (typeof parsed === "string") {
        dependencies.stderr(parsed);
        return 2;
    }
    const root = resolveLifecycleDataRoot(dependencies.env);
    if (!root.ok) {
        dependencies.stderr(
            `Review ${parsed.command} failed: no data directory could be resolved.`,
        );
        return 1;
    }
    let projectRoot: string | null = null;
    if (parsed.command !== "status") {
        try {
            projectRoot = dependencies.resolveProjectRoot(parsed.project ?? dependencies.cwd());
        } catch {
            dependencies.stderr(
                `Review ${parsed.command} failed: the project path does not exist.`,
            );
            return 2;
        }
    }
    let connection: ReviewConnection;
    try {
        connection = await dependencies.connect(connectionFilePath(root.root));
    } catch (error) {
        dependencies.stderr(failureText(parsed.command, error));
        return 1;
    }
    try {
        const rendered = await render(parsed, connection, projectRoot);
        dependencies.stdout(rendered.text);
        return rendered.ok ? 0 : 1;
    } catch (error) {
        dependencies.stderr(failureText(parsed.command, error));
        return 1;
    } finally {
        await connection.closeAsync().catch(() => {});
    }
}

async function render(
    parsed: ReviewArgs,
    connection: ReviewConnection,
    projectRoot: string | null,
): Promise<{ text: string; ok: boolean }> {
    if (parsed.command === "status" || projectRoot === null) {
        const snapshot = await connection.hostStatus();
        const status = decodeReviewStatus(snapshot.metrics);
        return {
            ok: true,
            text: parsed.json
                ? toJson({ kind: "status", health: snapshot.health, ...status })
                : [`Host health: ${snapshot.health}`, ...statusLines(status)].join("\n"),
        };
    }
    // Compatibility is discovered on the same connection: the context module must be served before a route is opened to it.
    const catalog = await connection.catalogList();
    if (!catalog.some((entry) => entry.module_id === "context")) {
        return {
            ok: false,
            text: parsed.json
                ? toJson({ kind: "incompatible", detail: "context module absent" })
                : "Review access is unavailable: the daemon serves no context module.",
        };
    }
    const session = `eidnara-review:${randomUUID()}`;
    const handle = await connection.routeOpen(
        { kind: "tool_provider", module_id: "context" },
        { project_root: projectRoot, harness: REVIEW_HARNESS, session },
        { consumerIdentity: null },
    );
    const envelope = { v: 1, session_id: session, project_root: projectRoot };
    if (parsed.command === "list") {
        const raw = await connection.request(
            handle,
            {
                ...envelope,
                method: "review.list",
                limit: parsed.limit,
                after: parsed.after,
            },
            { exactIntegers: true },
        );
        const answer = decodePage(raw, parsed.limit);
        if (answer.kind !== "body") return { ok: false, text: refusalLines(answer, parsed.json) };
        const { items, next } = answer.body;
        if (parsed.json) {
            return {
                ok: true,
                text: toJson({ kind: "page", project_root: projectRoot, items, next }),
            };
        }
        const lines = [`Project: ${printableLine(projectRoot, MAX_PATH_LINE)}`];
        if (items.length === 0) lines.push("No completed outcomes on this page.");
        for (const item of items) {
            const reason = item.reason ? ` (${item.reason})` : "";
            const selected = item.selected ? " selected" : "";
            lines.push(
                `${item.causal_identity} gen ${integerText(item.generation)} ${item.outcome}${reason}${selected}`,
            );
        }
        lines.push(
            next === null
                ? "End of walk. Outcomes completing behind the cursor appear on a fresh walk."
                : `Next page: ${nextPageCommand(projectRoot, parsed.limit, next)}`,
        );
        return { ok: true, text: lines.join("\n") };
    }
    const raw = await connection.request(
        handle,
        {
            ...envelope,
            method: "review.read",
            causal_identity: parsed.causalIdentity,
        },
        { exactIntegers: true },
    );
    const answer = decodeSelected(raw, parsed.causalIdentity);
    if (answer.kind !== "body") return { ok: false, text: refusalLines(answer, parsed.json) };
    const selected = answer.body;
    if (parsed.json) {
        return {
            ok: true,
            text: toJson({ kind: "proposal", project_root: projectRoot, ...selected }),
        };
    }
    return {
        ok: true,
        text: [
            `Project: ${printableLine(projectRoot, MAX_PATH_LINE)}`,
            `Causal identity: ${selected.causal_identity}`,
            `Reference: ${printableLine(selected.reference.database_incarnation_id, MAX_IDENTITY_BYTES)} ${printableLine(selected.reference.candidate_id, MAX_IDENTITY_BYTES)} ${selected.reference.payload_digest}`,
            ...proposalLines(selected.proposal),
            `Review expires at: ${integerText(selected.review_expires_at)} ms`,
            "Reference only: this proposal is not applied, and viewing it changes nothing.",
        ].join("\n"),
    };
}
