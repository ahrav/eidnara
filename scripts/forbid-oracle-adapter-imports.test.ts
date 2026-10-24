import { describe, expect, test } from "bun:test";

import {
    forbiddenReaderSources,
    forbiddenRustReaderLines,
    forbiddenTypeScriptReaderLines,
    RUST_READER,
} from "./forbid-oracle-adapter-imports";

const { readFileSync } = process.getBuiltinModule("node:fs");

describe("forbiddenRustReaderLines", () => {
    test("rejects a seeded import of a harness source adapter", () => {
        expect(
            forbiddenRustReaderLines(
                "r.rs",
                "use serde::Deserialize;\nuse crate::harness_sources::opencode_units;\n",
            ),
        ).toEqual(["r.rs:2: use crate::harness_sources::opencode_units;"]);
    });

    test("rejects the outcome module, shared types, sibling paths, and smuggled source", () => {
        const seeded = [
            "use super::tool_outcome;",
            "fn f(s: &SessionIdentity) {}",
            "let b: ToolBlock;",
            "daemon::pi_units_within(x);",
            '#[path = "../harness_sources.rs"]',
            'include!("x.rs");',
        ];
        expect(forbiddenRustReaderLines("r.rs", seeded.join("\n"))).toHaveLength(seeded.length);
    });

    test("refuses an adapter name in a comment or behind a URL literal", () => {
        const text =
            'use sha2::Sha256;\n// see harness_sources\nlet u = "http://x"; use crate::a;\n';
        expect(forbiddenRustReaderLines("r.rs", text)).toEqual([
            "r.rs:2: // see harness_sources",
            'r.rs:3: let u = "http://x"; use crate::a;',
        ]);
    });

    test("rejects a rebound crate root before any aliased path can reach the daemon", () => {
        const seeded = [
            "use crate as product;",
            "use crate::{self as product};",
            "extern crate self as d;",
            "extern crate daemon;",
        ];
        expect(forbiddenRustReaderLines("r.rs", seeded.join("\n"))).toHaveLength(seeded.length);
    });

    test("rejects an external module declaration, which would move source outside the scan", () => {
        const seeded = ["mod helper;", "pub mod helper;", "pub(crate) mod helper ;"];
        expect(forbiddenRustReaderLines("r.rs", seeded.join("\n"))).toHaveLength(seeded.length);
        expect(forbiddenRustReaderLines("r.rs", "mod inline {\n    fn f() {}\n}\n")).toEqual([]);
    });
});

describe("forbiddenTypeScriptReaderLines", () => {
    test("rejects an import outside the runtime builtins and sibling files", () => {
        const text = [
            'import { Database } from "bun:sqlite";',
            'import { join } from "node:path";',
            'import { x } from "./corpus.ts";',
            'import { adapt } from "../../../opencode-plugin/src/transform.ts";',
            'import type { T } from "@eidnara/opencode/shared/rpc-types";',
            'const m = await import("../rust-harness.ts");',
        ].join("\n");
        expect(forbiddenTypeScriptReaderLines("r.ts", text)).toEqual([
            "r.ts:4: imports ../../../opencode-plugin/src/transform.ts",
            "r.ts:5: imports @eidnara/opencode/shared/rpc-types",
            "r.ts:6: imports ../rust-harness.ts",
        ]);
    });
});

describe("forbiddenTypeScriptReaderLines across lines and strings", () => {
    test("rejects an import split across lines or behind a URL literal", () => {
        const text = [
            'const u = "http://x"; await import("../adapter.ts");',
            "const m = await import(",
            '    "../rust-harness.ts");',
        ].join("\n");
        expect(forbiddenTypeScriptReaderLines("r.ts", text)).toEqual([
            "r.ts:1: imports ../adapter.ts",
            "r.ts:2: imports ../rust-harness.ts",
        ]);
    });
});

describe("forbiddenTypeScriptReaderLines fails closed", () => {
    test("reports a commented-out import and unparsable source", () => {
        expect(
            forbiddenTypeScriptReaderLines("c.ts", '// import x from "../adapter.ts"\n'),
        ).toEqual(["c.ts:1: imports ../adapter.ts"]);
        expect(forbiddenTypeScriptReaderLines("c.ts", "import {")[0]).toContain("does not parse");
    });
});

describe("forbiddenReaderSources", () => {
    test("reports an empty scan as a finding", () => {
        expect(forbiddenReaderSources({}, {})).toHaveLength(2);
    });

    test("passes on the landed Rust reader", () => {
        const text = readFileSync(`${import.meta.dir}/../${RUST_READER}`, "utf8");
        expect(forbiddenRustReaderLines(RUST_READER, text)).toEqual([]);
    });
});
