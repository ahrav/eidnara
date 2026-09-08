import { expect, test } from "bun:test";
import { homedir } from "node:os";
import { join } from "node:path";

test("the preload points every test at an isolated data root owned by this package", () => {
    const isolated = process.env.EIDNARA_TEST_DATA_DIR;
    expect(isolated).toMatch(/eidnara-pi-test-xdg-/);
    expect(process.env.XDG_DATA_HOME).toBe(isolated);
    expect(isolated).not.toBe(join(homedir(), ".local", "share"));
});
