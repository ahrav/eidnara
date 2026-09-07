import { mkdtempSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";

const isolatedDataHome = mkdtempSync(join(tmpdir(), "eidnara-plugin-test-xdg-"));

// EIDNARA_TEST_DATA_DIR remains set when tests delete XDG_DATA_HOME.
process.env.EIDNARA_TEST_DATA_DIR = isolatedDataHome;
process.env.XDG_DATA_HOME = isolatedDataHome;

process.on("exit", () => {
    rmSync(isolatedDataHome, { recursive: true, force: true });
});
