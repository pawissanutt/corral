// One owner per ambient datum: role, binding, session cost, context window and alerts in the status bar.
import { stripTerminalSequences, visibleWidth, Text } from "@earendil-works/pi-tui";
import type { HostIdentity } from "../roles/types.ts";
import { STATUS_BINDING, STATUS_ALERTS } from "./chrome.ts";
import {
  pad, plainLabel, PANEL_BG, PANEL_BLUE, PANEL_MUTED, PANEL_TEXT, PANEL_RULE, PANEL_SUCCESS, PANEL_WARNING, PANEL_ERROR, RESET,
  type PanelSnapshot,
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
 * The bottom bar owns only what matters in every view: role and binding, session cost, the context
 * window and alerts. Below 105 columns the `unbound` word drops (the `○` dot stays); below 52 the
 * context takes its own row. Alerts that do not fit wrap, never drop. `partialCost` marks a cost
 * that leaves out unknown parts with `+`.
 */
export function statusBar(identity: HostIdentity, snapshot: PanelSnapshot | null,
  statuses: ReadonlyMap<string, string>, width: number, partialCost = false): string[] {
  const role = identity.role.replaceAll("_", " ").toUpperCase();
  const bound = snapshot?.bound ?? false;
  const dot = bound ? PANEL_SUCCESS + "●" : PANEL_WARNING + "○";
  const badge = width >= 80
    ? "\x1b[1m" + PANEL_BLUE + "▌" + role + "\x1b[22m " + dot + PANEL_MUTED + ` g${identity.generation}` + (bound || width < 105 ? "" : PANEL_WARNING + " unbound")
    : "\x1b[1m" + PANEL_BLUE + "▌" + (role[0] ?? "") + "\x1b[22m " + dot;
  // Binding is the dot; every other status (refusals first) is kept.
  const alerts = [...statuses].filter(([key]) => key !== STATUS_BINDING)
    .sort(([a], [b]) => Number(b === STATUS_ALERTS) - Number(a === STATUS_ALERTS))
    .map(([, value]) => plainLabel(stripTerminalSequences(value))).filter(Boolean).join(" · ");
  const sep = PANEL_MUTED + " │ ";
  const cost = snapshot?.cost === undefined ? "" : sep + PANEL_TEXT + "$" + snapshot.cost.toFixed(2) + (partialCost ? "+" : "");
  const row = (text: string) => PANEL_BG + pad(" " + text, width) + RESET;
  const wrapped = (text: string) => new Text(text, 0, 0).render(Math.max(1, width - 1)).map(line => row(PANEL_WARNING + line));
  if (width < 52) return [row(badge + cost + (alerts ? sep + PANEL_WARNING + alerts : "")), row(contextSegment(snapshot, width - 1))];
  const room = width - 1 - visibleWidth(badge) - visibleWidth(cost) - 3;
  const shortest = visibleWidth(contextSegment(snapshot, 0));
  const inline = alerts && visibleWidth(alerts) + 3 + shortest <= room;
  const context = contextSegment(snapshot, room - (inline ? visibleWidth(alerts) + 3 : 0));
  return [row(badge + cost + sep + context + (inline ? sep + PANEL_WARNING + alerts : "")), ...(alerts && !inline ? wrapped(alerts) : [])];
}
