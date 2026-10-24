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
		expect(forbiddenRustReaderLines("r.rs", seeded.join("\n"))).toHaveLength(
			seeded.length,
		);
	});

	test("accepts external crates and adapter names that appear only in comments", () => {
		const text =
			"use sha2::Sha256;\n// The harness_sources adapters stay unnamed.\nlet x = 1; // tool_outcome\n";
		expect(forbiddenRustReaderLines("r.rs", text)).toEqual([]);
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

describe("forbiddenReaderSources", () => {
	test("reports an empty scan as a finding", () => {
		expect(forbiddenReaderSources({}, {})).toHaveLength(2);
	});

	test("passes on the landed Rust reader", () => {
		const text = readFileSync(`${import.meta.dir}/../${RUST_READER}`, "utf8");
		expect(forbiddenRustReaderLines(RUST_READER, text)).toEqual([]);
	});
});
