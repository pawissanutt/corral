// The three docked dialogs of a build session: the exit choice, a child's question and the board plan.
import { basename } from "node:path";
import { truncateToWidth, visibleWidth, wrapTextWithAnsi } from "@earendil-works/pi-tui";
import type { BoardPlan, BoardPreview, BuildPanelState, ChildSnapshot, ExitOption } from "../../build/types.ts";
import { plainLabel, PANEL_ERROR, PANEL_MUTED, PANEL_TEXT } from "../panels-view.ts";
import type { ChoiceDialog } from "./choice-dock.ts";

export type PreviewState = { readonly state: "loading" } | { readonly state: "ready"; readonly value: BoardPreview }
  | { readonly state: "failed"; readonly error: string };

/** Preview table tiers by the dialog's inner width (the dock columns minus the border): 104 and 60 dock columns. */
const PREVIEW_FULL = 100;
const PREVIEW_MED = 56;
const BOLD = "\x1b[1m", NORMAL = "\x1b[22m";

/** "mitiplay policy: gate allowed, review optional"; outside a workspace, the reason instead of a policy. */
export function policyLabel(state: BuildPanelState): string {
  const project = plainLabel(state.session.project ?? basename(state.session.repoRoot));
  if (!state.session.workspaceRoot) return `${project} · outside a corral workspace`;
  const policy = state.settings.exitPolicy;
  return `${project} policy: ${policy === "gate" ? "gate allowed, review optional" : policy === "review" ? "review required" : "no corral exit"}`;
}

/**
 * Text wrapped into at most `rows` rows in `color`; paragraphs stay apart. Overflow ends with a muted
 * `… n more lines` row; a single row gets the whole text clipped with an ellipsis.
 */
function wrapped(text: string, color: string, width: number, rows: number): string[] {
  if (rows < 1 || width < 1) return [];
  const paragraphs = text.split(/\r?\n/).map(plainLabel).filter(Boolean);
  if (rows === 1) return [color + truncateToWidth(paragraphs.join(" "), width, "…")];
  const lines = paragraphs.flatMap(paragraph => wrapTextWithAnsi(paragraph, width));
  if (lines.length <= rows) return lines.map(line => color + line);
  const kept = lines.slice(0, rows - 1).map(line => color + line);
  return [...kept, PANEL_MUTED + `… ${lines.length - kept.length} more lines`];
}

export function exitDialog(state: BuildPanelState, options: readonly ExitOption[], episode: number, live: number): ChoiceDialog {
  const policy = policyLabel(state);
  return {
    key: `exit:${episode}`,
    title: "How should this leave the build session?",
    short: "How should this leave?",
    meta: live ? `${policy} · ${live} ${live === 1 ? "agent" : "agents"} still running` : policy,
    options: options.map(option => ({ label: option.label, detail: option.detail, ...(option.unavailable !== undefined ? { unavailable: option.unavailable } : {}) })),
  };
}

/** The child's options, then `Write my own answer`, which puts the composer in answer mode for it. */
export function questionDialog(child: ChildSnapshot, color: string): ChoiceDialog {
  const question = child.question;
  const model = child.model.slice(child.model.lastIndexOf("/") + 1);
  return {
    key: `question:${child.id}:${question?.askedAt ?? 0}`,
    title: `${child.id} asks`,
    titleColor: color,
    meta: `${child.profile} · ${model} · paused until you answer`,
    ...(question ? { body: (width: number, rows: number) => wrapped(question.question, PANEL_TEXT, width, rows) } : {}),
    options: [
      ...(question?.options ?? []).map(option => ({ label: option.label, ...(option.detail ? { detail: option.detail } : {}) })),
      { label: "Write my own answer", detail: `type it in the composer; Enter sends it to ${child.id}` },
    ],
  };
}

/** Dependency edges the plan declares, counting edges to existing tasks. */
export function planEdges(plan: BoardPlan): number {
  return plan.tasks.reduce((sum, task) => sum + task.depends.length, 0);
}

/** `error` is a failed filing: it heads the body and the options stay available for another try. */
export function planDialog(plan: BoardPlan, preview: PreviewState, episode: number, error?: string): ChoiceDialog {
  const count = plan.tasks.length, edges = planEdges(plan);
  const waiting = preview.state === "loading" ? "the preview is still being read" : preview.state === "failed" ? "the preview failed" : undefined;
  const meta = [plan.project, `${count} ${count === 1 ? "task" : "tasks"}`, `${edges} dependency ${edges === 1 ? "edge" : "edges"}`, "preview only"];
  const table = (width: number, rows: number) => preview.state === "ready" ? previewLines(plan, preview.value, width, rows)
    : preview.state === "failed" ? wrapped(preview.error, PANEL_ERROR, width, rows)
    : wrapped("Reading the preview from corral import --dry-run…", PANEL_MUTED, width, rows);
  return {
    key: `plan:${episode}`,
    title: "File this plan on the board?",
    short: "File this plan?",
    meta: meta.filter(Boolean).join(" · "),
    body: (width, rows) => {
      if (!error) return table(width, rows);
      const head = wrapped(error, PANEL_ERROR, width, Math.min(rows, 2));
      return [...head, ...table(width, rows - head.length)];
    },
    options: [
      { label: "File and start", detail: `your confirmation queues these ${count} ${count === 1 ? "task" : "tasks"} only; they start as slots free`,
        ...(waiting ? { unavailable: waiting } : {}) },
      { label: "File only", detail: "they land in Backlog; you start them from the board", ...(waiting ? { unavailable: waiting } : {}) },
      { label: "Not now", detail: "nothing is written; the plan is discarded" },
    ],
  };
}

function cell(text: string, width: number): string {
  if (width <= 0) return "";
  const clipped = visibleWidth(text) > width ? truncateToWidth(text, width - 1, "") + "…" : text;
  return clipped + " ".repeat(Math.max(0, width - visibleWidth(clipped)));
}

/**
 * The preview table: `task · title · kind · profile · starts` from 100 cells, `task · title · starts`
 * from 56, `task · title` below. Ids come from the preview by plan key; a dependency on another plan
 * task reads as its id. More tasks than rows end with `+ k more tasks`; one row gets a summary.
 */
export function previewLines(plan: BoardPlan, preview: BoardPreview, width: number, rows: number): string[] {
  if (rows < 1 || width < 1) return [];
  const ids = new Map(preview.tasks.map(task => [task.key, task.id]));
  const id = (key: string) => plainLabel(ids.get(key) ?? key);
  if (rows === 1) {
    const list = plan.tasks.map(task => id(task.key)).join(", ");
    return [PANEL_TEXT + truncateToWidth(`${plan.tasks.length} ${plan.tasks.length === 1 ? "task" : "tasks"}: ${list}`, width, "…")];
  }
  const tier = width >= PREVIEW_FULL ? "full" : width >= PREVIEW_MED ? "med" : "min";
  const gap = "  ", task = 6;
  const starts = (depends: readonly string[]) => depends.length ? `after ${depends.map(id).join(", ")}` : "no dependencies";
  const kind = (value: BoardPlan["tasks"][number]) => [value.kind, value.size].filter(Boolean).map(text => plainLabel(text!)).join(" ");
  const taskId = (value: BoardPlan["tasks"][number]) => PANEL_TEXT + BOLD + cell(id(value.key), task) + NORMAL;
  let layout: (value: BoardPlan["tasks"][number]) => string;
  let header: string | undefined;
  if (tier === "full") {
    const title = Math.min(46, Math.max(5, ...plan.tasks.map(value => visibleWidth(plainLabel(value.title)))));
    const rest = Math.max(0, width - task - title - 12 - 10 - gap.length * 4);
    layout = value => taskId(value) + gap + cell(plainLabel(value.title), title) + gap + PANEL_MUTED
      + cell(kind(value), 12) + gap + cell(plainLabel(value.profile ?? "default"), 10) + gap + cell(starts(value.depends), rest);
    header = PANEL_MUTED + cell("task", task) + gap + cell("title", title) + gap + cell("kind", 12) + gap + cell("profile", 10) + gap + cell("starts", rest);
  } else if (tier === "med") {
    const title = Math.max(0, width - task - 22 - gap.length * 2);
    layout = value => taskId(value) + gap + cell(plainLabel(value.title), title) + gap + PANEL_MUTED + cell(starts(value.depends), 22);
  } else {
    layout = value => taskId(value) + gap + cell(plainLabel(value.title), Math.max(0, width - task - gap.length));
  }
  const fits = (header ? 1 : 0) + plan.tasks.length <= rows;
  if (header && !fits && rows - 2 < 1) header = undefined;
  const room = rows - (header ? 1 : 0);
  const shown = plan.tasks.length <= room ? plan.tasks.length : room - 1;
  const lines = [...(header ? [header] : []), ...plan.tasks.slice(0, shown).map(layout)];
  if (shown < plan.tasks.length) lines.push(PANEL_MUTED + `+ ${plan.tasks.length - shown} more tasks`);
  return lines.map(line => truncateToWidth(line, width, ""));
}
