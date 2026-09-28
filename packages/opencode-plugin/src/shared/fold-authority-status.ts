import { dirname, normalize } from "node:path";
import { inspectPluginConfig } from "../config";
import { resolveEidnaraUserConfigPath } from "../config/config-paths";
import {
    describeFoldAuthority,
    type FoldAuthority,
    foldAuthorityOf,
} from "../config/fold-authority";
import { BoundedSessionMap } from "./bounded-session-map";
import { type ConflictWarning, formatConflictShort, warningTextPrefix } from "./conflict-detector";

export const RESTART_OTHER_INSTANCES_STEP = "restart other OpenCode instances";
export const DAEMON_RESTART_STEP = "run `eidnara daemon restart`";
export const AUTHORITY_PENDING_WARNING = `authority pending: ${RESTART_OTHER_INSTANCES_STEP}`;
export const ROOT_MISMATCH_WARNING = "configuration root mismatch";
/** Every live fold-authority message begins with one of these, because its first reason is a pending change or a root mismatch. */
export const FOLD_AUTHORITY_WARNING_MARKERS = [
    AUTHORITY_PENDING_WARNING,
    ROOT_MISMATCH_WARNING,
].map(warningTextPrefix);

type AuthorityName = "eidnara" | "native";

export interface DaemonFoldAuthority {
    applied: AuthorityName | "unadopted" | "unknown";
    pending?: { target: AuthorityName; reason: string };
    userConfigPath?: string;
    stalled: boolean;
}

export interface PluginFoldAuthority {
    startup: FoldAuthority;
    userConfigPath?: string;
    readDisk: () => FoldAuthority;
    /** Receives the session's current warning, or `undefined` when the session has none. */
    publish: (warning: ConflictWarning | undefined, sessionId: string) => void;
}

export interface FoldAuthorityStatus {
    /** The authority the configuration on disk derives; only the surfaces that print it read the files. */
    disk?: string;
    startup: string;
    applied: DaemonFoldAuthority["applied"];
    pending?: { target: AuthorityName; reason: string; step: string };
    stalled: boolean;
    daemon_user_config?: string;
    plugin_user_config?: string;
    warnings: string[];
    label?: string;
}

export function pluginFoldAuthority(
    directory: string,
    startup: Parameters<typeof foldAuthorityOf>[0],
    publish: PluginFoldAuthority["publish"],
): PluginFoldAuthority {
    return {
        startup: foldAuthorityOf(startup),
        userConfigPath: resolveEidnaraUserConfigPath(),
        readDisk: () => {
            try {
                return foldAuthorityOf(inspectPluginConfig(directory));
            } catch (error) {
                return {
                    kind: "unresolved",
                    reason: error instanceof Error ? error.message : String(error),
                };
            }
        },
        publish,
    };
}

/**
 * Forwards a session's first reported state and each later change, so a repeated poll costs no
 * delivery while a session's first report still clears a warning an earlier process left.
 */
export function publishOnChange(
    deliver: PluginFoldAuthority["publish"],
    maxSessions: number,
): PluginFoldAuthority["publish"] {
    const published = new BoundedSessionMap<string>(maxSessions);
    return (warning, sessionId) => {
        const text = warning === undefined ? "" : formatConflictShort(warning);
        if (published.get(sessionId) === text) return;
        published.set(sessionId, text);
        deliver(warning, sessionId);
    };
}

function authorityName(value: string): AuthorityName | undefined {
    return value === "eidnara" || value === "native" ? value : undefined;
}

const FOLD_AUTHORITY = "fold authority ";
const PENDING = "fold authority pending ";
const STALLED = ", summarizer stalled";
const LEGACY_STALL = "last history_summarizer: no fire: no_models";
const USER_CONFIG = "user config ";
const SESSION = "; session ";

const ENCODED = "encoded:";

function decodedPath(text: string): string | undefined {
    if (!text.startsWith(ENCODED)) return /\s/.test(text) ? undefined : text;
    try {
        return decodeURIComponent(text.slice(ENCODED.length));
    } catch {
        return undefined;
    }
}

export function parseDaemonFoldAuthority(summary: unknown): DaemonFoldAuthority {
    const unknown: DaemonFoldAuthority = { applied: "unknown", stalled: false };
    if (typeof summary !== "string" || !summary.startsWith(FOLD_AUTHORITY)) return unknown;
    const userConfigAt = summary.indexOf(`; ${USER_CONFIG}`);
    if (userConfigAt < 0) return unknown;
    const [appliedText = "", ...pendingParts] = summary
        .slice(FOLD_AUTHORITY.length, userConfigAt)
        .split("; ");
    const appliedName = appliedText.split(", ")[0] ?? "";
    const applied: DaemonFoldAuthority["applied"] =
        authorityName(appliedName) ??
        (appliedName.startsWith("unadopted") ? "unadopted" : "unknown");
    const pendingText = pendingParts.join("; ");
    const colon = pendingText.indexOf(": ");
    const target = pendingText.startsWith(PENDING)
        ? authorityName(pendingText.slice(PENDING.length, colon < 0 ? undefined : colon))
        : undefined;
    const pathStart = userConfigAt + 2 + USER_CONFIG.length;
    const pathEnd = summary.indexOf(SESSION, pathStart);
    const path = summary.slice(pathStart, pathEnd < 0 ? undefined : pathEnd);
    return {
        applied,
        pending: target && { target, reason: colon < 0 ? "" : pendingText.slice(colon + 2) },
        userConfigPath: pathEnd < 0 || path === "none" ? undefined : decodedPath(path),
        stalled:
            applied === "eidnara" &&
            (appliedText.startsWith(`eidnara${STALLED}`) || summary.includes(LEGACY_STALL)),
    };
}

/** The daemon joins paths without normalizing them, so `//` and `.` segments are removed before the roots compare. */
function configRoot(path: string): string {
    return dirname(normalize(path));
}

function pendingStep(reason: string): string {
    if (reason.startsWith("another binding is open")) return RESTART_OTHER_INSTANCES_STEP;
    if (reason.startsWith("the summarizer is busy")) {
        return "wait for the summarizer to finish, then restart this OpenCode instance";
    }
    return "keep other OpenCode instances on this session closed, then restart this one so it binds the session again";
}

export function foldAuthorityStatus(
    plugin: PluginFoldAuthority,
    summary: unknown,
): FoldAuthorityStatus {
    const daemon = parseDaemonFoldAuthority(summary);
    const pending =
        daemon.pending && daemon.pending.target !== daemon.applied
            ? { ...daemon.pending, step: pendingStep(daemon.pending.reason) }
            : undefined;
    const rootMismatch =
        daemon.userConfigPath !== undefined &&
        plugin.userConfigPath !== undefined &&
        configRoot(daemon.userConfigPath) !== configRoot(plugin.userConfigPath);
    const warnings: string[] = [];
    const labels: string[] = [];
    if (pending) {
        warnings.push(
            `${AUTHORITY_PENDING_WARNING}. The daemon folds with ${daemon.applied} and applies ${pending.target} at the next bind while the session is quiescent (${pending.reason}); next step: ${pending.step}.`,
        );
        labels.push(`fold authority pending ${pending.target}: ${pending.step}`);
    }
    if (rootMismatch) {
        warnings.push(
            `${ROOT_MISMATCH_WARNING}: the daemon reads ${daemon.userConfigPath} and this plugin reads ${plugin.userConfigPath}; ${DAEMON_RESTART_STEP}.`,
        );
        labels.push(`${ROOT_MISMATCH_WARNING}: ${DAEMON_RESTART_STEP}`);
    }
    if (daemon.stalled) labels.push("summarizer stalled at the last pass: no summarizer model");
    return {
        startup: describeFoldAuthority(plugin.startup),
        applied: daemon.applied,
        pending,
        stalled: daemon.stalled,
        daemon_user_config: daemon.userConfigPath,
        plugin_user_config: plugin.userConfigPath,
        warnings,
        label: labels.length > 0 ? labels.join(" · ") : undefined,
    };
}

const APPLIED_TEXT: Record<FoldAuthorityStatus["applied"], string> = {
    eidnara: "Eidnara folds",
    native: "OpenCode's native compaction folds",
    unadopted: "not adopted yet; the session's first committed pass adopts it",
    unknown: "unknown (the daemon did not report it)",
};

export function withDiskAuthority(
    status: FoldAuthorityStatus,
    plugin: PluginFoldAuthority,
): FoldAuthorityStatus {
    return { ...status, disk: describeFoldAuthority(plugin.readDisk()) };
}

export function formatFoldAuthorityLines(status: FoldAuthorityStatus): string[] {
    const lines = [
        "### Fold Authority",
        ...(status.disk === undefined ? [] : [`- On disk: ${status.disk}`]),
        `- Plugin startup: ${status.startup}`,
        `- Daemon applied (session.status): ${APPLIED_TEXT[status.applied]}`,
    ];
    if (status.pending) {
        lines.push(
            `- Pending: ${status.pending.target} (${status.pending.reason}); ${status.pending.step}`,
        );
    }
    if (status.stalled) {
        lines.push(
            "- Summarizer stalled: the session folds with Eidnara and its last pass found no summarizer model",
        );
    }
    lines.push(
        `- User config: daemon ${status.daemon_user_config ?? "unknown"}, plugin ${status.plugin_user_config ?? "unknown"}`,
        ...status.warnings.map((warning) => `- ⚠ ${warning}`),
    );
    return lines;
}

export function reportFoldAuthority(
    plugin: PluginFoldAuthority,
    summary: unknown,
    sessionId: string,
): FoldAuthorityStatus {
    const status = foldAuthorityStatus(plugin, summary);
    plugin.publish(
        status.warnings.length > 0
            ? { disposition: "warn", reasons: status.warnings, unresolved: [] }
            : undefined,
        sessionId,
    );
    return status;
}
