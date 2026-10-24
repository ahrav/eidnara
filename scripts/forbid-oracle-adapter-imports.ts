// Usage: `bun scripts/forbid-oracle-adapter-imports.ts` (exit 1 on a hit, 2 on a tool failure).
//
// The source-corpus readers produce the expected side of every capture reconciliation, so a
// reader that reaches the daemon's harness source adapters, the tool outcome module, or the
// shared identity and block types would let an adapter defect move the expected side too.

/** The Rust reader module, relative to the workspace root. */
export const RUST_READER = "crates/daemon/src/source_corpus.rs";

/** The TypeScript reader modules, relative to the workspace root. */
export const TYPESCRIPT_READERS =
	"packages/e2e-tests/src/source-corpus/**/*.ts";

/**
 * Rust paths and names the reader must not spell: any path into the daemon crate or a sibling
 * module, the adapter, outcome, and capture modules and their entry points, the shared identity
 * and block types, and the `#[path]` and `include!` forms that pull source from elsewhere.
 */
const RUST_FORBIDDEN = new RegExp(
	[
		String.raw`\b(?:crate|super|daemon)::`,
		String.raw`\b(?:harness_sources|tool_outcome|source_capture|SessionIdentity|ToolBlock|SourceUnit)\b`,
		String.raw`\b(?:opencode|pi)_units\w*`,
		String.raw`#\[path\b`,
		String.raw`\binclude(?:_str|_bytes)?!`,
	].join("|"),
);

/** A TypeScript module specifier the reader may import: a runtime builtin or a sibling file. */
function allowedSpecifier(specifier: string): boolean {
	return /^(?:bun|node):/.test(specifier) || /^\.\/[^/]+$/.test(specifier);
}

const TYPESCRIPT_SPECIFIER =
	/\b(?:from|import|require|getBuiltinModule)\s*\(?\s*["'`]([^"'`]+)["'`]/g;

function withoutLineComment(line: string): string {
	const at = line.indexOf("//");
	return at < 0 ? line : line.slice(0, at);
}

export function forbiddenRustReaderLines(path: string, text: string): string[] {
	const findings: string[] = [];
	text.split("\n").forEach((line, index) => {
		if (RUST_FORBIDDEN.test(withoutLineComment(line))) {
			findings.push(`${path}:${index + 1}: ${line.trim()}`);
		}
	});
	return findings;
}

export function forbiddenTypeScriptReaderLines(
	path: string,
	text: string,
): string[] {
	const findings: string[] = [];
	text.split("\n").forEach((line, index) => {
		for (const match of withoutLineComment(line).matchAll(
			TYPESCRIPT_SPECIFIER,
		)) {
			if (!allowedSpecifier(match[1]!)) {
				findings.push(`${path}:${index + 1}: imports ${match[1]}`);
			}
		}
	});
	return findings;
}

/** An empty TypeScript reader set is a finding: a scan that matched no file proves nothing. */
export function forbiddenReaderSources(
	rust: Record<string, string>,
	typescript: Record<string, string>,
): string[] {
	const findings: string[] = [];
	if (Object.keys(rust).length === 0)
		findings.push(`${RUST_READER}: no reader scanned`);
	if (Object.keys(typescript).length === 0) {
		findings.push(`${TYPESCRIPT_READERS}: no reader scanned`);
	}
	for (const [path, text] of Object.entries(rust)) {
		findings.push(...forbiddenRustReaderLines(path, text));
	}
	for (const [path, text] of Object.entries(typescript)) {
		findings.push(...forbiddenTypeScriptReaderLines(path, text));
	}
	return findings.sort();
}

if (import.meta.main) {
	const root = Bun.spawnSync(["git", "rev-parse", "--show-toplevel"], {
		stdout: "pipe",
	});
	if (root.exitCode !== 0) {
		console.error(`git rev-parse exited ${root.exitCode}`);
		process.exit(2);
	}
	const workspace = root.stdout.toString().trim();
	const rust: Record<string, string> = {};
	const file = Bun.file(`${workspace}/${RUST_READER}`);
	if (await file.exists()) rust[RUST_READER] = await file.text();
	const typescript: Record<string, string> = {};
	for (const path of new Bun.Glob(TYPESCRIPT_READERS).scanSync(workspace)) {
		typescript[path] = await Bun.file(`${workspace}/${path}`).text();
	}
	const findings = forbiddenReaderSources(rust, typescript);
	if (findings.length > 0) {
		console.error("a source-corpus reader names adapter code:");
		for (const finding of findings) console.error(`  ${finding}`);
		process.exit(1);
	}
	console.log(
		`source-corpus readers: ${Object.keys(rust).length + Object.keys(typescript).length} files name no adapter code`,
	);
}
