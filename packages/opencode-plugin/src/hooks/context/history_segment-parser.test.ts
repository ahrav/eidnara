import { describe, expect, it } from "bun:test";
import { parseHistorySegmentOutput } from "./history_segment-parser";

describe("parseHistorySegmentOutput — v2 5-category facts", () => {
    it("parses each of the 5 world categories", () => {
        const parsed = parseHistorySegmentOutput(`
<output>
<history_segments>
<history_segment start="1" end="2" title="Setup" episode_type="infra" importance="40">
<p1>did setup</p1><p2>setup</p2><p3>setup</p3><p4/>
</history_segment>
</history_segments>
<facts>
<PROJECT_RULES>
* Always commit + build after every fix.
</PROJECT_RULES>
<ARCHITECTURE>
* Storage is a single SQLite DB.
</ARCHITECTURE>
<CONSTRAINTS>
* Provider rejects empty assistant messages.
</CONSTRAINTS>
<CONFIG_VALUES>
* execute_threshold defaults to 65.
</CONFIG_VALUES>
<NAMING>
* The helper is named requireTenantContext.
</NAMING>
</facts>
<meta>
<unprocessed_from>3</unprocessed_from>
</meta>
</output>`);

        const cats = parsed.facts.map((f) => f.category);
        expect(cats).toEqual([
            "PROJECT_RULES",
            "ARCHITECTURE",
            "CONSTRAINTS",
            "CONFIG_VALUES",
            "NAMING",
        ]);
        expect(parsed.facts[0]).toEqual({
            category: "PROJECT_RULES",
            content: "Always commit + build after every fix.",
        });
    });

    it("does NOT parse legacy 9-cat fact categories (they exited history_summarizer output)", () => {
        const parsed = parseHistorySegmentOutput(`
<output>
<facts>
<USER_DIRECTIVES>
* Keep the eidnara rename broad.
</USER_DIRECTIVES>
<WORKFLOW_RULES>
* Follow the project release checklist.
</WORKFLOW_RULES>
<ARCHITECTURE_DECISIONS>
* This should not parse as a fact.
</ARCHITECTURE_DECISIONS>
</facts>
</output>`);
        expect(parsed.facts).toEqual([]);
    });

    it("unescapes escaped XML in titles, history_segments, and facts", () => {
        const parsed = parseHistorySegmentOutput(`
<output>
<history_segments>
<history_segment start="5" end="6" title="Team&apos;s &quot;rules&quot;">Keep &lt;instruction&gt; blocks &amp; notes safe.</history_segment>
</history_segments>
<facts>
<PROJECT_RULES>
* Preserve Sam&apos;s decision &amp; keep &lt;eidnara&gt; wording.
</PROJECT_RULES>
</facts>
<meta>
<messages_processed>5-6</messages_processed>
</meta>
</output>`);

        expect(parsed.history_segments).toEqual([
            {
                startMessage: 5,
                endMessage: 6,
                title: `Team's "rules"`,
                content: "Keep <instruction> blocks & notes safe.",
                importance: undefined,
                episodeType: undefined,
            },
        ]);
        expect(parsed.facts).toContainEqual({
            category: "PROJECT_RULES",
            content: "Preserve Sam's decision & keep <eidnara> wording.",
        });
    });

    it("decodes one entity layer so an escaped entity stays literal text", () => {
        const parsed = parseHistorySegmentOutput(`
<output>
<history_segments>
<history_segment start="1" end="2" title="Prose about &amp;lt;">Write &amp;lt; for a literal &lt; and &amp;amp; for &amp;.</history_segment>
</history_segments>
<facts>
<PROJECT_RULES>
* Escape as &amp;quot; in attributes.
</PROJECT_RULES>
</facts>
</output>`);

        expect(parsed.history_segments[0].title).toBe("Prose about &lt;");
        expect(parsed.history_segments[0].content).toBe(
            "Write &lt; for a literal < and &amp; for &.",
        );
        expect(parsed.facts).toContainEqual({
            category: "PROJECT_RULES",
            content: "Escape as &quot; in attributes.",
        });
    });
});

describe("parseHistorySegmentOutput — v2 tiers/importance/episode_type", () => {
    it("extracts four tiers, importance, and episode_type", () => {
        const parsed = parseHistorySegmentOutput(`
<output>
<history_segments>
<history_segment start="10" end="20" title="Tiered" episode_type="design,feature" importance="88">
<p1>full narrative with U: line</p1>
<p2>condensed</p2>
<p3>outcome</p3>
<p4>anchorA; anchorB</p4>
</history_segment>
</history_segments>
</output>`);
        const c = parsed.history_segments[0];
        expect(c.importance).toBe(88);
        expect(c.episodeType).toBe("design,feature");
        expect(c.p1).toBe("full narrative with U: line");
        expect(c.p2).toBe("condensed");
        expect(c.p3).toBe("outcome");
        expect(c.p4).toBe("anchorA; anchorB");
        expect(c.content).toBe("full narrative with U: line"); // mirrors P1
    });
});

describe("parseHistorySegmentOutput — events (v2, stored not rendered)", () => {
    it("parses causal_incident and trajectory_correction kind-agnostically", () => {
        const parsed = parseHistorySegmentOutput(`
<output>
<history_segments>
<history_segment start="1" end="9" title="x" importance="50"><p1>a</p1><p2>b</p2><p3>c</p3><p4/></history_segment>
</history_segments>
<events>
<causal_incident at_history_segment="1">
<summary>thing broke</summary>
<affected_surface>module X</affected_surface>
<symptom>500s</symptom>
<cause_summary>missing guard</cause_summary>
<disposition>fixed</disposition>
<evidence>logs</evidence>
<fix_summary>added guard</fix_summary>
</causal_incident>
<trajectory_correction at_history_segment="1">
<summary>pivoted approach</summary>
<before_strategy>old way</before_strategy>
<correction_source>user</correction_source>
<correction_signal>U: "do it differently"</correction_signal>
<after_strategy>new way</after_strategy>
<evidence>final impl</evidence>
</trajectory_correction>
</events>
</output>`);

        expect(parsed.events).toHaveLength(2);
        const [incident, correction] = parsed.events;
        expect(incident.kind).toBe("causal_incident");
        expect(incident.atHistorySegment).toBe(1);
        expect(incident.fields.summary).toBe("thing broke");
        expect(incident.fields.disposition).toBe("fixed");
        expect(incident.fields.fix_summary).toBe("added guard");
        expect(correction.kind).toBe("trajectory_correction");
        expect(correction.fields.before_strategy).toBe("old way");
        expect(correction.fields.correction_signal).toBe('U: "do it differently"');
    });

    it("anchors at_history_segment as a 1-based index into the EMITTED history_segment list (discard-last contract)", () => {
        // `at_history_segment` must index emitted history_segments from 1 for the discard-last filter.
        const parsed = parseHistorySegmentOutput(`
<output>
<history_segments>
<history_segment start="1" end="5" title="first" importance="50"><p1>a</p1><p2>b</p2><p3>c</p3><p4/></history_segment>
<history_segment start="6" end="9" title="second (provisional tail)" importance="50"><p1>a</p1><p2>b</p2><p3>c</p3><p4/></history_segment>
</history_segments>
<events>
<causal_incident at_history_segment="1">
<summary>anchored to the first (kept) history_segment</summary>
<disposition>fixed</disposition>
</causal_incident>
<causal_incident at_history_segment="2">
<summary>anchored to the second (discarded tail) history_segment</summary>
<disposition>fixed</disposition>
</causal_incident>
</events>
</output>`);
        expect(parsed.events).toHaveLength(2);
        expect(parsed.events[0].atHistorySegment).toBe(1);
        expect(parsed.events[1].atHistorySegment).toBe(2);

        const persistedLength = 1;
        const publishable = parsed.events.filter(
            (e) => e.atHistorySegment == null || e.atHistorySegment <= persistedLength,
        );
        expect(publishable).toHaveLength(1);
        expect(publishable[0].fields.summary).toContain("first");
    });

    it("does not mis-read fact categories or history_segments as events", () => {
        const parsed = parseHistorySegmentOutput(`
<output>
<history_segments>
<history_segment start="1" end="2" title="x" importance="50"><p1>a</p1><p2>b</p2><p3>c</p3><p4/></history_segment>
</history_segments>
<facts>
<PROJECT_RULES>
* a rule
</PROJECT_RULES>
</facts>
</output>`);
        expect(parsed.events).toEqual([]);
        expect(parsed.facts).toHaveLength(1);
    });
});

describe("parseHistorySegmentOutput — user_observations", () => {
    it("parses observation bullets", () => {
        const parsed = parseHistorySegmentOutput(`
<output>
<user_observations>
* User prefers evidence-backed root-cause analysis.
* User dislikes low-value config knobs.
</user_observations>
</output>`);
        expect(parsed.userObservations).toEqual([
            "User prefers evidence-backed root-cause analysis.",
            "User dislikes low-value config knobs.",
        ]);
    });
});

describe("parseHistorySegmentOutput — primer_candidates", () => {
    it("parses optional primer candidate questions", () => {
        const parsed = parseHistorySegmentOutput(`
<output>
<history_segments>
<history_segment start="1" end="2" title="cache" episode_type="debug" importance="50">
<p1>Cache work.</p1><p2>Cache.</p2><p3>Cache.</p3><p4>cache</p4>
</history_segment>
</history_segments>
<primer_candidates>
* How does prompt caching work?
- How does the materialization cache avoid busts?
</primer_candidates>
<meta><messages_processed>1-2</messages_processed><unprocessed_from>3</unprocessed_from></meta>
</output>`);

        expect(parsed.primerCandidates.map((candidate) => candidate.question)).toEqual([
            "How does prompt caching work?",
            "How does the materialization cache avoid busts?",
        ]);
        expect(
            parsed.primerCandidates.every((c) => c.originHistorySegmentIndex === undefined),
        ).toBe(true);
    });

    it("parses the origin-tagged <primer at_history_segment> form (1-based index, same as events)", () => {
        const parsed = parseHistorySegmentOutput(`
<output>
<history_segments>
<history_segment start="5" end="9" title="cache" episode_type="debug" importance="50">
<p1>Cache work.</p1><p2>Cache.</p2><p3>Cache.</p3><p4>cache</p4>
</history_segment>
</history_segments>
<primer_candidates>
<primer at_history_segment="1">How does the m[0]/m[1] cache split work?</primer>
</primer_candidates>
<meta><messages_processed>5-9</messages_processed><unprocessed_from>10</unprocessed_from></meta>
</output>`);

        // `at_history_segment` is a 1-based emitted-history_segment index, not a start ordinal.
        expect(parsed.primerCandidates).toEqual([
            { question: "How does the m[0]/m[1] cache split work?", originHistorySegmentIndex: 1 },
        ]);
    });
});

describe("parseHistorySegmentOutput — fact scoping (audit Fix 6)", () => {
    it("does NOT misread a category tag inside <events> as a promotable fact", () => {
        // A causal_incident's field text legitimately contains a 5-cat tag name.
        // Fact extraction must be scoped to <facts>, not the whole response.
        const parsed = parseHistorySegmentOutput(`
<output>
<facts>
<ARCHITECTURE>
* Real fact: storage is one SQLite DB.
</ARCHITECTURE>
</facts>
<events>
<causal_incident at_history_segment="1">
<finding>The provider enforced a CONSTRAINTS-style limit we must respect.</finding>
</causal_incident>
</events>
</output>`);
        // event field must not become a phantom CONSTRAINTS fact.
        expect(parsed.facts).toHaveLength(1);
        expect(parsed.facts[0].category).toBe("ARCHITECTURE");
        expect(parsed.facts.some((f) => f.category === "CONSTRAINTS")).toBe(false);
        expect(parsed.events).toHaveLength(1);
    });

    it("strips every <events> block before the fallback scan, not only the first", () => {
        // Fallback parsing accepts bare category blocks without a `<facts>` wrapper.
        const parsed = parseHistorySegmentOutput(`
<output>
<PROJECT_RULES>
* Follow the project release checklist.
</PROJECT_RULES>
<events>
<trajectory_correction at_history_segment="1">
<from>first block</from>
</trajectory_correction>
</events>
<events>
<causal_incident at_history_segment="1">
<PROJECT_RULES>
* phantom rule
</PROJECT_RULES>
</causal_incident>
</events>
</output>`);
        expect(parsed.facts).toEqual([
            { category: "PROJECT_RULES", content: "Follow the project release checklist." },
        ]);
    });
});

describe("parseHistorySegmentOutput — lenient tier closing (issue #246)", () => {
    it("parses a mismatched close (<p1>…</p2>) into the correct tiers", () => {
        // Mismatched tier closing tags occur in parser input.
        // The parser terminates `<p1>` at the next closing tier tag, regardless of its digit.
        // A mismatched `</p2>` closing `<p1>` must not suppress the actual `<p2>` tier.
        const parsed = parseHistorySegmentOutput(`
<output>
<history_segments>
<history_segment start="1" end="2" title="mangled" episode_type="bug" importance="55">
<p1>
the full p1 narrative
</p2>
<p2>the condensed p2</p2>
<p3>the outcome</p3>
<p4/>
</history_segment>
</history_segments>
</output>`);
        const c = parsed.history_segments[0];
        // A nonempty `<p1>` identifies a v2 tiered row with `legacy = 0`.
        expect(c.p1).toBe("the full p1 narrative");
        expect(c.content).toBe("the full p1 narrative"); // mirrors P1
        expect(c.p2).toBe("the condensed p2");
        expect(c.p3).toBe("the outcome");
        expect(c.p4).toBe("");
    });

    it("bounds an unterminated tier at the next opening tag (close omitted)", () => {
        const parsed = parseHistorySegmentOutput(`
<history_segment start="1" end="2" title="x" importance="50">
<p1>first tier body<p2>second tier body</p2><p3>third</p3><p4/>
</history_segment>`);
        const c = parsed.history_segments[0];
        expect(c.p1).toBe("first tier body");
        expect(c.p2).toBe("second tier body");
        expect(c.p3).toBe("third");
        expect(c.p4).toBe("");
    });

    it("over-capture guard never swallows a later tier's opener into an earlier body", () => {
        // A stray closing tag after the next opener does not extend the earlier tier's body.
        // The parser ends a tier's body at the next `<p\d>` opener, not at a later closing tag.
        const parsed = parseHistorySegmentOutput(`
<history_segment start="1" end="2" title="x" importance="50">
<p1>alpha<p2>beta</p1><p3>gamma</p3><p4/>
</history_segment>`);
        const c = parsed.history_segments[0];
        expect(c.p1).toBe("alpha");
        expect(c.p2).toBe("beta");
    });

    it("still honors the self-closing <p4/> arm", () => {
        const parsed = parseHistorySegmentOutput(`
<history_segment start="1" end="2" title="x" importance="50">
<p1>a</p1><p2>b</p2><p3>c</p3><p4 />
</history_segment>`);
        expect(parsed.history_segments[0].p4).toBe("");
    });

    it("leaves p1 undefined for genuinely tier-free flat output (still rejects downstream)", () => {
        const parsed = parseHistorySegmentOutput(`
<history_segment start="1" end="2" title="flat">just flat content, no tiers here</history_segment>`);
        const c = parsed.history_segments[0];
        expect(c.p1).toBeUndefined();
        expect(c.content).toBe("just flat content, no tiers here");
    });
});
