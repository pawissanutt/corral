// The side panel is quick navigation plus two live sections (plan, running work), sized never to
// scroll. Context and spend live in the bottom bar; detail lives in the main views.
import type { MainView } from "./workbench.ts";
import type { ReviewStore } from "./review-store.ts";
import type { InspectionStore } from "./inspection.ts";
import type { DecisionInbox } from "./decision-inbox.ts";
import { helperActive } from "./activity-sidebar.ts";
import { helperKind } from "./helper-view.ts";
import { splitRow } from "./panels-chrome.ts";
import {
  pad, plainLabel, PANEL_BG, PANEL_BLUE, PANEL_MUTED, PANEL_TEXT, PANEL_RULE, PANEL_SUCCESS, PANEL_WARNING,
  PANEL_ERROR, PANEL_SELECTED, RESET, type PanelSnapshot,
} from "./panels-view.ts";

export interface NavRow {
  readonly view: MainView; readonly key: string; readonly label: string;
  readonly badge: string; readonly footer?: boolean;
}
export interface NavItem { readonly text: string; readonly right?: string; readonly state?: "closed" | "current" }
export interface NavSection { readonly title: string; readonly view?: MainView; readonly items: readonly NavItem[] }
export interface NavSource {
  readonly taskBound: boolean; readonly proof: boolean; readonly snapshot: PanelSnapshot | null;
  readonly review: ReviewStore; readonly inspection: InspectionStore; readonly inbox: DecisionInbox;
  readonly view: MainView;
}

function elapsed(start: number | undefined, now: number): string {
  if (start === undefined) return "";
  const seconds = Math.max(0, Math.round((now - start) / 1000));
  return seconds < 60 ? `${seconds}s` : `${Math.floor(seconds / 60)}m${String(seconds % 60).padStart(2, "0")}s`;
}

export function navRows(source: NavSource): NavRow[] {
  const { snapshot, review, inspection, inbox, view } = source;
  const rows: NavRow[] = [];
  if (source.taskBound) rows.push({ view: "task", key: "F5", label: "Task", badge: "" });
  rows.push({ view: "conversation", key: "F9", label: "Chat", badge: snapshot?.streaming ? PANEL_BLUE + "●" : "" });
  // Untracked and binary files have no line counts: never show them as +0−0.
  let added = 0, removed = 0, counted = 0;
  for (const stat of review.stats.values()) { if (stat.added === null) continue; counted++; added += stat.added; removed += stat.removed ?? 0; }
  const files = review.checkedAt ? review.files.length : 0;
  rows.push({ view: "changes", key: "F7", label: "Changes",
    badge: review.scopeRequired ? PANEL_WARNING + "?" : review.error ? PANEL_WARNING + "!"
      : files ? counted ? PANEL_SUCCESS + `+${added}` + PANEL_ERROR + `−${removed}` : PANEL_TEXT + `Δ${files}` : "" });
  const active = (snapshot?.jobs ?? []).filter(job => job.state === "running").length + (snapshot?.helpers ?? []).filter(helperActive).length;
  rows.push({ view: "activity", key: "F8", label: "Activity", badge: active ? PANEL_BLUE + `● ${active}` : "" });
  if (source.proof) {
    const data = inspection.current, gate = data?.gate, verdict = data?.review;
    // Glyphs differ by state, so colour is never the only signal.
    const gateMark = !gate ? PANEL_MUTED + "–" : gate.result === "pass" && gate.fresh ? PANEL_SUCCESS + "✓"
      : gate.result === "fail" ? PANEL_ERROR + "✗" : gate.result === "running" ? PANEL_BLUE + "●" : PANEL_WARNING + "…";
    const reviewMark = !verdict ? PANEL_MUTED + "–" : !verdict.fresh || !verdict.verdict ? PANEL_WARNING + "…" : PANEL_TEXT + "R";
    rows.push({ view: "evidence", key: "F10", label: "Proof",
      badge: data ? gateMark + reviewMark + (inspection.stale ? PANEL_WARNING + "·" : "") : inspection.error ? PANEL_WARNING + "!" : "" });
  }
  const pending = inbox.snapshot?.items.filter(item => item.pending).length ?? 0;
  if (pending || view === "inbox") rows.push({ view: "inbox", key: "F11", label: "Inbox", badge: pending ? PANEL_WARNING + `⚑ ${pending}` : "", footer: true });
  return rows;
}

export function navSections(source: NavSource, now = Date.now()): NavSection[] {
  const { snapshot } = source, sections: NavSection[] = [];
  const ledger = snapshot?.ledger, steps = ledger?.phases.flatMap(phase => phase.steps) ?? [];
  if (steps.length) {
    const waiting = new Set(ledger?.waits?.map(wait => wait.step));
    const current = (steps.find(step => step.status.state === "in_progress") ?? steps.find(step => step.status.state === "pending"))?.id;
    const done = steps.filter(step => step.status.state === "done").length;
    sections.push({ title: `PLAN ${done}/${steps.length}` + (snapshot?.ledgerStale ? " · stale" : ""), ...(source.taskBound ? { view: "task" as const } : {}),
      items: steps.map(step => {
        const state = step.status.state, closed = state === "done" || state === "dropped";
        const mark = state === "done" ? PANEL_SUCCESS + "✓" : state === "dropped" ? PANEL_MUTED + "–" : state === "in_progress" ? PANEL_BLUE + "●" : PANEL_MUTED + "○";
        return { text: mark + " " + (closed ? PANEL_MUTED : step.id === current ? PANEL_BLUE : PANEL_TEXT) + plainLabel(step.label),
          ...(waiting.has(step.id) ? { right: PANEL_WARNING + "⏸" } : {}),
          ...(closed ? { state: "closed" as const } : step.id === current ? { state: "current" as const } : {}) };
      }) });
  }
  const running: NavItem[] = [
    ...(snapshot?.jobs ?? []).filter(job => job.state === "running")
      .map(job => ({ text: plainLabel(job.detail?.command ?? job.job), right: elapsed(job.detail?.startedAt, now) })),
    ...(snapshot?.helpers ?? []).filter(helperActive)
      .map(helper => ({ text: `${helperKind(helper.kind)} · ${plainLabel(helper.prompt)}`, right: elapsed(helper.startedAt, now) })),
  ].map(item => ({ text: PANEL_BLUE + "● " + PANEL_TEXT + item.text, right: PANEL_MUTED + item.right }));
  if (running.length) sections.push({ title: `RUNNING ${running.length}`, view: "activity", items: running });
  return sections;
}

const more = (count: number): NavItem => ({ text: PANEL_MUTED + `… ${count} more` });
function folded(items: readonly NavItem[]): readonly NavItem[] {
  const closed = items.filter(item => item.state === "closed").length;
  return closed > 1 ? [{ text: PANEL_SUCCESS + "✓ " + PANEL_MUTED + `${closed} done` }, ...items.filter(item => item.state !== "closed")] : items;
}
/** A window that keeps the current item (with one item of context before it) and counts the rest. */
function trimmed(items: readonly NavItem[], limit: number): readonly NavItem[] {
  if (items.length <= limit) return items;
  const at = items.findIndex(item => item.state === "current");
  if (limit <= 1) return [items[at] ?? more(items.length)];
  // One line of context before the current item only when the window has room for both.
  const keep = limit - 1, start = at < 0 ? 0 : Math.max(0, Math.min(keep > 1 ? at - 1 : at, items.length - keep));
  const shown = items.slice(start, start + keep);
  return [...shown, more(items.length - shown.length)];
}
/** Shrinks in order: finished plan steps fold, then sections trim to windows, then later sections drop. */
export function fitSections(sections: readonly NavSection[], room: number): NavSection[] {
  const cost = (list: readonly NavSection[]) => list.reduce((n, section) => n + 2 + section.items.length, 0);
  if (cost(sections) <= room) return [...sections];
  let list = sections.map(section => ({ ...section, items: folded(section.items) }));
  if (cost(list) <= room) return list;
  while (list.length && room < 3 * list.length) list = list.slice(0, -1);
  const limits = list.map(() => 1);
  for (let budget = room - 3 * list.length, grew = true; budget > 0 && grew;) {
    grew = false;
    for (let i = 0; i < list.length && budget > 0; i++) {
      if ((limits[i] ?? 0) < (list[i]?.items.length ?? 0)) { limits[i] = (limits[i] ?? 0) + 1; budget--; grew = true; }
    }
  }
  return list.map((section, i) => ({ ...section, items: trimmed(section.items, limits[i] ?? 1) }));
}

export interface NavFrame { readonly lines: string[]; readonly hits: ReadonlyMap<number, MainView> }
export function renderNav(rows: readonly NavRow[], active: (view: MainView) => boolean, width: number, height: number,
  heading = "GO TO", sections: readonly NavSection[] = []): NavFrame {
  const main = rows.filter(row => !row.footer), footer = rows.filter(row => row.footer);
  const fixed = 1 + main.length + (footer.length ? 1 + footer.length : 0) + 1;
  const lines: string[] = [], hits = new Map<number, MainView>();
  const line = (text: string, selected = false) => (selected ? PANEL_SELECTED : PANEL_BG) + pad(text, width) + RESET;
  const rule = () => lines.push(line(" " + PANEL_RULE + "─".repeat(Math.max(0, width - 2))));
  lines.push(line(" " + PANEL_MUTED + "\x1b[1m" + heading + "\x1b[22m"));
  const item = (row: NavRow) => {
    const on = active(row.view);
    const left = (on ? PANEL_BLUE + "▌\x1b[1m" : " ") + (on ? PANEL_BLUE : PANEL_MUTED) + row.key.padEnd(4)
      + (on ? PANEL_BLUE : PANEL_TEXT) + row.label + "\x1b[22m";
    hits.set(lines.length, row.view);
    lines.push(line(splitRow(left, row.badge, width - 1), on));
  };
  for (const row of main) item(row);
  for (const section of fitSections(sections, height - fixed)) {
    rule();
    if (section.view) hits.set(lines.length, section.view);
    lines.push(line(" " + PANEL_MUTED + "\x1b[1m" + section.title + "\x1b[22m"));
    for (const entry of section.items) {
      if (section.view) hits.set(lines.length, section.view);
      lines.push(line(splitRow(" " + entry.text, entry.right ?? "", width - 1)));
    }
  }
  if (footer.length) { rule(); for (const row of footer) item(row); }
  while (lines.length < height - 1) lines.push(line(""));
  if (lines.length < height) lines.push(line(" " + PANEL_MUTED + "Ctrl+B hide"));
  return { lines: lines.slice(0, Math.max(0, height)), hits };
}
