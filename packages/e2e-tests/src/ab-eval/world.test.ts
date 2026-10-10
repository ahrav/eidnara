import { describe, expect, it } from "bun:test";
import { buildWorld, grade, TIERS } from "./world";

describe("A/B world", () => {
    it("builds the same world from the same seed", () => {
        expect(JSON.stringify(buildWorld(11, "s"))).toBe(JSON.stringify(buildWorld(11, "s")));
        expect(JSON.stringify(buildWorld(11, "s"))).not.toBe(JSON.stringify(buildWorld(12, "s")));
    });

    for (const tier of Object.keys(TIERS).filter((name) => name !== "l")) {
        it(`probes every fact of tier ${tier} once, after its statement`, () => {
            const world = buildWorld(5, tier);
            const probes = world.sessions.flatMap((session) =>
                session.turns
                    .filter((turn) => turn.probe)
                    .map((turn) => ({
                        session: session.index,
                        turn: turn.index,
                        id: turn.probe?.factId,
                    })),
            );
            expect(probes.map((probe) => probe.id).sort()).toEqual(
                world.facts.map((fact) => fact.id).sort(),
            );
            for (const probe of probes) {
                const fact = world.facts.find((candidate) => candidate.id === probe.id);
                if (!fact || fact.statedSession < 0) continue;
                const stated = [fact.statedSession, fact.statedTurn];
                expect(
                    probe.session > (stated[0] as number) ||
                        (probe.session === stated[0] && probe.turn > (stated[1] as number)),
                ).toBe(true);
            }
        });
    }

    it("grades by whole-word match and separates stale and abstained answers", () => {
        const fact = {
            id: "update-1",
            kind: "update" as const,
            subject: "billing HTTP port",
            answer: "41234",
            stale: "51234",
            statedSession: 1,
            statedTurn: 4,
        };
        expect(grade(fact, "It is 41234 now.")).toBe("correct");
        expect(grade(fact, "It was 51234 before; now 41234.")).toBe("correct");
        expect(grade(fact, "51234")).toBe("stale");
        expect(grade(fact, "412345")).toBe("wrong");
        expect(grade(fact, "UNKNOWN")).toBe("abstained");
        const abstain = { ...fact, kind: "abstain" as const, answer: "UNKNOWN", stale: undefined };
        expect(grade(abstain, "UNKNOWN, we never settled it")).toBe("correct");
        expect(grade(abstain, "41234")).toBe("wrong");
    });
});
