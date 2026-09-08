import { afterAll } from "bun:test";
import { mkdirSync, mkdtempSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";

const isolatedRoot = mkdtempSync(join(tmpdir(), "eidnara-plugin-test-xdg-"));
const isolatedDataHome = join(isolatedRoot, "data");
const isolatedConfigHome = join(isolatedRoot, "config");
mkdirSync(isolatedDataHome);
mkdirSync(isolatedConfigHome);

// EIDNARA_TEST_DATA_DIR remains set when tests delete XDG_DATA_HOME.
process.env.EIDNARA_TEST_DATA_DIR = isolatedDataHome;
process.env.XDG_DATA_HOME = isolatedDataHome;

// getOpenCodeConfigDir prefers OPENCODE_CONFIG_DIR over XDG_CONFIG_HOME. Setting both keeps config
// writes out of the developer's real ~/.config/opencode, including from a test that clears
// OPENCODE_CONFIG_DIR to exercise the XDG fallback.
process.env.XDG_CONFIG_HOME = isolatedConfigHome;
process.env.OPENCODE_CONFIG_DIR = join(isolatedConfigHome, "opencode");

// Bun does not run process `exit` listeners when it tears down test workers, so cleanup uses a
// run-wide `afterAll` hook; hooks registered in a preload file run once for the whole invocation.
afterAll(() => {
    rmSync(isolatedRoot, { recursive: true, force: true });
});
