import { afterEach, describe, expect, it } from "bun:test";
import { mkdirSync, mkdtempSync, rmSync, writeFileSync } from "node:fs";
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
    RESTART_OTHER_INSTANCES_STEP,
    ROOT_MISMATCH_WARNING,
    reportFoldAuthority,
} from "./fold-authority-status";

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
        raise: () => {},
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

    it("reads `none` as an unknown path and keeps a path with spaces whole", () => {
        expect(parseDaemonFoldAuthority(summary("native", "none")).userConfigPath).toBeUndefined();
        expect(
            parseDaemonFoldAuthority(summary("native", "/Users/a b/eidnara.jsonc")).userConfigPath,
        ).toBe("/Users/a b/eidnara.jsonc");
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

    it("names the next step for each pending reason", () => {
        const step = (reason: string) =>
            foldAuthorityStatus(
                plugin(),
                summary(`native; fold authority pending eidnara: ${reason}`),
            ).pending?.step;
        expect(step("another binding is open on this session")).toBe(RESTART_OTHER_INSTANCES_STEP);
        expect(step("the summarizer is busy or a publication is pending")).toBe(
            "wait for the summarizer to finish, then restart this OpenCode instance",
        );
        expect(step("it applies at the next bind while the session is quiescent")).toBe(
            "keep other OpenCode instances on this session closed, then restart this one so it binds the session again",
        );
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
            summary("eidnara", "/home/a%20%20b%3Bc/.config/eidnara/eidnara.jsonc"),
        );
        expect(same.daemon_user_config).toBe(spacedRoot);
        expect(same.warnings).toEqual([]);
        const distinct = foldAuthorityStatus(
            plugin({ userConfigPath: "/home/a b;c/.config/eidnara/eidnara.jsonc" }),
            summary("eidnara", "/home/a%20%20b%3Bc/.config/eidnara/eidnara.jsonc"),
        );
        expect(distinct.warnings[0]).toStartWith(ROOT_MISMATCH_WARNING);
    });

    it("shows the disk, startup, and applied authorities with their sources", () => {
        const status = foldAuthorityStatus(
            plugin({
                readDisk: () => ({ kind: "native", reason: "no summarizer model is configured" }),
            }),
            summary(
                "eidnara; fold authority pending native: it applies at the next bind while the session is quiescent",
            ),
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

    it("raises a warn conflict that formats under the warning header", () => {
        const raised: ConflictWarning[] = [];
        const status = reportFoldAuthority(
            plugin({ raise: (conflict) => raised.push(conflict) }),
            summary(
                "native; fold authority pending eidnara: another binding is open on this session",
            ),
        );
        expect(raised).toEqual([{ disposition: "warn", reasons: status.warnings, unresolved: [] }]);
        expect(formatConflictShort(raised[0]!).startsWith(CONFLICT_WARNING_HEADER)).toBe(true);
        const quiet: ConflictWarning[] = [];
        reportFoldAuthority(
            plugin({ raise: (conflict) => quiet.push(conflict) }),
            summary("eidnara"),
        );
        expect(quiet).toEqual([]);
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
