import { afterEach, describe, expect, it } from "bun:test";
import { execFileSync } from "node:child_process";
import { chmodSync, mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { delimiter, join } from "node:path";
import {
    childPathWithLauncherDir,
    getCommandInvocation,
    invocationSpawnOptions,
} from "./command-invocation";

const originalComSpec = process.env.ComSpec;
const tempDirs: string[] = [];

afterEach(() => {
    if (originalComSpec === undefined) delete process.env.ComSpec;
    else process.env.ComSpec = originalComSpec;
    for (const dir of tempDirs.splice(0)) rmSync(dir, { recursive: true, force: true });
});

describe("childPathWithLauncherDir", () => {
    it("puts the launcher's directory ahead of the parent PATH", () => {
        expect(childPathWithLauncherDir("/home/fox/.bun/bin/omp", "/usr/bin")).toBe(
            `/home/fox/.bun/bin${delimiter}/usr/bin`,
        );
    });

    it("uses the launcher's directory alone when the parent has no PATH", () => {
        expect(childPathWithLauncherDir("/home/fox/.bun/bin/omp", "")).toBe("/home/fox/.bun/bin");
    });
});

describe("getCommandInvocation", () => {
    it("runs native executables directly with their directory on the child PATH", () => {
        expect(getCommandInvocation("/usr/local/bin/tool", ["--version"], "TOOL")).toEqual({
            command: "/usr/local/bin/tool",
            args: ["--version"],
            env: { PATH: childPathWithLauncherDir("/usr/local/bin/tool") },
        });
    });

    it("keeps a spaced shim path inside one quoted cmd.exe command string", () => {
        process.env.ComSpec = "custom-cmd.exe";
        const shim = "C:\\Users\\John Doe\\AppData\\Roaming\\npm\\tool.cmd";

        const invocation = getCommandInvocation(shim, ["models", "list"], "EIDNARA_TOOL_BINARY");
        expect(invocation).toEqual({
            command: "custom-cmd.exe",
            args: ["/d", "/s", "/v:off", "/c", '""%EIDNARA_TOOL_BINARY%" "models" "list""'],
            env: { PATH: childPathWithLauncherDir(shim), EIDNARA_TOOL_BINARY: shim },
            windowsVerbatimArguments: true,
        });

        const options = invocationSpawnOptions(invocation);
        expect(options.windowsVerbatimArguments).toBe(true);
        expect(options.env?.EIDNARA_TOOL_BINARY).toBe(shim);
        expect(options.env?.PATH).toBe(childPathWithLauncherDir(shim));
        expect(options.env?.HOME).toBe(process.env.HOME);
    });

    it("treats .bat like .cmd regardless of case", () => {
        expect(getCommandInvocation("C:\\tool.BAT", [], "TOOL").windowsVerbatimArguments).toBe(
            true,
        );
    });

    it.if(process.platform !== "win32")(
        "lets an env-shebang launcher find a sibling runtime absent from the parent PATH",
        () => {
            const dir = mkdtempSync(join(tmpdir(), "eidnara-launcher-"));
            tempDirs.push(dir);
            const runtime = join(dir, "fake-runtime");
            writeFileSync(runtime, '#!/bin/sh\necho "runtime ran $2"\n');
            chmodSync(runtime, 0o755);
            const launcher = join(dir, "tool");
            writeFileSync(launcher, "#!/usr/bin/env fake-runtime\n");
            chmodSync(launcher, 0o755);

            const invocation = getCommandInvocation(launcher, ["--version"], "TOOL");
            const output = execFileSync(invocation.command, invocation.args, {
                encoding: "utf-8",
                ...invocationSpawnOptions(invocation),
            });
            expect(output.trim()).toBe("runtime ran --version");
        },
    );
});
