import { describe, expect, test } from "bun:test";
import { type ModuleGraph, reachableModules } from "./module-graph";

const graph: ModuleGraph = {
    inputs: ["src/a/forbidden.ts", "src/b/forbidden.ts", "src/c/allowed.ts"],
    externals: ["@scope/forbidden-external", "@scope/allowed-external"],
    text: "",
};

describe("reachableModules", () => {
    test("returns every input and external matching the pattern", () => {
        expect(reachableModules(graph, /forbidden/)).toEqual([
            "src/a/forbidden.ts",
            "src/b/forbidden.ts",
            "@scope/forbidden-external",
        ]);
    });

    test("returns an empty list when nothing matches", () => {
        expect(reachableModules(graph, /never-present/)).toEqual([]);
    });

    // `RegExp.prototype.test` advances `lastIndex` on `g` and `y` patterns, so a
    // silent under-count would make a reachable module look unreachable.
    test("rejects a global pattern instead of under-counting", () => {
        expect(() => reachableModules(graph, /forbidden/g)).toThrow(TypeError);
    });

    test("rejects a sticky pattern instead of under-counting", () => {
        expect(() => reachableModules(graph, /forbidden/y)).toThrow(TypeError);
    });
});
