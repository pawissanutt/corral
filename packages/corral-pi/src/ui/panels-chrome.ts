// One owner per ambient datum: role, binding, session cost, context window and alerts in the status bar.
import { stripTerminalSequences, visibleWidth, Text } from "@earendil-works/pi-tui";
import type { HostIdentity, Role } from "../roles/types.ts";
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
/** The role pill's three-letter form below 64 columns. */
const ROLE_SHORT: Readonly<Record<Role, string>> = {
  worker: "WRK", reviewer: "REV", assistant: "AST", build: "BLD",
};
/** The model and effort the folded composer bar no longer shows (below 28 rows); `model` is `provider/id`. */
export interface FoldedComposer { readonly model: string | null; readonly effort?: string }
/** One step of the status bar's measured drop order. */
interface BarLevel { readonly word: boolean; readonly composer: 0 | 1 | 2 | 3 }
/** Measured in order: the folded composer's `effort` word goes, then its effort, then the model, and last the binding's word. */
const LEVELS: readonly BarLevel[] = [
  { word: true, composer: 0 }, { word: true, composer: 1 }, { word: true, composer: 2 }, { word: true, composer: 3 }, { word: false, composer: 3 },
];
/**
 * The status bar owns only what matters in every view: the role as a filled accent pill and the binding,
 * session cost, the context window and alerts, separated by `│` in the line colour. A build session is
 * never bound, so it has no binding segment; a bound role reads `● bound`,
 * an unbound one `○ unbound`. Below 64 columns the pill keeps a three-letter form (`WRK`, `BLD`). While the
 * composer bar is folded, `folded` appends `<model> · effort <value>` after the meter. Every width keeps the
 * measured drop order of `LEVELS`; when the row still cannot hold the meter it takes a second row; alerts
 * that do not fit wrap, never drop. `partialCost` marks a cost that leaves out unknown parts with `+`.
 */
export function statusBar(identity: HostIdentity, snapshot: PanelSnapshot | null,
  statuses: ReadonlyMap<string, string>, width: number, partialCost = false, folded?: FoldedComposer): string[] {
  const tiny = width < 64, small = width < 105;
  const role = tiny ? ROLE_SHORT[identity.role] : identity.role.replaceAll("_", " ").toUpperCase();
  const pill = `${PANEL_ON_ACCENT} ${role} \x1b[22m${PANEL_BG}`;
  const bound = snapshot?.bound ?? false;
  const binding = (level: BarLevel) => identity.role === "build" ? ""
    : " " + (bound ? PANEL_SUCCESS + "●" + (level.word ? PANEL_MUTED + " bound" : "") : PANEL_WARNING + "○" + (level.word ? " unbound" : ""));
  const model = folded?.model ? plainLabel(folded.model.slice(folded.model.indexOf("/") + 1)) : "";
  const effort = folded?.effort ? plainLabel(folded.effort) : "";
  const composer = (level: BarLevel) => {
    const effortText = level.composer === 0 && effort ? "effort " + effort : level.composer <= 1 ? effort : "";
    const modelText = level.composer <= 2 ? model : "";
    if (!modelText && !effortText) return "";
    return PANEL_LINE + " │ " + PANEL_TEXT + modelText + (modelText && effortText ? PANEL_MUTED + " · " : "") + PANEL_MUTED + effortText;
  };
  // Binding is the dot; every other status (refusals first) is kept.
  const alerts = [...statuses].filter(([key]) => key !== STATUS_BINDING)
    .sort(([a], [b]) => Number(b === STATUS_ALERTS) - Number(a === STATUS_ALERTS))
    .map(([, value]) => plainLabel(stripTerminalSequences(value))).filter(Boolean).join(" · ");
  const sep = PANEL_LINE + " │ ";
  const cost = snapshot?.cost === undefined ? "" : sep + PANEL_TEXT + "$" + snapshot.cost.toFixed(2) + (partialCost ? "+" : "");
  const row = (text: string) => PANEL_BG + PANEL_TEXT + pad(" " + text, width).replaceAll(RESET, RESET + PANEL_BG) + RESET;
  const wrapped = (text: string) => new Text(text, 0, 0).render(Math.max(1, width - 1)).map(line => row(PANEL_WARNING + line));
  const form = tiny ? "tiny" : small ? "mid" : "full";
  // The folded composer stays only beside a meter that still reads `ctx ▰▰▱▱▱▱▱▱ 24%`; without it the meter may shorten further.
  const shortest = visibleWidth(contextSegment(snapshot, 0, form)), meter = visibleWidth(contextSegment(snapshot, width, "tiny"));
  const roomFor = (level: BarLevel) => width - 1 - visibleWidth(pill + binding(level) + cost + composer(level)) - 3;
  const level = LEVELS.find(candidate => roomFor(candidate) >= (composer(candidate) ? Math.max(meter, shortest) : shortest));
  if (!level) {
    const last: BarLevel = LEVELS[LEVELS.length - 1] ?? { word: false, composer: 3 };
    return [row(pill + binding(last) + cost + (alerts ? sep + PANEL_WARNING + alerts : "")), row(contextSegment(snapshot, width - 1, form))];
  }
  const room = roomFor(level), tail = composer(level);
  const inline = alerts && visibleWidth(alerts) + 3 + shortest <= room;
  const context = contextSegment(snapshot, room - (inline ? visibleWidth(alerts) + 3 : 0), form);
  return [row(pill + binding(level) + cost + sep + context + tail + (inline ? sep + PANEL_WARNING + alerts : "")), ...(alerts && !inline ? wrapped(alerts) : [])];
}
