import { afterEach, beforeEach, describe, expect, test } from "bun:test";
import { mkdirSync, mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import {
    eidnaraProjectConfigBasePath,
    eidnaraUserConfigBasePath,
    resolveEidnaraProjectConfigPath,
    resolveEidnaraUserConfigPath,
} from "./config-paths";

const saved = { XDG_CONFIG_HOME: process.env.XDG_CONFIG_HOME, HOME: process.env.HOME };

beforeEach(() => {
    delete process.env.XDG_CONFIG_HOME;
    process.env.HOME = "/home/user";
});

afterEach(() => {
    for (const [key, value] of Object.entries(saved)) {
        if (value === undefined) delete process.env[key];
        else process.env[key] = value;
    }
});

describe("config paths", () => {
    test("user config lives under $XDG_CONFIG_HOME/eidnara when the value is absolute", () => {
        process.env.XDG_CONFIG_HOME = "/xdg";
        expect(eidnaraUserConfigBasePath()).toBe(join("/xdg", "eidnara", "eidnara"));
        expect(resolveEidnaraUserConfigPath()).toBe(join("/xdg", "eidnara", "eidnara.jsonc"));
    });

    test("a relative or empty XDG_CONFIG_HOME falls back to ~/.config", () => {
        for (const value of ["relative/config", ""]) {
            process.env.XDG_CONFIG_HOME = value;
            expect(eidnaraUserConfigBasePath()).toBe(
                join("/home/user", ".config", "eidnara", "eidnara"),
            );
        }
    });

    test("project config lives under <root>/.eidnara", () => {
        expect(eidnaraProjectConfigBasePath("/work/proj")).toBe(
            join("/work/proj", ".eidnara", "eidnara"),
        );
        expect(resolveEidnaraProjectConfigPath("/work/proj")).toBe(
            join("/work/proj", ".eidnara", "eidnara.jsonc"),
        );
    });

    test("resolves to an existing eidnara.json when no eidnara.jsonc exists", () => {
        const xdg = mkdtempSync(join(tmpdir(), "eidnara-config-paths-"));
        try {
            process.env.XDG_CONFIG_HOME = xdg;
            mkdirSync(join(xdg, "eidnara"), { recursive: true });
            writeFileSync(join(xdg, "eidnara", "eidnara.json"), "{}");
            expect(resolveEidnaraUserConfigPath()).toBe(join(xdg, "eidnara", "eidnara.json"));

            writeFileSync(join(xdg, "eidnara", "eidnara.jsonc"), "{}");
            expect(resolveEidnaraUserConfigPath()).toBe(join(xdg, "eidnara", "eidnara.jsonc"));
        } finally {
            rmSync(xdg, { recursive: true, force: true });
        }
    });
});
