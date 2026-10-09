import { describe, expect, it } from "bun:test";
import { capabilityDrift, PINNED_EIDNARA_TOOLS, surfaceOfRequest } from "./capabilities";

function body(names: readonly string[], sources: unknown = ["memory"]) {
    return {
        tools: names.map((name) =>
            name === "eidnara_search"
                ? { name, input_schema: { properties: { sources: { items: { enum: sources } } } } }
                : { name, input_schema: {} },
        ),
    };
}

describe("compression fidelity capability pins", () => {
    it("match the pinned surface", () => {
        const surface = surfaceOfRequest(body(["read", "bash", ...PINNED_EIDNARA_TOOLS]), true);
        expect(capabilityDrift(surface)).toEqual([]);
    });

    it("report an added expansion tool, a widened search, a missing tool, and Pi without transform", () => {
        expect(
            capabilityDrift(
                surfaceOfRequest(body([...PINNED_EIDNARA_TOOLS, "eidnara_expand"]), true),
            ),
        ).toEqual([
            "eidnara tools eidnara_expand,eidnara_memory,eidnara_note,eidnara_reduce,eidnara_search",
            "exact expansion eidnara_expand",
        ]);
        expect(
            capabilityDrift(
                surfaceOfRequest(body(["read", ...PINNED_EIDNARA_TOOLS, "source_read"]), true),
            ),
        ).toEqual(["exact expansion source_read"]);
        expect(
            capabilityDrift(
                surfaceOfRequest(body(PINNED_EIDNARA_TOOLS, ["memory", "transcript"]), true),
            ),
        ).toEqual(["eidnara_search sources memory,transcript"]);
        expect(
            capabilityDrift(surfaceOfRequest(body(PINNED_EIDNARA_TOOLS.slice(1)), true)),
        ).toEqual(["eidnara tools eidnara_note,eidnara_reduce,eidnara_search"]);
        expect(capabilityDrift(surfaceOfRequest(body(PINNED_EIDNARA_TOOLS), false))).toEqual([
            "pi transform unavailable",
        ]);
    });
});
