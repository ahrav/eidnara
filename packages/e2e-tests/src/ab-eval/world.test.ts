import { describe, expect, it } from "bun:test";
import { buildWorld, grade, PROJECT_SIZES, TIERS } from "./world";

describe("A/B world", () => {
    it("builds the same world from the same seed", () => {
        expect(JSON.stringify(buildWorld(11, "s"))).toBe(JSON.stringify(buildWorld(11, "s")));
        expect(JSON.stringify(buildWorld(11, "s"))).not.toBe(JSON.stringify(buildWorld(12, "s")));
    });

    for (const tier of Object.keys(TIERS)) {
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

    for (const seed of [5, 21]) {
        for (const tier of Object.keys(TIERS)) {
            it(`asks each question of seed ${seed} tier ${tier} about one fact only`, () => {
                const world = buildWorld(seed, tier);
                const factByQuestion = new Map<string, string>();
                for (const session of world.sessions) {
                    for (const turn of session.turns) {
                        if (!turn.probe) continue;
                        const earlier = factByQuestion.get(turn.user);
                        expect(earlier ?? turn.probe.factId).toBe(turn.probe.factId);
                        factByQuestion.set(turn.user, turn.probe.factId);
                    }
                }
            });
        }
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

describe("A/B project sizes", () => {
    it("builds one source tree per component with distinct names at every size", () => {
        for (const [size, components] of Object.entries(PROJECT_SIZES)) {
            const world = buildWorld(3, "xs", components);
            const dirs = new Set(
                Object.keys(world.files)
                    .filter((path) => path.startsWith("src/") && !path.startsWith("src/shared/"))
                    .map((path) => path.split("/")[1]),
            );
            expect(dirs.size, size).toBe(components);
        }
    });
});
