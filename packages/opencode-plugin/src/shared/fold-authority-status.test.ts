import { afterEach, describe, expect, it } from "bun:test";
import { mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { loadPluginConfigDetailed } from "../config";
import {
    CONFLICT_WARNING_HEADER,
    type ConflictWarning,
    formatConflictShort,
} from "./conflict-detector";
import {
    AUTHORITY_PENDING_WARNING,
    foldAuthorityStatus,
    formatFoldAuthorityLines,
    type PluginFoldAuthority,
    parseDaemonFoldAuthority,
    pluginFoldAuthority,
    publishOnChange,
    RESTART_OTHER_INSTANCES_STEP,
    ROOT_MISMATCH_WARNING,
    reportFoldAuthority,
    withDiskAuthority,
} from "./fold-authority-status";

const STATUS_FIXTURE = JSON.parse(
    readFileSync(join(import.meta.dir, "__fixtures__", "fold-authority-status.json"), "utf-8"),
) as {
    pending: Array<{ reason: string; segment: string; step: string }>;
    stalled_applied: string;
};
const USER_CONFIG = "/home/u/.config/eidnara/eidnara.jsonc";
const TAIL =
    "session ses (last active 0s ago): 0 history_segments, coverage ordinal none, boundary absent, 0 pending drops, 0 tags, pending m1 delta false, last history_summarizer: idle, publish failures: 0, surface inactive";

function summary(authority: string, path = USER_CONFIG, tail = TAIL): string {
    return `fold authority ${authority}; user config ${path}; ${tail}`;
}

function plugin(overrides: Partial<PluginFoldAuthority> = {}): PluginFoldAuthority {
    return {
        startup: { kind: "eidnara", reason: "summarizer chain: test/model" },
        userConfigPath: USER_CONFIG,
        readDisk: () => ({ kind: "eidnara", reason: "summarizer chain: test/model" }),
        publish: () => {},
        ...overrides,
    };
}

describe("parseDaemonFoldAuthority", () => {
    it("reads an older summary without the prefix as unknown with no path", () => {
        expect(parseDaemonFoldAuthority(TAIL)).toEqual({ applied: "unknown", stalled: false });
        expect(parseDaemonFoldAuthority(undefined)).toEqual({ applied: "unknown", stalled: false });
    });

    it("reads the applied authority and the user-tier path", () => {
        expect(parseDaemonFoldAuthority(summary("native"))).toEqual({
            applied: "native",
            pending: undefined,
            userConfigPath: USER_CONFIG,
            stalled: false,
        });
        expect(parseDaemonFoldAuthority(summary("unadopted (intent eidnara)")).applied).toBe(
            "unadopted",
        );
    });

    it("reads a pending target and its reason", () => {
        const parsed = parseDaemonFoldAuthority(
            summary(
                "eidnara; fold authority pending native: another binding is open on this session",
            ),
        );
        expect(parsed.applied).toBe("eidnara");
        expect(parsed.pending).toEqual({
            target: "native",
            reason: "another binding is open on this session",
        });
    });

    it("decodes a marked path and reads an unmarked one literally", () => {
        const path = (text: string) =>
            parseDaemonFoldAuthority(summary("native", text)).userConfigPath;
        expect(path("none")).toBeUndefined();
        expect(path("encoded:/Users/a%20b/eidnara.jsonc")).toBe("/Users/a b/eidnara.jsonc");
        expect(path("encoded:/Users/%E0%A4%A/eidnara.jsonc")).toBeUndefined();
        expect(path("/Users/a%20b/eidnara.jsonc")).toBe("/Users/a%20b/eidnara.jsonc");
        expect(path("/Users/a b/eidnara.jsonc")).toBeUndefined();
    });

    it("reads a stalled summarizer from the authority prefix", () => {
        const stalled = summary("eidnara, summarizer stalled (no models at the last pass)");
        expect(parseDaemonFoldAuthority(stalled)).toEqual({
            applied: "eidnara",
            pending: undefined,
            userConfigPath: USER_CONFIG,
            stalled: true,
        });
        const noModelsTail = TAIL.replace(
            "last history_summarizer: idle",
            "last history_summarizer: no fire: no_models",
        );
        expect(parseDaemonFoldAuthority(summary("native", USER_CONFIG, noModelsTail)).stalled).toBe(
            false,
        );
        expect(
            parseDaemonFoldAuthority(summary("eidnara", USER_CONFIG, noModelsTail)).stalled,
        ).toBe(true);
    });

    it("keeps the prefix when the daemon truncates the summary inside the tail", () => {
        const long = `/home/u/${"deep/".repeat(90)}eidnara.jsonc`;
        const truncated = summary(
            "eidnara, summarizer stalled (no models at the last pass); fold authority pending native: another binding is open on this session",
            long,
        ).slice(0, 500);
        const parsed = parseDaemonFoldAuthority(truncated);
        expect(parsed.stalled).toBe(true);
        expect(parsed.pending?.target).toBe("native");
        expect(parsed.userConfigPath).toBeUndefined();
    });
});

describe("foldAuthorityStatus", () => {
    it("raises the pending warning with the restart step", () => {
        const status = foldAuthorityStatus(
            plugin(),
            summary(
                "native; fold authority pending eidnara: another binding is open on this session",
            ),
        );
        expect(status.warnings).toHaveLength(1);
        expect(status.warnings[0]?.startsWith(AUTHORITY_PENDING_WARNING)).toBe(true);
        expect(status.label).toBe(
            `fold authority pending eidnara: ${RESTART_OTHER_INSTANCES_STEP}`,
        );
    });

    it("raises the root mismatch naming both paths and the daemon restart", () => {
        const status = foldAuthorityStatus(
            plugin(),
            summary("eidnara", "/root/.config/eidnara/eidnara.jsonc"),
        );
        expect(status.warnings).toEqual([
            `${ROOT_MISMATCH_WARNING}: the daemon reads /root/.config/eidnara/eidnara.jsonc and this plugin reads ${USER_CONFIG}; run \`eidnara daemon restart\`.`,
        ]);
        expect(status.label).toBe(`${ROOT_MISMATCH_WARNING}: run \`eidnara daemon restart\``);
    });

    it("raises nothing for an identical or an absent path", () => {
        expect(foldAuthorityStatus(plugin(), summary("eidnara")).warnings).toEqual([]);
        const absent = foldAuthorityStatus(plugin(), summary("eidnara", "none"));
        expect(absent.warnings).toEqual([]);
        expect(absent.daemon_user_config).toBeUndefined();
        expect(formatFoldAuthorityLines(absent)).toContain(
            `- User config: daemon unknown, plugin ${USER_CONFIG}`,
        );
        expect(foldAuthorityStatus(plugin(), TAIL).warnings).toEqual([]);
    });

    it("labels a stalled summarizer apart from a native authority and a pending change", () => {
        const stalled = foldAuthorityStatus(
            plugin(),
            summary("eidnara, summarizer stalled (no models at the last pass)"),
        );
        expect(stalled.label).toBe("summarizer stalled at the last pass: no summarizer model");
        expect(stalled.warnings).toEqual([]);
        expect(foldAuthorityStatus(plugin(), summary("native")).label).toBeUndefined();
        const both = foldAuthorityStatus(
            plugin(),
            summary(
                "eidnara, summarizer stalled (no models at the last pass); fold authority pending native: another binding is open on this session",
            ),
        );
        expect(both.label).toBe(
            `fold authority pending native: ${RESTART_OTHER_INSTANCES_STEP} · summarizer stalled at the last pass: no summarizer model`,
        );
    });

    it("parses every daemon segment in the shared status fixture and names its step", () => {
        expect(STATUS_FIXTURE.pending.map((row) => row.reason)).toEqual([
            "sibling_bound",
            "not_quiescent",
            "later_bind",
        ]);
        for (const row of STATUS_FIXTURE.pending) {
            const status = foldAuthorityStatus(plugin(), summary(`native; ${row.segment}`));
            expect(status.pending).toEqual({
                target: "eidnara",
                reason: row.segment.slice(row.segment.indexOf(": ") + 2),
                step: row.step,
            });
        }
        expect(STATUS_FIXTURE.pending[0]?.step).toBe(RESTART_OTHER_INSTANCES_STEP);
        const stalled = parseDaemonFoldAuthority(summary(STATUS_FIXTURE.stalled_applied));
        expect(stalled).toMatchObject({ applied: "eidnara", stalled: true });
    });

    it("compares configuration roots from the daemon's encoded path, not file names", () => {
        const jsonFile = foldAuthorityStatus(
            plugin({ userConfigPath: "/home/u/.config/eidnara/eidnara.json" }),
            summary("eidnara"),
        );
        expect(jsonFile.warnings).toEqual([]);
        const spacedRoot = "/home/a  b;c/.config/eidnara/eidnara.jsonc";
        const same = foldAuthorityStatus(
            plugin({ userConfigPath: spacedRoot }),
            summary("eidnara", "encoded:/home/a%20%20b%3Bc/.config/eidnara/eidnara.jsonc"),
        );
        expect(same.daemon_user_config).toBe(spacedRoot);
        expect(same.warnings).toEqual([]);
        const distinct = foldAuthorityStatus(
            plugin({ userConfigPath: "/home/a b;c/.config/eidnara/eidnara.jsonc" }),
            summary("eidnara", "encoded:/home/a%20%20b%3Bc/.config/eidnara/eidnara.jsonc"),
        );
        expect(distinct.warnings[0]).toStartWith(ROOT_MISMATCH_WARNING);
    });

    it("compares configuration roots after normalizing each path", () => {
        const daemonPaths = [
            "encoded:/home/u//.config/eidnara/eidnara.jsonc",
            "encoded:/home/u/.config/./eidnara/eidnara.jsonc",
            "/home/u/.config/eidnara//eidnara.jsonc",
        ];
        for (const path of daemonPaths) {
            expect(foldAuthorityStatus(plugin(), summary("eidnara", path)).warnings).toEqual([]);
        }
        expect(
            foldAuthorityStatus(
                plugin({ userConfigPath: "/home/u//.config/eidnara/eidnara.jsonc" }),
                summary("eidnara"),
            ).warnings,
        ).toEqual([]);
    });

    it("reads the configuration on disk only for the surfaces that show it", () => {
        let reads = 0;
        const counted = plugin({
            readDisk: () => {
                reads += 1;
                return { kind: "native", reason: "no summarizer model is configured" };
            },
        });
        const status = foldAuthorityStatus(counted, summary("eidnara"));
        expect(reads).toBe(0);
        expect(status.disk).toBeUndefined();
        expect(formatFoldAuthorityLines(status).some((line) => line.startsWith("- On disk"))).toBe(
            false,
        );
        expect(withDiskAuthority(status, counted).disk).toBe(
            "OpenCode's native compaction folds (no summarizer model is configured)",
        );
        expect(reads).toBe(1);
    });

    it("shows the disk, startup, and applied authorities with their sources", () => {
        const withNativeDisk = plugin({
            readDisk: () => ({ kind: "native", reason: "no summarizer model is configured" }),
        });
        const status = withDiskAuthority(
            foldAuthorityStatus(
                withNativeDisk,
                summary(
                    "eidnara; fold authority pending native: it applies at the next bind while the session is quiescent",
                ),
            ),
            withNativeDisk,
        );
        expect(formatFoldAuthorityLines(status)).toEqual([
            "### Fold Authority",
            "- On disk: OpenCode's native compaction folds (no summarizer model is configured)",
            "- Plugin startup: Eidnara folds (summarizer chain: test/model)",
            "- Daemon applied (session.status): Eidnara folds",
            "- Pending: native (it applies at the next bind while the session is quiescent); keep other OpenCode instances on this session closed, then restart this one so it binds the session again",
            `- User config: daemon ${USER_CONFIG}, plugin ${USER_CONFIG}`,
            `- ⚠ ${status.warnings[0]}`,
        ]);
    });

    it("publishes a warn conflict that formats under the warning header, and a clear state", () => {
        const published: Array<[ConflictWarning | undefined, string]> = [];
        const publish = (warning: ConflictWarning | undefined, sessionId: string) =>
            published.push([warning, sessionId]);
        const status = reportFoldAuthority(
            plugin({ publish }),
            summary(
                "native; fold authority pending eidnara: another binding is open on this session",
            ),
            "ses_pending",
        );
        const [[warning]] = published as [[ConflictWarning, string]];
        expect(published).toEqual([
            [{ disposition: "warn", reasons: status.warnings, unresolved: [] }, "ses_pending"],
        ]);
        expect(formatConflictShort(warning).startsWith(CONFLICT_WARNING_HEADER)).toBe(true);
        reportFoldAuthority(plugin({ publish }), summary("eidnara"), "ses_pending");
        expect(published[1]).toEqual([undefined, "ses_pending"]);
    });
});

describe("publishOnChange", () => {
    const pending: ConflictWarning = {
        disposition: "warn",
        reasons: [`${AUTHORITY_PENDING_WARNING}. first`],
        unresolved: [],
    };
    const changed: ConflictWarning = {
        ...pending,
        reasons: [`${AUTHORITY_PENDING_WARNING}. second`],
    };

    it("forwards a session's first state and every change, and nothing else", () => {
        const delivered: Array<[ConflictWarning | undefined, string]> = [];
        const publish = publishOnChange(async (warning, sessionId) => {
            delivered.push([warning, sessionId]);
            return true;
        }, 8);

        publish(undefined, "ses_a");
        publish(undefined, "ses_a");
        publish(pending, "ses_a");
        publish(pending, "ses_a");
        publish(changed, "ses_a");
        publish(undefined, "ses_a");
        publish(pending, "ses_b");

        expect(delivered).toEqual([
            [undefined, "ses_a"],
            [pending, "ses_a"],
            [changed, "ses_a"],
            [undefined, "ses_a"],
            [pending, "ses_b"],
        ]);
    });

    it("forwards an unchanged state again after a delivery that failed or threw", async () => {
        const outcomes = [Promise.resolve(false), Promise.reject(new Error("prompt down"))];
        let delivered = 0;
        const publish = publishOnChange(() => {
            delivered += 1;
            return outcomes.shift() ?? Promise.resolve(true);
        }, 8);

        publish(pending, "ses_a");
        await Promise.resolve();
        publish(pending, "ses_a");
        await Promise.resolve();
        publish(pending, "ses_a");
        await Promise.resolve();
        publish(pending, "ses_a");

        expect(delivered).toBe(3);
    });
});

describe("pluginFoldAuthority", () => {
    const origXdg = process.env.XDG_CONFIG_HOME;
    const dirs: string[] = [];
    afterEach(() => {
        if (origXdg === undefined) delete process.env.XDG_CONFIG_HOME;
        else process.env.XDG_CONFIG_HOME = origXdg;
        for (const dir of dirs.splice(0)) rmSync(dir, { recursive: true, force: true });
    });

    it("keeps the startup authority while the disk authority follows the file", () => {
        const xdg = mkdtempSync(join(tmpdir(), "eidnara-fold-status-"));
        const project = mkdtempSync(join(tmpdir(), "eidnara-fold-project-"));
        dirs.push(xdg, project);
        process.env.XDG_CONFIG_HOME = xdg;
        const file = join(xdg, "eidnara", "eidnara.jsonc");
        mkdirSync(join(xdg, "eidnara"), { recursive: true });
        writeFileSync(file, JSON.stringify({ history_summarizer: { model: "prov/model" } }));

        const authority = pluginFoldAuthority(project, loadPluginConfigDetailed(project), () => {});
        writeFileSync(file, JSON.stringify({}));

        expect(authority.userConfigPath).toBe(file);
        expect(authority.startup.kind).toBe("eidnara");
        expect(authority.readDisk().kind).toBe("native");

        writeFileSync(file, JSON.stringify({ not_a_key: true }));
        const refused = authority.readDisk();
        expect(refused.kind).toBe("unresolved");
    });
});
