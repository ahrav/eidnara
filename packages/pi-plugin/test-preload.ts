import { mkdtempSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";

const isolatedDataHome = mkdtempSync(join(tmpdir(), "eidnara-pi-test-xdg-"));

// `getEidnaraStorageDir` falls back to EIDNARA_TEST_DATA_DIR once a test unsets XDG_DATA_HOME.
process.env.EIDNARA_TEST_DATA_DIR = isolatedDataHome;
process.env.XDG_DATA_HOME = isolatedDataHome;

process.on("exit", () => {
    rmSync(isolatedDataHome, { recursive: true, force: true });
});
