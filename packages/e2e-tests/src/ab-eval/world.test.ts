import { describe, expect, it } from "bun:test";
import { spawnSync } from "node:child_process";
import { mkdirSync, mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { buildWorld, grade, gradeOutcome, PROJECT_SIZES, TIERS } from "./world";

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
        // A probe whose turn failed is graded failed, whatever text arrived before the failure.
        expect(gradeOutcome(fact, { answer: "41234", error: "stream failed" })).toBe("failed");
        expect(gradeOutcome(fact, { answer: "41234" })).toBe("correct");
        // The question's own subject is not a guess, even when it looks like a codename.
        const sized = { ...abstain, subject: "rate-limiter-15 max_payload" };
        expect(grade(sized, "We never settled the rate-limiter-15 max payload size")).toBe(
            "correct",
        );
        expect(grade(sized, "We never settled rate-limiter-15; maybe amber-falcon-42")).toBe(
            "wrong",
        );
        // A hedge beside a guessed value is a guess.
        expect(grade(abstain, "I don't know; maybe 41234")).toBe("wrong");
        expect(grade(abstain, "Not sure, perhaps amber-falcon-42")).toBe("wrong");
    });
});

describe("A/B project sizes", () => {
    it("gives every work session exactly the tier's turn count", () => {
        for (const [seed, tier] of [
            [3, "xs"],
            [7, "xs"],
            [7, "s"],
            [21, "m"],
        ] as const) {
            const world = buildWorld(seed, tier);
            const expected = TIERS[tier]?.turnsPerSession;
            for (const session of world.sessions.slice(0, -1)) {
                const work = session.turns.filter((t) => t.probe?.scope !== "cross_session");
                expect([seed, tier, session.index, work.length]).toEqual([
                    seed,
                    tier,
                    session.index,
                    expected,
                ]);
            }
        }
    });

    it("refuses a component count the facts cannot draw from", () => {
        expect(() => buildWorld(7, "xs", 1)).toThrow(/componentCount/);
        expect(() => buildWorld(7, "xs", 2.5)).toThrow(/componentCount/);
    });

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

describe("repository setup", () => {
    it("commits under ambient Git signing and hook settings", () => {
        const root = mkdtempSync(join(tmpdir(), "ab-init-repo-"));
        try {
            // A global config that signs every commit with a program that fails, and runs hooks
            // from a directory whose pre-commit hook fails.
            const hooks = join(root, "hooks");
            mkdirSync(hooks);
            writeFileSync(join(hooks, "pre-commit"), "#!/bin/sh\nexit 1\n", { mode: 0o755 });
            writeFileSync(
                join(root, "gitconfig"),
                `[commit]\n\tgpgsign = true\n[gpg]\n\tprogram = /bin/false\n[core]\n\thooksPath = ${hooks}\n`,
            );
            const repo = join(root, "repo");
            mkdirSync(repo);
            writeFileSync(join(repo, "README.md"), "x\n");
            // Bun hands children the environment it started with, so the config applies in a child.
            const probe = `import { initRepo } from ${JSON.stringify(join(import.meta.dir, "world.ts"))}; initRepo(${JSON.stringify(repo)}); console.log("committed");`;
            const result = spawnSync(process.execPath, ["-e", probe], {
                encoding: "utf8",
                env: { ...process.env, GIT_CONFIG_GLOBAL: join(root, "gitconfig") },
            });
            expect(result.stdout.trim()).toBe("committed");
        } finally {
            rmSync(root, { recursive: true, force: true });
        }
    });
});
