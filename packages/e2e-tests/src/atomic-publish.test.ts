import { afterEach, describe, expect, test } from "bun:test";
import {
    chmodSync,
    mkdirSync,
    mkdtempSync,
    readdirSync,
    readFileSync,
    rmSync,
    statSync,
} from "node:fs";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";
import { publishJsonAtomically, publishPrivateJson } from "./atomic-publish";

const dirs: string[] = [];
function scratch(): string {
    const dir = mkdtempSync(join(tmpdir(), "atomic-publish-"));
    dirs.push(dir);
    return dir;
}
afterEach(() => {
    for (const dir of dirs.splice(0)) rmSync(dir, { recursive: true, force: true });
});

describe("atomic JSON publication", () => {
    test("writes indented JSON with the requested mode and leaves no temporary file", () => {
        const dir = scratch();
        const path = join(dir, "nested", "value.json");
        publishJsonAtomically({ b: [1, "é"], a: null }, path, { mode: 0o640 });
        expect(readFileSync(path, "utf8")).toBe(
            `${JSON.stringify({ b: [1, "é"], a: null }, null, 4)}\n`,
        );
        expect(statSync(path).mode & 0o777).toBe(0o640);
        expect(readdirSync(join(dir, "nested"))).toEqual(["value.json"]);
    });

    test("a private publication is owner-only and outside the repository", () => {
        const out = join(scratch(), "private");
        const path = publishPrivateJson({ ok: true }, out, "report.json");
        expect(path).toBe(join(out, "report.json"));
        expect(statSync(out).mode & 0o777).toBe(0o700);
        expect(statSync(path).mode & 0o777).toBe(0o600);
        expect(readdirSync(out)).toEqual(["report.json"]);
        publishPrivateJson({ ok: false }, out, "report.json");
        expect(JSON.parse(readFileSync(path, "utf8"))).toEqual({ ok: false });
        expect(readdirSync(out)).toEqual(["report.json"]);
    });

    test("a private publication refuses unsafe names, parents, shared directories, and the repository", () => {
        const root = scratch();
        expect(() => publishPrivateJson({}, root, ".hidden.json")).toThrow("plain file label");
        expect(() => publishPrivateJson({}, root, "a/b.json")).toThrow("plain file label");
        expect(() => publishPrivateJson({}, `${root}/x/../y`, "r.json")).toThrow(
            "parent directory",
        );
        const shared = join(root, "shared");
        mkdirSync(shared);
        chmodSync(shared, 0o755);
        expect(() => publishPrivateJson({}, shared, "r.json")).toThrow("owner-only directory");
        expect(() =>
            publishPrivateJson({}, resolve(import.meta.dir, "publish-out"), "r.json"),
        ).toThrow("inside the repository");
        expect(readdirSync(shared)).toEqual([]);
    });
});
