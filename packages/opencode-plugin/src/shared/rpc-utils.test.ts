/// <reference types="bun-types" />

import { afterEach, describe, expect, test } from "bun:test";
import { type execFileSync, execFileSync as spawnSyncExecFile } from "node:child_process";
import type { readFileSync } from "node:fs";
import { chmodSync, mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { delimiter, join } from "node:path";
import {
    __resetRpcIdentityTestHooks,
    __setRpcIdentityTestHooks,
    classifyProcessKind,
    discoverLivePiProcessIds,
    inspectLivePiProcesses,
    isPidAlive,
    isPidIdentityPlausible,
    parseRpcPortFile,
    type RpcPortFileRecord,
    rpcPortDir,
    rpcPortFilePath,
} from "./rpc-utils";

const PID = 1234;
const NOW_MS = 2_000_000;
const UPTIME_SECONDS = 1_000;

function record(startedAt: number): RpcPortFileRecord {
    return { port: 43123, pid: PID, started_at: startedAt };
}

function procStat(startTimeTicks: number): string {
    // After the command-name closing parenthesis, field 3 is `state`.
    // Field 22 is the twentieth value after the command-name parenthesis.
    return `${PID} (opencode) S ${Array.from({ length: 18 }, () => "0").join(" ")} ${startTimeTicks}`;
}

function linuxFiles(files: Record<string, string | Error>): typeof readFileSync {
    return ((path: string | URL) => {
        const value = files[String(path)];
        if (value instanceof Error) throw value;
        if (value === undefined) throw new Error(`unexpected read: ${String(path)}`);
        return value;
    }) as typeof readFileSync;
}

function psOutput(output: string | Error): typeof execFileSync {
    return (() => {
        if (output instanceof Error) throw output;
        return output;
    }) as typeof execFileSync;
}

function tasklistOutput(entries: Array<[number, string]>): string {
    return [
        '"Image Name","PID","Session Name","Session#","Mem Usage"',
        ...entries.map(([pid, command]) => `"${command}","${pid}","Console","1","10,000 K"`),
    ].join("\r\n");
}

/** `ConvertTo-Csv` writes a null `CommandLine` as an empty, unquoted field. */
function cimProcessListOutput(entries: Array<[number, string, string | null]>): string {
    return [
        '"ProcessId","Name","CommandLine"',
        ...entries.map(
            ([pid, name, commandLine]) =>
                `"${pid}","${name}",${commandLine === null ? "" : `"${commandLine.replaceAll('"', '""')}"`}`,
        ),
    ].join("\r\n");
}

afterEach(() => {
    __resetRpcIdentityTestHooks();
});

describe("rpcPortDir", () => {
    test("scopes a directory to one hash regardless of separator spelling", () => {
        const storage = join(tmpdir(), "eidnara-storage");
        expect(rpcPortDir(storage, "C:\\repo\\")).toBe(rpcPortDir(storage, "C:\\repo"));
        expect(rpcPortDir(storage, "C:\\repo\\sub")).toBe(rpcPortDir(storage, "C:/repo/sub"));
        expect(rpcPortDir(storage, "/proj/")).toBe(rpcPortDir(storage, "/proj"));
        expect(rpcPortDir(storage, "/proj")).not.toBe(rpcPortDir(storage, "/other"));
    });
});

describe("rpcPortFilePath", () => {
    const storage = join(tmpdir(), "eidnara-storage");
    const dir = rpcPortDir(storage, "/proj");

    test("keeps the port file inside the project RPC directory for any instance id", () => {
        expect(rpcPortFilePath(storage, "/proj", 1234, "abc-DEF_09")).toBe(
            join(dir, "port-1234-abc-DEF_09.json"),
        );
        expect(() => rpcPortFilePath(storage, "/proj", 1234, "../../../../../tmp/pwned")).toThrow(
            /instance id/,
        );
        expect(() => rpcPortFilePath(storage, "/proj", 1234, "a/b")).toThrow(/instance id/);
        expect(() => rpcPortFilePath(storage, "/proj", 1234, "")).not.toThrow();
    });
});

describe("parseRpcPortFile", () => {
    test("drops an instance id that could not have been produced by a server", () => {
        const base = { port: 43123, pid: PID, started_at: 5 };
        expect(
            parseRpcPortFile(JSON.stringify({ ...base, instance_id: "../../../../etc" })),
        ).toEqual({
            ...base,
            kind: undefined,
            harness: undefined,
            token: undefined,
            instance_id: undefined,
        });
        expect(
            parseRpcPortFile(JSON.stringify({ ...base, instance_id: "i-01" }))?.instance_id,
        ).toBe("i-01");
    });

    test("accepts a legacy record only when the whole value is a decimal port", () => {
        expect(parseRpcPortFile("43123", 7)).toEqual({ port: 43123, pid: 7, started_at: 0 });
        expect(parseRpcPortFile(" 8080\n")).toEqual({ port: 8080, pid: 0, started_at: 0 });
        expect(parseRpcPortFile("43123garbage")).toBeNull();
        expect(parseRpcPortFile("0x1F90")).toBeNull();
        expect(parseRpcPortFile("+8080")).toBeNull();
        expect(parseRpcPortFile("0")).toBeNull();
        expect(parseRpcPortFile("65536")).toBeNull();
    });
});

describe("classifyProcessKind", () => {
    test("classifies OpenCode server, OpenCode instance, Pi, and unknown commands", () => {
        expect(classifyProcessKind("/usr/local/bin/opencode serve --hostname 127.0.0.1")).toBe(
            "OpenCode server",
        );
        expect(classifyProcessKind("node /opt/opencode/bin/opencode --continue")).toBe(
            "OpenCode instance (TUI/CLI)",
        );
        expect(
            classifyProcessKind(
                "bun /workspace/node_modules/@mariozechner/pi-coding-agent/dist/cli.js",
            ),
        ).toBe("Pi");
        expect(classifyProcessKind("/usr/bin/other --flag")).toBe("process");
        expect(classifyProcessKind(null)).toBe("process");
    });

    test("recognizes serve flags and Windows-style executable paths", () => {
        expect(classifyProcessKind("opencode --serve=true")).toBe("OpenCode server");
        expect(classifyProcessKind("C:\\Tools\\opencode.exe")).toBe("OpenCode instance (TUI/CLI)");
        expect(classifyProcessKind("pi.cmd --model test")).toBe("Pi");
    });

    test("recognizes a Pi harness when interpreter flags precede the pi-coding-agent path", () => {
        expect(
            classifyProcessKind(
                "node --enable-source-maps /opt/node_modules/@earendil-works/pi-coding-agent/dist/cli.js",
            ),
        ).toBe("Pi");
        expect(
            classifyProcessKind(
                "bun run /opt/node_modules/@mariozechner/pi-coding-agent/dist/cli.js",
            ),
        ).toBe("Pi");
    });

    test("ignores pi-named option values and program arguments", () => {
        expect(classifyProcessKind("node --require pi app.js")).toBe("process");
        expect(classifyProcessKind("node app.js --model pi")).toBe("process");
        expect(classifyProcessKind("python worker.py --format pi")).toBe("process");
        expect(classifyProcessKind("python worker.py --format opencode")).toBe("process");
        expect(classifyProcessKind("/usr/bin/vim /home/dev/notes/pi")).toBe("process");
        expect(classifyProcessKind("bash -c cd /work && pi --model test")).toBe("process");
    });

    test("tokenizes quoted paths and NUL-separated cmdline arguments", () => {
        expect(
            classifyProcessKind(
                '"C:\\Program Files\\nodejs\\node.exe" "C:\\Users\\dev\\AppData\\Roaming\\npm\\node_modules\\@mariozechner\\pi-coding-agent\\dist\\cli.js"',
            ),
        ).toBe("Pi");
        expect(classifyProcessKind('"C:\\Program Files\\OpenCode\\opencode.exe" serve')).toBe(
            "OpenCode server",
        );
        expect(classifyProcessKind("/opt/pi/bin/pi\u0000--model\u0000test\u0000")).toBe("Pi");
        expect(classifyProcessKind("node\u0000/tmp/my app/worker.js\u0000--model\u0000pi")).toBe(
            "process",
        );
    });
});

describe("discoverLivePiProcessIds", () => {
    test("finds Pi-family harness commands while excluding the current process", () => {
        __setRpcIdentityTestHooks({
            processListExecFileSync: (() =>
                [
                    ` ${process.pid} /usr/local/bin/pi`,
                    " 41001 /usr/local/bin/pi --model test",
                    " 41002 node /opt/node_modules/@mariozechner/pi-coding-agent/dist/cli.js",
                    " 41003 bun /opt/node_modules/@oh-my-pi/pi-coding-agent/dist/cli.js",
                    " 41004 /Applications/OpenCode.app/Contents/MacOS/opencode",
                    " 41005 node /workspace/pi-plugin/src/index.ts",
                    " 41006 npm install @earendil-works/pi-coding-agent",
                    " 41007 /usr/local/bin/omp --model test",
                    " 41008 /usr/bin/node --max-old-space-size=4096 /opt/pi/bin/pi",
                    " 41009 '/opt/pi/bin/pi' --model test",
                    " 41010 /opt/tools/omp.cmd --flag",
                    " 41011 sh -c exec pi --model test",
                    " 41012 /usr/bin/timeout 3600 /usr/local/bin/pi --resume",
                    " 41013 node /opt/pi/bin/pi.cmd",
                ].join("\n")) as typeof execFileSync,
        });

        expect(discoverLivePiProcessIds()).toEqual([
            41001, 41002, 41003, 41007, 41008, 41009, 41010, 41011, 41012, 41013,
        ]);
    });

    test("discovery and classification agree on every Pi-family command shape", () => {
        const commands = [
            "/usr/local/bin/pi --model test",
            "node /opt/node_modules/@mariozechner/pi-coding-agent/dist/cli.js",
            "node --enable-source-maps /opt/node_modules/@earendil-works/pi-coding-agent/dist/cli.js",
            "/usr/bin/node --max-old-space-size=4096 /opt/pi/bin/pi",
            "'/opt/pi/bin/pi' --model test",
            "/opt/tools/omp.cmd --flag",
            "/Applications/OpenCode.app/Contents/MacOS/opencode",
            "node /workspace/pi-plugin/src/index.ts",
            "npm install @earendil-works/pi-coding-agent",
        ];
        __setRpcIdentityTestHooks({
            processListExecFileSync: (() =>
                commands
                    .map((command, index) => ` ${50_000 + index} ${command}`)
                    .join("\n")) as typeof execFileSync,
        });

        const discovered = new Set(discoverLivePiProcessIds());
        for (const [index, command] of commands.entries()) {
            expect(discovered.has(50_000 + index)).toBe(classifyProcessKind(command) === "Pi");
        }
    });

    test("reports uncertainty instead of treating an unavailable process list as empty", () => {
        __setRpcIdentityTestHooks({
            processListExecFileSync: (() => {
                throw new Error("ps unavailable");
            }) as typeof execFileSync,
        });

        expect(inspectLivePiProcesses()).toEqual({
            state: "unreadable",
            processIds: [],
            error: "ps unavailable",
        });
    });

    test("reads Windows command lines through CIM so a Pi hosted by node.exe is found", () => {
        const calls: Array<{ file: string; args: readonly string[] }> = [];
        __setRpcIdentityTestHooks({
            platform: "win32",
            processListExecFileSync: ((file: string | URL, args: readonly string[] = []) => {
                calls.push({ file: String(file), args });
                return cimProcessListOutput([
                    [process.pid, "pi.exe", "pi.exe --model test"],
                    [
                        41001,
                        "node.exe",
                        '"C:\\Program Files\\nodejs\\node.exe" "C:\\Users\\dev\\AppData\\Roaming\\npm\\node_modules\\@mariozechner\\pi-coding-agent\\dist\\cli.js"',
                    ],
                    [
                        41002,
                        "node.exe",
                        '"C:\\Program Files\\nodejs\\node.exe" C:\\work\\server.js',
                    ],
                    [41003, "opencode.exe", "C:\\Tools\\opencode.exe serve"],
                    [41004, "pi.exe", null],
                    [41005, "chrome.exe", null],
                ]);
            }) as typeof execFileSync,
        });

        expect(inspectLivePiProcesses()).toEqual({ state: "known", processIds: [41001, 41004] });
        expect(calls).toHaveLength(1);
        expect(calls[0].file).toBe("powershell.exe");
        expect(calls[0].args.at(-1)).toContain("Get-CimInstance Win32_Process");
    });

    test("fails closed on Windows when a runtime image hides its command line", () => {
        __setRpcIdentityTestHooks({
            platform: "win32",
            processListExecFileSync: (() =>
                cimProcessListOutput([
                    [41001, "pi.exe", "pi.exe --model test"],
                    [41002, "node.exe", null],
                ])) as typeof execFileSync,
        });

        expect(inspectLivePiProcesses()).toEqual({
            state: "unreadable",
            processIds: [],
            error: "command line unavailable for node.exe (pid 41002)",
        });
    });

    test("treats Windows process-list output without a CSV header as unreadable", () => {
        __setRpcIdentityTestHooks({
            platform: "win32",
            processListExecFileSync: (() =>
                "Get-CimInstance : Access denied") as typeof execFileSync,
        });

        expect(inspectLivePiProcesses()).toEqual({
            state: "unreadable",
            processIds: [],
            error: "PowerShell process list unavailable",
        });
    });

    test("keeps a Windows process whose command line spans lines, and fails closed on a torn record", () => {
        const multiline = cimProcessListOutput([
            [
                41001,
                "node.exe",
                'node.exe "C:\\x\\pi-coding-agent\\dist\\cli.js" --prompt "line one\r\nline two"',
            ],
            [41002, "pi.exe", "pi.exe --model test"],
        ]);
        __setRpcIdentityTestHooks({
            platform: "win32",
            processListExecFileSync: (() => multiline) as typeof execFileSync,
        });
        expect(inspectLivePiProcesses()).toEqual({ state: "known", processIds: [41001, 41002] });

        // An unterminated quote means the output was cut; nothing after it can be trusted.
        __setRpcIdentityTestHooks({
            platform: "win32",
            processListExecFileSync: (() =>
                `${cimProcessListOutput([[41002, "pi.exe", "pi.exe --model test"]])}\r\n"41003","node.exe","node.exe C:\\x`) as typeof execFileSync,
        });
        expect(inspectLivePiProcesses()).toEqual({
            state: "unreadable",
            processIds: [],
            error: "PowerShell process list unavailable",
        });
    });

    test.skipIf(process.platform === "win32")(
        "probes the real process list even when NODE_ENV is test",
        () => {
            // Bun resolves executables against the launch-time PATH, so only a child process can see the fake `ps`.
            const binDir = mkdtempSync(join(tmpdir(), "eidnara-fake-ps-"));
            const fakePs = join(binDir, "ps");
            writeFileSync(fakePs, "#!/bin/sh\nprintf ' 41999 /usr/local/bin/pi --model test\\n'\n");
            chmodSync(fakePs, 0o755);
            try {
                const output = spawnSyncExecFile(
                    process.execPath,
                    [
                        "-e",
                        'import { inspectLivePiProcesses } from "./rpc-utils.ts"; console.log(JSON.stringify(inspectLivePiProcesses()));',
                    ],
                    {
                        cwd: import.meta.dir,
                        encoding: "utf8",
                        env: {
                            ...process.env,
                            NODE_ENV: "test",
                            PATH: `${binDir}${delimiter}${process.env.PATH ?? ""}`,
                        },
                    },
                );
                expect(JSON.parse(String(output).trim())).toEqual({
                    state: "known",
                    processIds: [41999],
                });
            } finally {
                rmSync(binDir, { recursive: true, force: true });
            }
        },
    );
});

describe("isPidAlive", () => {
    test("distinguishes confirmed, dead, and inaccessible PID probes", () => {
        const probeFailure = (code: string): NodeJS.ErrnoException => {
            const error = new Error(`kill failed: ${code}`) as NodeJS.ErrnoException;
            error.code = code;
            return error;
        };

        __setRpcIdentityTestHooks({
            processKill: (() => true) as typeof process.kill,
        });
        expect(isPidAlive(PID)).toBe("alive");

        __setRpcIdentityTestHooks({
            processKill: (() => {
                throw probeFailure("ESRCH");
            }) as typeof process.kill,
        });
        expect(isPidAlive(PID)).toBe("dead");

        __setRpcIdentityTestHooks({
            processKill: (() => {
                throw probeFailure("EPERM");
            }) as typeof process.kill,
        });
        expect(isPidAlive(PID)).toBe("inconclusive");
    });

    test("uses tasklist for Windows liveness and captures probe stderr", () => {
        const calls: Array<{ file: string; args: readonly string[]; stdio: unknown }> = [];
        __setRpcIdentityTestHooks({
            platform: "win32",
            execFileSync: ((
                file: string | URL,
                args: readonly string[] = [],
                options: { stdio?: unknown } = {},
            ) => {
                calls.push({ file: String(file), args, stdio: options.stdio });
                if (String(file) === "ps") throw new Error("ps must not run on Windows");
                return tasklistOutput([
                    [4, "System"],
                    [PID, "OpenCode.exe"],
                ]);
            }) as typeof execFileSync,
        });

        expect(isPidAlive(PID)).toBe("alive");
        expect(calls).toEqual([
            {
                file: "tasklist",
                args: ["/FO", "CSV"],
                stdio: ["ignore", "pipe", "pipe"],
            },
        ]);
    });

    test("treats a PID absent from the tasklist CSV as dead regardless of locale", () => {
        // A localized `tasklist` never enters the verdict: the unfiltered list is CSV in every locale.
        __setRpcIdentityTestHooks({
            platform: "win32",
            execFileSync: (() => tasklistOutput([[4, "System"]])) as typeof execFileSync,
        });
        expect(isPidAlive(PID)).toBe("dead");

        // Only the header means the list is empty, not missing.
        __setRpcIdentityTestHooks({
            platform: "win32",
            execFileSync: (() => tasklistOutput([])) as typeof execFileSync,
        });
        expect(isPidAlive(PID)).toBe("dead");

        // Prose without the CSV header, such as the English or a translated no-match sentence, is not a verdict.
        __setRpcIdentityTestHooks({
            platform: "win32",
            execFileSync: (() =>
                "INFO: No tasks are running which match the specified criteria.") as typeof execFileSync,
        });
        expect(isPidAlive(PID)).toBe("inconclusive");
    });

    test("returns inconclusive when the Windows tasklist probe cannot spawn", () => {
        __setRpcIdentityTestHooks({
            platform: "win32",
            execFileSync: (() => {
                throw new Error("tasklist unavailable");
            }) as typeof execFileSync,
        });

        expect(isPidAlive(PID)).toBe("inconclusive");
        expect(isPidIdentityPlausible(record(0))).toBe("inconclusive");
    });
});

describe("isPidIdentityPlausible", () => {
    test("rejects a reused Linux PID when proc start time is substantially newer", () => {
        const readPaths: string[] = [];
        __setRpcIdentityTestHooks({
            platform: "linux",
            nowMs: () => NOW_MS,
            readFileSync: ((path: string | URL) => {
                readPaths.push(String(path));
                const files = {
                    [`/proc/${PID}/stat`]: procStat(10_000),
                    "/proc/uptime": `${UPTIME_SECONDS}.0 0.0`,
                };
                return files[String(path) as keyof typeof files];
            }) as typeof readFileSync,
            execFileSync: (() => {
                throw new Error("ps must not run on Linux");
            }) as typeof execFileSync,
        });

        expect(isPidIdentityPlausible(record(500_000))).toBe("implausible");
        expect(readPaths).toEqual([`/proc/${PID}/stat`, "/proc/uptime"]);
    });

    test("accepts a genuine Linux record when the process started no later than the record", () => {
        __setRpcIdentityTestHooks({
            platform: "linux",
            nowMs: () => NOW_MS,
            readFileSync: linuxFiles({
                [`/proc/${PID}/stat`]: procStat(10_000),
                "/proc/uptime": `${UPTIME_SECONDS}.0 0.0`,
            }),
        });

        // The 120-second tolerance accounts for port-file creation after process startup.
        expect(isPidIdentityPlausible(record(1_000_000))).toBe("plausible");
    });

    test("reports an unreadable Linux start-time probe as inconclusive", () => {
        __setRpcIdentityTestHooks({
            platform: "linux",
            readFileSync: linuxFiles({
                [`/proc/${PID}/stat`]: new Error("procfs unavailable"),
            }),
        });

        expect(isPidIdentityPlausible(record(500_000))).toBe("inconclusive");
    });

    test("uses the legacy Linux command fallback and distinguishes probe errors", () => {
        __setRpcIdentityTestHooks({
            platform: "linux",
            readFileSync: linuxFiles({
                [`/proc/${PID}/cmdline`]: "/usr/sbin/opendkim --config /etc/opendkim.conf",
            }),
        });
        expect(isPidIdentityPlausible(record(0))).toBe("implausible");

        __setRpcIdentityTestHooks({
            platform: "linux",
            readFileSync: linuxFiles({
                [`/proc/${PID}/cmdline`]: "/usr/local/bin/opencode serve",
            }),
        });
        expect(isPidIdentityPlausible(record(0))).toBe("plausible");

        // A reused PID running an unrelated Node script is not evidence for OpenCode.
        __setRpcIdentityTestHooks({
            platform: "linux",
            readFileSync: linuxFiles({
                [`/proc/${PID}/cmdline`]: "node\u0000/tmp/worker.js",
            }),
        });
        expect(isPidIdentityPlausible(record(0))).toBe("inconclusive");

        __setRpcIdentityTestHooks({
            platform: "linux",
            readFileSync: linuxFiles({
                [`/proc/${PID}/cmdline`]: new Error("procfs unavailable"),
            }),
        });
        expect(isPidIdentityPlausible(record(0))).toBe("inconclusive");
    });

    test("uses ps start time and command probes on non-Linux platforms", () => {
        const startTimeCalls: Array<{
            args: readonly string[];
            env: NodeJS.ProcessEnv | undefined;
        }> = [];
        __setRpcIdentityTestHooks({
            platform: "darwin",
            execFileSync: ((
                _file: string | URL,
                args: readonly string[] = [],
                options: { env?: NodeJS.ProcessEnv } = {},
            ) => {
                startTimeCalls.push({ args, env: options.env });
                return "Mon Aug  7 00:00:00 1970";
            }) as typeof execFileSync,
        });
        expect(
            isPidIdentityPlausible(record(Date.parse("Mon Aug  7 00:00:00 1970") - 121_000)),
        ).toBe("implausible");
        // `lstart` is locale-formatted, so the probe pins the C locale.
        expect(startTimeCalls).toHaveLength(1);
        expect(startTimeCalls[0].args).toEqual(["-p", String(PID), "-o", "lstart="]);
        expect(startTimeCalls[0].env?.LC_ALL).toBe("C");

        __setRpcIdentityTestHooks({
            platform: "darwin",
            execFileSync: psOutput("Mon Aug  7 00:00:00 1970"),
        });
        expect(
            isPidIdentityPlausible(record(Date.parse("Mon Aug  7 00:00:00 1970") - 120_000)),
        ).toBe("plausible");

        __setRpcIdentityTestHooks({
            platform: "darwin",
            execFileSync: psOutput("/usr/sbin/opendkim -f"),
        });
        expect(isPidIdentityPlausible(record(0))).toBe("implausible");

        __setRpcIdentityTestHooks({
            platform: "darwin",
            execFileSync: psOutput("/Applications/OpenCode.app/Contents/MacOS/opencode"),
        });
        expect(isPidIdentityPlausible(record(0))).toBe("plausible");

        __setRpcIdentityTestHooks({
            platform: "darwin",
            execFileSync: psOutput(new Error("ps unavailable")),
        });
        expect(isPidIdentityPlausible(record(0))).toBe("inconclusive");
    });

    test("uses the tasklist command check on Windows whether or not the record carries a start time", () => {
        const calls: Array<{ file: string; args: readonly string[] }> = [];
        __setRpcIdentityTestHooks({
            platform: "win32",
            execFileSync: ((file: string | URL, args: readonly string[] = []) => {
                calls.push({ file: String(file), args });
                if (String(file) === "ps") throw new Error("ps must not run on Windows");
                return tasklistOutput([[PID, "OpenCode.exe"]]);
            }) as typeof execFileSync,
        });

        expect(isPidIdentityPlausible(record(0))).toBe("plausible");
        expect(calls).toEqual([{ file: "tasklist", args: ["/FO", "CSV"] }]);

        // Windows exposes no start time here, so a modern record still gets the command verdict.
        calls.length = 0;
        expect(isPidIdentityPlausible(record(NOW_MS))).toBe("plausible");
        expect(calls).toEqual([{ file: "tasklist", args: ["/FO", "CSV"] }]);

        __setRpcIdentityTestHooks({
            platform: "win32",
            execFileSync: (() => tasklistOutput([[PID, "opendkim.exe"]])) as typeof execFileSync,
        });
        expect(isPidIdentityPlausible(record(NOW_MS))).toBe("implausible");

        // `tasklist` shows only the image, and `node.exe` may or may not be hosting OpenCode.
        __setRpcIdentityTestHooks({
            platform: "win32",
            execFileSync: (() => tasklistOutput([[PID, "node.exe"]])) as typeof execFileSync,
        });
        expect(isPidIdentityPlausible(record(NOW_MS))).toBe("inconclusive");
    });
});
