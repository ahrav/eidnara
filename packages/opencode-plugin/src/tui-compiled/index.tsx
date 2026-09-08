import { memo as _$memo } from "opentui:runtime-module:%40opentui%2Fsolid";
import { createTextNode as _$createTextNode } from "opentui:runtime-module:%40opentui%2Fsolid";
import { effect as _$effect } from "opentui:runtime-module:%40opentui%2Fsolid";
import { insertNode as _$insertNode } from "opentui:runtime-module:%40opentui%2Fsolid";
import { insert as _$insert } from "opentui:runtime-module:%40opentui%2Fsolid";
import { setProp as _$setProp } from "opentui:runtime-module:%40opentui%2Fsolid";
import { createElement as _$createElement } from "opentui:runtime-module:%40opentui%2Fsolid";
import { createComponent as _$createComponent } from "opentui:runtime-module:%40opentui%2Fsolid";
/** @jsxImportSource @opentui/solid */

import { createMemo } from "opentui:runtime-module:solid-js";
import packageJson from "../../package.json";
import { loadPluginConfig } from "../config";
import { isCompactionEnabled } from "../config/agent-disable";
import { detectConflicts, resolveCompactionForBoot } from "../shared/conflict-detector";
import { fixConflicts } from "../shared/conflict-fixer";
import { formatThresholdPercent } from "../shared/format-threshold";
import { formatMemoryCount } from "../shared/rpc-types";
import { formatTailHygiene } from "../shared/tail-hygiene-status";
import { formatWindowDerivationLine } from "../shared/window-geometry";
import { compactionOffSidebarRows, nativeCompactionContextLabel } from "./compaction-off";
import { startNotificationSocket, stopNotificationSocket } from "./data/notification-socket";
import { closeRpc, getRpcGeneration, initRpcClient, loadStatusDetail, loadToastDurationMs } from "./data/session-rpc";
import { createSidebarContentSlot, kickRecompProgressRefresh, refreshSidebarSnapshot } from "./slots/sidebar-content";
const DEFAULT_TOAST_DURATION_MS = 5000;
let unifiedToastDurationMs = DEFAULT_TOAST_DURATION_MS;
async function refreshToastDurationMs() {
  try {
    const resolved = await loadToastDurationMs();
    if (typeof resolved === "number" && Number.isFinite(resolved)) {
      unifiedToastDurationMs = resolved;
    }
  } catch {
    // The catch preserves the current value so later refreshes can retry.
  }
}
function getToastDurationMs() {
  return unifiedToastDurationMs;
}
function showToast(api, input) {
  const duration = typeof input.durationOverrideMs === "number" && Number.isFinite(input.durationOverrideMs) ? input.durationOverrideMs : getToastDurationMs();
  // A positive per-call override still shows a toast when toast_duration_ms is 0.
  if (!(duration > 0)) {
    return;
  }
  api.ui.toast({
    message: input.message,
    variant: input.variant,
    duration
  });
}
function showConflictDialog(api, directory, reasons, conflicts) {
  api.ui.dialog.replace(() => _$createComponent(api.ui.DialogConfirm, {
    title: "\u26A0\uFE0F Eidnara Disabled",
    get message() {
      return `${reasons.join("\n")}\n\nFix these conflicts automatically?`;
    },
    onConfirm: () => {
      // `fixConflicts` edits only existing files and lets `writeFileSync` errors escape, so both
      // an empty action list and a thrown error mean the conflict stands.
      let actions = [];
      let failure = null;
      try {
        actions = fixConflicts(directory, conflicts);
      } catch (error) {
        failure = error instanceof Error ? error.message : String(error);
      }
      // DialogConfirm calls dialog.clear() after onConfirm, so defer the next dialog
      setTimeout(() => {
        if (failure !== null || actions.length === 0) {
          const outcome = failure !== null ? `Editing the configuration failed: ${failure}\nEdits made before the failure were kept.` : "No configuration file could be edited, so nothing changed.";
          api.ui.dialog.replace(() => _$createComponent(api.ui.DialogAlert, {
            title: "\u26A0\uFE0F Eidnara Still Disabled",
            get message() {
              return `${outcome}\n\n${reasons.join("\n")}\n\nResolve these by hand (for native compaction, set compaction.auto and compaction.prune to false in opencode.json), then restart OpenCode.`;
            },
            onConfirm: () => {
              showToast(api, {
                message: "Eidnara remains disabled. Run: npx @eidnara/opencode@latest doctor",
                variant: "warning"
              });
            }
          }));
          return;
        }
        const actionSummary = actions.map(a => `• ${a}`).join("\n");
        api.ui.dialog.replace(() => _$createComponent(api.ui.DialogAlert, {
          title: "\u2705 Configuration Fixed",
          message: `${actionSummary}\n\nPlease restart OpenCode for changes to take effect.`,
          onConfirm: () => {
            showToast(api, {
              message: "Restart OpenCode to enable Eidnara",
              variant: "warning",
              durationOverrideMs: 10_000
            });
          }
        }));
      }, 50);
    },
    onCancel: () => {
      showToast(api, {
        message: "Eidnara remains disabled. Run: npx @eidnara/opencode@latest doctor",
        variant: "warning"
      });
    }
  }));
}
function fmt(n) {
  if (n >= 1_000_000) return `${(n / 1_000_000).toFixed(1)}M`;
  if (n >= 1_000) return `${Math.round(n / 1_000)}K`;
  return String(n);
}
function fmtBytes(n) {
  if (n >= 1_048_576) return `${(n / 1_048_576).toFixed(1)} MB`;
  if (n >= 1_024) return `${Math.round(n / 1_024)} KB`;
  return `${n} B`;
}
function relTime(ms) {
  const d = Date.now() - ms;
  if (d < 60_000) return "just now";
  if (d < 3_600_000) return `${Math.floor(d / 60_000)}m ago`;
  if (d < 86_400_000) return `${Math.floor(d / 3_600_000)}h ago`;
  return `${Math.floor(d / 86_400_000)}d ago`;
}
function getSessionId(api) {
  try {
    const route = api.route.current;
    if (route?.name === "session" && route.params?.sessionID) {
      return String(route.params.sessionID);
    }
  } catch {
    // ignore
  }
  return null;
}
const R = props => (() => {
  var _el$ = _$createElement("box"),
    _el$2 = _$createElement("text"),
    _el$3 = _$createElement("text");
  _$insertNode(_el$, _el$2);
  _$insertNode(_el$, _el$3);
  _$setProp(_el$, "width", "100%");
  _$setProp(_el$, "flexDirection", "row");
  _$setProp(_el$, "justifyContent", "space-between");
  _$insert(_el$2, () => props.l);
  _$insert(_el$3, () => props.v);
  _$effect(_p$ => {
    var _v$ = props.t.textMuted,
      _v$2 = props.fg ?? props.t.text;
    _v$ !== _p$.e && (_p$.e = _$setProp(_el$2, "fg", _v$, _p$.e));
    _v$2 !== _p$.t && (_p$.t = _$setProp(_el$3, "fg", _v$2, _p$.t));
    return _p$;
  }, {
    e: undefined,
    t: undefined
  });
  return _el$;
})();
const StatusDialog = props => {
  const theme = createMemo(() => props.api.theme.current);
  const t = () => theme();
  const s = () => props.s;
  const compactionOff = () => s().compaction_enabled === false;
  const contextLimit = () => s().contextLimit > 0 ? s().contextLimit : s().usagePercentage > 0 ? Math.round(s().inputTokens / (s().usagePercentage / 100)) : 0;
  const elapsed = () => s().lastResponseTime > 0 ? Date.now() - s().lastResponseTime : 0;
  const COLORS = {
    system: "#c084fc",
    docs: "#22d3ee",
    compartments: "#60a5fa",
    facts: "#fbbf24",
    memories: "#34d399",
    profile: "#a3e635",
    conversation: "#f87171",
    toolCalls: "#fb923c",
    toolDefs: "#f472b6"
  };
  const breakdownSegments = () => {
    const d = s();
    const total = d.inputTokens || 1;
    const segs = [];
    if (d.systemPromptTokens > 0) segs.push({
      label: "System",
      tokens: d.systemPromptTokens,
      color: COLORS.system
    });
    if (d.docsTokens > 0) segs.push({
      label: "Docs",
      tokens: d.docsTokens,
      color: COLORS.docs
    });
    if (!compactionOff() && d.compartmentTokens > 0) segs.push({
      label: "Compartments",
      tokens: d.compartmentTokens,
      color: COLORS.compartments,
      detail: `(${d.compartmentCount})`
    });
    if (d.factTokens > 0) segs.push({
      label: "Facts",
      tokens: d.factTokens,
      color: COLORS.facts
    });
    if (d.memoryTokens > 0) segs.push({
      label: "Memories",
      tokens: d.memoryTokens,
      color: COLORS.memories,
      detail: `(${d.memoryBlockCount})`
    });
    if (d.profileTokens > 0) segs.push({
      label: "User Profile",
      tokens: d.profileTokens,
      color: COLORS.profile
    });
    if (d.conversationTokens > 0) segs.push({
      label: "Conversation*",
      tokens: d.conversationTokens,
      color: COLORS.conversation
    });
    if (d.toolCallTokens > 0) segs.push({
      label: "Tool Calls",
      tokens: d.toolCallTokens,
      color: COLORS.toolCalls
    });
    if (d.toolDefinitionTokens > 0) segs.push({
      label: "Tool Defs",
      tokens: d.toolDefinitionTokens,
      color: COLORS.toolDefs
    });
    return {
      segs,
      total
    };
  };
  const barSegments = () => breakdownSegments().segs.filter(seg => seg.tokens > 0);
  return (() => {
    var _el$4 = _$createElement("box"),
      _el$5 = _$createElement("box"),
      _el$6 = _$createElement("text"),
      _el$7 = _$createElement("b"),
      _el$9 = _$createElement("text"),
      _el$0 = _$createTextNode(`v`),
      _el$1 = _$createElement("box"),
      _el$10 = _$createElement("text"),
      _el$11 = _$createTextNode(` / `),
      _el$12 = _$createTextNode(` tokens`),
      _el$13 = _$createElement("box"),
      _el$14 = _$createElement("box"),
      _el$15 = _$createElement("text"),
      _el$17 = _$createElement("box"),
      _el$18 = _$createElement("box"),
      _el$19 = _$createElement("text"),
      _el$20 = _$createElement("b"),
      _el$22 = _$createElement("box"),
      _el$23 = _$createElement("text");
    _$insertNode(_el$4, _el$5);
    _$insertNode(_el$4, _el$1);
    _$insertNode(_el$4, _el$13);
    _$insertNode(_el$4, _el$14);
    _$insertNode(_el$4, _el$17);
    _$insertNode(_el$4, _el$18);
    _$insertNode(_el$4, _el$22);
    _$setProp(_el$4, "flexDirection", "column");
    _$setProp(_el$4, "width", "100%");
    _$setProp(_el$4, "paddingLeft", 2);
    _$setProp(_el$4, "paddingRight", 2);
    _$setProp(_el$4, "paddingTop", 1);
    _$setProp(_el$4, "paddingBottom", 1);
    _$insertNode(_el$5, _el$6);
    _$insertNode(_el$5, _el$9);
    _$setProp(_el$5, "justifyContent", "center");
    _$setProp(_el$5, "width", "100%");
    _$setProp(_el$5, "marginBottom", 1);
    _$setProp(_el$5, "flexDirection", "row");
    _$setProp(_el$5, "gap", 2);
    _$insertNode(_el$6, _el$7);
    _$insertNode(_el$7, _$createTextNode(`⚡ Eidnara Status`));
    _$insertNode(_el$9, _el$0);
    _$insert(_el$9, () => packageJson.version, null);
    _$insertNode(_el$1, _el$10);
    _$setProp(_el$1, "flexDirection", "row");
    _$setProp(_el$1, "justifyContent", "space-between");
    _$setProp(_el$1, "width", "100%");
    _$insert(_el$1, (() => {
      var _c$ = _$memo(() => !!compactionOff());
      return () => _c$() ? (() => {
        var _el$25 = _$createElement("text"),
          _el$26 = _$createElement("b");
        _$insertNode(_el$25, _el$26);
        _$insert(_el$26, () => nativeCompactionContextLabel(s()));
        _$effect(_$p => _$setProp(_el$25, "fg", t().accent, _$p));
        return _el$25;
      })() : (() => {
        var _el$27 = _$createElement("text"),
          _el$28 = _$createElement("b"),
          _el$29 = _$createTextNode(`%`),
          _el$30 = _$createTextNode(` / `),
          _el$32 = _$createTextNode(`%`);
        _$insertNode(_el$27, _el$28);
        _$insertNode(_el$27, _el$30);
        _$insertNode(_el$27, _el$32);
        _$insertNode(_el$28, _el$29);
        _$insert(_el$28, () => s().usagePercentage.toFixed(1), _el$29);
        _$insert(_el$27, () => formatThresholdPercent(s().executeThreshold), _el$32);
        _$insert(_el$27, () => s().executeThresholdClamped ? "*" : "", null);
        _$effect(_$p => _$setProp(_el$27, "fg", s().usagePercentage >= 80 ? t().error : s().usagePercentage >= 65 ? t().warning : t().accent, _$p));
        return _el$27;
      })();
    })(), _el$10);
    _$insertNode(_el$10, _el$11);
    _$insertNode(_el$10, _el$12);
    _$insert(_el$10, () => fmt(s().inputTokens), _el$11);
    _$insert(_el$10, (() => {
      var _c$2 = _$memo(() => contextLimit() > 0);
      return () => _c$2() ? fmt(contextLimit()) : "?";
    })(), _el$12);
    _$insert(_el$4, (() => {
      var _c$3 = _$memo(() => !!s().windowGeometry);
      return () => _c$3() && (() => {
        var _el$33 = _$createElement("text");
        _$insert(_el$33, () => formatWindowDerivationLine(s().inputTokens, s().windowGeometry));
        _$effect(_$p => _$setProp(_el$33, "fg", t().textMuted, _$p));
        return _el$33;
      })();
    })(), _el$13);
    _$setProp(_el$13, "width", "100%");
    _$setProp(_el$13, "flexDirection", "row");
    _$setProp(_el$13, "height", 1);
    _$insert(_el$13, () => barSegments().map(seg => (() => {
      var _el$34 = _$createElement("box");
      _$setProp(_el$34, "flexBasis", 0);
      _$setProp(_el$34, "height", 1);
      _$effect(_p$ => {
        var _v$9 = Math.max(1, seg.tokens),
          _v$0 = seg.color;
        _v$9 !== _p$.e && (_p$.e = _$setProp(_el$34, "flexGrow", _v$9, _p$.e));
        _v$0 !== _p$.t && (_p$.t = _$setProp(_el$34, "backgroundColor", _v$0, _p$.t));
        return _p$;
      }, {
        e: undefined,
        t: undefined
      });
      return _el$34;
    })()));
    _$insertNode(_el$14, _el$15);
    _$setProp(_el$14, "flexDirection", "column");
    _$insert(_el$14, () => breakdownSegments().segs.map(seg => {
      const pct = (seg.tokens / breakdownSegments().total * 100).toFixed(1);
      return (() => {
        var _el$35 = _$createElement("box"),
          _el$36 = _$createElement("text"),
          _el$37 = _$createTextNode(` `),
          _el$38 = _$createElement("text"),
          _el$39 = _$createTextNode(` (`),
          _el$40 = _$createTextNode(`%)`);
        _$insertNode(_el$35, _el$36);
        _$insertNode(_el$35, _el$38);
        _$setProp(_el$35, "width", "100%");
        _$setProp(_el$35, "flexDirection", "row");
        _$setProp(_el$35, "justifyContent", "space-between");
        _$insertNode(_el$36, _el$37);
        _$insert(_el$36, () => seg.label, _el$37);
        _$insert(_el$36, () => seg.detail ?? "", null);
        _$insertNode(_el$38, _el$39);
        _$insertNode(_el$38, _el$40);
        _$insert(_el$38, () => fmt(seg.tokens), _el$39);
        _$insert(_el$38, pct, _el$40);
        _$effect(_p$ => {
          var _v$1 = seg.color,
            _v$10 = t().textMuted;
          _v$1 !== _p$.e && (_p$.e = _$setProp(_el$36, "fg", _v$1, _p$.e));
          _v$10 !== _p$.t && (_p$.t = _$setProp(_el$38, "fg", _v$10, _p$.t));
          return _p$;
        }, {
          e: undefined,
          t: undefined
        });
        return _el$35;
      })();
    }), _el$15);
    _$insertNode(_el$15, _$createTextNode(`* Conversation includes Reasoning; hygiene excludes it`));
    _$insert(_el$14, (() => {
      var _c$4 = _$memo(() => s().tailHygiene !== undefined);
      return () => _c$4() && _$createComponent(R, {
        get t() {
          return t();
        },
        l: "Hygiene",
        get v() {
          return formatTailHygiene(s().tailHygiene);
        },
        get fg() {
          return _$memo(() => !!s().tailHygiene.evaluable)() ? t().accent : t().warning;
        }
      });
    })(), null);
    _$insert(_el$4, (() => {
      var _c$5 = _$memo(() => !!(!compactionOff() && s().recompProgress));
      return () => _c$5() && (() => {
        const p = s().recompProgress;
        const verb = p.kind === "upgrade" ? "Upgrade" : p.kind === "embed" ? "Embed" : "Recomp";
        return (() => {
          var _el$41 = _$createElement("box"),
            _el$42 = _$createElement("text"),
            _el$43 = _$createElement("b");
          _$insertNode(_el$41, _el$42);
          _$setProp(_el$41, "marginTop", 1);
          _$setProp(_el$41, "width", "100%");
          _$setProp(_el$41, "flexDirection", "column");
          _$insertNode(_el$42, _el$43);
          _$insert(_el$43, verb);
          _$insert(_el$41, () => {
            if (p.phase === "recomp") {
              const frac = p.totalMessages > 0 ? p.processedMessages / p.totalMessages : 0;
              const width = 24;
              const filled = Math.round(Math.max(0, Math.min(1, frac)) * width);
              const bar = p.totalMessages > 0 ? `[${"█".repeat(filled)}${"░".repeat(width - filled)}]` : "(starting…)";
              const activeLabel = p.kind === "upgrade" ? "upgrading" : p.kind === "embed" ? "embedding" : "comparting";
              return [_$createComponent(R, {
                get t() {
                  return t();
                },
                l: activeLabel,
                get v() {
                  return _$memo(() => p.totalMessages > 0)() ? `${bar} ${Math.round(frac * 100)}%` : bar;
                },
                get fg() {
                  return t().warning;
                }
              }), _$memo(() => _$memo(() => !!p.note)() ? _$createComponent(R, {
                get t() {
                  return t();
                },
                l: "Status",
                get v() {
                  return p.note;
                },
                get fg() {
                  return t().textMuted;
                }
              }) : null), _$memo(() => _$memo(() => p.kind === "embed")() ? _$createComponent(R, {
                get t() {
                  return t();
                },
                l: "Compartments",
                get v() {
                  return `${p.processedMessages}/${p.totalMessages} embedded`;
                },
                get fg() {
                  return t().textMuted;
                }
              }) : _$createComponent(R, {
                get t() {
                  return t();
                },
                l: "Compartments",
                get v() {
                  return `${p.compartmentsCreated} (${p.passCount} pass${p.passCount === 1 ? "" : "es"})`;
                },
                get fg() {
                  return t().textMuted;
                }
              }))];
            }
            if (p.phase === "migration") return _$createComponent(R, {
              get t() {
                return t();
              },
              l: "Status",
              get v() {
                return p.note ?? "Migrating memories ⟳";
              },
              get fg() {
                return t().warning;
              }
            });
            if (p.phase === "done") return _$createComponent(R, {
              get t() {
                return t();
              },
              l: "Status",
              v: `✓ ${verb} complete`,
              get fg() {
                return t().accent;
              }
            });
            if (p.phase === "skipped") return _$createComponent(R, {
              get t() {
                return t();
              },
              l: "Status",
              get v() {
                return p.message ?? `${verb} stopped early`;
              },
              get fg() {
                return t().textMuted;
              }
            });
            return _$createComponent(R, {
              get t() {
                return t();
              },
              l: "Status",
              get v() {
                return `✗ ${verb} failed${p.message ? `: ${p.message}` : ""}`;
              },
              get fg() {
                return t().error;
              }
            });
          }, null);
          _$effect(_$p => _$setProp(_el$42, "fg", t().text, _$p));
          return _el$41;
        })();
      })();
    })(), _el$17);
    _$setProp(_el$17, "flexDirection", "row");
    _$setProp(_el$17, "width", "100%");
    _$setProp(_el$17, "marginTop", 1);
    _$setProp(_el$17, "gap", 4);
    _$insert(_el$17, (() => {
      var _c$6 = _$memo(() => !!compactionOff());
      return () => _c$6() ? (() => {
        var _el$44 = _$createElement("box"),
          _el$45 = _$createElement("text"),
          _el$46 = _$createElement("b");
        _$insertNode(_el$44, _el$45);
        _$setProp(_el$44, "flexDirection", "column");
        _$setProp(_el$44, "flexGrow", 1);
        _$setProp(_el$44, "flexBasis", 0);
        _$insertNode(_el$45, _el$46);
        _$insertNode(_el$46, _$createTextNode(`Knowledge`));
        _$insert(_el$44, () => compactionOffSidebarRows(s()).map(row => _$createComponent(R, {
          get t() {
            return t();
          },
          get l() {
            return row.label;
          },
          get v() {
            return row.value;
          },
          get fg() {
            return _$memo(() => row.label === "Memories")() ? t().accent : t().textMuted;
          }
        })), null);
        _$insert(_el$44, (() => {
          var _c$0 = _$memo(() => s().readySmartNoteCount > 0);
          return () => _c$0() && _$createComponent(R, {
            get t() {
              return t();
            },
            l: "Smart Notes",
            get v() {
              return `${s().readySmartNoteCount} ready`;
            },
            get fg() {
              return t().accent;
            }
          });
        })(), null);
        _$insert(_el$44, (() => {
          var _c$1 = _$memo(() => !!s().lastDreamerRunAt);
          return () => _c$1() && _$createComponent(R, {
            get t() {
              return t();
            },
            l: "Dreamer",
            get v() {
              return `last ${relTime(s().lastDreamerRunAt)}`;
            },
            get fg() {
              return t().textMuted;
            }
          });
        })(), null);
        _$effect(_$p => _$setProp(_el$45, "fg", t().text, _$p));
        return _el$44;
      })() : [(() => {
        var _el$48 = _$createElement("box"),
          _el$49 = _$createElement("text"),
          _el$50 = _$createElement("b"),
          _el$52 = _$createElement("box"),
          _el$53 = _$createElement("text"),
          _el$54 = _$createElement("b"),
          _el$56 = _$createElement("box"),
          _el$57 = _$createElement("text"),
          _el$58 = _$createElement("b"),
          _el$60 = _$createElement("box"),
          _el$61 = _$createElement("text"),
          _el$62 = _$createElement("b");
        _$insertNode(_el$48, _el$49);
        _$insertNode(_el$48, _el$52);
        _$insertNode(_el$48, _el$56);
        _$insertNode(_el$48, _el$60);
        _$setProp(_el$48, "flexDirection", "column");
        _$setProp(_el$48, "flexGrow", 1);
        _$setProp(_el$48, "flexBasis", 0);
        _$insertNode(_el$49, _el$50);
        _$insertNode(_el$50, _$createTextNode(`Tags`));
        _$insert(_el$48, _$createComponent(R, {
          get t() {
            return t();
          },
          l: "Active",
          get v() {
            return `${s().activeTags} (~${fmtBytes(s().activeBytes)})`;
          }
        }), _el$52);
        _$insert(_el$48, _$createComponent(R, {
          get t() {
            return t();
          },
          l: "Dropped",
          get v() {
            return String(s().droppedTags);
          }
        }), _el$52);
        _$insert(_el$48, _$createComponent(R, {
          get t() {
            return t();
          },
          l: "Total",
          get v() {
            return String(s().totalTags);
          },
          get fg() {
            return t().textMuted;
          }
        }), _el$52);
        _$insertNode(_el$52, _el$53);
        _$setProp(_el$52, "marginTop", 1);
        _$insertNode(_el$53, _el$54);
        _$insertNode(_el$54, _$createTextNode(`Pending Queue`));
        _$insert(_el$48, _$createComponent(R, {
          get t() {
            return t();
          },
          l: "Drops",
          get v() {
            return String(s().pendingOpsCount);
          },
          get fg() {
            return _$memo(() => s().pendingOpsCount > 0)() ? t().warning : t().textMuted;
          }
        }), _el$56);
        _$insertNode(_el$56, _el$57);
        _$setProp(_el$56, "marginTop", 1);
        _$insertNode(_el$57, _el$58);
        _$insertNode(_el$58, _$createTextNode(`Cache TTL`));
        _$insert(_el$48, _$createComponent(R, {
          get t() {
            return t();
          },
          l: "Configured",
          get v() {
            return s().cacheTtl;
          }
        }), _el$60);
        _$insert(_el$48, _$createComponent(R, {
          get t() {
            return t();
          },
          l: "Last response",
          get v() {
            return _$memo(() => s().lastResponseTime > 0)() ? `${Math.round(elapsed() / 1000)}s ago` : "never";
          }
        }), _el$60);
        _$insert(_el$48, _$createComponent(R, {
          get t() {
            return t();
          },
          l: "Remaining",
          get v() {
            return _$memo(() => !!s().cacheExpired)() ? "expired" : _$memo(() => !!s().cacheNeverExpires)() ? "never expires (always-warm lane)" : `${Math.round(s().cacheRemainingMs / 1000)}s`;
          },
          get fg() {
            return _$memo(() => !!s().cacheExpired)() ? t().warning : t().textMuted;
          }
        }), _el$60);
        _$insert(_el$48, _$createComponent(R, {
          get t() {
            return t();
          },
          l: "Auto-execute",
          get v() {
            return _$memo(() => !!s().cacheExpired)() ? "yes (expired)" : _$memo(() => !!s().cacheNeverExpires)() ? `at ≥${formatThresholdPercent(s().executeThreshold)}%` : `at TTL or ≥${formatThresholdPercent(s().executeThreshold)}%`;
          },
          get fg() {
            return t().textMuted;
          }
        }), _el$60);
        _$insertNode(_el$60, _el$61);
        _$setProp(_el$60, "marginTop", 1);
        _$insertNode(_el$61, _el$62);
        _$insertNode(_el$62, _$createTextNode(`Memory`));
        _$insert(_el$48, _$createComponent(R, {
          get t() {
            return t();
          },
          l: "Active",
          get v() {
            return _$memo(() => !!(s().memoryState && s().memoryState !== "available"))() ? String(s().memoryState) : formatMemoryCount(s());
          },
          get fg() {
            return _$memo(() => !!(s().memoryState && s().memoryState !== "available"))() ? t().warning : t().accent;
          }
        }), null);
        _$insert(_el$48, _$createComponent(R, {
          get t() {
            return t();
          },
          l: "Injected",
          get v() {
            return String(s().memoryBlockCount);
          },
          get fg() {
            return t().textMuted;
          }
        }), null);
        _$effect(_p$ => {
          var _v$11 = t().text,
            _v$12 = t().text,
            _v$13 = t().text,
            _v$14 = t().text;
          _v$11 !== _p$.e && (_p$.e = _$setProp(_el$49, "fg", _v$11, _p$.e));
          _v$12 !== _p$.t && (_p$.t = _$setProp(_el$53, "fg", _v$12, _p$.t));
          _v$13 !== _p$.a && (_p$.a = _$setProp(_el$57, "fg", _v$13, _p$.a));
          _v$14 !== _p$.o && (_p$.o = _$setProp(_el$61, "fg", _v$14, _p$.o));
          return _p$;
        }, {
          e: undefined,
          t: undefined,
          a: undefined,
          o: undefined
        });
        return _el$48;
      })(), (() => {
        var _el$64 = _$createElement("box"),
          _el$65 = _$createElement("text"),
          _el$66 = _$createElement("b"),
          _el$68 = _$createElement("box"),
          _el$69 = _$createElement("text"),
          _el$70 = _$createElement("b"),
          _el$72 = _$createElement("box"),
          _el$73 = _$createElement("text"),
          _el$74 = _$createElement("b");
        _$insertNode(_el$64, _el$65);
        _$insertNode(_el$64, _el$68);
        _$insertNode(_el$64, _el$72);
        _$setProp(_el$64, "flexDirection", "column");
        _$setProp(_el$64, "flexGrow", 1);
        _$setProp(_el$64, "flexBasis", 0);
        _$insertNode(_el$65, _el$66);
        _$insertNode(_el$66, _$createTextNode(`Reductions`));
        _$insert(_el$64, _$createComponent(R, {
          get t() {
            return t();
          },
          l: "Execute threshold",
          get v() {
            return `${formatThresholdPercent(s().executeThreshold)}%${s().executeThresholdClamped ? "*" : ""}`;
          }
        }), _el$68);
        _$insert(_el$64, _$createComponent(R, {
          get t() {
            return t();
          },
          l: "Last reduce anchor",
          get v() {
            return `${fmt(s().lastNudgeTokens)} tok`;
          }
        }), _el$68);
        _$insertNode(_el$68, _el$69);
        _$setProp(_el$68, "marginTop", 1);
        _$insertNode(_el$69, _el$70);
        _$insertNode(_el$70, _$createTextNode(`Context Details`));
        _$insert(_el$64, _$createComponent(R, {
          get t() {
            return t();
          },
          l: "Protected tags",
          get v() {
            return String(s().protectedTagCount);
          },
          get fg() {
            return t().textMuted;
          }
        }), _el$72);
        _$insert(_el$64, _$createComponent(R, {
          get t() {
            return t();
          },
          l: "Subagent",
          get v() {
            return s().isSubagent ? "yes" : "no";
          },
          get fg() {
            return t().textMuted;
          }
        }), _el$72);
        _$insertNode(_el$72, _el$73);
        _$setProp(_el$72, "marginTop", 1);
        _$insertNode(_el$73, _el$74);
        _$insertNode(_el$74, _$createTextNode(`History Compression`));
        _$insert(_el$64, (() => {
          var _c$10 = _$memo(() => typeof s().boundaryPresent === "boolean");
          return () => _c$10() && _$createComponent(R, {
            get t() {
              return t();
            },
            l: "Boundary",
            get v() {
              return s().boundaryPresent ? "present" : "absent";
            }
          });
        })(), null);
        _$insert(_el$64, (() => {
          var _c$11 = _$memo(() => s().coverageOrdinal !== undefined);
          return () => _c$11() && _$createComponent(R, {
            get t() {
              return t();
            },
            l: "Coverage ordinal",
            get v() {
              return _$memo(() => s().coverageOrdinal == null)() ? "none" : String(s().coverageOrdinal);
            }
          });
        })(), null);
        _$insert(_el$64, (() => {
          var _c$12 = _$memo(() => typeof s().boundaryPresent === "boolean");
          return () => _c$12() && _$createComponent(R, {
            get t() {
              return t();
            },
            l: "Compartments",
            get v() {
              return String(s().compartmentCount);
            }
          });
        })(), null);
        _$insert(_el$64, _$createComponent(R, {
          get t() {
            return t();
          },
          l: "History block",
          get v() {
            return `~${fmt(s().historyBlockTokens)} tok`;
          }
        }), null);
        _$insert(_el$64, (() => {
          var _c$13 = _$memo(() => s().compressionBudget != null);
          return () => _c$13() && _$createComponent(R, {
            get t() {
              return t();
            },
            l: "Budget",
            get v() {
              return `~${fmt(s().compressionBudget)} tok (${s().compressionUsage} used)`;
            }
          });
        })(), null);
        _$insert(_el$64, (() => {
          var _c$14 = _$memo(() => !!s().lastDreamerRunAt);
          return () => _c$14() && _$createComponent(R, {
            get t() {
              return t();
            },
            l: "Dreamer",
            get v() {
              return `last ${relTime(s().lastDreamerRunAt)}`;
            },
            get fg() {
              return t().textMuted;
            }
          });
        })(), null);
        _$effect(_p$ => {
          var _v$15 = t().text,
            _v$16 = t().text,
            _v$17 = t().text;
          _v$15 !== _p$.e && (_p$.e = _$setProp(_el$65, "fg", _v$15, _p$.e));
          _v$16 !== _p$.t && (_p$.t = _$setProp(_el$69, "fg", _v$16, _p$.t));
          _v$17 !== _p$.a && (_p$.a = _$setProp(_el$73, "fg", _v$17, _p$.a));
          return _p$;
        }, {
          e: undefined,
          t: undefined,
          a: undefined
        });
        return _el$64;
      })()];
    })());
    _$insert(_el$4, (() => {
      var _c$7 = _$memo(() => !!s().lastTransformError);
      return () => _c$7() && (() => {
        var _el$76 = _$createElement("box"),
          _el$77 = _$createElement("text"),
          _el$78 = _$createTextNode(`⚠ `);
        _$insertNode(_el$76, _el$77);
        _$setProp(_el$76, "marginTop", 1);
        _$setProp(_el$76, "width", "100%");
        _$insertNode(_el$77, _el$78);
        _$insert(_el$77, () => s().lastTransformError, null);
        _$effect(_$p => _$setProp(_el$77, "fg", t().error, _$p));
        return _el$76;
      })();
    })(), _el$18);
    _$insertNode(_el$18, _el$19);
    _$setProp(_el$18, "marginTop", 1);
    _$setProp(_el$18, "width", "100%");
    _$insertNode(_el$19, _el$20);
    _$insertNode(_el$20, _$createTextNode(`Logger`));
    _$insert(_el$18, _$createComponent(R, {
      get t() {
        return t();
      },
      l: "Swallowed writes",
      get v() {
        return String(s().loggerDiagnostics?.swallowedWriteCount ?? 0);
      },
      get fg() {
        return _$memo(() => (s().loggerDiagnostics?.swallowedWriteCount ?? 0) > 0)() ? t().error : t().textMuted;
      }
    }), null);
    _$insert(_el$18, (() => {
      var _c$8 = _$memo(() => !!s().loggerDiagnostics?.lastErrorMessage);
      return () => _c$8() && _$createComponent(R, {
        get t() {
          return t();
        },
        l: "Last error",
        get v() {
          return s().loggerDiagnostics?.lastErrorMessage ?? "";
        },
        get fg() {
          return t().error;
        }
      });
    })(), null);
    _$insert(_el$18, (() => {
      var _c$9 = _$memo(() => !!s().loggerDiagnostics?.lastErrorTime);
      return () => _c$9() && _$createComponent(R, {
        get t() {
          return t();
        },
        l: "Last error time",
        get v() {
          return s().loggerDiagnostics?.lastErrorTime ?? "";
        },
        get fg() {
          return t().textMuted;
        }
      });
    })(), null);
    _$insertNode(_el$22, _el$23);
    _$setProp(_el$22, "marginTop", 1);
    _$setProp(_el$22, "justifyContent", "flex-end");
    _$setProp(_el$22, "width", "100%");
    _$insertNode(_el$23, _$createTextNode(`Esc to close`));
    _$effect(_p$ => {
      var _v$3 = t().accent,
        _v$4 = t().textMuted,
        _v$5 = compactionOff() ? t().accent : s().usagePercentage >= 80 ? t().error : s().usagePercentage >= 65 ? t().warning : t().accent,
        _v$6 = t().textMuted,
        _v$7 = t().text,
        _v$8 = t().textMuted;
      _v$3 !== _p$.e && (_p$.e = _$setProp(_el$6, "fg", _v$3, _p$.e));
      _v$4 !== _p$.t && (_p$.t = _$setProp(_el$9, "fg", _v$4, _p$.t));
      _v$5 !== _p$.a && (_p$.a = _$setProp(_el$10, "fg", _v$5, _p$.a));
      _v$6 !== _p$.o && (_p$.o = _$setProp(_el$15, "fg", _v$6, _p$.o));
      _v$7 !== _p$.i && (_p$.i = _$setProp(_el$19, "fg", _v$7, _p$.i));
      _v$8 !== _p$.n && (_p$.n = _$setProp(_el$23, "fg", _v$8, _p$.n));
      return _p$;
    }, {
      e: undefined,
      t: undefined,
      a: undefined,
      o: undefined,
      i: undefined,
      n: undefined
    });
    return _el$4;
  })();
};
function getModelKeyFromMessages(api, sessionId) {
  try {
    const msgs = api.state.session.messages(sessionId);
    for (let i = msgs.length - 1; i >= 0; i--) {
      const msg = msgs[i];
      if (msg.role === "assistant" && msg.providerID && msg.modelID) {
        return `${msg.providerID}/${msg.modelID}`;
      }
      if (msg.role === "user") {
        const model = msg.model;
        if (model?.providerID && model?.modelID) {
          return `${model.providerID}/${model.modelID}`;
        }
      }
    }
  } catch {}
  return undefined;
}
async function showStatusDialog(api, targetSessionId = getSessionId(api)) {
  const sessionId = targetSessionId;
  if (!sessionId) {
    showToast(api, {
      message: "No active session",
      variant: "warning"
    });
    return false;
  }
  const directory = api.state.path.directory ?? "";
  const modelKey = getModelKeyFromMessages(api, sessionId);
  const detail = await loadStatusDetail(sessionId, directory, modelKey);
  if (getSessionId(api) !== sessionId) return false;

  // Resolve only after the dialog closes so callers queue subsequent dialogs afterward.
  return new Promise(resolve => {
    api.ui.dialog.replace(() => _$createComponent(StatusDialog, {
      api: api,
      s: detail
    }), () => resolve(true));
  });
}
function showResultDialog(api, title, message) {
  return new Promise(resolve => {
    api.ui.dialog.replace(() => _$createComponent(api.ui.DialogAlert, {
      title: title,
      message: message,
      onConfirm: () => {}
    }), () => resolve(true));
  });
}
function probeErrorMessage(error) {
  const message = error instanceof Error ? error.message : String(error);
  return message.replace(/\s+/g, " ").trim() || "unknown error";
}
function probeVersion(api) {
  try {
    const version = api.app?.version;
    return typeof version === "string" && version.length > 0 ? version : "unavailable";
  } catch {
    return "unavailable";
  }
}
function renderTuiProbeHostArm(api, result) {
  try {
    api.ui.dialog.replace(() => {
      try {
        const element = _$createComponent(api.ui.DialogAlert, {
          title: "Eidnara TUI probe: host arm",
          message: "Host-owned dialog probe is rendering. It will be replaced after 500ms.",
          onConfirm: () => {}
        });
        result.hostConstructed = true;
        return element;
      } catch (error) {
        result.hostThrew = probeErrorMessage(error);
        return null;
      }
    });
  } catch (error) {
    result.hostThrew ??= probeErrorMessage(error);
  }
}
function renderTuiProbeCustomArm(api, result) {
  try {
    api.ui.dialog.replace(() => {
      try {
        return (() => {
          var _el$79 = _$createElement("box"),
            _el$80 = _$createElement("text");
          _$insertNode(_el$79, _el$80);
          _$insertNode(_el$80, _$createTextNode(`probe`));
          return _el$79;
        })();
      } catch (error) {
        result.customThrew = probeErrorMessage(error);
        return null;
      }
    });
  } catch (error) {
    result.customThrew ??= probeErrorMessage(error);
  }
}
async function waitForTuiProbeHostPaint(api, result) {
  if (result.hostThrew !== null) {
    result.hostPainted = false;
    result.hostPaint = "not_reached_host_threw";
    return;
  }
  let renderer;
  try {
    renderer = api.renderer;
  } catch {}
  if (!renderer || typeof renderer.once !== "function") {
    await new Promise(resolve => setTimeout(resolve, 500));
    result.hostPainted = null;
    result.hostPaint = "no_frame_signal_after_500ms_visual_confirmation_required";
    return;
  }
  await new Promise(resolve => {
    let settled = false;
    const onFrame = () => {
      if (settled) return;
      settled = true;
      if (timer) clearTimeout(timer);
      renderer.removeListener?.("frame", onFrame);
      result.hostPainted = true;
      result.hostPaint = "observed_renderer_frame";
      resolve();
    };
    const timer = setTimeout(() => {
      if (settled) return;
      settled = true;
      renderer?.removeListener?.("frame", onFrame);
      result.hostPainted = null;
      result.hostPaint = "no_frame_after_500ms_visual_confirmation_required";
      resolve();
    }, 500);
    try {
      renderer.once?.("frame", onFrame);
    } catch (error) {
      if (settled) return;
      settled = true;
      clearTimeout(timer);
      renderer.removeListener?.("frame", onFrame);
      result.hostPainted = null;
      result.hostPaint = `frame_signal_error_${probeErrorMessage(error)}`;
      resolve();
    }
  });
}
function tuiProbeSummary(result) {
  return [`host_constructed=${String(result.hostConstructed)}`, `host_threw=${result.hostThrew ?? "false"}`, `custom_threw=${result.customThrew ?? "false"}`, `opencode_version=${result.opencodeVersion}`, `host_painted=${result.hostPainted === null ? "unknown" : String(result.hostPainted)}`, `host_paint=${result.hostPaint}`];
}
function reportTuiProbe(api, result) {
  const lines = tuiProbeSummary(result);
  for (const line of lines) {
    console.error(`[eidnara-probe] ${line}`);
  }
  const summary = lines.join("\n");
  if (result.customThrew === null) {
    try {
      api.ui.dialog.replace(() => (() => {
        var _el$82 = _$createElement("box"),
          _el$83 = _$createElement("text");
        _$insertNode(_el$82, _el$83);
        _$insert(_el$83, summary);
        return _el$82;
      })());
      return;
    } catch (error) {
      console.error(`[eidnara-probe] summary_custom_threw=${probeErrorMessage(error)}`);
    }
  }
  if (result.hostThrew === null) {
    try {
      api.ui.dialog.replace(() => _$createComponent(api.ui.DialogAlert, {
        title: "Eidnara TUI probe",
        message: summary,
        onConfirm: () => {}
      }));
      return;
    } catch (error) {
      console.error(`[eidnara-probe] summary_host_threw=${probeErrorMessage(error)}`);
    }
  }
  console.error("[eidnara-probe] summary_rendered=console_only");
}
async function runTuiProbe(api) {
  const result = {
    hostConstructed: false,
    hostThrew: null,
    customThrew: null,
    opencodeVersion: probeVersion(api),
    hostPainted: null,
    hostPaint: "not_checked"
  };
  renderTuiProbeHostArm(api, result);
  await waitForTuiProbeHostPaint(api, result);
  renderTuiProbeCustomArm(api, result);
  reportTuiProbe(api, result);
}

/**
 * Registers the palette entries through `api.keymap.registerLayer` and falls
 * back to `api.command.register` on hosts without a keymap layer API. Hosts
 * with neither get no palette entries; the server-registered `/ctx-*` slash
 * commands still reach the same dialogs through notifications.
 */
function registerCommandPaletteEntries(api) {
  const apiAny = api;
  if (typeof apiAny.keymap?.registerLayer === "function") {
    try {
      apiAny.keymap.registerLayer({
        commands: [{
          namespace: "palette",
          name: "eidnara.status",
          title: "Eidnara: Status",
          category: "Eidnara",
          run() {
            showStatusDialog(api);
          }
        }, {
          namespace: "palette",
          name: "ctx-tui-probe",
          title: "Eidnara: TUI Probe",
          category: "Eidnara",
          run() {
            void runTuiProbe(api);
          }
        }],
        bindings: []
      });
      return;
    } catch (err) {
      console.debug("[eidnara-tui] keymap.registerLayer threw; falling back to command.register", err);
    }
  }
  if (typeof apiAny.command?.register === "function") {
    apiAny.command.register(() => [{
      title: "Eidnara: Status",
      value: "eidnara.status",
      category: "Eidnara",
      onSelect() {
        showStatusDialog(api);
      }
    }, {
      title: "Eidnara: TUI Probe",
      value: "ctx-tui-probe",
      category: "Eidnara",
      onSelect() {
        void runTuiProbe(api);
      }
    }]);
    return;
  }
}
const tui = async (api, _options, meta) => {
  const directory = api.state.path.directory ?? "";
  // The TUI gates RPC discovery and socket startup so disabled installations perform no idle work.
  // `isCompactionEnabled` receives the loaded config to avoid deriving compaction mode from `directory` alone.
  // `pluginConfig` remains undefined after config-load failure, so `isCompactionEnabled` defaults to `true`.
  let pluginConfig;
  try {
    pluginConfig = loadPluginConfig(directory);
  } catch {}
  if (pluginConfig?.enabled === false) return;
  // `resolveCompactionForBoot` uses host-resolved config because the scanner treats missing config as enabled.
  const resolvedCompaction = await resolveCompactionForBoot(api.client);
  const conflictResult = detectConflicts(directory, {
    compactionEnabled: isCompactionEnabled(pluginConfig ?? {}),
    resolvedCompaction: resolvedCompaction ?? undefined
  });
  if (conflictResult.hasConflict) {
    showConflictDialog(api, directory, conflictResult.reasons, conflictResult.conflicts);
    return;
  }
  initRpcClient(directory);
  const sidebarSlot = createSidebarContentSlot(api);
  api.slots.register(sidebarSlot);

  // The server registers the `/ctx-*` slash commands and pushes dialog
  // requests over the notification socket; the palette entries below are the
  // TUI-side shortcuts to the same dialogs.
  registerCommandPaletteEntries(api);

  // The server pushes queued notifications over one persistent WebSocket.
  // A persistent WebSocket avoids the idle CPU cost of a 500 ms HTTP poll.
  // The socket includes the active session in its hello so the server scopes delivery.
  // The notification handler rechecks the active session because it can change between queueing and delivery.
  const handleNotification = async n => {
    const requestedSessionId = getSessionId(api);
    const generation = getRpcGeneration();
    // The notification handler returns `false` for another session so the notification remains unacknowledged.
    // gets it.
    if (n.sessionId !== undefined && n.sessionId !== requestedSessionId) {
      return false;
    }
    if (n.type === "toast") {
      const p = n.payload;
      showToast(api, {
        message: String(p.message ?? ""),
        variant: p.variant ?? "info",
        durationOverrideMs: typeof p.duration === "number" && Number.isFinite(p.duration) ? p.duration : undefined
      });
      return true;
    }
    if (n.type !== "action") return false;
    const action = n.payload?.action;
    const stillActive = () => getRpcGeneration() === generation && getSessionId(api) === requestedSessionId;
    if (action === "show-status-dialog") {
      return stillActive() && (await showStatusDialog(api, requestedSessionId));
    }
    if (action === "refresh-sidebar") {
      if (!stillActive()) return false;
      refreshSidebarSnapshot();
      return true;
    }
    if (action === "wrapup-progress-kick") {
      // The wrapup handler starts the fast progress poll after `/ctx-wrapup` because it emits no message events for the sidebar poll to observe.
      // The start toast arrives through the ignored-message notification path.
      if (!stillActive()) return false;
      kickRecompProgressRefresh();
      return true;
    }
    if (action === "show-flush-dialog") {
      const flushMsg = String(n.payload?.message ?? "Flushed.");
      return stillActive() && (await showResultDialog(api, "Flush", flushMsg));
    }
    if (action === "show-result-dialog") {
      const title = String(n.payload?.title ?? "Eidnara");
      const body = String(n.payload?.message ?? "");
      return stillActive() && (await showResultDialog(api, title, body));
    }
    return false;
  };
  startNotificationSocket({
    getSessionId: () => getSessionId(api),
    onNotification: handleNotification,
    // The socket opens only once the server is reachable, so the configured duration loads then and
    // again after each reconnect; the socket holds that connection's backlog until the load settles.
    onConnected: refreshToastDurationMs
  });
  api.lifecycle.onDispose(() => {
    sidebarSlot.dispose();
    stopNotificationSocket();
    closeRpc();
  });
};
const id = "eidnara-opencode";
export default {
  id,
  tui
};