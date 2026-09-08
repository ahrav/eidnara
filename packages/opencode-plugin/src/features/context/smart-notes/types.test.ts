import { describe, expect, test } from "bun:test";
import { parseSmartNoteManifest } from "./types";

describe("parseSmartNoteManifest", () => {
    test("keeps only allow-listed string capabilities", () => {
        const manifest = parseSmartNoteManifest(
            JSON.stringify({ capabilities: ["readFile", "gitTag", "shell", "httpGet"] }),
        );
        expect(manifest.capabilities).toEqual(["readFile", "gitTag", "httpGet"]);
    });

    // `String(["readFile"])` is `"readFile"`, so a coercing check would admit the array itself.
    test("rejects non-string entries whose string form is an allow-listed name", () => {
        const manifest = parseSmartNoteManifest(
            JSON.stringify({
                capabilities: [["readFile"], { toString: "gitTag" }, 5, null, "gitLog"],
            }),
        );
        expect(manifest.capabilities).toEqual(["gitLog"]);
        for (const capability of manifest.capabilities) {
            expect(typeof capability).toBe("string");
        }
    });

    test("returns an empty manifest for null, invalid JSON, or a non-array capabilities field", () => {
        expect(parseSmartNoteManifest(null)).toEqual({ capabilities: [] });
        expect(parseSmartNoteManifest("{not json")).toEqual({ capabilities: [] });
        expect(
            parseSmartNoteManifest(JSON.stringify({ capabilities: "readFile" })).capabilities,
        ).toEqual([]);
    });
});
