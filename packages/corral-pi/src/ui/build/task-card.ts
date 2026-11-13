// The live `task` tool card in the conversation: a rounded card with one row per slice the call started (state,
// name, what it delivers, profile and time), and under it one line per slice that came back. Rows come from the
// live fleet while it knows the call, from the settled result after a resume, and from the call's arguments
// (outcome unknown) when neither exists. The helpers here are shared by the result card and the build entries.
import { truncateToWidth, visibleWidth, wrapTextWithAnsi } from "@earendil-works/pi-tui";
import type { ChildSnapshot, ChildSpec, ChildState, FleetSnapshot } from "../../build/types.ts";
import { agentStatus, duration, toneColor } from "../agents-model.ts";
import type { Tone } from "../agents-model.ts";
import { agentDensity } from "../agents-view.ts";
import { card, onSurface, spinner } from "../frame.ts";
import { plainLabel, PANEL_BG, PANEL_BLUE, PANEL_ERROR, PANEL_MUTED, PANEL_SUCCESS, PANEL_TEXT } from "../panels-view.ts";
import { childHue } from "./contract.ts";

export interface TaskCardRow {
  readonly id: string; readonly hue: number; readonly state: ChildState | "unknown"; readonly title: string;
  readonly profile: string; readonly model: string; readonly time?: string; readonly activity?: string;
}
export interface TaskCardHit { readonly id: string; readonly needs: boolean }

export const BOLD = "\x1b[1m";
export const UNBOLD = "\x1b[22m";
/** The widest conversation card (the mock's `.blk`). */
export const CARD_MAX = 108;
/** The line of a conversation card that holds its first body row: top border, then the header row. */
export const CARD_BODY = 2;
const GAP = "  ", NAME = 11, PROFILE_FULL = 28, PROFILE_MED = 9, TIME = 10, TIME_MIN = 9, LABEL_MAX = 15;
/** Below this many title cells a slice row puts its title on a second row. */
const TITLE_MIN = 8;

/** The cells inside a conversation card drawn in a row `width` cells wide: borders and one cell of padding each side. */
export function cardInner(width: number): number {
  return Math.max(0, Math.min(CARD_MAX, width) - 4);
}
/** A conversation card in a row `width` cells wide: `card` at most CARD_MAX cells wide; body row `n` is line `CARD_BODY + n`. */
export function conversationCard(header: string, body: readonly string[], width: number): string[] {
  return card(header, body, Math.min(CARD_MAX, width));
}
/** Styled text clipped with `…` and padded to exactly `width` cells; ends in the text colour at normal weight. */
export function cardCell(text: string, width: number, right = false): string {
  if (width <= 0) return "";
  const clipped = visibleWidth(text) > width ? truncateToWidth(text, width, "…") : text;
  const room = " ".repeat(Math.max(0, width - visibleWidth(clipped)));
  return right ? room + clipped + UNBOLD + PANEL_TEXT : clipped + UNBOLD + PANEL_TEXT + room;
}
/** One line, never wider than `width`; a reset inside it returns to the canvas. */
export function cardLine(text: string, width: number): string {
  const room = Math.max(0, width);
  const clipped = visibleWidth(text) > room ? truncateToWidth(text, room, "…") : text;
  return onSurface(clipped, PANEL_BG) + UNBOLD + PANEL_TEXT;
}
/** `text` wrapped to `width` in one colour, each row prefixed with `indent` spaces. */
export function cardWrap(text: string, color: string, width: number, indent = 0): string[] {
  const room = Math.max(1, width - indent);
  return wrapTextWithAnsi(text, room).map(row => cardLine(" ".repeat(indent) + color + row, width));
}
export function firstLine(text: string): string {
  return plainLabel(text.split(/\r?\n/).find(line => line.trim()) ?? "");
}

/** True when every row has finished, failed, been cancelled or lost its outcome. */
export function taskRowsSettled(rows: readonly TaskCardRow[]): boolean {
  return rows.every(row => row.state !== "queued" && row.state !== "running" && row.state !== "needs");
}

function childCardRow(child: ChildSnapshot, hue: number, now: number): TaskCardRow {
  const time = child.startedAt === undefined ? undefined : duration((child.endedAt ?? now) - child.startedAt);
  const activity = child.state === "running" ? plainLabel(child.activity ?? "") || "working" : undefined;
  return {
    id: child.id, hue, state: child.state, title: plainLabel(child.title), profile: plainLabel(child.profile), model: plainLabel(child.model),
    ...(time === undefined ? {} : { time }), ...(activity === undefined ? {} : { activity }),
  };
}
function hueOf(live: FleetSnapshot | undefined, id: string, index: number): number {
  return live?.children.some(child => child.id === id) ? childHue(live, id) : index;
}
function specs(args: { readonly agents: readonly ChildSpec[] } | undefined): ChildSpec[] {
  const agents: unknown = args?.agents;
  if (!Array.isArray(agents)) return [];
  return agents.filter((spec): spec is ChildSpec => typeof spec === "object" && spec !== null && typeof (spec as ChildSpec).id === "string");
}

/** The children of call `call`: the live fleet's while it knows the call, else the settled result's. */
export function callChildren(call: string, live: FleetSnapshot | undefined, settled: readonly ChildSnapshot[] | undefined): readonly ChildSnapshot[] {
  const mine = live?.children.filter(child => child.call === call) ?? [];
  return mine.length ? mine : settled ?? [];
}

/** Live rows for this call when the fleet has them, else the settled result's, else the arguments with `unknown`. */
export function taskCardRows(args: { readonly agents: readonly ChildSpec[] }, call: string, live: FleetSnapshot | undefined,
  settled: readonly ChildSnapshot[] | undefined, now: number): TaskCardRow[] {
  const children = callChildren(call, live, settled);
  if (children.length) return children.map((child, index) => childCardRow(child, hueOf(live, child.id, index), now));
  return specs(args).map((spec, index) => ({
    id: spec.id, hue: hueOf(live, spec.id, index), state: "unknown", title: plainLabel(typeof spec.title === "string" ? spec.title : ""),
    profile: plainLabel(typeof spec.profile === "string" && spec.profile ? spec.profile : "default"), model: "",
  }));
}

interface RowLook { readonly glyph: string; readonly tone: Tone; readonly label: string; readonly labelTone: Tone }
/** State glyph and the time column: a running row's spinner is staggered by `phase`. */
function look(row: TaskCardRow, now: number, phase: number): RowLook {
  if (row.state === "unknown") return { glyph: "!", tone: "warning", label: "outcome unknown", labelTone: "warning" };
  const status = agentStatus(row.state, row.time);
  switch (row.state) {
    case "queued": return { glyph: status.glyph, tone: status.tone, label: "waiting", labelTone: "muted" };
    case "running": return { glyph: spinner(now, phase), tone: "accent", label: row.time ?? status.word, labelTone: "muted" };
    case "done": return { glyph: status.glyph, tone: status.tone, label: row.time ?? status.word, labelTone: "success" };
    default: return { glyph: status.glyph, tone: status.tone, label: status.word, labelTone: status.tone };
  }
}
function plural(count: number, one: string, many: string): string { return `${count} ${count === 1 ? one : many}`; }

/**
 * The `task` card for a row `width` cells wide. Columns follow the mock's `.mini` grid, two cells apart: state,
 * name (11), title, `profile · model` (28, full density) or the profile (9, med), the time (10, right-aligned,
 * growing to the longest state word), one cell of right inset. Min density drops the profile; a card too narrow
 * for a title column puts the title on a second row. `hits` maps a line of the card to the slice it shows.
 */
export function taskCardLines(rows: readonly TaskCardRow[], width: number, options: {
  readonly expanded: boolean; readonly color: (hue: number) => string; readonly now: number;
}): { readonly lines: string[]; readonly hits: ReadonlyMap<number, TaskCardHit> } {
  const density = agentDensity(width), inner = cardInner(width);
  const body: string[] = [], hits = new Map<number, TaskCardHit>();
  const header = PANEL_BLUE + "task" + PANEL_TEXT + " · " + plural(rows.length, "slice", "slices") + (density === "min" ? "" : ", one worktree");
  const looks = rows.map((row, index) => look(row, options.now, index));
  // The time column grows to its longest word (`outcome unknown`), so a state is never clipped.
  const widest = Math.max(0, ...looks.map(shown => visibleWidth(shown.label)));
  const timeWidth = Math.min(LABEL_MAX, Math.max(density === "min" ? TIME_MIN : TIME, widest));
  const profileWidth = density === "full" ? PROFILE_FULL : density === "med" ? PROFILE_MED : 0;
  const titleWidth = inner - (1 + NAME + timeWidth + 1 + GAP.length * 3 + (profileWidth ? profileWidth + GAP.length : 0));
  const stacked = titleWidth < TITLE_MIN;
  rows.forEach((row, index) => {
    const hit = { id: row.id, needs: row.state === "needs" }, first = body.length;
    const shown = looks[index] ?? look(row, options.now, index);
    const glyph = toneColor(shown.tone) + shown.glyph + PANEL_TEXT;
    const name = BOLD + options.color(row.hue) + row.id + UNBOLD;
    const time = cardCell(toneColor(shown.labelTone) + shown.label, timeWidth, true);
    if (stacked) {
      body.push(glyph + GAP + cardCell(name, Math.max(1, inner - 6 - timeWidth)) + GAP + time);
      if (row.title) body.push("   " + PANEL_TEXT + row.title);
    } else {
      // `<provider>/<id>` shows as its id, as the agents table does.
      const model = density === "full" && row.model ? " · " + row.model.slice(row.model.lastIndexOf("/") + 1) : "";
      const profile = profileWidth ? cardCell(PANEL_MUTED + row.profile + model, profileWidth) + GAP : "";
      body.push(glyph + GAP + cardCell(name, NAME) + GAP + cardCell(PANEL_TEXT + row.title, titleWidth) + GAP + profile + time);
    }
    if (options.expanded && row.state === "running" && row.activity) {
      body.push(" ".repeat(stacked ? 3 : 3 + NAME + GAP.length) + PANEL_MUTED + row.activity);
    }
    for (let line = first; line < body.length; line++) hits.set(CARD_BODY + line, hit);
  });
  return { lines: conversationCard(header, body, width), hits };
}

function editCounts(child: ChildSnapshot): string {
  const paths = new Set(child.edits.map(edit => edit.path)), outside = new Set(child.edits.filter(edit => edit.outside).map(edit => edit.path));
  const inside = paths.size - outside.size;
  return plural(inside, "file", "files") + (outside.size ? ` + ${outside.size} outside its set` : "");
}
/**
 * One line per child of the call that came back, in the order they ended, as each posts it under the card:
 * `✓ host-slots finished · 2 files · cargo test slots 9 passed`, `✗ tests failed · <error>`, `✗ tests cancelled · …`.
 * The name is bold in its hue; `hits` maps a line to its child.
 */
export function finishedLines(children: readonly ChildSnapshot[], fleet: FleetSnapshot | undefined, width: number,
  color: (hue: number) => string): { readonly lines: string[]; readonly hits: ReadonlyMap<number, TaskCardHit> } {
  const back = children.map((child, index) => ({ child, hue: hueOf(fleet, child.id, index) }))
    .filter(({ child }) => child.state === "done" || child.state === "failed" || child.state === "cancelled")
    .sort((a, b) => (a.child.endedAt ?? 0) - (b.child.endedAt ?? 0));
  const lines: string[] = [], hits = new Map<number, TaskCardHit>();
  for (const { child, hue } of back) {
    const name = (tone: string) => BOLD + color(hue) + plainLabel(child.id) + UNBOLD + tone;
    let text: string;
    if (child.state === "done") {
      const report = firstLine(child.report ?? "").replace(/^#+\s*/, "");
      text = PANEL_SUCCESS + "✓ " + name(PANEL_SUCCESS) + " finished · " + editCounts(child) + (report ? " · " + report : "");
    } else if (child.state === "failed") {
      const error = firstLine(child.error ?? "");
      text = PANEL_ERROR + "✗ " + name(PANEL_ERROR) + " failed" + (error ? " · " + error : "");
    } else {
      text = PANEL_ERROR + "✗ " + name(PANEL_ERROR) + " cancelled · its edits so far stay in the worktree";
    }
    hits.set(lines.length, { id: child.id, needs: false });
    lines.push(cardLine(text, width));
  }
  return { lines, hits };
}
