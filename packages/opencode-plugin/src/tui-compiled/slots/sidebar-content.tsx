import { createComponent as _$createComponent } from "opentui:runtime-module:%40opentui%2Fsolid";
import { createTextNode as _$createTextNode } from "opentui:runtime-module:%40opentui%2Fsolid";
import { effect as _$effect } from "opentui:runtime-module:%40opentui%2Fsolid";
import { insertNode as _$insertNode } from "opentui:runtime-module:%40opentui%2Fsolid";
import { memo as _$memo } from "opentui:runtime-module:%40opentui%2Fsolid";
import { insert as _$insert } from "opentui:runtime-module:%40opentui%2Fsolid";
import { setProp as _$setProp } from "opentui:runtime-module:%40opentui%2Fsolid";
import { createElement as _$createElement } from "opentui:runtime-module:%40opentui%2Fsolid";
/** @jsxImportSource @opentui/solid */

import { createEffect, createMemo, createSignal, For, on, onCleanup, Show } from "opentui:runtime-module:solid-js";
import packageJson from "../../../package.json";
import { formatThresholdPercent } from "../../shared/format-threshold";
import { formatMemoryCount, formatMemoryStatus } from "../../shared/rpc-types";
import { formatTailHygiene } from "../../shared/tail-hygiene-status";
import { computeEffectiveOrder, DEFAULT_SLOT_ORDER, PLUGIN_KEY, queueTuiPreferenceUpdate, readTuiPreferencesFile, readTuiPreferencesFileSync, resolveEidnaraPrefs, watchTuiPreferences } from "../../shared/tui-preferences";
import { badgeTextColor } from "../badge-contrast";
import { compactionOffSidebarRows, nativeCompactionContextLabel, nativeContextLimit } from "../compaction-off";
import { loadSidebarSnapshot } from "../data/session-rpc";

// External callers can trigger the mounted sidebar's recomp refresh.
// mounted SidebarContent registers its refresh here.
let activeRecompPollKick = null;
let activeSidebarRefresh = null;
export function kickRecompProgressRefresh() {
  activeRecompPollKick?.();
}

/** External callers can request an out-of-band sidebar status update. */
export function refreshSidebarSnapshot() {
  activeSidebarRefresh?.();
}
const SINGLE_BORDER = {
  type: "single"
};
const REFRESH_DEBOUNCE_MS = 150;
// The TUI may unmount and remount sidebar_content when the user switches views
// (main -> subagent -> main). A remount re-runs the component body, so a signal
// created inside the component would reset to its seed. The controller lives in
// the slot-factory closure (plugin/process lifetime) and owns the durable
// prefs/collapse signals plus the single shared file watcher, so collapse state
// and live pref reloads survive remounts. No Solid effects/memos here — those
// need an owner; the poll-interval effect stays inside the component.
function createSidebarController(initialPrefs) {
  const [prefs, setPrefs] = createSignal(initialPrefs);
  const seedCollapsed = initialPrefs.rememberCollapsed && initialPrefs.collapsed != null ? initialPrefs.collapsed : initialPrefs.startCollapsed;
  const [collapsed, setCollapsed] = createSignal(seedCollapsed);
  let lastPersistedCollapsed = initialPrefs.collapsed;
  let lastApplied = JSON.stringify(initialPrefs);

  // The watcher applies `next.collapsed` only when it differs from `lastPersistedCollapsed`, so unrelated preference reloads cannot overwrite an unpersisted click.
  const stopWatchingPreferences = watchTuiPreferences(() => {
    void (async () => {
      const next = resolveEidnaraPrefs(await readTuiPreferencesFile());
      const serialized = JSON.stringify(next);
      if (serialized === lastApplied) return;
      lastApplied = serialized;
      setPrefs(next);
      if (next.rememberCollapsed && next.collapsed != null && next.collapsed !== lastPersistedCollapsed) {
        lastPersistedCollapsed = next.collapsed;
        setCollapsed(next.collapsed);
      }
    })();
  });
  function toggleCollapsed() {
    const next = !collapsed();
    setCollapsed(next);
    if (prefs().rememberCollapsed) {
      void queueTuiPreferenceUpdate(PLUGIN_KEY, ["collapsed"], next).then(() => {
        lastPersistedCollapsed = next;
      });
    }
  }
  return {
    prefs,
    collapsed,
    toggleCollapsed,
    dispose: stopWatchingPreferences
  };
}
function compactTokens(value) {
  if (value >= 1_000_000) return `${(value / 1_000_000).toFixed(1)}M`;
  if (value >= 1_000) return `${(value / 1_000).toFixed(0)}K`;
  return String(value);
}
function relativeTime(ms) {
  const diff = Date.now() - ms;
  if (diff < 60_000) return "just now";
  if (diff < 3_600_000) return `${Math.floor(diff / 60_000)}m ago`;
  if (diff < 86_400_000) return `${Math.floor(diff / 3_600_000)}h ago`;
  return `${Math.floor(diff / 86_400_000)}d ago`;
}
function progressBar(fraction, width = 14) {
  const clamped = Math.max(0, Math.min(1, fraction));
  const filled = Math.round(clamped * width);
  return `[${"█".repeat(filled)}${"░".repeat(width - filled)}]`;
}
const COLORS = {
  // Plugin-injected structured traffic occupies `message[0]`.
  system: "#c084fc",
  // Purple
  docs: "#22d3ee",
  // Cyan — <project-docs>
  history_segments: "#60a5fa",
  // Blue
  facts: "#fbbf24",
  // Yellow/orange
  memories: "#34d399",
  // Green
  profile: "#a3e635",
  // Lime — <user-profile>
  conversation: "#f87171",
  // Red
  toolCalls: "#fb923c",
  // Orange
  toolDefs: "#f472b6" // Pink
};
const TokenBreakdown = props => {
  // OpenTUI uses `flexGrow` with `flexBasis={0}` to fill the sidebar proportionally.
  const segments = createMemo(() => {
    const s = props.snapshot;
    const result = [];
    if (s.systemPromptTokens > 0) {
      result.push({
        tokens: s.systemPromptTokens,
        color: COLORS.system,
        label: "System"
      });
    }

    // Docs represents the injected `<project-docs>` block.
    if (s.docsTokens > 0) {
      result.push({
        tokens: s.docsTokens,
        color: COLORS.docs,
        label: "Docs"
      });
    }

    // HistorySegments (blue)
    if (s.compaction_enabled !== false && s.history_segmentTokens > 0) {
      result.push({
        tokens: s.history_segmentTokens,
        color: COLORS.history_segments,
        label: "HistorySegments"
      });
    }

    // Facts (yellow/orange)
    if (s.factTokens > 0) {
      result.push({
        tokens: s.factTokens,
        color: COLORS.facts,
        label: "Facts"
      });
    }

    // Memories (green)
    if (s.memoryTokens > 0) {
      result.push({
        tokens: s.memoryTokens,
        color: COLORS.memories,
        label: "Memories"
      });
    }

    // The injected `<user-profile>` block contains promoted user memories.
    if (s.profileTokens > 0) {
      result.push({
        tokens: s.profileTokens,
        color: COLORS.profile,
        label: "User Profile"
      });
    }

    // `Conversation` contains user and assistant text, reasoning, and images.
    // `Conversation` excludes injected session history and tool-call I/O.
    //
    // The `Conversation` row remains visible when its token count is zero.
    result.push({
      tokens: s.conversationTokens,
      color: COLORS.conversation,
      label: "Conversation*"
    });

    // `Tool Calls` includes `tool_use`, `tool_result`, `tool`, and `tool-invocation` message parts.
    if (s.toolCallTokens > 0) {
      result.push({
        tokens: s.toolCallTokens,
        color: COLORS.toolCalls,
        label: "Tool Calls"
      });
    }

    // `Tool Definitions` measures tool descriptions and JSON-schema parameters.
    // OpenCode sends each tool in the `tools` request parameter.
    // The `tool.definition` plugin hook records definitions by `{provider, model, agent}`.
    // `toolDefinitionTokens` remains zero until the first turn measures the active agent's tool set.
    if (s.toolDefinitionTokens > 0) {
      result.push({
        tokens: s.toolDefinitionTokens,
        color: COLORS.toolDefs,
        label: "Tool Defs"
      });
    }
    return result;
  });
  const totalTokens = createMemo(() => props.snapshot.inputTokens || 1);

  // The legend retains zero-token segments to keep its rows stable.
  const barSegments = createMemo(() => segments().filter(seg => seg.tokens > 0));
  return (() => {
    var _el$ = _$createElement("box"),
      _el$2 = _$createElement("box");
    _$insertNode(_el$, _el$2);
    _$setProp(_el$, "width", "100%");
    _$setProp(_el$, "flexDirection", "column");
    _$setProp(_el$2, "width", "100%");
    _$setProp(_el$2, "flexDirection", "row");
    _$setProp(_el$2, "height", 1);
    _$insert(_el$2, () => barSegments().map(seg => (() => {
      var _el$3 = _$createElement("box");
      _$setProp(_el$3, "flexBasis", 0);
      _$setProp(_el$3, "height", 1);
      _$effect(_p$ => {
        var _v$ = Math.max(1, seg.tokens),
          _v$2 = seg.color;
        _v$ !== _p$.e && (_p$.e = _$setProp(_el$3, "flexGrow", _v$, _p$.e));
        _v$2 !== _p$.t && (_p$.t = _$setProp(_el$3, "backgroundColor", _v$2, _p$.t));
        return _p$;
      }, {
        e: undefined,
        t: undefined
      });
      return _el$3;
    })()));
    _$insert(_el$, (() => {
      var _c$ = _$memo(() => !!!props.collapsed);
      return () => _c$() && (() => {
        var _el$4 = _$createElement("box"),
          _el$5 = _$createElement("text");
        _$insertNode(_el$4, _el$5);
        _$setProp(_el$4, "flexDirection", "column");
        _$setProp(_el$4, "marginTop", 0);
        _$insert(_el$4, () => segments().map(seg => {
          const pct = (seg.tokens / totalTokens() * 100).toFixed(0);
          return (() => {
            var _el$7 = _$createElement("box"),
              _el$8 = _$createElement("text"),
              _el$9 = _$createElement("text"),
              _el$0 = _$createTextNode(` (`),
              _el$1 = _$createTextNode(`%)`);
            _$insertNode(_el$7, _el$8);
            _$insertNode(_el$7, _el$9);
            _$setProp(_el$7, "width", "100%");
            _$setProp(_el$7, "flexDirection", "row");
            _$setProp(_el$7, "justifyContent", "space-between");
            _$insert(_el$8, () => seg.label);
            _$insertNode(_el$9, _el$0);
            _$insertNode(_el$9, _el$1);
            _$insert(_el$9, () => compactTokens(seg.tokens), _el$0);
            _$insert(_el$9, pct, _el$1);
            _$effect(_p$ => {
              var _v$3 = seg.color,
                _v$4 = props.theme.textMuted;
              _v$3 !== _p$.e && (_p$.e = _$setProp(_el$8, "fg", _v$3, _p$.e));
              _v$4 !== _p$.t && (_p$.t = _$setProp(_el$9, "fg", _v$4, _p$.t));
              return _p$;
            }, {
              e: undefined,
              t: undefined
            });
            return _el$7;
          })();
        }), _el$5);
        _$insertNode(_el$5, _$createTextNode(`* includes Reasoning; hygiene excludes it`));
        _$effect(_$p => _$setProp(_el$5, "fg", props.theme.textMuted, _$p));
        return _el$4;
      })();
    })(), null);
    return _el$;
  })();
};
const StatRow = props => {
  const fg = createMemo(() => {
    if (props.warning) return props.theme.warning;
    if (props.accent) return props.theme.accent;
    if (props.dim) return props.theme.textMuted;
    return props.theme.text;
  });
  return (() => {
    var _el$10 = _$createElement("box"),
      _el$11 = _$createElement("text"),
      _el$12 = _$createElement("text"),
      _el$13 = _$createElement("b");
    _$insertNode(_el$10, _el$11);
    _$insertNode(_el$10, _el$12);
    _$setProp(_el$10, "width", "100%");
    _$setProp(_el$10, "flexDirection", "row");
    _$setProp(_el$10, "justifyContent", "space-between");
    _$insert(_el$11, () => props.label);
    _$insertNode(_el$12, _el$13);
    _$insert(_el$13, () => props.value);
    _$effect(_p$ => {
      var _v$5 = props.theme.textMuted,
        _v$6 = fg();
      _v$5 !== _p$.e && (_p$.e = _$setProp(_el$11, "fg", _v$5, _p$.e));
      _v$6 !== _p$.t && (_p$.t = _$setProp(_el$12, "fg", _v$6, _p$.t));
      return _p$;
    }, {
      e: undefined,
      t: undefined
    });
    return _el$10;
  })();
};
const SectionHeader = props => (() => {
  var _el$14 = _$createElement("box"),
    _el$15 = _$createElement("text"),
    _el$16 = _$createElement("b");
  _$insertNode(_el$14, _el$15);
  _$setProp(_el$14, "width", "100%");
  _$setProp(_el$14, "marginTop", 1);
  _$insertNode(_el$15, _el$16);
  _$insert(_el$16, () => props.title);
  _$effect(_$p => _$setProp(_el$15, "fg", props.theme.text, _$p));
  return _el$14;
})();
const RecompProgressSection = props => {
  const phase = () => props.progress.phase;
  const fraction = () => props.progress.totalMessages > 0 ? props.progress.processedMessages / props.progress.totalMessages : 0;
  const pct = () => Math.round(fraction() * 100);
  const verb = () => props.progress.kind === "upgrade" ? "Upgrade" : props.progress.kind === "embed" ? "Embed" : props.progress.kind === "wrapup" ? "Wrapup" : "Recomp";
  const activeText = () => props.progress.kind === "upgrade" ? "upgrading ⟳" : props.progress.kind === "embed" ? "embedding ⟳" : props.progress.kind === "wrapup" ? "wrapping ⟳" : "comparting ⟳";
  const label = createMemo(() => {
    switch (props.progress.phase) {
      case "recomp":
        return {
          text: activeText(),
          color: props.theme.warning
        };
      case "migration":
        return {
          text: "Migrating memories ⟳",
          color: props.theme.warning
        };
      case "done":
        return {
          text: `✓ ${verb()} complete`,
          color: props.theme.success ?? props.theme.accent
        };
      case "skipped":
        return {
          text: "stopped",
          color: props.theme.textMuted
        };
      case "failed":
        return {
          text: `✗ ${verb()} failed`,
          color: props.theme.error
        };
    }
  });
  return [(() => {
    var _el$17 = _$createElement("box"),
      _el$18 = _$createElement("text"),
      _el$19 = _$createElement("b"),
      _el$20 = _$createElement("text");
    _$insertNode(_el$17, _el$18);
    _$insertNode(_el$17, _el$20);
    _$setProp(_el$17, "width", "100%");
    _$setProp(_el$17, "marginTop", 1);
    _$setProp(_el$17, "flexDirection", "row");
    _$setProp(_el$17, "justifyContent", "space-between");
    _$insertNode(_el$18, _el$19);
    _$insert(_el$19, verb);
    _$insert(_el$20, () => label().text);
    _$effect(_p$ => {
      var _v$7 = props.theme.text,
        _v$8 = label().color;
      _v$7 !== _p$.e && (_p$.e = _$setProp(_el$18, "fg", _v$7, _p$.e));
      _v$8 !== _p$.t && (_p$.t = _$setProp(_el$20, "fg", _v$8, _p$.t));
      return _p$;
    }, {
      e: undefined,
      t: undefined
    });
    return _el$17;
  })(), _$memo(() => _$memo(() => !!(phase() === "recomp" && props.progress.totalMessages > 0))() && (() => {
    var _el$21 = _$createElement("box"),
      _el$22 = _$createElement("text"),
      _el$23 = _$createElement("text"),
      _el$24 = _$createTextNode(`%`);
    _$insertNode(_el$21, _el$22);
    _$insertNode(_el$21, _el$23);
    _$setProp(_el$21, "width", "100%");
    _$setProp(_el$21, "flexDirection", "row");
    _$setProp(_el$21, "justifyContent", "space-between");
    _$insert(_el$22, () => progressBar(fraction()));
    _$insertNode(_el$23, _el$24);
    _$insert(_el$23, pct, _el$24);
    _$effect(_p$ => {
      var _v$9 = props.theme.accent,
        _v$0 = props.theme.textMuted;
      _v$9 !== _p$.e && (_p$.e = _$setProp(_el$22, "fg", _v$9, _p$.e));
      _v$0 !== _p$.t && (_p$.t = _$setProp(_el$23, "fg", _v$0, _p$.t));
      return _p$;
    }, {
      e: undefined,
      t: undefined
    });
    return _el$21;
  })()), _$memo(() => _$memo(() => !!((phase() === "recomp" || phase() === "migration") && props.progress.note))() && (() => {
    var _el$25 = _$createElement("text");
    _$insert(_el$25, () => props.progress.note);
    _$effect(_$p => _$setProp(_el$25, "fg", props.theme.textMuted, _$p));
    return _el$25;
  })()), _$memo(() => _$memo(() => !!(phase() === "recomp" && props.progress.kind !== "embed"))() && _$createComponent(StatRow, {
    get theme() {
      return props.theme;
    },
    label: "HistorySegments",
    get value() {
      return `${props.progress.history_segmentsCreated} (${props.progress.passCount} pass${props.progress.passCount === 1 ? "" : "es"})`;
    },
    dim: true
  })), _$memo(() => _$memo(() => !!(phase() === "recomp" && props.progress.kind === "embed"))() && _$createComponent(StatRow, {
    get theme() {
      return props.theme;
    },
    label: "HistorySegments",
    get value() {
      return `${props.progress.processedMessages}/${props.progress.totalMessages} embedded`;
    },
    dim: true
  })), _$memo(() => _$memo(() => !!((phase() === "failed" || phase() === "skipped") && props.progress.message))() && (() => {
    var _el$26 = _$createElement("text");
    _$insert(_el$26, () => props.progress.message);
    _$effect(_$p => _$setProp(_el$26, "fg", props.theme.textMuted, _$p));
    return _el$26;
  })())];
};
const SidebarContent = props => {
  const [snapshot, setSnapshot] = createSignal(null);
  const collapsed = props.controller.collapsed;
  const sections = () => props.controller.prefs().sections;
  const headerLabel = () => props.controller.prefs().header.label;
  let refreshTimer;
  let recompPollTimer;
  const RECOMP_POLL_MS = 1200;
  let recompActive = false;
  let recompSawPhase = false;
  let recompPollCount = 0;
  let recompConsecutiveAbsent = 0;
  let recompSessionId = null;
  let snapshotRequestSequence = 0;
  const RECOMP_PROBE_MAX = 12; // ~15s for the server's "Starting…" to land
  // FIRST absent-after-active.
  const RECOMP_ABSENT_GIVEUP = 40; // ~48s of continuous absence → stop
  const RECOMP_MAX_POLLS = 1500; // ~30min absolute safety cap

  const refresh = () => {
    const sid = props.sessionID();
    if (!sid) return;
    const sequence = ++snapshotRequestSequence;
    const directory = props.api.state.path.directory ?? "";
    void loadSidebarSnapshot(sid, directory).then(data => {
      if (props.sessionID() !== sid || sequence !== snapshotRequestSequence) return;
      setSnapshot(data);
      try {
        props.api.renderer.requestRender();
      } catch {}
      const phase = data?.recompProgress?.phase;
      if ((phase === "recomp" || phase === "migration") && !recompActive) {
        kickRecompPoll();
      } else if (recompActive && recompSessionId === sid) {
        scheduleRecompTick();
      }
    }).catch(() => {
      if (recompActive && recompSessionId === sid && props.sessionID() === sid) {
        scheduleRecompTick();
      }
    });
  };
  const scheduleRefresh = () => {
    if (refreshTimer) clearTimeout(refreshTimer);
    refreshTimer = setTimeout(() => {
      refreshTimer = undefined;
      refresh();
    }, REFRESH_DEBOUNCE_MS);
  };
  const stopRecompPoll = () => {
    recompActive = false;
    recompSessionId = null;
    snapshotRequestSequence += 1;
    if (recompPollTimer) clearTimeout(recompPollTimer);
    recompPollTimer = undefined;
  };
  const scheduleRecompTick = () => {
    if (!recompActive) return;
    if (recompPollTimer) clearTimeout(recompPollTimer);
    recompPollTimer = setTimeout(recompTick, RECOMP_POLL_MS);
  };
  function recompTick() {
    const sid = recompSessionId;
    if (!recompActive || !sid || props.sessionID() !== sid) {
      stopRecompPoll();
      return;
    }
    recompPollCount += 1;
    if (recompPollCount > RECOMP_MAX_POLLS) {
      stopRecompPoll();
      return;
    }
    const sequence = ++snapshotRequestSequence;
    const directory = props.api.state.path.directory ?? "";
    void loadSidebarSnapshot(sid, directory).then(data => {
      if (!recompActive || recompSessionId !== sid || props.sessionID() !== sid || sequence !== snapshotRequestSequence) return;
      const phase = data?.recompProgress?.phase;
      const prevProgress = snapshot()?.recompProgress;
      const merged = !phase && recompSawPhase && prevProgress ? {
        ...data,
        recompProgress: prevProgress
      } : data;
      setSnapshot(merged);
      try {
        props.api.renderer.requestRender();
      } catch {}
      if (phase === "recomp" || phase === "migration") {
        recompSawPhase = true;
        recompConsecutiveAbsent = 0;
        scheduleRecompTick();
      } else if (phase === "done" || phase === "failed" || phase === "skipped") {
        stopRecompPoll();
      } else {
        recompConsecutiveAbsent += 1;
        if (!recompSawPhase) {
          if (recompPollCount < RECOMP_PROBE_MAX) scheduleRecompTick();else {
            stopRecompPoll();
          }
        } else if (recompConsecutiveAbsent < RECOMP_ABSENT_GIVEUP) {
          scheduleRecompTick();
        } else {
          stopRecompPoll();
        }
      }
    }).catch(() => {
      if (recompActive && recompSessionId === sid && props.sessionID() === sid && sequence === snapshotRequestSequence) scheduleRecompTick();
    });
  }

  // The server emits "Starting…" immediately after it detects an active recomp.
  // The probe window covers the RPC race before the server emits the immediate "Starting…" entry.
  function kickRecompPoll() {
    const sid = props.sessionID();
    if (!sid) return;
    if (recompActive && recompSessionId === sid) return;
    stopRecompPoll();
    recompActive = true;
    recompSessionId = sid;
    recompSawPhase = false;
    recompPollCount = 0;
    recompConsecutiveAbsent = 0;
    recompTick();
  }
  activeRecompPollKick = kickRecompPoll;
  activeSidebarRefresh = refresh;
  onCleanup(() => {
    if (refreshTimer) clearTimeout(refreshTimer);
    stopRecompPoll();
    if (activeRecompPollKick === kickRecompPoll) activeRecompPollKick = null;
    if (activeSidebarRefresh === refresh) activeSidebarRefresh = null;
  });
  createEffect(on(props.sessionID, () => {
    stopRecompPoll();
    setSnapshot(null);
    refresh();
  }));
  createEffect(on(props.sessionID, sessionID => {
    const unsubs = [props.api.event.on("message.updated", event => {
      if (event.properties.info.sessionID !== sessionID) return;
      scheduleRefresh();
    }), props.api.event.on("session.updated", event => {
      if (event.properties.info.id !== sessionID) return;
      scheduleRefresh();
    }), props.api.event.on("message.removed", event => {
      if (event.properties.sessionID !== sessionID) return;
      scheduleRefresh();
    }),
    // Compaction drops the server's live usage and sticky snapshot; without a re-poll the pre-compaction counts stay on screen.
    props.api.event.on("session.compacted", event => {
      if (event.properties.sessionID !== sessionID) return;
      scheduleRefresh();
    })];
    onCleanup(() => {
      for (const unsub of unsubs) unsub();
    });
  }, {
    defer: false
  }));
  const s = createMemo(() => snapshot());
  const compactionOff = () => s()?.compaction_enabled === false;
  const contextSummaryColor = createMemo(() => {
    if (compactionOff()) return props.theme.accent;
    const usage = s()?.usagePercentage ?? 0;
    if (usage >= 80) return props.theme.error;
    if (usage >= 65) return props.theme.warning;
    return props.theme.accent;
  });
  return (() => {
    var _el$27 = _$createElement("box"),
      _el$28 = _$createElement("box"),
      _el$29 = _$createElement("box"),
      _el$30 = _$createElement("text"),
      _el$31 = _$createElement("b"),
      _el$32 = _$createElement("text"),
      _el$33 = _$createTextNode(`v`);
    _$insertNode(_el$27, _el$28);
    _$setProp(_el$27, "width", "100%");
    _$setProp(_el$27, "flexDirection", "column");
    _$setProp(_el$27, "border", SINGLE_BORDER);
    _$setProp(_el$27, "paddingTop", 1);
    _$setProp(_el$27, "paddingBottom", 1);
    _$setProp(_el$27, "paddingLeft", 1);
    _$setProp(_el$27, "paddingRight", 1);
    _$insertNode(_el$28, _el$29);
    _$insertNode(_el$28, _el$32);
    _$setProp(_el$28, "flexDirection", "row");
    _$setProp(_el$28, "justifyContent", "space-between");
    _$setProp(_el$28, "alignItems", "center");
    _$setProp(_el$28, "onMouseDown", () => props.controller.toggleCollapsed());
    _$insertNode(_el$29, _el$30);
    _$setProp(_el$29, "paddingLeft", 1);
    _$setProp(_el$29, "paddingRight", 1);
    _$insertNode(_el$30, _el$31);
    _$insert(_el$31, () => collapsed() ? "▶ " : "▼ ", null);
    _$insert(_el$31, headerLabel, null);
    _$insertNode(_el$32, _el$33);
    _$insert(_el$32, () => packageJson.version, null);
    _$insert(_el$27, (() => {
      var _c$2 = _$memo(() => !!s()?.lastTransformError);
      return () => _c$2() && (() => {
        var _el$34 = _$createElement("box"),
          _el$35 = _$createElement("text"),
          _el$36 = _$createTextNode(`⚠ `);
        _$insertNode(_el$34, _el$35);
        _$setProp(_el$34, "marginTop", 1);
        _$setProp(_el$34, "width", "100%");
        _$insertNode(_el$35, _el$36);
        _$insert(_el$35, () => s().lastTransformError, null);
        _$effect(_$p => _$setProp(_el$35, "fg", props.theme.error, _$p));
        return _el$34;
      })();
    })(), null);
    _$insert(_el$27, (() => {
      var _c$3 = _$memo(() => !!s()?.memory_classifierProgress);
      return () => _c$3() && (() => {
        var _el$37 = _$createElement("box"),
          _el$38 = _$createElement("text"),
          _el$39 = _$createTextNode(`MemoryClassifier `),
          _el$40 = _$createTextNode(`: `),
          _el$42 = _$createTextNode(`/`),
          _el$43 = _$createTextNode(` processed`);
        _$insertNode(_el$37, _el$38);
        _$setProp(_el$37, "marginTop", 1);
        _$setProp(_el$37, "width", "100%");
        _$insertNode(_el$38, _el$39);
        _$insertNode(_el$38, _el$40);
        _$insertNode(_el$38, _el$42);
        _$insertNode(_el$38, _el$43);
        _$insert(_el$38, () => s().memory_classifierProgress.task, _el$40);
        _$insert(_el$38, () => s().memory_classifierProgress.processed, _el$42);
        _$insert(_el$38, () => s().memory_classifierProgress.total, _el$43);
        _$effect(_$p => _$setProp(_el$38, "fg", props.theme.warning, _$p));
        return _el$37;
      })();
    })(), null);
    _$insert(_el$27, (() => {
      var _c$4 = _$memo(() => !!(s() && s().inputTokens > 0));
      return () => _c$4() && (() => {
        var _el$44 = _$createElement("box");
        _$setProp(_el$44, "flexDirection", "column");
        _$insert(_el$44, (() => {
          var _c$7 = _$memo(() => (s()?.contextLimit ?? 0) > 0);
          return () => _c$7() && (() => {
            var _el$45 = _$createElement("box"),
              _el$46 = _$createElement("text"),
              _el$47 = _$createTextNode(` / `);
            _$insertNode(_el$45, _el$46);
            _$setProp(_el$45, "width", "100%");
            _$setProp(_el$45, "flexDirection", "row");
            _$setProp(_el$45, "justifyContent", "space-between");
            _$insert(_el$45, (() => {
              var _c$0 = _$memo(() => !!compactionOff());
              return () => _c$0() ? (() => {
                var _el$49 = _$createElement("text"),
                  _el$50 = _$createElement("b");
                _$insertNode(_el$49, _el$50);
                _$insert(_el$50, () => nativeCompactionContextLabel(s()));
                _$effect(_$p => _$setProp(_el$49, "fg", contextSummaryColor(), _$p));
                return _el$49;
              })() : (() => {
                var _el$51 = _$createElement("text"),
                  _el$52 = _$createElement("b"),
                  _el$53 = _$createTextNode(`%`),
                  _el$54 = _$createTextNode(` / `),
                  _el$56 = _$createTextNode(`%`);
                _$insertNode(_el$51, _el$52);
                _$insertNode(_el$51, _el$54);
                _$insertNode(_el$51, _el$56);
                _$insertNode(_el$52, _el$53);
                _$insert(_el$52, () => s().usagePercentage.toFixed(1), _el$53);
                _$insert(_el$51, () => formatThresholdPercent(s().executeThreshold), _el$56);
                _$insert(_el$51, () => s().executeThresholdClamped ? "*" : "", null);
                _$effect(_$p => _$setProp(_el$51, "fg", contextSummaryColor(), _$p));
                return _el$51;
              })();
            })(), _el$46);
            _$insertNode(_el$46, _el$47);
            _$insert(_el$46, () => compactTokens(s().inputTokens), _el$47);
            _$insert(_el$46, () => compactTokens(compactionOff() ? nativeContextLimit(s()) : s().contextLimit), null);
            _$effect(_$p => _$setProp(_el$46, "fg", contextSummaryColor(), _$p));
            return _el$45;
          })();
        })(), null);
        _$insert(_el$44, _$createComponent(TokenBreakdown, {
          get theme() {
            return props.theme;
          },
          get snapshot() {
            return s();
          },
          get collapsed() {
            return collapsed();
          }
        }), null);
        _$insert(_el$44, (() => {
          var _c$8 = _$memo(() => !!!collapsed());
          return () => _c$8() && (() => {
            var _el$57 = _$createElement("text");
            _$insertNode(_el$57, _$createTextNode(`Conversation includes reasoning estimates; hygiene excludes reasoning.`));
            _$effect(_$p => _$setProp(_el$57, "fg", props.theme.textMuted, _$p));
            return _el$57;
          })();
        })(), null);
        _$insert(_el$44, (() => {
          var _c$9 = _$memo(() => s().tailHygiene !== undefined);
          return () => _c$9() && _$createComponent(StatRow, {
            get theme() {
              return props.theme;
            },
            label: "Hygiene",
            get value() {
              return formatTailHygiene(s().tailHygiene);
            },
            get warning() {
              return !s().tailHygiene.evaluable;
            }
          });
        })(), null);
        _$effect(_$p => _$setProp(_el$44, "marginTop", collapsed() ? 0 : 1, _$p));
        return _el$44;
      })();
    })(), null);
    _$insert(_el$27, (() => {
      var _c$5 = _$memo(() => !!collapsed());
      return () => _c$5() && (() => {
        var _el$59 = _$createElement("box");
        _$setProp(_el$59, "width", "100%");
        _$setProp(_el$59, "flexDirection", "column");
        _$insert(_el$59, (() => {
          var _c$1 = _$memo(() => !!compactionOff());
          return () => _c$1() ? compactionOffSidebarRows(s()).map(row => _$createComponent(StatRow, {
            get theme() {
              return props.theme;
            },
            get label() {
              return row.label;
            },
            get value() {
              return row.value;
            },
            get accent() {
              return row.label === "Memories";
            },
            get dim() {
              return row.label !== "Memories";
            }
          })) : [(() => {
            var _el$60 = _$createElement("box"),
              _el$61 = _$createElement("text");
            _$insertNode(_el$60, _el$61);
            _$setProp(_el$60, "width", "100%");
            _$setProp(_el$60, "flexDirection", "row");
            _$setProp(_el$60, "justifyContent", "space-between");
            _$insertNode(_el$61, _$createTextNode(`HistorySummarizer`));
            _$insert(_el$60, (() => {
              var _c$10 = _$memo(() => !!s()?.history_summarizerRunning);
              return () => _c$10() ? (() => {
                var _el$73 = _$createElement("text");
                _$insertNode(_el$73, _$createTextNode(`comparting ⟳`));
                _$effect(_$p => _$setProp(_el$73, "fg", props.theme.warning, _$p));
                return _el$73;
              })() : (() => {
                var _el$75 = _$createElement("text");
                _$insertNode(_el$75, _$createTextNode(`idle`));
                _$effect(_$p => _$setProp(_el$75, "fg", props.theme.textMuted, _$p));
                return _el$75;
              })();
            })(), null);
            _$effect(_$p => _$setProp(_el$61, "fg", props.theme.textMuted, _$p));
            return _el$60;
          })(), _$createComponent(Show, {
            get when() {
              return s()?.memory_classifierProgress;
            },
            children: progress => (() => {
              var _el$77 = _$createElement("box"),
                _el$78 = _$createElement("text"),
                _el$80 = _$createElement("text"),
                _el$81 = _$createTextNode(` `),
                _el$82 = _$createTextNode(`/`);
              _$insertNode(_el$77, _el$78);
              _$insertNode(_el$77, _el$80);
              _$setProp(_el$77, "width", "100%");
              _$setProp(_el$77, "flexDirection", "row");
              _$setProp(_el$77, "justifyContent", "space-between");
              _$insertNode(_el$78, _$createTextNode(`MemoryClassifier`));
              _$insertNode(_el$80, _el$81);
              _$insertNode(_el$80, _el$82);
              _$insert(_el$80, () => progress().task, _el$81);
              _$insert(_el$80, () => progress().processed, _el$82);
              _$insert(_el$80, () => progress().total, null);
              _$effect(_p$ => {
                var _v$17 = props.theme.textMuted,
                  _v$18 = props.theme.warning;
                _v$17 !== _p$.e && (_p$.e = _$setProp(_el$78, "fg", _v$17, _p$.e));
                _v$18 !== _p$.t && (_p$.t = _$setProp(_el$80, "fg", _v$18, _p$.t));
                return _p$;
              }, {
                e: undefined,
                t: undefined
              });
              return _el$77;
            })()
          }), (() => {
            var _el$63 = _$createElement("box"),
              _el$64 = _$createElement("text"),
              _el$66 = _$createElement("text");
            _$insertNode(_el$63, _el$64);
            _$insertNode(_el$63, _el$66);
            _$setProp(_el$63, "width", "100%");
            _$setProp(_el$63, "flexDirection", "row");
            _$setProp(_el$63, "justifyContent", "space-between");
            _$insertNode(_el$64, _$createTextNode(`Memories`));
            _$insert(_el$66, (() => {
              var _c$11 = _$memo(() => !!(s()?.memoryState && s()?.memoryState !== "available"));
              return () => _c$11() ? String(s().memoryState) : _$memo(() => (s()?.memoryBlockCount ?? 0) > 0)() ? `${s().memoryBlockCount}/${formatMemoryCount(s() ?? {
                memoryCount: 0
              })}` : formatMemoryCount(s() ?? {
                memoryCount: 0
              });
            })());
            _$effect(_p$ => {
              var _v$13 = props.theme.textMuted,
                _v$14 = s()?.memoryState && s()?.memoryState !== "available" ? props.theme.warning : props.theme.textMuted;
              _v$13 !== _p$.e && (_p$.e = _$setProp(_el$64, "fg", _v$13, _p$.e));
              _v$14 !== _p$.t && (_p$.t = _$setProp(_el$66, "fg", _v$14, _p$.t));
              return _p$;
            }, {
              e: undefined,
              t: undefined
            });
            return _el$63;
          })(), (() => {
            var _el$67 = _$createElement("box"),
              _el$68 = _$createElement("text"),
              _el$70 = _$createElement("text"),
              _el$71 = _$createTextNode(`C:`),
              _el$72 = _$createTextNode(` Q:`);
            _$insertNode(_el$67, _el$68);
            _$insertNode(_el$67, _el$70);
            _$setProp(_el$67, "width", "100%");
            _$setProp(_el$67, "flexDirection", "row");
            _$setProp(_el$67, "justifyContent", "space-between");
            _$insertNode(_el$68, _$createTextNode(`Status`));
            _$insertNode(_el$70, _el$71);
            _$insertNode(_el$70, _el$72);
            _$insert(_el$70, () => s()?.history_segmentCount ?? 0, _el$72);
            _$insert(_el$70, () => s()?.pendingOpsCount ?? 0, null);
            _$insert(_el$70, (() => {
              var _c$12 = _$memo(() => (s()?.sessionNoteCount ?? 0) > 0);
              return () => _c$12() ? ` N:${s().sessionNoteCount}` : "";
            })(), null);
            _$effect(_p$ => {
              var _v$15 = props.theme.textMuted,
                _v$16 = props.theme.textMuted;
              _v$15 !== _p$.e && (_p$.e = _$setProp(_el$68, "fg", _v$15, _p$.e));
              _v$16 !== _p$.t && (_p$.t = _$setProp(_el$70, "fg", _v$16, _p$.t));
              return _p$;
            }, {
              e: undefined,
              t: undefined
            });
            return _el$67;
          })(), _$createComponent(Show, {
            get when() {
              return s()?.recompProgress;
            },
            children: progress => _$createComponent(RecompProgressSection, {
              get theme() {
                return props.theme;
              },
              get progress() {
                return progress();
              }
            })
          })];
        })());
        return _el$59;
      })();
    })(), null);
    _$insert(_el$27, (() => {
      var _c$6 = _$memo(() => !!!collapsed());
      return () => _c$6() && [_$memo(() => _$memo(() => !!(!compactionOff() && sections().history_summarizer))() && [(() => {
        var _el$83 = _$createElement("box"),
          _el$84 = _$createElement("text"),
          _el$85 = _$createElement("b");
        _$insertNode(_el$83, _el$84);
        _$setProp(_el$83, "width", "100%");
        _$setProp(_el$83, "marginTop", 1);
        _$setProp(_el$83, "flexDirection", "row");
        _$setProp(_el$83, "justifyContent", "space-between");
        _$insertNode(_el$84, _el$85);
        _$insertNode(_el$85, _$createTextNode(`HistorySummarizer`));
        _$insert(_el$83, (() => {
          var _c$13 = _$memo(() => !!s()?.history_summarizerRunning);
          return () => _c$13() ? (() => {
            var _el$87 = _$createElement("text");
            _$insertNode(_el$87, _$createTextNode(`comparting ⟳`));
            _$effect(_$p => _$setProp(_el$87, "fg", props.theme.warning, _$p));
            return _el$87;
          })() : (() => {
            var _el$89 = _$createElement("text");
            _$insertNode(_el$89, _$createTextNode(`idle`));
            _$effect(_$p => _$setProp(_el$89, "fg", props.theme.textMuted, _$p));
            return _el$89;
          })();
        })(), null);
        _$effect(_$p => _$setProp(_el$84, "fg", props.theme.text, _$p));
        return _el$83;
      })(), _$createComponent(StatRow, {
        get theme() {
          return props.theme;
        },
        label: "HistorySegments",
        get value() {
          return String(s()?.history_segmentCount ?? 0);
        }
      }), _$createComponent(Show, {
        get when() {
          return s()?.recompProgress;
        },
        children: progress => _$createComponent(RecompProgressSection, {
          get theme() {
            return props.theme;
          },
          get progress() {
            return progress();
          }
        })
      })]), _$memo(() => _$memo(() => !!sections().memory)() && [_$createComponent(SectionHeader, {
        get theme() {
          return props.theme;
        },
        title: "Memory"
      }), _$memo(() => _$memo(() => !!compactionOff())() ? compactionOffSidebarRows(s()).filter(row => row.label === "Memories").map(row => _$createComponent(StatRow, {
        get theme() {
          return props.theme;
        },
        get label() {
          return row.label;
        },
        get value() {
          return row.value;
        },
        accent: true
      })) : [_$createComponent(StatRow, {
        get theme() {
          return props.theme;
        },
        label: "Memories",
        get value() {
          return formatMemoryStatus(s() ?? {
            memoryCount: 0,
            memoryState: null
          });
        },
        accent: true
      }), _$memo(() => _$memo(() => (s()?.memoryBlockCount ?? 0) > 0)() && _$createComponent(StatRow, {
        get theme() {
          return props.theme;
        },
        label: "Injected",
        get value() {
          return String(s().memoryBlockCount);
        },
        dim: true
      }))])]), _$memo(() => _$memo(() => !!(sections().status && (compactionOff() ? compactionOffSidebarRows(s()).some(row => row.label !== "Memories") : (s()?.pendingOpsCount ?? 0) > 0 || (s()?.sessionNoteCount ?? 0) > 0 || (s()?.readyConditionalNoteCount ?? 0) > 0)))() && [_$createComponent(SectionHeader, {
        get theme() {
          return props.theme;
        },
        title: "Status"
      }), _$memo(() => _$memo(() => !!compactionOff())() ? compactionOffSidebarRows(s()).filter(row => row.label !== "Memories").map(row => _$createComponent(StatRow, {
        get theme() {
          return props.theme;
        },
        get label() {
          return row.label;
        },
        get value() {
          return row.value;
        },
        dim: true
      })) : [_$memo(() => _$memo(() => (s()?.pendingOpsCount ?? 0) > 0)() && _$createComponent(StatRow, {
        get theme() {
          return props.theme;
        },
        label: "Queue",
        get value() {
          return `${s().pendingOpsCount} pending`;
        },
        warning: true
      })), _$memo(() => _$memo(() => (s()?.sessionNoteCount ?? 0) > 0)() && _$createComponent(StatRow, {
        get theme() {
          return props.theme;
        },
        label: "Notes",
        get value() {
          return String(s().sessionNoteCount);
        }
      })), _$memo(() => _$memo(() => (s()?.readyConditionalNoteCount ?? 0) > 0)() && _$createComponent(StatRow, {
        get theme() {
          return props.theme;
        },
        label: "Conditional Notes",
        get value() {
          return `${s().readyConditionalNoteCount} ready`;
        },
        accent: true
      }))])]), _$memo(() => _$memo(() => !!(sections().memory_classifier && (s()?.lastMemoryClassifierRunAt || s()?.memory_classifierProgress)))() && [_$createComponent(SectionHeader, {
        get theme() {
          return props.theme;
        },
        title: "MemoryClassifier"
      }), _$createComponent(Show, {
        get when() {
          return s()?.memory_classifierProgress;
        },
        children: progress => _$createComponent(StatRow, {
          get theme() {
            return props.theme;
          },
          label: "Current",
          get value() {
            return `${progress().task} ${progress().processed}/${progress().total}`;
          },
          warning: true
        })
      }), _$createComponent(Show, {
        get when() {
          return s()?.lastMemoryClassifierRunAt;
        },
        children: lastRunAt => _$createComponent(StatRow, {
          get theme() {
            return props.theme;
          },
          label: "Last run",
          get value() {
            return relativeTime(lastRunAt());
          },
          dim: true
        })
      }), _$createComponent(For, {
        get each() {
          return Object.entries(s()?.memory_classifierBacklog ?? {});
        },
        children: ([task, backlog]) => _$createComponent(StatRow, {
          get theme() {
            return props.theme;
          },
          label: task,
          get value() {
            return `${backlog.pending}/${backlog.total}`;
          },
          dim: true
        })
      })]), _$memo(() => _$memo(() => !!(sections().stats && s()?.totalInputTokens != null))() && [_$createComponent(SectionHeader, {
        get theme() {
          return props.theme;
        },
        title: "Stats"
      }), _$createComponent(StatRow, {
        get theme() {
          return props.theme;
        },
        label: "Total tokens",
        get value() {
          return compactTokens(s().totalInputTokens ?? 0);
        },
        dim: true
      })])];
    })(), null);
    _$effect(_p$ => {
      var _v$1 = props.theme.borderActive,
        _v$10 = props.theme.accent,
        _v$11 = badgeTextColor(props.theme.accent, props.theme.background),
        _v$12 = props.theme.textMuted;
      _v$1 !== _p$.e && (_p$.e = _$setProp(_el$27, "borderColor", _v$1, _p$.e));
      _v$10 !== _p$.t && (_p$.t = _$setProp(_el$29, "backgroundColor", _v$10, _p$.t));
      _v$11 !== _p$.a && (_p$.a = _$setProp(_el$30, "fg", _v$11, _p$.a));
      _v$12 !== _p$.o && (_p$.o = _$setProp(_el$32, "fg", _v$12, _p$.o));
      return _p$;
    }, {
      e: undefined,
      t: undefined,
      a: undefined,
      o: undefined
    });
    return _el$27;
  })();
};
export function createSidebarContentSlot(api) {
  const seedRoot = readTuiPreferencesFileSync();
  const controller = createSidebarController(resolveEidnaraPrefs(seedRoot));
  const effectiveOrder = computeEffectiveOrder(seedRoot, PLUGIN_KEY, DEFAULT_SLOT_ORDER);
  return {
    order: effectiveOrder,
    dispose: controller.dispose,
    slots: {
      sidebar_content: (ctx, value) => {
        const theme = createMemo(() => ctx.theme.current);
        return _$createComponent(SidebarContent, {
          api: api,
          sessionID: () => value.session_id,
          get theme() {
            return theme();
          },
          controller: controller
        });
      }
    }
  };
}