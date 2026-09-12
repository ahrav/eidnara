import { afterEach, describe, expect, it } from "bun:test";
import {
    _resetKeepSubagentsForTesting,
    setKeepSubagents,
    shouldKeepSubagents,
} from "./keep-subagents";

afterEach(() => {
    _resetKeepSubagentsForTesting();
});

describe("keep-subagents flag", () => {
    it("#given default, true, false, and non-true values #then only strict true keeps sessions", () => {
        expect(shouldKeepSubagents()).toBe(false);
        setKeepSubagents(true);
        expect(shouldKeepSubagents()).toBe(true);
        setKeepSubagents(false);
        expect(shouldKeepSubagents()).toBe(false);
        setKeepSubagents(true);
        setKeepSubagents(undefined as unknown as boolean);
        expect(shouldKeepSubagents()).toBe(false);
    });
});
