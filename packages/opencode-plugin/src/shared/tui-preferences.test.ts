import { afterEach, beforeEach, describe, expect, test } from "bun:test";
import { existsSync } from "node:fs";
import {
    chmod,
    lstat,
    mkdir,
    mkdtemp,
    readdir,
    readFile,
    rename,
    rm,
    stat,
    symlink,
    writeFile,
} from "node:fs/promises";
import { tmpdir } from "node:os";
import { basename, isAbsolute, join } from "node:path";
import { parse } from "comment-json";
import { getOpenCodeConfigPaths } from "./opencode-config-dir";
import {
    __resetTuiPreferencesWatchTestHooks,
    __setTuiPreferencesWatchTestHooks,
    computeEffectiveOrder,
    DEFAULT_PREFS,
    DEFAULT_SLOT_ORDER,
    getTuiPreferencesFile,
    PLUGIN_KEY,
    queueTuiPreferenceUpdate,
    readTuiPreferencesFile,
    readTuiPreferencesFileSync,
    resolveEidnaraPrefs,
    TUI_PREFS_FILE_ENV,
    watchTuiPreferences,
} from "./tui-preferences";

let dir: string;
let file: string;
const savedEnv: Record<string, string | undefined> = {};
const ENV_KEYS = [TUI_PREFS_FILE_ENV, "OPENCODE_CONFIG_DIR", "XDG_CONFIG_HOME"];

beforeEach(async () => {
    for (const key of ENV_KEYS) savedEnv[key] = process.env[key];
    dir = await mkdtemp(join(tmpdir(), "eidnara-tui-prefs-test-"));
    file = join(dir, "tui-preferences.jsonc");
    process.env[TUI_PREFS_FILE_ENV] = file;
});

afterEach(async () => {
    __resetTuiPreferencesWatchTestHooks();
    for (const key of ENV_KEYS) {
        if (savedEnv[key] === undefined) delete process.env[key];
        else process.env[key] = savedEnv[key];
    }
    await rm(dir, { recursive: true, force: true });
});

describe("getTuiPreferencesFile", () => {
    test("env override wins", () => {
        expect(getTuiPreferencesFile()).toBe(file);
    });

    test("falls back to OPENCODE_CONFIG_DIR then XDG then ~/.config", () => {
        delete process.env[TUI_PREFS_FILE_ENV];
        process.env.OPENCODE_CONFIG_DIR = "/tmp/cfgdir";
        expect(getTuiPreferencesFile()).toBe("/tmp/cfgdir/tui-preferences.jsonc");
        delete process.env.OPENCODE_CONFIG_DIR;
        process.env.XDG_CONFIG_HOME = "/tmp/xdg";
        expect(getTuiPreferencesFile()).toBe("/tmp/xdg/opencode/tui-preferences.jsonc");
    });

    test("resolves OPENCODE_CONFIG_DIR exactly like the shared config-dir resolver", () => {
        delete process.env[TUI_PREFS_FILE_ENV];
        process.env.OPENCODE_CONFIG_DIR = "  /tmp/cfgdir  ";
        expect(getTuiPreferencesFile()).toBe(
            join(getOpenCodeConfigPaths({ binary: "opencode" }).configDir, "tui-preferences.jsonc"),
        );
        expect(getTuiPreferencesFile()).toBe("/tmp/cfgdir/tui-preferences.jsonc");

        process.env.OPENCODE_CONFIG_DIR = "relative-cfg";
        expect(getTuiPreferencesFile()).toBe(
            join(getOpenCodeConfigPaths({ binary: "opencode" }).configDir, "tui-preferences.jsonc"),
        );
        expect(isAbsolute(getTuiPreferencesFile())).toBe(true);
    });

    test("a blank env override falls back to the config directory instead of cwd", () => {
        process.env[TUI_PREFS_FILE_ENV] = "   ";
        process.env.OPENCODE_CONFIG_DIR = "/tmp/cfgdir";
        expect(getTuiPreferencesFile()).toBe("/tmp/cfgdir/tui-preferences.jsonc");
    });
});

describe("readTuiPreferencesFile (tolerant)", () => {
    test("missing file → {}", async () => {
        expect(await readTuiPreferencesFile()).toEqual({});
    });

    test("malformed JSON → {}", async () => {
        await writeFile(file, "{ this is not json ", "utf8");
        expect(await readTuiPreferencesFile()).toEqual({});
    });

    test("non-object root → {}", async () => {
        await writeFile(file, "[1, 2, 3]", "utf8");
        expect(await readTuiPreferencesFile()).toEqual({});
        // comment-json boxes a scalar root into a String object; it must not pass as a record.
        await writeFile(file, '"just a string"', "utf8");
        expect(await readTuiPreferencesFile()).toEqual({});
    });

    test.skipIf(process.platform === "win32")(
        "a FIFO at the preferences path reads as defaults instead of blocking",
        async () => {
            expect(Bun.spawnSync({ cmd: ["mkfifo", file] }).exitCode).toBe(0);
            // Bun's per-test timeout fails this test if any preference operation blocks on the FIFO.
            expect(await readTuiPreferencesFile()).toEqual({});
            expect(readTuiPreferencesFileSync()).toEqual({});
            const stop = watchTuiPreferences(() => {});
            stop();
            await queueTuiPreferenceUpdate(PLUGIN_KEY, ["collapsed"], true);
            expect((await lstat(file)).isFIFO()).toBe(true);
        },
    );

    test("jsonc with comments + trailing comma parses", async () => {
        await writeFile(
            file,
            `{
  // a comment
  "eidnara": { "order": 205, },
}`,
            "utf8",
        );
        const root = await readTuiPreferencesFile();
        expect(resolveEidnaraPrefs(root).order).toBe(205);
    });
});

describe("watchTuiPreferences", () => {
    test("notifies when the initial read resolves after an atomic replacement", async () => {
        const previous = `{"eidnara":{"sections":{"memory":true}}}\n`;
        const next = `{"eidnara":{"sections":{"memory":false}}}\n`;
        await writeFile(file, previous, "utf8");

        let resolveInitialRead!: (text: string) => void;
        const pendingInitialRead = new Promise<string>((resolve) => {
            resolveInitialRead = resolve;
        });
        let emitWatchEvent!: (event: string, filename: string | null) => void;
        let readCount = 0;
        __setTuiPreferencesWatchTestHooks({
            readFile: () => {
                readCount += 1;
                return readCount === 1 ? pendingInitialRead : Promise.resolve(next);
            },
            watch: (_directory, listener) => {
                emitWatchEvent = listener;
                return { close() {} };
            },
        });

        const observed = new Promise<{ memory: boolean }>((resolve) => {
            const stop = watchTuiPreferences(() => {
                void readTuiPreferencesFile().then((root) => {
                    stop();
                    resolve({ memory: resolveEidnaraPrefs(root).sections.memory });
                });
            });
        });

        const replacement = `${file}.external.tmp`;
        await writeFile(replacement, next, "utf8");
        await rename(replacement, file);
        emitWatchEvent("rename", basename(file));
        resolveInitialRead(next);

        const result = await Promise.race([
            observed,
            new Promise<"timeout">((resolve) => setTimeout(() => resolve("timeout"), 300)),
        ]);
        expect(result).toEqual({ memory: false });
    });

    test("notifies when a previously loaded file is deleted so readers fall back to defaults", async () => {
        await writeFile(file, `{"eidnara":{"sections":{"memory":false}}}\n`, "utf8");

        let emitWatchEvent!: (event: string, filename: string | null) => void;
        let deleted = false;
        __setTuiPreferencesWatchTestHooks({
            readFile: (path) =>
                deleted
                    ? Promise.reject(Object.assign(new Error("ENOENT"), { code: "ENOENT" }))
                    : readFile(path, "utf8"),
            watch: (_directory, listener) => {
                emitWatchEvent = listener;
                return { close() {} };
            },
        });

        const observed = new Promise<{ memory: boolean }>((resolve) => {
            const stop = watchTuiPreferences(() => {
                void readTuiPreferencesFile().then((root) => {
                    stop();
                    resolve({ memory: resolveEidnaraPrefs(root).sections.memory });
                });
            });
        });

        // The baseline and the post-registration reconcile see identical
        // content, so neither fires onChange; only the deletion may resolve `observed`.
        await rm(file);
        deleted = true;
        emitWatchEvent("rename", basename(file));

        const result = await Promise.race([
            observed,
            new Promise<"timeout">((resolve) => setTimeout(() => resolve("timeout"), 600)),
        ]);
        expect(result).toEqual({ memory: true });
    });

    test("stays silent when a file that never existed keeps failing to read", async () => {
        let emitWatchEvent!: (event: string, filename: string | null) => void;
        __setTuiPreferencesWatchTestHooks({
            readFile: () => Promise.reject(Object.assign(new Error("ENOENT"), { code: "ENOENT" })),
            watch: (_directory, listener) => {
                emitWatchEvent = listener;
                return { close() {} };
            },
        });

        let changes = 0;
        const stop = watchTuiPreferences(() => {
            changes += 1;
        });
        emitWatchEvent("rename", basename(file));
        await new Promise((resolve) => setTimeout(resolve, 400));
        stop();
        expect(changes).toBe(0);
    });

    test("a transient read error keeps the last-known content instead of announcing a deletion", async () => {
        await writeFile(file, `{"eidnara":{"order":1}}\n`, "utf8");
        let emitWatchEvent!: (event: string, filename: string | null) => void;
        let readCount = 0;
        __setTuiPreferencesWatchTestHooks({
            readFile: (path) => {
                readCount += 1;
                return readCount === 1
                    ? readFile(path, "utf8")
                    : Promise.reject(Object.assign(new Error("EACCES"), { code: "EACCES" }));
            },
            watch: (_directory, listener) => {
                emitWatchEvent = listener;
                return { close() {} };
            },
        });

        let changes = 0;
        const stop = watchTuiPreferences(() => {
            changes += 1;
        });
        emitWatchEvent("change", basename(file));
        await new Promise((resolve) => setTimeout(resolve, 400));
        stop();
        expect(readCount).toBeGreaterThanOrEqual(2);
        expect(changes).toBe(0);
    });

    test("retries a transient read failure and picks up the change once it clears", async () => {
        await writeFile(file, `{"eidnara":{"order":1}}\n`, "utf8");
        const next = `{"eidnara":{"order":2}}\n`;
        let emitWatchEvent!: (event: string, filename: string | null) => void;
        let failuresLeft = 2;
        let readCount = 0;
        __setTuiPreferencesWatchTestHooks({
            readFile: (path) => {
                readCount += 1;
                if (readCount === 1) return readFile(path, "utf8");
                if (failuresLeft > 0) {
                    failuresLeft -= 1;
                    return Promise.reject(Object.assign(new Error("EMFILE"), { code: "EMFILE" }));
                }
                return Promise.resolve(next);
            },
            watch: (_directory, listener) => {
                emitWatchEvent = listener;
                return { close() {} };
            },
        });

        let changes = 0;
        const stop = watchTuiPreferences(() => {
            changes += 1;
        });
        emitWatchEvent("change", basename(file));
        // Debounce 150ms, then retries at 200ms and 400ms before the third read succeeds.
        await new Promise((resolve) => setTimeout(resolve, 1200));
        stop();
        expect(failuresLeft).toBe(0);
        expect(changes).toBe(1);
    });

    test("gives up retrying after the bounded number of transient failures", async () => {
        await writeFile(file, `{"eidnara":{"order":1}}\n`, "utf8");
        let emitWatchEvent!: (event: string, filename: string | null) => void;
        let failedReads = 0;
        let readCount = 0;
        __setTuiPreferencesWatchTestHooks({
            readFile: (path) => {
                readCount += 1;
                if (readCount === 1) return readFile(path, "utf8");
                failedReads += 1;
                return Promise.reject(Object.assign(new Error("EIO"), { code: "EIO" }));
            },
            watch: (_directory, listener) => {
                emitWatchEvent = listener;
                return { close() {} };
            },
        });

        let changes = 0;
        const stop = watchTuiPreferences(() => {
            changes += 1;
        });
        emitWatchEvent("change", basename(file));
        // One read from the event plus three retries (200, 400, 800ms), then nothing.
        await new Promise((resolve) => setTimeout(resolve, 2200));
        stop();
        expect(failedReads).toBe(4);
        expect(changes).toBe(0);
    });

    test("a read still pending when the watcher stops never reaches onChange", async () => {
        await writeFile(file, `{"eidnara":{"order":1}}\n`, "utf8");
        let resolvePendingRead!: (text: string) => void;
        __setTuiPreferencesWatchTestHooks({
            readFile: () =>
                new Promise<string>((resolve) => {
                    resolvePendingRead = resolve;
                }),
            watch: () => ({ close() {} }),
        });

        let changes = 0;
        const stop = watchTuiPreferences(() => {
            changes += 1;
        });
        stop();
        resolvePendingRead(`{"eidnara":{"order":2}}\n`);
        await new Promise((resolve) => setTimeout(resolve, 50));
        expect(changes).toBe(0);
    });

    test.skipIf(process.platform === "win32")(
        "creates the target directory of a dangling preferences symlink before writing",
        async () => {
            const target = join(dir, "dotfiles", "not-yet", "tui-preferences.jsonc");
            await symlink(target, file);

            await queueTuiPreferenceUpdate(PLUGIN_KEY, ["collapsed"], true);

            expect((await lstat(file)).isSymbolicLink()).toBe(true);
            expect(resolveEidnaraPrefs(await readTuiPreferencesFile()).collapsed).toBe(true);
            expect(existsSync(target)).toBe(true);
        },
    );

    test.skipIf(process.platform === "win32")(
        "watches and writes through a symlinked preferences file without replacing the link",
        async () => {
            const dotfiles = join(dir, "dotfiles");
            await mkdir(dotfiles);
            const target = join(dotfiles, "tui-preferences.jsonc");
            await writeFile(target, `{ "anthropic-auth": { "order": 160 } }\n`, "utf8");
            await symlink(target, file);

            const watched: string[] = [];
            __setTuiPreferencesWatchTestHooks({
                watch: (directory, _listener) => {
                    watched.push(directory);
                    return { close() {} };
                },
            });
            watchTuiPreferences(() => {})();
            expect(watched).toEqual([dotfiles]);

            await queueTuiPreferenceUpdate(PLUGIN_KEY, ["collapsed"], true);

            expect((await lstat(file)).isSymbolicLink()).toBe(true);
            const root = parse(await readFile(target, "utf8")) as Record<string, unknown>;
            expect(resolveEidnaraPrefs(root).collapsed).toBe(true);
            expect((root["anthropic-auth"] as Record<string, unknown>).order).toBe(160);
            expect((await readdir(dir)).sort()).toEqual(["dotfiles", basename(file)].sort());
        },
    );

    test("creates a missing config directory so the watcher can be installed on first run", async () => {
        const nested = join(dir, "not-yet", "opencode");
        process.env[TUI_PREFS_FILE_ENV] = join(nested, "tui-preferences.jsonc");
        const watched: string[] = [];
        __setTuiPreferencesWatchTestHooks({
            watch: (directory, _listener) => {
                watched.push(directory);
                return { close() {} };
            },
        });

        const stop = watchTuiPreferences(() => {});
        stop();

        expect(watched).toEqual([nested]);
        expect(existsSync(nested)).toBe(true);
    });

    test("retries watcher registration after a transient failure and then observes changes", async () => {
        await writeFile(file, `{"eidnara":{"order":1}}\n`, "utf8");
        let attempts = 0;
        let emitWatchEvent: ((event: string, filename: string | null) => void) | null = null;
        __setTuiPreferencesWatchTestHooks({
            readFile: () => Promise.resolve(`{"eidnara":{"order":2}}\n`),
            watch: (_directory, listener) => {
                attempts += 1;
                if (attempts === 1) {
                    throw Object.assign(new Error("EMFILE"), { code: "EMFILE" });
                }
                emitWatchEvent = listener;
                return { close() {} };
            },
        });

        let changes = 0;
        const stop = watchTuiPreferences(() => {
            changes += 1;
        });
        // No watcher yet, so nothing has been reconciled or observed.
        expect(emitWatchEvent).toBeNull();
        expect(changes).toBe(0);

        // The first retry lands after 500ms; the registration-time reconcile then sees the new content.
        await new Promise((resolve) => setTimeout(resolve, 700));
        expect(attempts).toBe(2);
        expect(emitWatchEvent).not.toBeNull();
        expect(changes).toBe(1);
        stop();
    });

    test("stops retrying watcher registration once disposed", async () => {
        let attempts = 0;
        __setTuiPreferencesWatchTestHooks({
            watch: () => {
                attempts += 1;
                throw Object.assign(new Error("ENOSPC"), { code: "ENOSPC" });
            },
        });

        const stop = watchTuiPreferences(() => {});
        stop();
        await new Promise((resolve) => setTimeout(resolve, 700));
        expect(attempts).toBe(1);
    });

    test("an error emitted by an installed watcher closes it and installs a replacement", async () => {
        await writeFile(file, `{"eidnara":{"order":1}}\n`, "utf8");
        const handles: Array<{ closed: boolean; fail: (error: Error) => void }> = [];
        __setTuiPreferencesWatchTestHooks({
            watch: () => {
                let errorListener: ((error: unknown) => void) | null = null;
                const handle = {
                    closed: false,
                    fail: (error: Error) => errorListener?.(error),
                };
                handles.push(handle);
                return {
                    close() {
                        handle.closed = true;
                    },
                    on(_event: "error", listener: (error: unknown) => void) {
                        errorListener = listener;
                    },
                };
            },
        });

        const stop = watchTuiPreferences(() => {});
        expect(handles).toHaveLength(1);

        // An unhandled watcher `error` event throws; the listener closes and replaces the watcher.
        handles[0].fail(Object.assign(new Error("EIO"), { code: "EIO" }));
        expect(handles[0].closed).toBe(true);
        await new Promise((resolve) => setTimeout(resolve, 700));
        expect(handles).toHaveLength(2);
        expect(handles[1].closed).toBe(false);
        stop();
        expect(handles[1].closed).toBe(true);
    });

    test("a stale read completing after a newer one cannot roll lastSeen back", async () => {
        const v1 = `{"eidnara":{"order":1}}\n`;
        const v2 = `{"eidnara":{"order":2}}\n`;
        await writeFile(file, v1, "utf8");

        let resolveStaleRead!: (text: string) => void;
        const staleRead = new Promise<string>((resolve) => {
            resolveStaleRead = resolve;
        });
        let emitWatchEvent!: (event: string, filename: string | null) => void;
        let readCount = 0;
        __setTuiPreferencesWatchTestHooks({
            // The registration-time read stalls; every later read sees v2.
            readFile: () => {
                readCount += 1;
                return readCount === 1 ? staleRead : Promise.resolve(v2);
            },
            watch: (_directory, listener) => {
                emitWatchEvent = listener;
                return { close() {} };
            },
        });

        let changes = 0;
        const stop = watchTuiPreferences(() => {
            changes += 1;
        });
        const settle = () => new Promise((resolve) => setTimeout(resolve, 250));

        emitWatchEvent("rename", basename(file));
        await settle();
        expect(changes).toBe(1);

        // The stalled read now completes with the superseded v1 content.
        resolveStaleRead(v1);
        await settle();

        // v2 is still the newest content, so re-reading it must not look like a change.
        emitWatchEvent("rename", basename(file));
        await settle();
        stop();
        expect(changes).toBe(1);
    });
});

describe("resolveEidnaraPrefs (per-key validation)", () => {
    test("missing key → full defaults clone", () => {
        expect(resolveEidnaraPrefs({})).toEqual(DEFAULT_PREFS);
        expect(resolveEidnaraPrefs({})).not.toBe(DEFAULT_PREFS);
    });

    test("one bad value never poisons the rest", () => {
        const prefs = resolveEidnaraPrefs({
            eidnara: {
                order: "nope",
                rememberCollapsed: 1,
                collapsed: true,
                sections: { historian: false, memory: "bad" },
            },
        });
        expect(prefs.order).toBe(DEFAULT_SLOT_ORDER); // bad → default
        expect(prefs.rememberCollapsed).toBe(true); // bad → default true
        expect(prefs.collapsed).toBe(true); // valid bool preserved
        expect(prefs.sections.historian).toBe(false); // valid bool preserved
        expect(prefs.sections.memory).toBe(true); // bad → default true
    });

    test("order clamps to -10000..10000", () => {
        expect(resolveEidnaraPrefs({ eidnara: { order: 99999 } }).order).toBe(10000);
        expect(resolveEidnaraPrefs({ eidnara: { order: -99999 } }).order).toBe(-10000);
    });

    test("collapsed non-boolean → null (seed from startCollapsed)", () => {
        expect(resolveEidnaraPrefs({ eidnara: {} }).collapsed).toBeNull();
    });

    test("header label clamps length, empty → default", () => {
        expect(resolveEidnaraPrefs({ eidnara: { header: { label: "" } } }).header.label).toBe(
            DEFAULT_PREFS.header.label,
        );
        expect(
            resolveEidnaraPrefs({
                eidnara: { header: { label: "x".repeat(50) } },
            }).header.label.length,
        ).toBe(24);
    });

    test("header label clamp never splits a surrogate pair", () => {
        const label = `${"x".repeat(23)}😀 tail`;
        const clamped = resolveEidnaraPrefs({ eidnara: { header: { label } } }).header.label;
        expect(clamped).toBe(`${"x".repeat(23)}😀`);
        expect(Array.from(clamped)).toHaveLength(24);
        expect(clamped.isWellFormed()).toBe(true);
    });
});

describe("computeEffectiveOrder (cross-plugin convention)", () => {
    test("default when key missing", () => {
        expect(computeEffectiveOrder({}, PLUGIN_KEY, DEFAULT_SLOT_ORDER)).toBe(DEFAULT_SLOT_ORDER);
    });

    test("explicit order clamped", () => {
        expect(computeEffectiveOrder({ eidnara: { order: 250 } }, PLUGIN_KEY, 200)).toBe(250);
    });

    test("forceToTop sorts below FORCE_TOP_BASE by key position", () => {
        const root = { aft: { forceToTop: true }, eidnara: { forceToTop: true } };
        expect(computeEffectiveOrder(root, "aft", 200)).toBe(-100000 + 0);
        expect(computeEffectiveOrder(root, "eidnara", 200)).toBe(-100000 + 1);
        // forced always beats any manual order (clamped band is strictly above)
        expect(computeEffectiveOrder(root, "aft", 200)).toBeLessThan(-10000);
    });
});

describe("write path — comment-json full round-trip", () => {
    test("persists a nested key and reads back", async () => {
        await queueTuiPreferenceUpdate(PLUGIN_KEY, ["collapsed"], true);
        const prefs = resolveEidnaraPrefs(await readTuiPreferencesFile());
        expect(prefs.collapsed).toBe(true);
    });

    test("seeds the file from the template when absent", async () => {
        await queueTuiPreferenceUpdate(PLUGIN_KEY, ["order"], 205);
        const text = await readFile(file, "utf8");
        expect(text).toContain("Shared preferences for OpenCode TUI plugins");
        expect(resolveEidnaraPrefs(await readTuiPreferencesFile()).order).toBe(205);
    });

    test("writes into a comment-only file and keeps the comments", async () => {
        const preamble = "// preferences live here\n/* managed by hand */\n";
        await writeFile(file, preamble, "utf8");
        await queueTuiPreferenceUpdate(PLUGIN_KEY, ["order"], 205);
        const text = await readFile(file, "utf8");
        expect(text).toContain("// preferences live here");
        expect(text).toContain("/* managed by hand */");
        expect(resolveEidnaraPrefs(await readTuiPreferencesFile()).order).toBe(205);
    });

    test("INTEROP: a sibling plugin's values AND comments survive Eidnara writing only its key", async () => {
        // Eidnara must preserve anthropic-auth's comments and unknown appearance block.
        // Eidnara modifies only its own key.
        await writeFile(
            file,
            `{
  // Eidnara must preserve anthropic-auth's leading comment.
  "anthropic-auth": {
    "order": 160,
    "header": { "label": "CLAUDE" },
    // Eidnara must preserve anthropic-auth.appearance, which has no schema.
    "appearance": { "barWidth": 10, "barFilledChar": "#" },
    "pollMs": 2000 // INLINE trailing comment — must survive too
  },
  "eidnara": { "order": 200 }
}
`,
            "utf8",
        );

        await queueTuiPreferenceUpdate(PLUGIN_KEY, ["collapsed"], true);

        const text = await readFile(file, "utf8");
        expect(text).toContain("Eidnara must preserve anthropic-auth's leading comment");
        expect(text).toContain("Eidnara must preserve anthropic-auth.appearance");
        expect(text).toContain("INLINE trailing comment — must survive too");

        const root = parse(text) as Record<string, Record<string, unknown>>;
        const aa = root["anthropic-auth"] as Record<string, unknown>;
        expect(aa.order).toBe(160);
        expect((aa.header as Record<string, unknown>).label).toBe("CLAUDE");
        const appearance = aa.appearance as Record<string, unknown>;
        expect(appearance.barWidth).toBe(10);
        expect(appearance.barFilledChar).toBe("#");

        expect(resolveEidnaraPrefs(root).collapsed).toBe(true);
        expect(resolveEidnaraPrefs(root).order).toBe(200);
    });

    test("INTEROP: a sibling plugin's unsafe integer and formatting survive byte for byte", async () => {
        const sibling = `  "anthropic-auth": {\n    "sessionId": 9007199254740993,\n    "ratio": 0.1000\n  },`;
        await writeFile(file, `{\n${sibling}\n  "eidnara": { "order": 200 }\n}\n`, "utf8");

        await queueTuiPreferenceUpdate(PLUGIN_KEY, ["collapsed"], true);

        const text = await readFile(file, "utf8");
        // Reserializing would have rounded the integer to 9007199254740992 and dropped the trailing zeros.
        expect(text).toContain(sibling);
        expect(resolveEidnaraPrefs(await readTuiPreferencesFile()).collapsed).toBe(true);
    });

    test("malformed existing file → write is a no-op, sibling content untouched", async () => {
        const broken = `{ "anthropic-auth": { "order": 160 } broken `;
        await writeFile(file, broken, "utf8");
        await queueTuiPreferenceUpdate(PLUGIN_KEY, ["collapsed"], true);
        // The writer never clobbers a file it cannot safely parse.
        expect(await readFile(file, "utf8")).toBe(broken);
    });

    test("non-object root → write is a no-op instead of replacing the document", async () => {
        for (const original of ["[1, 2, 3]\n", '"just a string"\n', "null\n"]) {
            await writeFile(file, original, "utf8");
            await queueTuiPreferenceUpdate(PLUGIN_KEY, ["collapsed"], true);
            expect(await readFile(file, "utf8")).toBe(original);
        }
    });

    test("refuses prototype keys in the path and leaves Object.prototype untouched", async () => {
        const original = `{ "anthropic-auth": { "order": 160 } }\n`;
        await writeFile(file, original, "utf8");
        for (const path of [
            ["__proto__", "polluted"],
            ["sections", "constructor", "prototype", "polluted"],
            ["prototype", "polluted"],
        ]) {
            await queueTuiPreferenceUpdate(PLUGIN_KEY, path, true);
        }
        expect(await readFile(file, "utf8")).toBe(original);
        expect(({} as Record<string, unknown>).polluted).toBeUndefined();
    });

    test("repairs a non-object value inside Eidnara's own subtree instead of dropping the write", async () => {
        await writeFile(file, `{ "anthropic-auth": { "order": 160 }, "eidnara": 5 }\n`, "utf8");
        await queueTuiPreferenceUpdate(PLUGIN_KEY, ["collapsed"], true);
        let root = await readTuiPreferencesFile();
        expect(resolveEidnaraPrefs(root).collapsed).toBe(true);
        expect((root["anthropic-auth"] as Record<string, unknown>).order).toBe(160);

        await writeFile(file, `{ "eidnara": { "order": 7, "sections": "bad" } }\n`, "utf8");
        await queueTuiPreferenceUpdate(PLUGIN_KEY, ["sections", "memory"], false);
        root = await readTuiPreferencesFile();
        expect(resolveEidnaraPrefs(root).sections.memory).toBe(false);
        expect(resolveEidnaraPrefs(root).order).toBe(7);
    });

    test.skipIf(process.platform === "win32" || process.getuid?.() === 0)(
        "an unreadable existing file is never replaced by the template",
        async () => {
            const original = `{ "anthropic-auth": { "order": 160 } }\n`;
            await writeFile(file, original, "utf8");
            await chmod(file, 0o000);
            try {
                await queueTuiPreferenceUpdate(PLUGIN_KEY, ["collapsed"], true);
            } finally {
                await chmod(file, 0o600);
            }
            expect(await readFile(file, "utf8")).toBe(original);
        },
    );

    test.skipIf(process.platform === "win32")(
        "keeps the existing file mode across the atomic replacement",
        async () => {
            await writeFile(file, `{ "eidnara": { "order": 1 } }\n`, "utf8");
            await chmod(file, 0o600);
            await queueTuiPreferenceUpdate(PLUGIN_KEY, ["collapsed"], true);
            expect((await stat(file)).mode & 0o777).toBe(0o600);
            expect(resolveEidnaraPrefs(await readTuiPreferencesFile()).collapsed).toBe(true);
        },
    );
});
