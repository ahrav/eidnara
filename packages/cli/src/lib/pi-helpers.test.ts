import { afterEach, describe, expect, it } from "bun:test";
import { chmodSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { childPathWithLauncherDir } from "./command-invocation";
import {
    getAvailableModels,
    getPiCommandInvocation,
    getPiFallbackCandidates,
    getPiVersion,
    parseModelListOutput,
} from "./pi-helpers";

// Executable shell stubs require POSIX.
const isPosix = process.platform !== "win32";
const originalComSpec = process.env.ComSpec;
const tempDirs: string[] = [];

afterEach(() => {
    if (originalComSpec === undefined) delete process.env.ComSpec;
    else process.env.ComSpec = originalComSpec;
    for (const dir of tempDirs.splice(0)) rmSync(dir, { recursive: true, force: true });
});

function fakePi(body: string): string {
    const dir = mkdtempSync(join(tmpdir(), "eidnara-pi-bin-"));
    tempDirs.push(dir);
    const bin = join(dir, "pi");
    writeFileSync(bin, `#!/bin/sh\n${body}\n`);
    chmodSync(bin, 0o755);
    return bin;
}

const HEADER = "provider      model                context  max-out  thinking  images";

describe("parseModelListOutput", () => {
    it("parses validated rows below the models table header", () => {
        const output = [
            HEADER,
            "anthropic     claude-fable-5       1M       128K     yes       yes",
            "openai-codex  gpt-5.5              400K     128K     yes       yes",
            "opencode-go   kimi-k2.6            262.1K   65.5K    yes       yes",
        ].join("\n");
        expect(parseModelListOutput(output)).toEqual([
            "anthropic/claude-fable-5",
            "openai-codex/gpt-5.5",
            "opencode-go/kimi-k2.6",
        ]);
    });

    it("allows provider-qualified model ids in the model column", () => {
        const output = [HEADER, "openrouter anthropic/claude-sonnet-4 200K 64K yes no"].join("\n");
        expect(parseModelListOutput(output)).toEqual(["openrouter/anthropic/claude-sonnet-4"]);
    });

    it("allows scoped model ids that begin with @", () => {
        const output = [HEADER, "modal @modal/qwen/model-v1 128K 32K no no"].join("\n");
        expect(parseModelListOutput(output)).toEqual(["modal/@modal/qwen/model-v1"]);
    });

    it("ignores headings, prose, and rows before a recognized header", () => {
        const output = [
            "Available models:",
            "anthropic claude-fake 1M 128K yes yes",
            HEADER,
            "Documentation is available online now",
            "anthropic claude-real 1M 128K yes yes",
        ].join("\n");
        expect(parseModelListOutput(output)).toEqual(["anthropic/claude-real"]);
    });

    it("requires the expected metadata columns", () => {
        const output = [
            HEADER,
            "anthropic claude-prose words that look plausible here",
            "anthropic claude-real 1M 128K yes no",
        ].join("\n");
        expect(parseModelListOutput(output)).toEqual(["anthropic/claude-real"]);
    });

    it("dedupes rows and strips ANSI color codes", () => {
        const esc = String.fromCharCode(27);
        const output = [
            HEADER,
            `${esc}[32manthropic${esc}[0m claude-opus-4-8 1M 128K yes yes`,
            "anthropic claude-opus-4-8 1M 128K yes yes",
        ].join("\n");
        expect(parseModelListOutput(output)).toEqual(["anthropic/claude-opus-4-8"]);
    });
});

describe("Pi fallback discovery", () => {
    it("probes the installer directory, then Bun and ~/.local launchers on POSIX", () => {
        const home = "/virt/home";
        expect(getPiFallbackCandidates("linux", home)).toEqual([
            join(home, ".pi", "bin", "pi"),
            join(home, ".bun", "bin", "pi"),
            join(home, ".local", "bin", "pi"),
            "/usr/local/bin/pi",
            "/opt/homebrew/bin/pi",
        ]);
    });

    it("emits only system launchers when no absolute home is known", () => {
        expect(getPiFallbackCandidates("linux", undefined)).toEqual([
            "/usr/local/bin/pi",
            "/opt/homebrew/bin/pi",
        ]);
        expect(getPiFallbackCandidates("win32", undefined)).toEqual([]);
    });

    it("probes the installer directory, then npm and Bun launchers on Windows", () => {
        const home = "C:\\Users\\fox";
        const appData = "C:\\Users\\fox\\AppData\\Roaming";
        expect(getPiFallbackCandidates("win32", home, appData)).toEqual([
            join(home, ".pi", "bin", "pi.cmd"),
            join(appData, "npm", "pi.cmd"),
            join(appData, "npm", "pi.exe"),
            join(home, ".bun", "bin", "pi.exe"),
            join(home, ".bun", "bin", "pi.cmd"),
        ]);
    });
});

describe("Pi command execution", () => {
    it("routes cmd shims through ComSpec as one quoted command", () => {
        process.env.ComSpec = "custom-cmd.exe";
        const shim = "C:\\Users\\John Doe\\AppData\\Roaming\\npm\\pi.cmd";

        expect(getPiCommandInvocation(shim, ["--version"])).toEqual({
            command: "custom-cmd.exe",
            args: ["/d", "/s", "/v:off", "/c", '""%EIDNARA_PI_BINARY%" "--version""'],
            env: { PATH: childPathWithLauncherDir(shim), EIDNARA_PI_BINARY: shim },
            windowsVerbatimArguments: true,
        });
    });

    it.if(isPosix)(
        "runs a cmd shim through a stand-in ComSpec and parses its output",
        () => {
            const root = mkdtempSync(join(tmpdir(), "eidnara pi command "));
            tempDirs.push(root);
            const comSpec = join(root, "fake-cmd");
            writeFileSync(
                comSpec,
                `#!/bin/sh\ncase "$5" in\n  *--version*) printf '0.75.1\\n' ;;\n  *) printf '${HEADER}\\nanthropic claude-fable-5 1M 128K yes yes\\n' ;;\nesac\n`,
            );
            chmodSync(comSpec, 0o755);
            process.env.ComSpec = comSpec;
            const shim = join(root, "pi.cmd");

            expect(getPiVersion(shim)).toBe("0.75.1");
            expect(getAvailableModels(shim)).toEqual(["anthropic/claude-fable-5"]);
        },
        30_000,
    );

    it("invokes a POSIX binary directly with its directory on the child PATH", () => {
        expect(getPiCommandInvocation("/usr/local/bin/pi", ["--version"])).toEqual({
            command: "/usr/local/bin/pi",
            args: ["--version"],
            env: { PATH: childPathWithLauncherDir("/usr/local/bin/pi") },
        });
    });
});

describe.if(isPosix)("getPiVersion", () => {
    it("returns null when the probe exits nonzero, even with stderr output", () => {
        const pi = fakePi('echo "pi: unknown option --version" >&2; exit 2');
        expect(getPiVersion(pi)).toBeNull();
    });

    it("returns null when the probe times out after writing to stderr", () => {
        const pi = fakePi('echo "starting" >&2; sleep 5');
        const started = performance.now();
        expect(getPiVersion(pi, 200)).toBeNull();
        expect(performance.now() - started).toBeLessThan(3_000);
    });

    it("accepts stderr output after a clean exit", () => {
        const pi = fakePi('echo "0.80.0" >&2');
        expect(getPiVersion(pi)).toBe("0.80.0");
    });
});

describe("getAvailableModels", () => {
    it("returns [] when pi output parses to no models (no static fallback)", () => {
        const piPath = process.platform === "win32" ? "where" : "true";
        expect(getAvailableModels(piPath)).toEqual([]);
    });

    it.if(isPosix)("does not run the compatibility probe when --list-models succeeds", () => {
        const root = mkdtempSync(join(tmpdir(), "eidnara-pi-models-"));
        tempDirs.push(root);
        const log = join(root, "calls.log");
        const pi = fakePi(
            `echo "$*" >> "${log}"\nif [ "$1" = "--list-models" ]; then printf '${HEADER}\\nanthropic claude-fable-5 1M 128K yes yes\\n'; fi`,
        );

        expect(getAvailableModels(pi)).toEqual(["anthropic/claude-fable-5"]);
        expect(readFileSync(log, "utf-8").trim().split("\n")).toEqual(["--list-models"]);
    });

    it.if(isPosix)("falls back to `models list` when --list-models yields nothing", () => {
        const root = mkdtempSync(join(tmpdir(), "eidnara-pi-models-"));
        tempDirs.push(root);
        const log = join(root, "calls.log");
        const pi = fakePi(
            `echo "$*" >> "${log}"\nif [ "$1" = "models" ]; then printf '${HEADER}\\nanthropic claude-fable-5 1M 128K yes yes\\n'; fi`,
        );

        expect(getAvailableModels(pi)).toEqual(["anthropic/claude-fable-5"]);
        expect(readFileSync(log, "utf-8").trim().split("\n")).toEqual([
            "--list-models",
            "models list",
        ]);
    });
});
