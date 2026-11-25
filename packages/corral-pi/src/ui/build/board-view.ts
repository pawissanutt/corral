import { truncateToWidth, visibleWidth, wrapTextWithAnsi } from "@earendil-works/pi-tui";
import type { BoardPhase, BoardPlan, BoardTaskStatus } from "../../build/types.ts";
import { taskMetadataLabel } from "./task-metadata.ts";
import { duration, toneColor } from "../agents-model.ts";
import { agentDensity } from "../agents-view.ts";
import { keyCap, spinner } from "../frame.ts";
import { plainLabel, PANEL_BG, PANEL_BLUE, PANEL_MUTED, PANEL_SELECTED, PANEL_TEXT, PANEL_WARNING, RESET } from "../panels-view.ts";
import type { BuildTone } from "./contract.ts";

export type BoardAction = "details" | "refresh" | "plan" | "review";
export interface BoardStage { readonly glyph: string; readonly tone: BuildTone; readonly word: string; readonly now: string }
export interface BoardHit { readonly id?: string; readonly action?: BoardAction; readonly from?: number; readonly to?: number }
const BOLD = "\x1b[1m", UNBOLD = "\x1b[22m", GAP = "  ";
/** Id column floor and cap, the title column at full density, the stage column, the narrowest `now` column. */
const ID_MIN = 6, ID_MAX = 10, TITLE_MAX = 40, STAGE = 10, NOW_MIN = 12;
/** The task's actions under the table: their key and label. */
const ROW_ACTIONS: Readonly<Record<"details" | "refresh", { readonly key: string; readonly label: string }>> = {
  details: { key: "⏎", label: "open task" }, refresh: { key: "r", label: "refresh" },
};

/** Styled text clipped to exactly `width` cells; resets inside the clip restore `base` so the row keeps its surface. */
function cell(text: string, width: number, base: string): string {
  if (width <= 0) return "";
  const clipped = truncateToWidth(text, width, "…");
  return clipped.replaceAll(RESET, RESET + base) + " ".repeat(Math.max(0, width - visibleWidth(clipped)));
}
function fill(text: string, width: number, surface = PANEL_BG): string {
  const base = surface + PANEL_TEXT;
  return base + cell(text, width, base) + RESET;
}
function wrapped(text: string, color: string, width: number, cap: number): string[] {
  const rows = wrapTextWithAnsi(text, Math.max(1, width - 2));
  const shown = rows.slice(0, cap);
  if (rows.length > cap && shown.length) shown[shown.length - 1] = truncateToWidth(shown[shown.length - 1] ?? "", Math.max(1, width - 3), "") + "…";
  return shown.map(line => fill(" " + color + line, width));
}

/** The glyph, tone, word and `now` text of a task's stage; `tasks` tells which of its dependencies landed. */
export function boardStage(task: BoardTaskStatus, tasks: readonly BoardTaskStatus[]): BoardStage {
  const detail = task.detail === undefined ? undefined : plainLabel(task.detail) || undefined;
  switch (task.phase) {
    case "backlog": return { glyph: "○", tone: "muted", word: "backlog", now: "filed, not started" };
    case "waiting": {
      // A dependency this session did not file is unknown here, so it still counts as waited for.
      const waits = task.depends.filter(id => tasks.find(other => other.id === id)?.phase !== "landed").map(plainLabel);
      return { glyph: "○", tone: "muted", word: "waiting", now: waits.length ? "waits " + waits.join(", ") : detail ?? "waits for its dependencies" };
    }
    case "queued": return { glyph: "○", tone: "muted", word: "queued", now: "waits for a slot" };
    case "running": return { glyph: "●", tone: "accent", word: "running", now: detail ?? "an agent works on it" };
    case "gating": return { glyph: "●", tone: "accent", word: "gate", now: detail ?? "the gate checks the candidate" };
    case "reviewing": return { glyph: "●", tone: "accent", word: "review", now: detail ?? "independent review" };
    case "landed": return { glyph: "✓", tone: "success", word: "landed", now: "on the target branch" };
    case "blocked": return { glyph: "▲", tone: "warning", word: "blocked", now: detail ?? "blocked without a stated reason" };
    case "failed": return { glyph: "✗", tone: "error", word: "failed", now: detail ?? "failed without a stated reason" };
  }
}

/** `From this session · 2 running · 1 in gate or review · 3/5 landed` as `lead`, blocked and failed counts as `alert`; `min` drops the source. */
function summaryParts(tasks: readonly BoardTaskStatus[], min: boolean): { readonly lead: string; readonly alert: string } {
  const count = (...phases: BoardPhase[]) => tasks.filter(task => phases.includes(task.phase)).length;
  const blocked = count("blocked"), failed = count("failed");
  const lead = (min ? "" : "From this session · ") + `${count("running")} running · ${count("gating", "reviewing")} in gate or review · `
    + `${count("landed")}/${tasks.length} landed`;
  const alert = (blocked ? ` · ${blocked} blocked` : "") + (failed ? ` · ${failed} failed` : "");
  return { lead, alert };
}

/** The task's stage, corral status and dependencies, its detail text and its age, as plain lines for the details pager. */
export function boardDetailLines(task: BoardTaskStatus, now: number): string[] {
  const stage = boardStage(task, []);
  const depends = task.depends.length ? "depends on " + task.depends.map(plainLabel).join(", ") : "no dependencies";
  const age = now - task.updatedAt;
  const metadata = taskMetadataLabel(task);
  return [
    // The stage's `now` repeats the detail for blocked and failed tasks, so it is named once.
    `${stage.glyph} ${stage.word}` + (stage.now === plainLabel(task.detail ?? "") ? "" : " · " + stage.now),
    `corral status: ${plainLabel(task.status) || "unknown"} · ${depends}`,
    ...(metadata ? [metadata] : []),
    ...(task.detail ?? "").split("\n").map(plainLabel).filter(Boolean),
    age < 1000 ? "updated just now" : `updated ${duration(age)} ago`,
  ];
}

function shownTask(tasks: readonly BoardTaskStatus[], selected: string | undefined): BoardTaskStatus | undefined {
  return tasks.find(task => task.id === selected) ?? tasks[0];
}
/** The task's actions as key caps after a one-cell inset; actions that do not fit are left out (their keys stay in the footer). */
function actionLine(id: string, actions: readonly ("details" | "refresh")[], width: number): { readonly line: string; readonly hits: BoardHit[] } {
  const hits: BoardHit[] = [];
  let text = " ", used = 1;
  for (const action of actions) {
    const { key, label } = ROW_ACTIONS[action];
    const from = used + (used > 1 ? 2 : 0), to = from + visibleWidth(key) + 3 + visibleWidth(label);
    if (to > width - 1) break;
    text += (used > 1 ? GAP : "") + keyCap(key) + " " + PANEL_TEXT + label;
    hits.push({ id, action, from, to });
    used = to;
  }
  return { line: fill(text, width), hits };
}

/**
 * The Board tasks section for `width` columns; `hits[row]` lists the click targets on that rendered row. A hit
 * without a range covers the row. Columns follow the mock's `.board` grid, two cells apart after the bar: full
 * density id, title (40), stage, now; med id, stage, now, then the title on a second row under the id; min id and
 * stage, then the title. The empty states are paragraphs one blank row apart (none when the view is short).
 */
export function renderBoard(tasks: readonly BoardTaskStatus[], proposed: BoardPlan | undefined, selected: string | null | undefined,
  width: number, options: { readonly now: number; readonly short: boolean; readonly unavailable?: string; readonly focused?: boolean; readonly selectReview?: boolean }):
  { readonly lines: string[]; readonly hits: ReadonlyMap<number, readonly BoardHit[]>; readonly selectedRow: number } {
  const density = agentDensity(width), lines: string[] = [], hits = new Map<number, BoardHit[]>();
  const push = (line: string, rowHits?: readonly BoardHit[]) => { if (rowHits?.length) hits.set(lines.length, [...rowHits]); lines.push(line); };
  const paragraph = () => { if (!options.short) push(fill("", width)); };
  const shown = selected === null ? undefined : shownTask(tasks, selected);
  let selectedRow = -1;

  if (tasks.length) {
    const { lead, alert } = summaryParts(tasks, density === "min");
    push(fill(" " + PANEL_MUTED + lead + PANEL_WARNING + alert, width));
  }
  if (proposed) {
    const count = proposed.tasks.length;
    if (options.selectReview) selectedRow = lines.length;
    push(fill((options.selectReview ? (options.focused === false ? PANEL_MUTED : PANEL_BLUE) + "▌" : " ") + PANEL_WARNING + `◆ A plan is waiting for you: ${plainLabel(proposed.title)} · ${count} ${count === 1 ? "task" : "tasks"}`, width, options.selectReview ? PANEL_SELECTED : PANEL_BG),
      [{ action: "review" }]);
  }

  if (!tasks.length) {
    if (proposed) paragraph();
    if (options.unavailable !== undefined) {
      push(fill(" " + BOLD + PANEL_TEXT + "The board is not available in this session." + UNBOLD, width));
      paragraph();
      lines.push(...wrapped(plainLabel(options.unavailable), PANEL_MUTED, width, 4));
    } else {
      push(fill(" " + BOLD + PANEL_TEXT + "Nothing from this session is on the board." + UNBOLD, width));
      paragraph();
      lines.push(...wrapped("A larger plan can be split into tasks that run there under corral's gate and review.",
        PANEL_MUTED, width, options.short ? 2 : 4));
      paragraph();
      const label = "Plan board tasks from this conversation…";
      push(fill(" " + PANEL_BLUE + label, width), [{ action: "plan", from: 1, to: Math.min(width, 1 + visibleWidth(label)) }]);
    }
    return { lines, hits, selectedRow };
  }

  const idWidth = Math.min(ID_MAX, Math.max(ID_MIN, ...tasks.map(task => visibleWidth(plainLabel(task.id)))));
  // The bar, the id and the stage with their gaps, and one cell of right inset; the rest is shared by title and now.
  const rest = width - (1 + GAP.length + idWidth + GAP.length + STAGE + GAP.length + 1);
  const titleWidth = density === "full" ? Math.max(0, Math.min(TITLE_MAX, rest - GAP.length - NOW_MIN)) : 0;
  const nowWidth = density === "full" ? rest - GAP.length - titleWidth : rest;
  const stageWidth = density === "min" ? Math.max(1, rest + GAP.length + STAGE) : STAGE;
  if (density === "full") {
    const base = PANEL_BG + PANEL_MUTED;
    push(fill(PANEL_MUTED + " " + GAP + cell("task", idWidth, base) + GAP + cell("title", titleWidth, base) + GAP + cell("stage", STAGE, base)
      + GAP + cell("now", nowWidth, base), width));
  }
  tasks.forEach((task, index) => {
    const isSelected = task.id === shown?.id, surface = isSelected ? PANEL_SELECTED : PANEL_BG, base = surface + PANEL_TEXT;
    if (isSelected) selectedRow = lines.length;
    const stage = boardStage(task, tasks), color = toneColor(stage.tone);
    // A task an agent, the gate or a review is working on shows the spinner.
    const glyph = stage.tone === "accent" ? spinner(options.now, index) : stage.glyph;
    const lead = (isSelected ? (options.focused === false ? PANEL_MUTED : PANEL_BLUE) + "▌" : " ") + GAP + BOLD + PANEL_TEXT + cell(plainLabel(task.id), idWidth, base) + UNBOLD;
    const stageCell = cell(color + glyph + " " + stage.word, stageWidth, base);
    const nowColor = stage.tone === "warning" || stage.tone === "error" ? color : PANEL_MUTED;
    const title = PANEL_TEXT + plainLabel(task.title);
    const rowHit: BoardHit[] = [{ id: task.id }];
    if (density === "full") {
      push(fill(lead + GAP + cell(title, titleWidth, base) + GAP + stageCell + GAP + cell(nowColor + stage.now, nowWidth, base), width, surface), rowHit);
      return;
    }
    push(fill(lead + GAP + stageCell + (density === "med" ? GAP + cell(nowColor + stage.now, nowWidth, base) : ""), width, surface), rowHit);
    push(fill(" " + GAP + cell(title, width - 4, base), width, surface), rowHit);
  });

  if (shown) {
    const row = actionLine(shown.id, options.unavailable === undefined ? ["details", "refresh"] : ["details"], width);
    push(row.line, row.hits);
  }
  return { lines, hits, selectedRow };
}
