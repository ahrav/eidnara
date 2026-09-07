import { mkdirSync, watch } from "node:fs";
import { mkdir } from "node:fs/promises";
import { basename, dirname, join } from "node:path";
import { parse } from "comment-json";
import { findNodeAtLocation, type Node } from "jsonc-parser";
import { writeFileAtomicSync } from "./atomic-file";
import { setJsoncValue } from "./jsonc-edit";
import {
    isCommentJsonObjectRoot,
    isJsoncEmpty,
    isPrototypePollutionKey,
    parseJsoncTree,
} from "./jsonc-parser";
import { getOpenCodeConfigPaths } from "./opencode-config-dir";
import { isRecord } from "./record-type-guard";
import { readRegularFile, readRegularFileSync } from "./regular-file";
import { resolveWriteTarget } from "./resolve-write-target";

// The file stores one top-level key for each OpenCode TUI plugin.
// Plugin keys must be non-integer-like names such as `eidnara`; the file is optional.
//
//
// Eidnara uses `comment-json` when writing to preserve comments and sibling plugin keys.
// Writers must preserve sibling plugins' values and comments.

export const TUI_PREFS_FILE_ENV = "OPENCODE_TUI_PREFERENCES_FILE";
const FILE_NAME = "tui-preferences.jsonc";

export function getTuiPreferencesFile(): string {
    // The path is used verbatim; only an all-whitespace value counts as unset.
    const override = process.env[TUI_PREFS_FILE_ENV];
    if (override?.trim()) return override;
    return join(getOpenCodeConfigPaths({ binary: "opencode" }).configDir, FILE_NAME);
}

export async function readTuiPreferencesFile(): Promise<Record<string, unknown>> {
    try {
        const raw = await readRegularFile(getTuiPreferencesFile());
        if (raw.trim() === "") return {};
        const root: unknown = parse(raw);
        return isCommentJsonObjectRoot(root) ? root : {};
    } catch {
        return {};
    }
}

// The synchronous read prevents async flicker in the sidebar's initial collapse state and effective order.
// The synchronous reader matches the async reader's tolerance contract and never throws.
export function readTuiPreferencesFileSync(): Record<string, unknown> {
    try {
        const raw = readRegularFileSync(getTuiPreferencesFile());
        if (raw.trim() === "") return {};
        const root: unknown = parse(raw);
        return isCommentJsonObjectRoot(root) ? root : {};
    } catch {
        return {};
    }
}

export const PLUGIN_KEY = "eidnara";
export const DEFAULT_SLOT_ORDER = 170;

export interface EidnaraTuiPrefs {
    forceToTop: boolean;
    order: number;
    startCollapsed: boolean;
    rememberCollapsed: boolean;
    // `collapsed: null` prevents persistence; the UI initializes from `startCollapsed`.
    collapsed: boolean | null;
    header: {
        label: string;
    };
    sections: {
        historian: boolean;
        memory: boolean;
        status: boolean;
        dreamer: boolean;
        stats: boolean;
    };
}

export type TuiSections = EidnaraTuiPrefs["sections"];

export const DEFAULT_PREFS: EidnaraTuiPrefs = {
    forceToTop: false,
    order: DEFAULT_SLOT_ORDER,
    startCollapsed: false,
    rememberCollapsed: true,
    collapsed: null,
    header: { label: "Eidnara" },
    sections: {
        historian: true,
        memory: true,
        status: true,
        dreamer: true,
        stats: true,
    },
};

function bool(value: unknown, fallback: boolean): boolean {
    return typeof value === "boolean" ? value : fallback;
}

function int(value: unknown, fallback: number, min: number, max: number): number {
    if (typeof value !== "number" || !Number.isFinite(value)) return fallback;
    return Math.min(Math.max(Math.round(value), min), max);
}

function label(value: unknown, fallback: string, maxLength: number): string {
    if (typeof value !== "string" || value.length === 0) return fallback;
    // Array.from creates code-point elements, so slice cannot split an astral character into lone surrogates.
    return Array.from(value).slice(0, maxLength).join("");
}

// Each preference is independently clamped or defaulted, so an invalid value does not affect valid preferences.
// The reader never throws; a missing or non-object `eidnara` value produces a full defaults clone.
export function resolveEidnaraPrefs(root: Record<string, unknown>): EidnaraTuiPrefs {
    const entry = root[PLUGIN_KEY];
    if (!isRecord(entry)) return structuredClone(DEFAULT_PREFS);

    const d = DEFAULT_PREFS;
    const header = isRecord(entry.header) ? entry.header : {};
    const sections = isRecord(entry.sections) ? entry.sections : {};

    return {
        forceToTop: bool(entry.forceToTop, d.forceToTop),
        order: int(entry.order, d.order, -10000, 10000),
        startCollapsed: bool(entry.startCollapsed, d.startCollapsed),
        rememberCollapsed: bool(entry.rememberCollapsed, d.rememberCollapsed),
        collapsed: typeof entry.collapsed === "boolean" ? entry.collapsed : null,
        header: {
            label: label(header.label, d.header.label, 24),
        },
        sections: {
            historian: bool(sections.historian, d.sections.historian),
            memory: bool(sections.memory, d.sections.memory),
            status: bool(sections.status, d.sections.status),
            dreamer: bool(sections.dreamer, d.sections.dreamer),
            stats: bool(sections.stats, d.sections.stats),
        },
    };
}

const FORCE_TOP_BASE = -100000;

// Forced plugins sort below `FORCE_TOP_BASE`; top-level key order breaks ties.
// Users reprioritize forced plugins by reordering their top-level keys.
// The `order` value clamps to -10000..10000, strictly above the forced band.
// A manual `order` never overrides `forceToTop`.
// Host slots render in ascending order.
//
// JavaScript iterates integer-like keys such as `"0"` and `"42"` before string keys.
// Integer-like keys iterate before string keys, skewing index-based forced-plugin ordering.
export function computeEffectiveOrder(
    root: Record<string, unknown>,
    pluginKey: string,
    defaultOrder: number,
): number {
    const entry = root[pluginKey];
    if (!isRecord(entry)) return defaultOrder;
    if (entry.forceToTop === true) {
        return FORCE_TOP_BASE + Object.keys(root).indexOf(pluginKey);
    }
    return int(entry.order, defaultOrder, -10000, 10000);
}

const TEMPLATE = `// Shared preferences for OpenCode TUI plugins.
// Plugins update individual keys and preserve all other values and comments.
{}
`;

type JsonValue = string | number | boolean | null;

function isErrnoException(error: unknown): error is NodeJS.ErrnoException {
    return error instanceof Error && "code" in error;
}

/**
 * Text-level edits keep sibling plugins' comments, formatting, and integers beyond `Number.MAX_SAFE_INTEGER` byte for byte.
 * Paths start with `pluginKey`, so replacing a non-object intermediate cannot modify sibling plugin keys.
 * Prototype keys are refused so the written document cannot carry a `__proto__` member.
 */
function applyPreference(text: string, fullPath: string[], value: JsonValue): string | null {
    if (fullPath.some(isPrototypePollutionKey)) return null;
    let tree: Node;
    try {
        tree = parseJsoncTree(text);
    } catch {
        return null;
    }
    // An array or scalar root is the user's document too; replacing it with `{}` would discard it.
    if (tree.type !== "object") return null;
    let next = text;
    for (let depth = 1; depth < fullPath.length; depth += 1) {
        const prefix = fullPath.slice(0, depth);
        const node = findNodeAtLocation(tree, prefix);
        if (node && node.type !== "object") {
            next = setJsoncValue(next, prefix, {});
            break;
        }
    }
    return setJsoncValue(next, fullPath, value);
}

async function writePreference(pluginKey: string, path: string[], value: JsonValue): Promise<void> {
    const file = getTuiPreferencesFile();
    // One resolution serves the read, the staging file, and the rename, so a
    // link retargeted mid-write cannot receive the previous target's snapshot.
    // The parent created is the target's: a dangling link may point into a directory that does not exist yet.
    const target = resolveWriteTarget(file);
    await mkdir(dirname(target), { recursive: true });
    let text: string;
    try {
        text = await readRegularFile(target);
    } catch (error) {
        // Only ENOENT permits seeding; renaming a template over a file that exists but cannot be read would erase every sibling plugin's data.
        if (!isErrnoException(error) || error.code !== "ENOENT") return;
        text = "";
    }
    if (text.trim() === "") {
        text = TEMPLATE;
    } else if (isJsoncEmpty(text)) {
        // The editor needs a value to edit; appending an empty object keeps the user's comments ahead of it.
        text = `${text}\n{}`;
    }

    const next = applyPreference(text, [pluginKey, ...path], value);
    if (next === null || next === text) return;
    writeFileAtomicSync(target, next.endsWith("\n") ? next : `${next}\n`);
}

let writeChain: Promise<void> = Promise.resolve();

// The promise chain serializes writes so each update reads the latest file.
// Same-directory temp-file renames replace the file atomically.
// Preference-write failures do not crash the TUI.
export function queueTuiPreferenceUpdate(
    pluginKey: string,
    path: string[],
    value: JsonValue,
): Promise<void> {
    writeChain = writeChain.then(() => writePreference(pluginKey, path, value)).catch(() => {});
    return writeChain;
}

const WATCH_DEBOUNCE_MS = 150;
// No filesystem event follows a transient EMFILE or EIO clearing, so the read that failed is retried on a timer.
const READ_RETRY_BASE_MS = 200;
const READ_RETRY_MAX = 3;
// `fs.watch` fails with EMFILE or ENOSPC when descriptors or inotify watches run out; registration is retried on a timer.
const WATCH_INSTALL_RETRY_BASE_MS = 500;
const WATCH_INSTALL_RETRY_MAX = 3;

type WatchReadFile = (file: string) => Promise<string>;
/** `on` is optional so a test double can implement only `close()`. */
type WatchHandle = {
    close(): void;
    on?(event: "error", listener: (error: unknown) => void): unknown;
};
type WatchDirectory = (
    directory: string,
    listener: (event: string, filename: string | null) => void,
) => WatchHandle;

let watchReadFile: WatchReadFile = readRegularFile;
let watchDirectory: WatchDirectory = (directory, listener) => watch(directory, listener);

export function __setTuiPreferencesWatchTestHooks(hooks: {
    readFile?: WatchReadFile;
    watch?: WatchDirectory;
}): void {
    watchReadFile = hooks.readFile ?? readRegularFile;
    watchDirectory = hooks.watch ?? ((directory, listener) => watch(directory, listener));
}

export function __resetTuiPreferencesWatchTestHooks(): void {
    watchReadFile = readRegularFile;
    watchDirectory = (directory, listener) => watch(directory, listener);
}

// The watcher observes the directory because renaming the preference file invalidates file-level watchers.
// A symlinked file is watched at its resolved target, where the writer's renames and an editor's saves land.
export function watchTuiPreferences(onChange: () => void): () => void {
    const file = getTuiPreferencesFile();
    const target = resolveWriteTarget(file);
    const name = basename(target);
    let timer: ReturnType<typeof setTimeout> | null = null;
    let retryTimer: ReturnType<typeof setTimeout> | null = null;
    let retries = 0;
    let lastSeen: string | null = null;
    // Reads may complete out of order; only the newest read is allowed to update `lastSeen`.
    let generation = 0;
    let stopped = false;
    try {
        lastSeen = readRegularFileSync(file);
    } catch {
        // A missing or unreadable baseline is retried after registration.
    }
    const reconcile = (): void => {
        generation += 1;
        const started = generation;
        void watchReadFile(file)
            .then(
                (text) => ({ text, missing: false }),
                (error: unknown) => ({
                    text: null,
                    missing: isErrnoException(error) && error.code === "ENOENT",
                }),
            )
            .then(({ text, missing }) => {
                if (stopped || started !== generation) return;
                if (text === null) {
                    if (missing) {
                        retries = 0;
                        // ENOENT after a loaded baseline is the file's removal; readers now resolve defaults.
                        if (lastSeen !== null) {
                            lastSeen = null;
                            onChange();
                        }
                        return;
                    }
                    // Any other failure keeps the last-known content and schedules a bounded retry.
                    if (retries < READ_RETRY_MAX) {
                        retries += 1;
                        retryTimer = setTimeout(
                            () => {
                                retryTimer = null;
                                reconcile();
                            },
                            READ_RETRY_BASE_MS * 2 ** (retries - 1),
                        );
                    }
                    return;
                }
                retries = 0;
                if (text === lastSeen) return;
                lastSeen = text;
                onChange();
            });
    };
    const onDirectoryEvent = (_event: string, filename: string | null): void => {
        const isOurs =
            filename === name || (filename?.startsWith(`${name}.`) && filename.endsWith(".tmp"));
        if (filename != null && !isOurs) return;
        if (timer) clearTimeout(timer);
        timer = setTimeout(() => {
            timer = null;
            reconcile();
        }, WATCH_DEBOUNCE_MS);
    };

    let watcher: WatchHandle | null = null;
    let installTimer: ReturnType<typeof setTimeout> | null = null;
    let installAttempts = 0;
    const scheduleInstall = (): void => {
        if (stopped || installAttempts >= WATCH_INSTALL_RETRY_MAX) return;
        installAttempts += 1;
        installTimer = setTimeout(
            () => {
                installTimer = null;
                install();
            },
            WATCH_INSTALL_RETRY_BASE_MS * 2 ** (installAttempts - 1),
        );
    };
    const install = (): void => {
        if (stopped) return;
        try {
            // `fs.watch` throws when its target directory does not exist.
            mkdirSync(dirname(target), { recursive: true });
            watcher = watchDirectory(dirname(target), onDirectoryEvent);
        } catch {
            scheduleInstall();
            return;
        }
        // An `error` emitted after installation is an uncaught exception without a listener; the watcher is replaced instead.
        watcher.on?.("error", () => {
            watcher?.close();
            watcher = null;
            scheduleInstall();
        });
        // Registration reconciles once to observe changes between the baseline read and watcher installation.
        reconcile();
    };
    install();
    return () => {
        // A read still in flight must not call `onChange` into torn-down state.
        stopped = true;
        if (timer) clearTimeout(timer);
        if (retryTimer) clearTimeout(retryTimer);
        if (installTimer) clearTimeout(installTimer);
        watcher?.close();
    };
}
