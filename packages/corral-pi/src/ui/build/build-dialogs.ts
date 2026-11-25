// The three docked dialogs of a build session: the exit choice, a child's question and the board plan.
import { basename } from "node:path";
import { truncateToWidth, visibleWidth, wrapTextWithAnsi } from "@earendil-works/pi-tui";
import { boardLabel } from "../../build/board.ts";
import type { BoardPlan, BoardPreview, BuildPanelState, ChildSnapshot, ExitOption, LandCandidate } from "../../build/types.ts";
import { plainLabel, PANEL_ERROR, PANEL_MUTED, PANEL_TEXT } from "../panels-view.ts";
import type { ChoiceDialog } from "./choice-dock.ts";
import { candidateLines, fastForward } from "./land-steps.ts";
import { taskMetadataLabel, wrapMetadata } from "./task-metadata.ts";

export type PreviewState = { readonly state: "loading" } | { readonly state: "ready"; readonly value: BoardPreview }
  | { readonly state: "failed"; readonly error: string };

/** Preview table tiers by the dialog's inner width (the dock columns minus the border): 104 and 60 dock columns. */
const PREVIEW_FULL = 100;
const PREVIEW_MED = 56;
const BOLD = "\x1b[1m", NORMAL = "\x1b[22m";

/**
 * "mitiplay · landing needs an approving review and the full gate"; outside a workspace, the reason instead of a policy.
 * A bare session has no corral project and no policy: it says what Land does there (a local fast-forward, no gate).
 */
export function policyLabel(state: BuildPanelState): string {
  const session = state.session, project = plainLabel(session.project ?? basename(session.repoRoot));
  if (session.bare) {
    if (session.repository === false) return `${project} · bare · not a Git repository, so nothing can land`;
    const base = session.baseBranch ? plainLabel(session.baseBranch) : "the branch it started from";
    return `${project} · bare · landing fast-forwards ${base} locally, no gate; review recommended`;
  }
  if (!session.workspaceRoot) return `${project} · outside a corral workspace`;
  const policy = state.settings.exitPolicy;
  return `${project} · ${policy === "review" ? "landing needs an approving review and the full gate" : policy === "gate" ? "landing needs the full gate" : "landing is off"}`;
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

/**
 * Keep, Land and Iterate as the host offers them; a Land that needs the owner's reason says why, ahead of its detail.
 * With a candidate the host prepared, the body says where it stands, and a Land that can run names the exact
 * fast-forward it authorizes first; without one (a host that prepares none) Land reads as the host words it.
 */
export function exitDialog(state: BuildPanelState, options: readonly ExitOption[], episode: number, live: number, candidate?: LandCandidate): ChoiceDialog {
  const policy = policyLabel(state), move = candidate?.state === "ready" ? fastForward(candidate) : undefined;
  return {
    // A new candidate is a new decision: the selection starts over.
    key: `exit:${episode}` + (candidate ? `:${candidate.state}:${candidate.baseTip ?? ""}:${candidate.head ?? ""}` : ""),
    title: "How should this leave the build session?",
    short: "How should this leave?",
    meta: live ? `${policy} · ${live} ${live === 1 ? "agent" : "agents"} still running` : policy,
    ...(candidate ? { body: (width: number, rows: number) => candidateLines(candidate, width).slice(0, Math.max(0, rows)) } : {}),
    options: options.map(option => {
      const open = option.unavailable === undefined;
      const detail = option.choice === "land" && open && move ? `fast-forward ${move} · ${option.detail}` : option.detail;
      return {
        label: option.label,
        detail: option.needsReason !== undefined && open ? `needs your reason: ${option.needsReason} · ${detail}` : detail,
        ...(open ? {} : { unavailable: option.unavailable }),
      };
    }),
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
export function planDialog(plan: BoardPlan, preview: PreviewState, episode: number, error?: string, retry?: boolean): ChoiceDialog {
  const count = plan.tasks.length, edges = planEdges(plan);
  const projects = new Set(plan.tasks.map(task => task.project ?? plan.project).filter(Boolean));
  const waiting = preview.state === "loading" ? "the preview is still being read" : preview.state === "failed" ? "the preview failed" : undefined;
  const meta = [`${count} ${count === 1 ? "task" : "tasks"}`, projects.size && `${projects.size} ${projects.size === 1 ? "project" : "projects"}`,
    `${edges} dependency ${edges === 1 ? "edge" : "edges"}`, retry === undefined ? "preview only" : "recover original choice"];
  const locked = retry === undefined ? undefined : "the original filing may already have succeeded";
  const table = (width: number, rows: number) => preview.state === "ready" ? previewLines(plan, preview.value, width, rows)
    : preview.state === "failed" ? wrapped(preview.error, PANEL_ERROR, width, rows)
    : wrapped("Preparing the board preview…", PANEL_MUTED, width, rows);
  return {
    key: `plan:${episode}`,
    title: retry === undefined ? `File ${count} ${count === 1 ? "task" : "tasks"} on the board?` : "Recover the original filing?",
    short: retry === undefined ? `File ${count} ${count === 1 ? "task" : "tasks"}?` : "Recover the filing?",
    meta: meta.filter(Boolean).join(" · "),
    body: (width, rows) => {
      if (!error) return table(width, rows);
      const head = wrapped(error, PANEL_ERROR, width, Math.min(rows, 2));
      return [...head, ...table(width, rows - head.length)];
    },
    options: [
      { label: retry === true ? "Retry file and start" : "File and start",
        detail: retry === true ? "recover the original filing, without duplicating tasks" : `queue these ${count} ${count === 1 ? "task" : "tasks"}; they start as slots free`,
        unavailable: retry === false ? locked : waiting },
      { label: retry === false ? "Retry file only" : "File only",
        detail: retry === false ? "recover the original filing, without duplicating tasks" : "they land in Backlog; you start them from the board",
        unavailable: retry === true ? locked : waiting },
      { label: "Not now", detail: "nothing is written; the plan is discarded", unavailable: locked },
    ],
  };
}

function cell(text: string, width: number): string {
  if (width <= 0) return "";
  const clipped = visibleWidth(text) > width ? truncateToWidth(text, width - 1, "") + "…" : text;
  return clipped + " ".repeat(Math.max(0, width - visibleWidth(clipped)));
}

/**
 * The preview table fits kind/profile/dependencies by width. Metadata has its own rows, not the title's
 * truncation budget. When rows run out, point to the complete proposal in the Chat tool result.
 */
export function previewLines(plan: BoardPlan, preview: BoardPreview, width: number, rows: number): string[] {
  if (rows < 1 || width < 1) return [];
  const tasks = new Map(preview.tasks.map(task => [task.key, task]));
  const id = (key: string) => boardLabel(tasks.get(key)?.id ?? key);
  const label = (task: BoardPlan["tasks"][number]) => plainLabel(task.title);
  if (rows === 1) {
    return [PANEL_MUTED + truncateToWidth(`${plan.tasks.length} ${plan.tasks.length === 1 ? "task" : "tasks"} · details in Chat`, width, "…")];
  }
  const tier = width >= PREVIEW_FULL ? "full" : width >= PREVIEW_MED ? "med" : "min";
  const gap = "  ", task = 6;
  const starts = (depends: readonly string[]) => depends.length ? `after ${depends.map(id).join(", ")}` : "no dependencies";
  const kind = (value: BoardPlan["tasks"][number]) => value.kind == null ? "unset" : boardLabel(value.kind, true);
  const taskId = (value: BoardPlan["tasks"][number]) => PANEL_TEXT + BOLD + cell(id(value.key), task) + NORMAL;
  let layout: (value: BoardPlan["tasks"][number]) => string;
  let header: string | undefined;
  if (tier === "full") {
    const title = Math.min(width - task - 12 - 10 - gap.length * 4 - 17, Math.max(5, ...plan.tasks.map(value => visibleWidth(label(value)))));
    const rest = Math.max(0, width - task - title - 12 - 10 - gap.length * 4);
    layout = value => taskId(value) + gap + cell(label(value), title) + gap + PANEL_MUTED
      + cell(kind(value), 12) + gap + cell(plainLabel(value.profile ?? "default"), 10) + gap + cell(starts(value.depends), rest);
    header = PANEL_MUTED + cell("task", task) + gap + cell("title", title) + gap + cell("kind", 12) + gap + cell("profile", 10) + gap + cell("starts", rest);
  } else if (tier === "med") {
    const title = Math.max(0, width - task - 22 - gap.length * 2);
    layout = value => taskId(value) + gap + cell(label(value), title) + gap + PANEL_MUTED + cell(starts(value.depends), 22);
  } else {
    layout = value => taskId(value) + gap + cell(label(value), Math.max(0, width - task - gap.length));
  }
  const metadata = (value: BoardPlan["tasks"][number]) => taskMetadataLabel(tasks.get(value.key) ?? value);
  const project = (value: BoardPlan["tasks"][number]) => {
    const project = tasks.get(value.key)?.project ?? value.project ?? plan.project;
    return project && boardLabel(project);
  };
  const lines = plan.tasks.flatMap(value => {
    const key = id(value.key);
    // The title row's key column is compact; the details must carry any key it clips.
    const identity = [visibleWidth(key) > task ? key : undefined, project(value), `kind: ${kind(value)}`, starts(value.depends)].filter(Boolean).join(" · ");
    return [layout(value),
      ...wrapMetadata(identity, width - 2).map(line => PANEL_MUTED + "  " + line),
      ...wrapMetadata(metadata(value), width - 2).map(line => PANEL_TEXT + "  " + line)];
  });
  if (header && lines.length < rows) lines.unshift(header);
  if (lines.length > rows) {
    const omitted = lines.length - (rows - 1);
    lines.length = rows - 1;
    const first = plan.tasks[0];
    if (rows === 2 && first && metadata(first) && width > task + gap.length) {
      // With just one content row before the omission notice, prefer identified metadata to a clipped title.
      lines[0] = taskId(first) + gap + truncateToWidth([project(first), metadata(first)].filter(Boolean).join(" · "), width - task - gap.length, "…");
    }
    lines.push(PANEL_MUTED + `… ${omitted} more ${omitted === 1 ? "row" : "rows"} · see plan in Chat`);
  }
  return lines.map(line => truncateToWidth(line, width, "…"));
}
