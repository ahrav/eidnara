// `bun test` imports the TUI from source and does not load `src/tui-compiled`. This
// script detects compiled-only import failures. The `entry.mjs` import takes the
// raw-TSX fallback that bare Bun reaches when the `opentui:runtime-module:*` probe
// fails. The compiled import uses a stand-in for OpenCode's runtime registry,
// resolving each runtime id to its installed package.

import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import { plugin } from "bun";
import { TUI_RUNTIME_SPECIFIERS } from "../src/shared/tui-runtime-specifiers";

const here = dirname(fileURLToPath(import.meta.url));
const pluginRoot = join(here, "..");
const entry = join(pluginRoot, "src/tui/entry.mjs");
const compiledEntry = join(pluginRoot, "src/tui-compiled/index.tsx");

type TuiModule = { default?: { id?: string; tui?: unknown } };

let failures = 0;
function check(name: string, cond: boolean, detail?: string): void {
    if (cond) {
        console.log(`  ok  ${name}`);
    } else {
        failures++;
        console.log(`FAIL  ${name}${detail ? ` — ${detail}` : ""}`);
    }
}

function checkPluginShape(label: string, mod: TuiModule): void {
    check(
        `${label} exports the { id, tui } plugin shape`,
        mod.default?.id === "eidnara-opencode" && typeof mod.default?.tui === "function",
        `got id=${mod.default?.id} tui=${typeof mod.default?.tui}`,
    );
}

try {
    const mod = (await import(entry)) as TuiModule;
    check("entry.mjs imports through the raw-TSX fallback", true);
    checkPluginShape("raw-TSX fallback", mod);
} catch (error) {
    check(
        "entry.mjs imports through the raw-TSX fallback",
        false,
        error instanceof Error ? error.message : String(error),
    );
}

// Bun parses `opentui:runtime-module:<encoded>` as namespace `opentui` with path
// `runtime-module:<encoded>`, so the stand-in matches on that namespace.
const RUNTIME_MODULE_PATH_PREFIX = "runtime-module:";
const knownSpecifiers = new Set<string>(TUI_RUNTIME_SPECIFIERS);
plugin({
    name: "opentui-runtime-registry-stand-in",
    setup(build) {
        build.onResolve({ filter: /.*/, namespace: "opentui" }, (args) => {
            if (!args.path.startsWith(RUNTIME_MODULE_PATH_PREFIX)) return undefined;
            const specifier = decodeURIComponent(
                args.path.slice(RUNTIME_MODULE_PATH_PREFIX.length),
            );
            if (!knownSpecifiers.has(specifier)) {
                throw new Error(
                    `compiled TUI imports ${specifier}, which is not in TUI_RUNTIME_SPECIFIERS`,
                );
            }
            return { path: Bun.resolveSync(specifier, pluginRoot) };
        });
    },
});

try {
    const mod = (await import(compiledEntry)) as TuiModule;
    check("tui-compiled/index.tsx imports through the runtime registry stand-in", true);
    checkPluginShape("compiled TUI", mod);
} catch (error) {
    check(
        "tui-compiled/index.tsx imports through the runtime registry stand-in",
        false,
        error instanceof Error ? error.message : String(error),
    );
}

if (failures > 0) {
    console.error(`\nsmoke-tui-import: ${failures} check(s) failed`);
    process.exit(1);
}
console.log("\nsmoke-tui-import: all checks passed");
