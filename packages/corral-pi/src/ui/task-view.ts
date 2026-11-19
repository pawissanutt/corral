// The bound task's spec and status, read from `corral show --json`. Presentation only: the engine's task record is
// canonical. The page is finished rows, each at most `width` cells: a chip header with the read note, the title, then
// the task's own body sections and the bound run. From SPLIT_MIN columns a facts column sits beside the prose. State
// words come from the shared helpers.
import { truncateToWidth, visibleWidth } from "@earendil-works/pi-tui";
import type { HostIdentity } from "../roles/types.ts";
import {
  bold, chip, chipHeader, field, glyphRows, sectionRule, stateBanner, stateRow, wrap, type Tone,
} from "./panel-parts.ts";
import { pad, plainLabel, PANEL_CODE, PANEL_MUTED, PANEL_RULE, PANEL_TEXT } from "./panels-view.ts";
import type { ReadState } from "./read-state.ts";
import type { TaskRun, TaskStore, TaskView } from "./task-store.ts";

const STATUS_GLYPH: Readonly<Record<string, string>> = { running: "●", review: "◐", done: "✓", blocked: "⚠", dropped: "✗", backlog: "○", ready: "○", idea: "·" };
export function statusChip(status: string): string {
  return `${STATUS_GLYPH[status] ?? "◇"} ${status.replaceAll("_", " ")}`;
}
/** Header facts: task ID, title and the small chips. Undefined when no task is bound. */
export function taskHeadline(identity: HostIdentity, store: TaskStore): { readonly id: string; readonly title: string; readonly chips: readonly string[] } | undefined {
  if (!identity.task) return undefined;
  const data = store.current?.task;
  if (!data) return { id: identity.task, title: store.error ? "Task unavailable" : "Reading task…", chips: [] };
  const chips = [statusChip(data.status), data.priority, data.size].filter((chip): chip is string => !!chip).map(plainLabel);
  return { id: data.id, title: plainLabel(data.title), chips: store.stale ? [...chips, "stale"] : chips };
}

export interface TaskViewOptions {
  readonly now: number;
  /** Append the reading rules and, for a failed or retained read, the whole failure reason. */
  readonly legend: boolean;
}

/** View width from which a facts column sits beside the prose. */
const SPLIT_MIN = 105;
const FACTS_WIDTH = 34;
const FACT_LABEL = 10;
const RUN_LABEL = 8;
/** Prose is wrapped to at most this many cells. */
const PROSE_MAX = 100;
const STATUS_TONE: Readonly<Record<string, Tone>> = { done: "ok", blocked: "warn" };

const muted = (text: string) => PANEL_MUTED + text + PANEL_TEXT;
const clean = (text: string) => text.replaceAll("\t", "  ").replace(/[\u0000-\u001f\u007f-\u009f]/g, " ");
const fit = (row: string, width: number) => visibleWidth(row) > width ? truncateToWidth(row, width, "…") : row;

function age(at: string | null | undefined, now: number): string {
  const time = at ? Date.parse(at) : NaN;
  if (Number.isNaN(time)) return "";
  const minutes = Math.max(0, Math.round((now - time) / 60_000));
  return minutes < 60 ? `${minutes}m` : minutes < 48 * 60 ? `${Math.round(minutes / 60)}h` : `${Math.round(minutes / 1440)}d`;
}
/** `10m ago`; an unreadable time is shown as recorded rather than dropped. */
function when(at: string | null | undefined, now: number): string {
  if (!at) return "";
  const since = age(at, now);
  return since ? `${since} ago` : plainLabel(at);
}

const GRAPHEMES = new Intl.Segmenter(undefined, { granularity: "grapheme" });
/** `text` cut into rows of at most `width` cells, at any character: code keeps its spaces. */
function hardWrap(text: string, width: number): string[] {
  const rows: string[] = [];
  let row = "", used = 0;
  for (const { segment } of GRAPHEMES.segment(text)) {
    const cells = visibleWidth(segment);
    if (row && used + cells > width) { rows.push(row); row = ""; used = 0; }
    row += segment; used += cells;
  }
  rows.push(row);
  return rows;
}
/** `first` before the first wrapped row of `text`, blanks of the same width before the rest. */
function hanging(first: string, text: string, width: number): string[] {
  const hang = visibleWidth(first);
  const parts = text ? wrap(text, Math.max(1, width - hang)) : [""];
  return parts.map((part, i) => (i ? " ".repeat(hang) : first) + part);
}
/** Free text, a line at a time, wrapped under `indent` blanks; runs of blank lines collapse to one. */
function prose(text: string, indent: number, width: number): string[] {
  const rows: string[] = [];
  for (const raw of text.split(/\r?\n/)) {
    const line = plainLabel(raw);
    if (!line) { if (rows.length && rows.at(-1) !== "") rows.push(""); continue; }
    rows.push(...hanging(" ".repeat(indent), line, width));
  }
  while (rows.at(-1) === "") rows.pop();
  return rows;
}

/** One body line: `first` leads its first row, `hang` blanks lead the wrapped rows. */
interface BodyLine { readonly text: string; readonly first: string; readonly hang: number; readonly code: boolean }
interface BodySection { readonly heading: string; readonly lines: readonly (BodyLine | null)[] }

const SECTION = /^#{1,3}\s+(.*)$/;
const FENCE = /^\s*(```|~~~)/;
const BULLET = /^(\s*)([-*+]|\d{1,3}[.)])\s+(.*)$/;
/**
 * The body's own sections in order; text before the first heading is "Summary". Fenced code is kept verbatim, in
 * order, and never read as headings; a bullet's indented lines continue it; `null` is one blank row.
 */
function sections(body: string): BodySection[] {
  const result: BodySection[] = [];
  let heading = "Summary", lines: (BodyLine | null)[] = [];
  const close = () => {
    while (lines.at(-1) === null) lines.pop();
    while (lines[0] === null) lines.shift();
    if (lines.length) result.push({ heading, lines });
    lines = [];
  };
  let fenced = false, hang = 0;
  for (const raw of body.split(/\r?\n/)) {
    const text = clean(raw);
    if (FENCE.test(text)) { fenced = !fenced; hang = 0; continue; }
    if (fenced) { lines.push({ text: text.trimEnd(), first: "  ", hang: 2, code: true }); continue; }
    const title = SECTION.exec(text);
    if (title) { close(); heading = plainLabel(title[1] ?? "") || "Section"; hang = 0; continue; }
    if (!text.trim()) { if (lines.length && lines.at(-1) !== null) lines.push(null); hang = 0; continue; }
    const bullet = BULLET.exec(text);
    if (bullet) {
      const mark = bullet[2] ?? "-", depth = Math.min(4, Math.floor((bullet[1] ?? "").length / 2));
      const first = `${" ".repeat(depth * 2)}${/^\d/.test(mark) ? mark : "•"} `;
      hang = visibleWidth(first);
      lines.push({ text: plainLabel(bullet[3] ?? ""), first, hang, code: false });
      continue;
    }
    if (hang && /^\s{2,}/.test(text)) { lines.push({ text: plainLabel(text), first: " ".repeat(hang), hang, code: false }); continue; }
    hang = 0;
    lines.push({ text: plainLabel(text), first: "", hang: 0, code: false });
  }
  close();
  return result;
}
function bodyRows(lines: readonly (BodyLine | null)[], width: number): string[] {
  return lines.flatMap(line => {
    if (!line) return [""];
    const room = Math.max(1, width - line.hang);
    const parts = line.code ? hardWrap(line.text, room) : line.text ? wrap(line.text, room) : [""];
    return parts.map((part, i) => (i ? " ".repeat(line.hang) : line.first) + (line.code ? PANEL_CODE + part + PANEL_TEXT : part));
  });
}

/** `T-950  Usage redesign`: the id in bold, the title wrapped under itself; the id alone while the title is unread. */
function titleRows(id: string, title: string | undefined, width: number): string[] {
  const lead = visibleWidth(id) + 2;
  if (!title) return [truncateToWidth(bold(id), width, "…")];
  if (lead > Math.max(8, width - 12)) return [truncateToWidth(bold(id), width, "…"), ...wrap(title, width)];
  return hanging(`${bold(id)}  `, title, width);
}

/** Facts of the run the page shows, in the order the record states them; a field the run lacks is left out. */
function runFacts(run: TaskRun, now: number): [string, string][] {
  const out: [string, string][] = [];
  const add = (label: string, value: string | null | undefined) => { if (value) out.push([label, plainLabel(value)]); };
  add("role", run.role); add("profile", run.profile); add("model", run.model); add("outcome", run.outcome);
  add("started", when(run.started, now)); add("ended", when(run.ended, now));
  return out;
}

/** The prose column: the body's sections and the bound run. */
function proseRows(data: TaskView, identity: HostIdentity, width: number, now: number, wide: boolean): string[] {
  const rows: string[] = [];
  const text = Math.min(width, PROSE_MAX);
  const open = (label: string, right: readonly string[] = []) => {
    if (rows.length) rows.push("");
    rows.push(sectionRule(label, width, right));
  };
  const body = sections(data.task.body);
  for (const section of body) { open(section.heading); rows.push(...bodyRows(section.lines, text)); }
  if (!body.length) { open("Summary"); rows.push(muted("○ No task description")); }

  const bound = identity.run, found = bound ? data.runs.find(run => run.id === bound) : undefined;
  const run = found ?? data.runs.at(-1);
  if (bound || run) {
    open(`Run ${plainLabel(run?.id ?? bound ?? "")}${found ? " · this session" : ""}`);
    if (bound && !found) rows.push(...glyphRows(`○ Run ${bound} is not in this task record${run ? ` · showing the latest run, ${run.id}` : " yet"}`, width));
    if (run && !wide) for (const [label, value] of runFacts(run, now)) rows.push(...field(label, value, width, RUN_LABEL));
    if (run?.summary?.trim()) rows.push(muted("summary"), ...prose(run.summary, 2, text));
    const others = run ? data.runs.length - 1 : data.runs.length;
    if (others > 0) rows.push(muted(`${others} ${run === data.runs.at(-1) ? "earlier" : "other"} run${others === 1 ? "" : "s"} · Browse history in F2`));
  }
  return rows.map(row => fit(row, width));
}

/** The facts column: every task field as a label and value, then the bound run's. */
function factRows(data: TaskView, identity: HostIdentity, width: number, now: number): string[] {
  const rows = [sectionRule("Facts", width)], task = data.task;
  const add = (label: string, value: string | null | undefined) => rows.push(...field(label, value ?? "", width, FACT_LABEL));
  add("status", `${task.status.replaceAll("_", " ")}${task.blocked ? ` · ${plainLabel(task.blocked.reason)}` : ""}`);
  add("kind", task.kind); add("size", task.size); add("priority", task.priority); add("project", task.project);
  const bound = identity.run, run = (bound ? data.runs.find(r => r.id === bound) : undefined) ?? data.runs.at(-1);
  if (run) {
    rows.push("", sectionRule("Run", width));
    add("run", `${run.id}${run.id === bound ? " · this session" : ""}`);
    for (const [label, value] of runFacts(run, now)) add(label, value);
  }
  return rows.map(row => fit(row, width));
}

/** The status chip, then (below SPLIT_MIN) the small chips; the shared header prepends the read-state chip and adds the read note. */
function chipRow(data: TaskView | undefined, state: ReadState, width: number, now: number, wide: boolean): string[] {
  const chips: string[] = [];
  if (data) {
    const task = data.task;
    chips.push(chip(statusChip(task.status), STATUS_TONE[task.status] ?? "none"));
    if (!wide) for (const value of [task.kind, task.size, task.priority, task.project]) if (value) chips.push(chip(value));
  }
  return chipHeader(chips, width, state, now);
}

function legend(state: ReadState, width: number): string[] {
  const rows = ["", sectionRule("Reading rules", width), ...stateBanner(state, "task", width, true)];
  for (const rule of [
    "r rereads the task record; nothing on this page changes it.",
    "Status, kind, size, priority and project are the task's own fields. One that is missing was not recorded; it is never zero.",
  ]) rows.push(...wrap(rule, width).map(muted));
  return rows;
}

function unboundWhy(identity: HostIdentity, state: ReadState): string {
  if (state.reason === "no_reader") return "no task reader is attached to this session.";
  return identity.role === "assistant" ? "this assistant session holds no task." : "this session has no task bound.";
}

/** The Task page as finished rows, each at most `width` cells. Never reads: it draws what `store` holds. */
export function taskLines(identity: HostIdentity, store: TaskStore, width: number, options: TaskViewOptions): string[] {
  const w = Math.max(1, Math.floor(width)), state = store.state, data = store.current, now = options.now;
  const id = plainLabel(identity.task ?? "");
  const rows: string[] = [];
  if (state.phase === "not_applicable") {
    if (id) rows.push(...titleRows(id, undefined, w));
    rows.push(...stateRow(state, "task", w, unboundWhy(identity, state)));
    return rows.map(row => fit(row, w));
  }
  const wide = w >= SPLIT_MIN;
  rows.push(...chipRow(data, state, w, now, wide), ...titleRows(id, data && plainLabel(data.task.title), w), ...stateBanner(state, "task", w));
  if (data) {
    if (wide) {
      const left = w - FACTS_WIDTH - 3, main = proseRows(data, identity, left, now, true), facts = factRows(data, identity, FACTS_WIDTH, now);
      for (let i = 0; i < Math.max(main.length, facts.length); i++) rows.push(`${pad(main[i] ?? "", left)} ${PANEL_RULE}│${PANEL_TEXT} ${facts[i] ?? ""}`);
    } else rows.push(...proseRows(data, identity, w, now, false));
  } else if (state.phase === "failed") {
    rows.push(...stateRow(state, "task", w, "○ No task content is shown · acceptance is not inferred"));
  } else rows.push(...stateRow(state, "task", w));
  if (options.legend) rows.push(...legend(state, w));
  return rows.map(row => fit(row, w));
}
