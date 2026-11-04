// One owner per ambient datum: model in the composer; role, context window and alerts in the bottom bar.
import { basename } from "node:path";
import { stripTerminalSequences, visibleWidth, Text } from "@earendil-works/pi-tui";
import type { HostIdentity } from "../roles/types.ts";
import { STATUS_BINDING, STATUS_ALERTS, target } from "./chrome.ts";
import { activitySidebar } from "./activity-sidebar.ts";
import {
  pad, plainLabel, PANEL_BG, PANEL_BLUE, PANEL_MUTED, PANEL_TEXT, PANEL_RULE, PANEL_SUCCESS, PANEL_WARNING, PANEL_ERROR, RESET,
  type PanelSnapshot, type FilesState, type PanelLine,
} from "./panels-view.ts";

export function splitRow(left: string, right: string, width: number): string {
  const tail = pad(right, Math.min(width, visibleWidth(right)));
  const room = Math.max(0, width - visibleWidth(tail) - (tail ? 2 : 0));
  return pad(pad(left, room) + (tail ? "  " + tail : ""), width);
}
export function contextLabel(snapshot: PanelSnapshot | null): string {
  const usage = snapshot?.context;
  if (usage?.tokens == null) return "Context unavailable";
  const compact = (n: number) => n >= 1000 ? `${Number((n / 1000).toFixed(1))}k` : String(n);
  return `${compact(usage.tokens)}/${compact(usage.contextWindow)} · ${usage.percent == null ? "context usage unknown" : Math.round(usage.percent) + "% context"}`;
}
export function panelHeader(identity: HostIdentity, cwd: string, branch: string | null,
  snapshot: PanelSnapshot | null, width: number): string[] {
  return [
    PANEL_BG + splitRow(PANEL_BLUE + "corral / " + plainLabel(target(identity)),
      PANEL_MUTED + (!snapshot ? "Unavailable" : snapshot.streaming ? "Working" : "Idle"), width) + RESET,
    PANEL_BG + PANEL_MUTED + pad(`${identity.role.replaceAll("_", " ").toUpperCase()} · g${identity.generation} · ` +
      (snapshot?.bound ? "Bound to Corral · " : "Unbound · ") +
      plainLabel(basename(cwd)) + (branch ? ` (${plainLabel(branch)})` : ""), width) + RESET,
  ];
}
interface ComposerHit { readonly from: number; readonly to: number }
export function composerLayout(snapshot: PanelSnapshot | null, width: number, hidden = 0, sendHint?: string): {
  readonly line: string; readonly model?: ComposerHit; readonly effort?: ComposerHit;
} {
  const full = snapshot?.model;
  const model = full ? plainLabel(full.slice(full.indexOf("/") + 1)) + " ▾" : "Model unavailable";
  const effort = snapshot ? ` · ${plainLabel(snapshot.effort)}${snapshot.effortAvailable ? " ↻" : ""}` : "";
  const hint = hidden > 0 ? `↓ ${hidden} more` : sendHint ?? (snapshot?.streaming ? "Enter steer" : "Enter send");
  const room = Math.max(0, width - 6 - visibleWidth(hint) - visibleWidth(effort));
  const label = pad(model, Math.min(room, visibleWidth(model))).trimEnd();
  const modelEnd = 2 + visibleWidth(label), effortEnd = modelEnd + visibleWidth(effort);
  return {
    line: PANEL_MUTED + "└ " + PANEL_TEXT + splitRow(label + PANEL_MUTED + effort,
      PANEL_MUTED + hint, Math.max(0, width - 4)) + PANEL_MUTED + " ┘",
    ...(full && label ? { model: { from: 2, to: modelEnd } } : {}),
    ...(snapshot?.effortAvailable && effortEnd < width - 2 ? { effort: { from: modelEnd, to: effortEnd } } : {}),
  };
}
export function composerBar(snapshot: PanelSnapshot | null, width: number, hidden = 0): string {
  return composerLayout(snapshot, width, hidden).line;
}
export function composerTitle(snapshot: PanelSnapshot | null): string {
  const input = snapshot?.input;
  if (!input) return "Message";
  const meaning = input.disposition === "pending" ? "admission unknown"
    : input.disposition === "queued" ? `${input.policy ?? "input"}; not execution`
    : input.disposition === "started" ? "not completion"
    : input.disposition === "handled" ? "consumed; not execution" : input.reason ?? "not admitted";
  return `Last typed input: ${input.disposition} · ${plainLabel(meaning)}`;
}
const compact1 = (n: number) => n >= 1000 ? `${Number((n / 1000).toFixed(1))}k` : String(n);
const compact0 = (n: number) => n >= 1000 ? `${Math.round(n / 1000)}k` : String(n);
/**
 * The context window in the widest form that fits `room`, shortening in steps and never dropped:
 * `ctx ▰▰▰▰▱▱ 62% · 124.1k / 200k` → `ctx ▰▰▱▱ 62% 124k/200k` → `62% 124k/200k`.
 */
export function contextSegment(snapshot: PanelSnapshot | null, room: number): string {
  const usage = snapshot?.context;
  if (usage?.tokens == null) return PANEL_MUTED + "Context unavailable";
  const window = usage.contextWindow;
  if (usage.percent == null) {
    return PANEL_TEXT + (room >= 40 ? `ctx usage unknown · ${compact1(usage.tokens)} / ${compact1(window)}`
      : `? ${compact0(usage.tokens)}/${compact0(window)}`);
  }
  const percent = Math.round(usage.percent);
  const tone = percent >= 95 ? PANEL_ERROR : percent >= 80 ? PANEL_WARNING : PANEL_TEXT;
  const fill = percent >= 80 ? tone : PANEL_BLUE;
  const bar = (cells: number) => {
    const filled = Math.max(0, Math.min(cells, Math.round(cells * percent / 100)));
    return fill + "▰".repeat(filled) + PANEL_RULE + "▱".repeat(cells - filled) + tone;
  };
  const full = ` ${percent}% · ${compact1(usage.tokens)} / ${compact1(window)}`;
  if (room >= 4 + 10 + visibleWidth(full)) return tone + "ctx " + bar(Math.min(24, room - 4 - visibleWidth(full))) + full;
  const mid = ` ${percent}% ${compact0(usage.tokens)}/${compact0(window)}`;
  if (room >= 4 + 4 + visibleWidth(mid)) return tone + "ctx " + bar(4) + mid;
  return tone + mid.trimStart();
}
/**
 * The bottom bar owns only what matters in every view: role and binding, the context window and
 * alerts. Below 52 columns the context takes its own row; alerts that do not fit wrap, never drop.
 */
export function statusBar(identity: HostIdentity, snapshot: PanelSnapshot | null,
  statuses: ReadonlyMap<string, string>, width: number): string[] {
  const role = identity.role.replaceAll("_", " ").toUpperCase();
  const bound = snapshot?.bound ?? false;
  const dot = bound ? PANEL_SUCCESS + "●" : PANEL_WARNING + "○";
  const badge = width >= 80
    ? "\x1b[1m" + PANEL_BLUE + "▌" + role + "\x1b[22m " + dot + PANEL_MUTED + ` g${identity.generation}` + (bound ? "" : PANEL_WARNING + " unbound")
    : "\x1b[1m" + PANEL_BLUE + "▌" + (role[0] ?? "") + "\x1b[22m " + dot;
  // Binding is the dot; every other status (refusals first) is kept.
  const alerts = [...statuses].filter(([key]) => key !== STATUS_BINDING)
    .sort(([a], [b]) => Number(b === STATUS_ALERTS) - Number(a === STATUS_ALERTS))
    .map(([, value]) => plainLabel(stripTerminalSequences(value))).filter(Boolean).join(" · ");
  const sep = PANEL_MUTED + " │ ";
  const row = (text: string) => PANEL_BG + pad(" " + text, width) + RESET;
  const wrapped = (text: string) => new Text(text, 0, 0).render(Math.max(1, width - 1)).map(line => row(PANEL_WARNING + line));
  if (width < 52) return [row(badge + (alerts ? sep + PANEL_WARNING + alerts : "")), row(contextSegment(snapshot, width - 1))];
  const room = width - 1 - visibleWidth(badge) - 3;
  const shortest = visibleWidth(contextSegment(snapshot, 0));
  const inline = alerts && visibleWidth(alerts) + 3 + shortest <= room;
  const context = contextSegment(snapshot, room - (inline ? visibleWidth(alerts) + 3 : 0));
  return [row(badge + sep + context + (inline ? sep + PANEL_WARNING + alerts : "")), ...(alerts && !inline ? wrapped(alerts) : [])];
}
export function overviewLines(snapshot: PanelSnapshot | null, files: FilesState, scope?: { readonly name: string; readonly required: boolean },
  selected?: string, history = false): PanelLine[] {
  if (!snapshot) return [{ text: "Session state unavailable" }];
  const rows: PanelLine[] = [{ text: "Changed files", heading: true }];
  if (scope) rows.push({ text: scope.required ? "Choose project · /panels project" : plainLabel(scope.name), tone: "muted", action: { kind: "scope" } });
  if (scope?.required) rows.push({ text: "Workspace changes are hidden", tone: "muted" });
  else switch (files.kind) {
    case "unread": rows.push({ text: "Open Files to read Git changes" }); break;
    case "loading": rows.push({ text: "Reading Git status…" }); break;
    case "error": rows.push({ text: files.reason }); break;
    case "ready":
      for (const file of files.files.slice(0, 4)) rows.push({ text: plainLabel(`${file.status} ${file.path}`), action: { kind: "diff", path: file.path } });
      if (!files.files.length) rows.push({ text: "Working tree clean" });
      if (files.files.length > 4) rows.push({ text: `${files.files.length - 4} more in Files`, action: { kind: "files" } });
      break;
  }
  rows.push({ text: "" }, ...activitySidebar(snapshot, selected, history));
  return rows;
}
