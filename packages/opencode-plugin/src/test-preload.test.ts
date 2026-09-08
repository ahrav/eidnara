import { expect, test } from "bun:test";
import { existsSync } from "node:fs";
import { homedir } from "node:os";
import { dirname, join } from "node:path";

import { getOpenCodeConfigPaths } from "./shared/opencode-config-dir";

test("the preload points every test at an isolated data root", () => {
    const isolated = process.env.EIDNARA_TEST_DATA_DIR;
    expect(isolated).toMatch(/eidnara-plugin-test-xdg-/);
    expect(process.env.XDG_DATA_HOME).toBe(isolated);
    expect(isolated).not.toBe(join(homedir(), ".local", "share"));
});

test("the preload points the OpenCode config dir at the same isolated root", () => {
    const dataHome = process.env.EIDNARA_TEST_DATA_DIR ?? "";
    const isolatedRoot = dirname(dataHome);
    const realConfigDir = join(homedir(), ".config", "opencode");

    const { configDir } = getOpenCodeConfigPaths({ binary: "opencode" });
    expect(configDir.startsWith(isolatedRoot)).toBe(true);
    expect(configDir).not.toBe(realConfigDir);

    // The XDG fallback is also isolated, so a test that clears OPENCODE_CONFIG_DIR stays safe.
    const saved = process.env.OPENCODE_CONFIG_DIR;
    delete process.env.OPENCODE_CONFIG_DIR;
    try {
        const fallback = getOpenCodeConfigPaths({ binary: "opencode" }).configDir;
        expect(fallback.startsWith(isolatedRoot)).toBe(true);
        expect(fallback).not.toBe(realConfigDir);
    } finally {
        process.env.OPENCODE_CONFIG_DIR = saved;
    }
});

test("the isolated root exists for the duration of the run", () => {
    const dataHome = process.env.EIDNARA_TEST_DATA_DIR ?? "";
    expect(existsSync(dataHome)).toBe(true);
    expect(existsSync(process.env.XDG_CONFIG_HOME ?? "")).toBe(true);
});
