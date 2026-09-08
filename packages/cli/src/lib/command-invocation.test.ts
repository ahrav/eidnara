import { afterEach, describe, expect, it } from "bun:test";
import { getCommandInvocation, invocationSpawnOptions } from "./command-invocation";

const originalComSpec = process.env.ComSpec;

afterEach(() => {
    if (originalComSpec === undefined) delete process.env.ComSpec;
    else process.env.ComSpec = originalComSpec;
});

describe("getCommandInvocation", () => {
    it("runs native executables directly", () => {
        expect(getCommandInvocation("/usr/local/bin/tool", ["--version"], "TOOL")).toEqual({
            command: "/usr/local/bin/tool",
            args: ["--version"],
        });
        expect(invocationSpawnOptions({ command: "/usr/local/bin/tool", args: [] })).toEqual({});
    });

    it("keeps a spaced shim path inside one quoted cmd.exe command string", () => {
        process.env.ComSpec = "custom-cmd.exe";
        const shim = "C:\\Users\\John Doe\\AppData\\Roaming\\npm\\tool.cmd";

        const invocation = getCommandInvocation(shim, ["models", "list"], "EIDNARA_TOOL_BINARY");
        expect(invocation).toEqual({
            command: "custom-cmd.exe",
            args: ["/d", "/s", "/v:off", "/c", '""%EIDNARA_TOOL_BINARY%" "models" "list""'],
            env: { EIDNARA_TOOL_BINARY: shim },
            windowsVerbatimArguments: true,
        });

        const options = invocationSpawnOptions(invocation);
        expect(options.windowsVerbatimArguments).toBe(true);
        expect(options.env?.EIDNARA_TOOL_BINARY).toBe(shim);
        expect(options.env?.PATH).toBe(process.env.PATH);
    });

    it("treats .bat like .cmd regardless of case", () => {
        expect(getCommandInvocation("C:\\tool.BAT", [], "TOOL").windowsVerbatimArguments).toBe(
            true,
        );
    });
});
