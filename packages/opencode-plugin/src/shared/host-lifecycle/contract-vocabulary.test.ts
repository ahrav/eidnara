import { describe, expect, test } from "bun:test";
import type { DaemonCommand } from "./contract-vocabulary";
import { VOCABULARY_SOURCES } from "./contract-vocabulary";

describe("contract vocabulary", () => {
    test("covers the thirteen literal unions the lifecycle types", () => {
        expect(VOCABULARY_SOURCES.map((source) => source.name)).toEqual([
            "cli.commands",
            "cli.states",
            "cli.check_ids",
            "cli.check_statuses",
            "cli.remediations",
            "cli.reasons.failing_by_precedence",
            "cli.reasons.non_failing",
            "cli.readiness_states.transport",
            "cli.readiness_states.storage",
            "cli.readiness_states.synapse",
            "cli.readiness_states.kernel",
            "harness_unavailable.reasons_by_precedence",
            "install_layouts",
        ]);
    });

    test.each(
        VOCABULARY_SOURCES.map((source) => [source.name, source] as const),
    )("%s equals its JSON array as a set and carries no duplicate", (_name, source) => {
        expect(new Set(source.tuple)).toEqual(new Set(source.json));
        expect(source.tuple.length).toBe(new Set(source.tuple).size);
    });

    test("precedence-ordered tuples keep the JSON order", () => {
        for (const source of VOCABULARY_SOURCES) {
            if (!source.name.endsWith("_by_precedence")) continue;
            expect([...source.tuple]).toEqual([...source.json]);
        }
    });

    test("a member is assignable and a non-member is a type error", () => {
        const command: DaemonCommand = "start";
        expect(command).toBe("start");
        // @ts-expect-error `probe` is not a daemon command.
        const rejected: DaemonCommand = "probe";
        expect(rejected).toBe("probe");
    });
});
