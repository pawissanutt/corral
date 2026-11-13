// The live `task` tool card in the conversation: one row per slice the call started, with its state, what it
// delivers, its profile and its time. Rows come from the live fleet while it knows the call, from the settled
// result after a resume, and from the call's arguments (outcome unknown) when neither exists. Cards sit inside
// Pi's tool box, so lines use foreground colours only and never a full reset, which would drop the box surface.
import { truncateToWidth, visibleWidth, wrapTextWithAnsi } from "@earendil-works/pi-tui";
import type { ChildSnapshot, ChildSpec, ChildState, FleetSnapshot } from "../../build/types.ts";
import { agentStatus, duration, money, toneColor, type Tone } from "../agents-model.ts";
import { plainLabel, PANEL_BLUE, PANEL_ERROR, PANEL_MUTED, PANEL_TEXT, PANEL_WARNING } from "../panels-view.ts";
import { childHue } from "./contract.ts";

export interface TaskCardRow {
  readonly id: string; readonly hue: number; readonly state: ChildState | "unknown"; readonly title: string;
  readonly profile: string; readonly model: string; readonly time?: string; readonly activity?: string; readonly question?: string;
}
export interface TaskTotals {
  readonly done: number; readonly failed: number; readonly cancelled: number;
  /** Sum of the known costs; absent when no child has a price. */
  readonly cost?: number;
  /** True when `cost` leaves out at least one child whose cost is unknown. */
  readonly partial?: boolean;
  readonly files: number; readonly outside: number;
}

export const BOLD = "\x1b[1m";
export const UNBOLD = "\x1b[22m";
/** Ends a card line: default foreground and weight, leaving the surrounding surface intact. */
export const PLAIN = "\x1b[39m\x1b[22m";
const FULL_RESET = "\x1b[0m";
const GAP = "  ", NAME = 11, PROFILE_FULL = 28, PROFILE_MED = 9, TIME = 10, TIME_MIN = 9, LABEL_MAX = 15;

/** Styled text clipped and padded to exactly `width` cells; a reset added by clipping keeps the card surface. */
export function cardCell(text: string, width: number, right = false): string {
  if (width <= 0) return "";
  const clipped = truncateToWidth(text, width, "…").replaceAll(FULL_RESET, PLAIN);
  const room = " ".repeat(Math.max(0, width - visibleWidth(clipped)));
  return right ? room + clipped + PLAIN : clipped + PLAIN + room;
}
/** One finished card line, never wider than `width`. */
export function cardLine(text: string, width: number): string {
  return truncateToWidth(text, Math.max(0, width), "…").replaceAll(FULL_RESET, PLAIN) + PLAIN;
}
/** `text` wrapped to `width` in one colour, each row prefixed with `indent` spaces. */
export function cardWrap(text: string, color: string, width: number, indent = 0): string[] {
  const room = Math.max(1, width - indent);
  return wrapTextWithAnsi(text, room).map(row => cardLine(" ".repeat(indent) + color + row, width));
}
export function firstLine(text: string): string {
  return plainLabel(text.split(/\r?\n/).find(line => line.trim()) ?? "");
}
/** `<provider>/<id>` shown as its id, as the agents table does. */
export function shortModel(model: string): string {
  return model.slice(model.lastIndexOf("/") + 1);
}

/** True when every row has finished, failed, been cancelled or lost its outcome. */
export function taskRowsSettled(rows: readonly TaskCardRow[]): boolean {
  return rows.every(row => row.state !== "queued" && row.state !== "running" && row.state !== "needs");
}

function childCardRow(child: ChildSnapshot, hue: number, now: number): TaskCardRow {
  const time = child.startedAt === undefined ? undefined : duration((child.endedAt ?? now) - child.startedAt);
  const activity = child.state === "running" ? plainLabel(child.activity ?? "") || "working" : undefined;
  const question = child.state === "needs" && child.question ? firstLine(child.question.question) : undefined;
  return {
    id: child.id, hue, state: child.state, title: plainLabel(child.title), profile: plainLabel(child.profile), model: plainLabel(child.model),
    ...(time === undefined ? {} : { time }), ...(activity === undefined ? {} : { activity }), ...(question === undefined ? {} : { question }),
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

/** Live rows for this call when the fleet has them, else the settled result's, else the arguments with `unknown`. */
export function taskCardRows(args: { readonly agents: readonly ChildSpec[] }, call: string, live: FleetSnapshot | undefined,
  settled: readonly ChildSnapshot[] | undefined, now: number): TaskCardRow[] {
  const mine = live?.children.filter(child => child.call === call) ?? [];
  if (mine.length) return mine.map(child => childCardRow(child, childHue(live, child.id), now));
  if (settled?.length) return settled.map((child, index) => childCardRow(child, hueOf(live, child.id, index), now));
  return specs(args).map((spec, index) => ({
    id: spec.id, hue: hueOf(live, spec.id, index), state: "unknown", title: plainLabel(typeof spec.title === "string" ? spec.title : ""),
    profile: plainLabel(typeof spec.profile === "string" && spec.profile ? spec.profile : "default"), model: "",
  }));
}

export function taskTotals(children: readonly ChildSnapshot[]): TaskTotals {
  const costs = children.map(child => child.usage.cost).filter((cost): cost is number => cost !== undefined && Number.isFinite(cost));
  const files = new Set<string>(), outside = new Set<string>();
  for (const child of children) for (const edit of child.edits) { files.add(edit.path); if (edit.outside) outside.add(edit.path); }
  return {
    done: children.filter(child => child.state === "done").length,
    failed: children.filter(child => child.state === "failed").length,
    cancelled: children.filter(child => child.state === "cancelled").length,
    ...(costs.length ? { cost: costs.reduce((sum, cost) => sum + cost, 0) } : {}),
    ...(costs.length && costs.length < children.length ? { partial: true } : {}),
    files: files.size, outside: outside.size,
  };
}

interface RowLook { readonly glyph: string; readonly tone: Tone; readonly label: string; readonly labelTone: Tone }
function look(row: TaskCardRow): RowLook {
  if (row.state === "unknown") return { glyph: "!", tone: "warning", label: "outcome unknown", labelTone: "warning" };
  const status = agentStatus(row.state, row.time);
  switch (row.state) {
    case "queued": return { glyph: status.glyph, tone: status.tone, label: "waiting", labelTone: "muted" };
    case "running": return { glyph: status.glyph, tone: status.tone, label: row.time ?? status.word, labelTone: "muted" };
    case "done": return { glyph: status.glyph, tone: status.tone, label: row.time ?? status.word, labelTone: "success" };
    default: return { glyph: status.glyph, tone: status.tone, label: status.word, labelTone: status.tone };
  }
}
function plural(count: number, one: string, many: string): string { return `${count} ${count === 1 ? one : many}`; }

/** The totals line after the call returned: counts, known cost, files and files outside their owner's paths. */
export function totalsText(totals: TaskTotals): string {
  const parts = [PANEL_TEXT + `${totals.done} done`];
  if (totals.failed) parts.push(PANEL_ERROR + `${totals.failed} failed`);
  if (totals.cancelled) parts.push(PANEL_ERROR + `${totals.cancelled} cancelled`);
  if (totals.cost !== undefined) parts.push(PANEL_TEXT + money(totals.cost) + (totals.partial ? "+" : ""));
  parts.push(PANEL_TEXT + plural(totals.files, "file", "files"));
  if (totals.outside) parts.push(PANEL_WARNING + `${totals.outside} outside its files`);
  return parts.join(PANEL_MUTED + " · ");
}

export function taskCardLines(rows: readonly TaskCardRow[], width: number, options: {
  readonly expanded: boolean; readonly color: (hue: number) => string; readonly totals?: TaskTotals;
}): { readonly lines: string[]; readonly hits: ReadonlyMap<number, { readonly id: string; readonly needs: boolean }> } {
  const density = width >= 112 ? "full" : width >= 60 ? "med" : width >= 40 ? "min" : "tiny";
  const lines: string[] = [], hits = new Map<number, { readonly id: string; readonly needs: boolean }>();
  const count = plural(rows.length, "slice", "slices");
  lines.push(cardLine(BOLD + PANEL_BLUE + "task" + UNBOLD + PANEL_MUTED + " · " + PANEL_TEXT + count + (density === "full" || density === "med" ? ", one worktree" : ""), width));
  const looks = rows.map(look);
  // The state column grows to its longest word (`outcome unknown`), so a state is never clipped.
  const widest = Math.max(0, ...looks.map(shown => visibleWidth(shown.label)));
  const timeWidth = Math.min(LABEL_MAX, Math.max(density === "full" || density === "med" ? TIME : TIME_MIN, widest));
  rows.forEach((row, index) => {
    const hit = { id: row.id, needs: row.state === "needs" }, first = lines.length;
    const shown = looks[index] ?? look(row), lead = toneColor(shown.tone) + shown.glyph + " ";
    const name = BOLD + options.color(row.hue) + row.id + UNBOLD;
    const title = PANEL_TEXT + row.title, time = cardCell(toneColor(shown.labelTone) + shown.label, timeWidth, true);
    if (density === "tiny") {
      lines.push(cardLine(lead + cardCell(name, Math.max(1, width - 2 - GAP.length - timeWidth)) + GAP + time, width));
      if (row.title) lines.push(cardLine("  " + title, width));
    } else if (density === "min") {
      const flex = width - 2 - NAME - GAP.length * 2 - timeWidth;
      lines.push(cardLine(lead + cardCell(name, NAME) + GAP + cardCell(title, flex) + GAP + time, width));
    } else {
      const profileWidth = density === "full" ? PROFILE_FULL : PROFILE_MED;
      const model = row.model ? shortModel(row.model) : "";
      const profile = PANEL_MUTED + row.profile + (density === "full" && model ? " · " + model : "");
      const flex = width - 2 - NAME - GAP.length * 3 - profileWidth - timeWidth;
      lines.push(cardLine(lead + cardCell(name, NAME) + GAP + cardCell(title, flex) + GAP + cardCell(profile, profileWidth) + GAP + time, width));
    }
    if (row.state === "needs" && row.question) lines.push(cardLine("  " + PANEL_WARNING + "▲ " + row.question, width));
    if (options.expanded && row.state === "running" && row.activity) lines.push(cardLine("  " + PANEL_MUTED + row.activity, width));
    for (let line = first; line < lines.length; line++) hits.set(line, hit);
  });
  if (options.totals) lines.push(...cardWrap(totalsText(options.totals), "", width));
  return { lines, hits };
}
