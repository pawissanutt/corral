// One owner per ambient datum: role, binding, session cost, context window and alerts in the status bar.
import { stripTerminalSequences, visibleWidth, Text } from "@earendil-works/pi-tui";
import type { HostIdentity } from "../roles/types.ts";
import { STATUS_BINDING, STATUS_ALERTS } from "./chrome.ts";
import {
  pad, plainLabel, PANEL_BG, PANEL_BLUE, PANEL_LINE, PANEL_MUTED, PANEL_ON_ACCENT, PANEL_TEXT, PANEL_SUCCESS, PANEL_WARNING, PANEL_ERROR, RESET,
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
/** The meter's form per tier: `full` from 105 columns, `mid` from 64, `tiny` below. */
export type ContextForm = "full" | "mid" | "tiny";
/**
 * The context window as a meter, in the tier's form when it fits `room` and shorter forms when it does not,
 * never dropped: `ctx ▰▰▰▱▱▱▱▱▱▱▱▱ 24% · 48.0k / 200k` → `ctx ▰▰▱▱▱▱▱▱ 24% 48k/200k` → `ctx ▰▰▱▱▱▱▱▱ 24%`
 * → `24% 48k/200k` → `24%`. Filled cells are accent (warning and error from 80% and 95%), empty cells the line colour.
 */
export function contextSegment(snapshot: PanelSnapshot | null, room: number, form: ContextForm = "full"): string {
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
    return fill + "▰".repeat(filled) + PANEL_LINE + "▱".repeat(cells - filled) + tone;
  };
  const full = ` ${percent}% · ${compact1(usage.tokens)} / ${compact1(window)}`;
  const mid = ` ${percent}% ${compact0(usage.tokens)}/${compact0(window)}`;
  const forms = [
    ...(form === "full" ? [{ cells: 12, text: full }] : []),
    ...(form !== "tiny" ? [{ cells: 8, text: mid }] : []),
    { cells: 8, text: ` ${percent}%` },
  ];
  for (const { cells, text } of forms) if (room >= 4 + cells + visibleWidth(text)) return tone + "ctx " + bar(cells) + text;
  return tone + (room >= visibleWidth(mid) - 1 ? mid.trimStart() : `${percent}%`);
}
/**
 * The status bar owns only what matters in every view: the role as a filled accent pill and the binding,
 * session cost, the context window and alerts, separated by `│` in the line colour. A build session has no
 * corral binding and reads `○ unbound`; a bound role reads `● g<n>`. Below 105 columns the `unbound` word
 * drops (the `○` stays), below 64 the pill shows the role's initial. When the row cannot hold the meter it
 * takes a second row; alerts that do not fit wrap, never drop. `partialCost` marks a cost that leaves out
 * unknown parts with `+`.
 */
export function statusBar(identity: HostIdentity, snapshot: PanelSnapshot | null,
  statuses: ReadonlyMap<string, string>, width: number, partialCost = false): string[] {
  const role = identity.role.replaceAll("_", " ").toUpperCase();
  const tiny = width < 64, small = width < 105;
  const pill = `${PANEL_ON_ACCENT} ${tiny ? role[0] ?? "" : role} \x1b[22m${PANEL_BG}`;
  const bound = identity.role !== "build" && (snapshot?.bound ?? false);
  const binding = identity.role === "build"
    ? PANEL_WARNING + "○" + (small ? "" : PANEL_MUTED + " unbound")
    : (bound ? PANEL_SUCCESS + "●" : PANEL_WARNING + "○") + (tiny ? "" : PANEL_MUTED + ` g${identity.generation}`) + (bound || small ? "" : PANEL_WARNING + " unbound");
  const badge = pill + " " + binding;
  // Binding is the dot; every other status (refusals first) is kept.
  const alerts = [...statuses].filter(([key]) => key !== STATUS_BINDING)
    .sort(([a], [b]) => Number(b === STATUS_ALERTS) - Number(a === STATUS_ALERTS))
    .map(([, value]) => plainLabel(stripTerminalSequences(value))).filter(Boolean).join(" · ");
  const sep = PANEL_LINE + " │ ";
  const cost = snapshot?.cost === undefined ? "" : sep + PANEL_TEXT + "$" + snapshot.cost.toFixed(2) + (partialCost ? "+" : "");
  const row = (text: string) => PANEL_BG + PANEL_TEXT + pad(" " + text, width).replaceAll(RESET, RESET + PANEL_BG) + RESET;
  const wrapped = (text: string) => new Text(text, 0, 0).render(Math.max(1, width - 1)).map(line => row(PANEL_WARNING + line));
  const form = tiny ? "tiny" : small ? "mid" : "full";
  const room = width - 1 - visibleWidth(badge) - visibleWidth(cost) - 3;
  const shortest = visibleWidth(contextSegment(snapshot, 0, form));
  if (room < shortest) return [row(badge + cost + (alerts ? sep + PANEL_WARNING + alerts : "")), row(contextSegment(snapshot, width - 1, form))];
  const inline = alerts && visibleWidth(alerts) + 3 + shortest <= room;
  const context = contextSegment(snapshot, room - (inline ? visibleWidth(alerts) + 3 : 0), form);
  return [row(badge + cost + sep + context + (inline ? sep + PANEL_WARNING + alerts : "")), ...(alerts && !inline ? wrapped(alerts) : [])];
}
