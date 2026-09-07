import { afterEach, describe, expect, it } from "bun:test";
import {
    existsSync,
    lstatSync,
    mkdirSync,
    mkdtempSync,
    readdirSync,
    readFileSync,
    rmSync,
    symlinkSync,
    writeFileSync,
} from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { parse } from "comment-json";

const roots: string[] = [];
afterEach(() => {
    for (const root of roots.splice(0)) {
        rmSync(root, { recursive: true, force: true });
    }
});

describe("ensureTuiPluginEntry", () => {
    it("preserves tuple dev-path plugin entry and does not add @latest", async () => {
        const root = mkdtempSync(join(tmpdir(), "eidnara-tui-"));
        roots.push(root);
        const devPath = "/Work/eidnara/packages/opencode-plugin";
        const tuiPath = join(root, "tui.json");
        writeFileSync(
            tuiPath,
            `${JSON.stringify({ plugin: [[devPath, { sidebar: true }], "other-plugin"] }, null, 2)}\n`,
        );

        const { ensureTuiPluginEntry } = await import("./tui-config");
        const changed = ensureTuiPluginEntry({ configDir: root });
        expect(changed).toBe(false);
        const parsed = JSON.parse(readFileSync(tuiPath, "utf-8")) as { plugin: unknown[] };
        expect(parsed.plugin).toHaveLength(2);
        expect(Array.isArray(parsed.plugin[0])).toBe(true);
        expect((parsed.plugin[0] as unknown[])[0]).toBe(devPath);
        expect(parsed.plugin[1]).toBe("other-plugin");
        expect(readdirSync(root).filter((name) => name.endsWith(".tmp"))).toEqual([]);
    });

    it("registers the plugin when tui.jsonc exists but is empty", async () => {
        // An existing empty file must not leave the plugin permanently unregistered:
        // comment-json rejects empty input, and the file stays empty on every later start.
        const root = mkdtempSync(join(tmpdir(), "eidnara-tui-empty-"));
        roots.push(root);
        const tuiPath = join(root, "tui.jsonc");
        writeFileSync(tuiPath, "");

        const { ensureTuiPluginEntry } = await import("./tui-config");
        expect(ensureTuiPluginEntry({ configDir: root })).toBe(true);
        const parsed = JSON.parse(readFileSync(tuiPath, "utf-8")) as { plugin: unknown[] };
        expect(parsed.plugin).toContain("@eidnara/opencode@latest");
    });

    it("registers the plugin when tui.jsonc holds only a comment and keeps that comment", async () => {
        const root = mkdtempSync(join(tmpdir(), "eidnara-tui-comment-only-"));
        roots.push(root);
        const tuiPath = join(root, "tui.jsonc");
        writeFileSync(tuiPath, "// configure the tui here\n/* keybinds live below */\n");

        const { ensureTuiPluginEntry } = await import("./tui-config");
        expect(ensureTuiPluginEntry({ configDir: root })).toBe(true);
        const text = readFileSync(tuiPath, "utf-8");
        expect(text).toContain("// configure the tui here");
        expect(text).toContain("/* keybinds live below */");
        const parsed = parse(text) as { plugin: unknown[] };
        expect(parsed.plugin).toContain("@eidnara/opencode@latest");
    });

    it("keeps user comments inside the plugin array when appending", async () => {
        const root = mkdtempSync(join(tmpdir(), "eidnara-tui-array-comment-"));
        roots.push(root);
        const tuiPath = join(root, "tui.jsonc");
        writeFileSync(
            tuiPath,
            `{\n  "plugin": [\n    // keep notify first\n    "opencode-notify"\n  ]\n}\n`,
        );

        const { ensureTuiPluginEntry } = await import("./tui-config");
        expect(ensureTuiPluginEntry({ configDir: root })).toBe(true);
        const text = readFileSync(tuiPath, "utf-8");
        expect(text).toContain("// keep notify first");
        const parsed = parse(text) as { plugin: unknown[] };
        expect(parsed.plugin).toEqual(["opencode-notify", "@eidnara/opencode@latest"]);
    });

    it("keeps user comments inside a tuple entry when upgrading it to @latest", async () => {
        const root = mkdtempSync(join(tmpdir(), "eidnara-tui-tuple-comment-"));
        roots.push(root);
        const tuiPath = join(root, "tui.jsonc");
        writeFileSync(
            tuiPath,
            `{\n  "plugin": [\n    [\n      "@eidnara/opencode",\n      // sidebar on by default\n      { "sidebar": true }\n    ]\n  ]\n}\n`,
        );

        const { ensureTuiPluginEntry } = await import("./tui-config");
        expect(ensureTuiPluginEntry({ configDir: root })).toBe(true);
        const text = readFileSync(tuiPath, "utf-8");
        expect(text).toContain("// sidebar on by default");
        const parsed = parse(text) as { plugin: unknown[][] };
        expect(parsed.plugin[0]?.[0]).toBe("@eidnara/opencode@latest");
        expect(parsed.plugin[0]?.[1]).toEqual({ sidebar: true });
    });

    it("upgrades bare npm name to @latest while preserving tuple options", async () => {
        const root = mkdtempSync(join(tmpdir(), "eidnara-tui-npm-"));
        roots.push(root);
        const tuiPath = join(root, "tui.json");
        writeFileSync(
            tuiPath,
            `${JSON.stringify(
                {
                    plugin: [["@eidnara/opencode", { enabled: true }]],
                },
                null,
                2,
            )}\n`,
        );

        const { ensureTuiPluginEntry } = await import("./tui-config");
        expect(ensureTuiPluginEntry({ configDir: root })).toBe(true);
        const parsed = JSON.parse(readFileSync(tuiPath, "utf-8")) as { plugin: unknown[] };
        const entry = parsed.plugin[0] as unknown[];
        expect(entry[0]).toBe("@eidnara/opencode@latest");
        expect(entry[1]).toEqual({ enabled: true });
    });

    it("creates tui.jsonc (not tui.json) on a fresh install", async () => {
        const root = mkdtempSync(join(tmpdir(), "eidnara-tui-fresh-"));
        roots.push(root);

        const { ensureTuiPluginEntry } = await import("./tui-config");
        expect(ensureTuiPluginEntry({ configDir: root })).toBe(true);

        // Creating tui.jsonc avoids leaving a tui.json stub beside a later tui.jsonc.
        expect(existsSync(join(root, "tui.jsonc"))).toBe(true);
        expect(existsSync(join(root, "tui.json"))).toBe(false);
        const parsed = JSON.parse(readFileSync(join(root, "tui.jsonc"), "utf-8")) as {
            plugin: unknown[];
        };
        expect(parsed.plugin).toContain("@eidnara/opencode@latest");
    });

    it("writes into the existing tui.jsonc when both files exist", async () => {
        const root = mkdtempSync(join(tmpdir(), "eidnara-tui-both-"));
        roots.push(root);
        writeFileSync(
            join(root, "tui.jsonc"),
            `${JSON.stringify({ keybinds: { x: "y" } }, null, 2)}\n`,
        );
        writeFileSync(join(root, "tui.json"), "{}\n");

        const { ensureTuiPluginEntry } = await import("./tui-config");
        expect(ensureTuiPluginEntry({ configDir: root })).toBe(true);

        // tui.jsonc takes precedence over tui.json.
        const jsonc = JSON.parse(readFileSync(join(root, "tui.jsonc"), "utf-8")) as {
            plugin: unknown[];
            keybinds: Record<string, string>;
        };
        expect(jsonc.plugin).toContain("@eidnara/opencode@latest");
        expect(jsonc.keybinds).toEqual({ x: "y" });
        expect(readFileSync(join(root, "tui.json"), "utf-8")).toBe("{}\n");
    });

    it("recognizes parent-relative and home-relative dev paths as the local plugin", async () => {
        const { ensureTuiPluginEntry } = await import("./tui-config");
        for (const devPath of ["../opencode-plugin", "../../packages/eidnara", "~/src/eidnara"]) {
            const root = mkdtempSync(join(tmpdir(), "eidnara-tui-relative-dev-"));
            roots.push(root);
            const tuiPath = join(root, "tui.jsonc");
            writeFileSync(tuiPath, `${JSON.stringify({ plugin: [devPath] }, null, 2)}\n`);

            expect(ensureTuiPluginEntry({ configDir: root })).toBe(false);
            const parsed = JSON.parse(readFileSync(tuiPath, "utf-8")) as { plugin: unknown[] };
            expect(parsed.plugin).toEqual([devPath]);
        }
    });

    it("does not mistake an unrelated plugin under a user named eidnara for the local plugin", async () => {
        const { ensureTuiPluginEntry } = await import("./tui-config");
        for (const otherPath of [
            "/home/eidnara/plugins/notify",
            "file:///home/eidnara/other-plugin/index.ts",
            "C:\\Users\\eidnara\\plugins\\notify",
        ]) {
            const root = mkdtempSync(join(tmpdir(), "eidnara-tui-unrelated-"));
            roots.push(root);
            const tuiPath = join(root, "tui.jsonc");
            writeFileSync(tuiPath, `${JSON.stringify({ plugin: [otherPath] }, null, 2)}\n`);

            expect(ensureTuiPluginEntry({ configDir: root })).toBe(true);
            const parsed = JSON.parse(readFileSync(tuiPath, "utf-8")) as { plugin: unknown[] };
            expect(parsed.plugin).toEqual([otherPath, "@eidnara/opencode@latest"]);
        }
    });

    it("leaves a file with a non-object root untouched instead of replacing it", async () => {
        const { ensureTuiPluginEntry } = await import("./tui-config");
        for (const original of [
            "[]\n",
            "null\n",
            '"just a string"\n',
            "[\n  // keep me\n  1\n]\n",
        ]) {
            const root = mkdtempSync(join(tmpdir(), "eidnara-tui-nonobject-"));
            roots.push(root);
            const tuiPath = join(root, "tui.jsonc");
            writeFileSync(tuiPath, original);

            expect(ensureTuiPluginEntry({ configDir: root })).toBe(false);
            expect(readFileSync(tuiPath, "utf-8")).toBe(original);
        }
    });

    it.skipIf(process.platform === "win32")(
        "writes through a symlinked tui.jsonc and keeps the link in place",
        async () => {
            const root = mkdtempSync(join(tmpdir(), "eidnara-tui-symlink-"));
            roots.push(root);
            const dotfiles = join(root, "dotfiles");
            mkdirSync(dotfiles);
            const target = join(dotfiles, "tui.jsonc");
            writeFileSync(target, `${JSON.stringify({ keybinds: { x: "y" } }, null, 2)}\n`);
            const configDir = join(root, "config");
            mkdirSync(configDir);
            const link = join(configDir, "tui.jsonc");
            symlinkSync(target, link);

            const { ensureTuiPluginEntry } = await import("./tui-config");
            expect(ensureTuiPluginEntry({ configDir })).toBe(true);

            expect(lstatSync(link).isSymbolicLink()).toBe(true);
            const parsed = JSON.parse(readFileSync(target, "utf-8")) as {
                plugin: unknown[];
                keybinds: Record<string, string>;
            };
            expect(parsed.plugin).toContain("@eidnara/opencode@latest");
            expect(parsed.keybinds).toEqual({ x: "y" });
            expect(readdirSync(configDir)).toEqual(["tui.jsonc"]);
            expect(readdirSync(dotfiles)).toEqual(["tui.jsonc"]);
        },
    );

    it.skipIf(process.platform === "win32")(
        "creates the target of a dangling tui.jsonc symlink instead of replacing the link",
        async () => {
            const root = mkdtempSync(join(tmpdir(), "eidnara-tui-dangling-"));
            roots.push(root);
            const dotfiles = join(root, "dotfiles");
            mkdirSync(dotfiles);
            const target = join(dotfiles, "tui.jsonc");
            const configDir = join(root, "config");
            mkdirSync(configDir);
            const link = join(configDir, "tui.jsonc");
            symlinkSync(target, link);

            const { ensureTuiPluginEntry } = await import("./tui-config");
            expect(ensureTuiPluginEntry({ configDir })).toBe(true);

            expect(lstatSync(link).isSymbolicLink()).toBe(true);
            const parsed = JSON.parse(readFileSync(target, "utf-8")) as { plugin: unknown[] };
            expect(parsed.plugin).toContain("@eidnara/opencode@latest");
            expect(readdirSync(dotfiles)).toEqual(["tui.jsonc"]);
        },
    );

    it.skipIf(process.platform === "win32")(
        "prefers a dangling tui.jsonc symlink over a sibling tui.json",
        async () => {
            const root = mkdtempSync(join(tmpdir(), "eidnara-tui-dangling-precedence-"));
            roots.push(root);
            const dotfiles = join(root, "dotfiles");
            mkdirSync(dotfiles);
            const target = join(dotfiles, "tui.jsonc");
            const configDir = join(root, "config");
            mkdirSync(configDir);
            symlinkSync(target, join(configDir, "tui.jsonc"));
            writeFileSync(join(configDir, "tui.json"), "{}\n");

            const { ensureTuiPluginEntry } = await import("./tui-config");
            expect(ensureTuiPluginEntry({ configDir })).toBe(true);

            // tui.jsonc keeps precedence even while its link is dangling; tui.json is untouched.
            expect(readFileSync(join(configDir, "tui.json"), "utf-8")).toBe("{}\n");
            const parsed = JSON.parse(readFileSync(target, "utf-8")) as { plugin: unknown[] };
            expect(parsed.plugin).toContain("@eidnara/opencode@latest");
        },
    );
});
