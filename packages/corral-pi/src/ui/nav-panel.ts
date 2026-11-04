// The side panel is quick navigation: one row per destination with its key and one small badge,
// at most one sub-line, sized never to scroll. Detail lives in the main views, not here.
import { basename } from "node:path";
import type { MainView } from "./workbench.ts";
import type { ReviewStore } from "./review-store.ts";
import type { InspectionStore } from "./inspection.ts";
import type { DecisionInbox } from "./decision-inbox.ts";
import { helperActive } from "./activity-sidebar.ts";
import { splitRow } from "./panels-chrome.ts";
import {
  pad, plainLabel, PANEL_BG, PANEL_BLUE, PANEL_MUTED, PANEL_TEXT, PANEL_RULE, PANEL_SUCCESS, PANEL_WARNING,
  PANEL_ERROR, PANEL_SELECTED, RESET, type PanelSnapshot,
} from "./panels-view.ts";

export interface NavRow {
  readonly view: MainView; readonly key: string; readonly label: string;
  readonly badge: string; readonly sub?: string; readonly footer?: boolean;
}
export interface NavSource {
  readonly taskBound: boolean; readonly proof: boolean; readonly snapshot: PanelSnapshot | null;
  readonly review: ReviewStore; readonly inspection: InspectionStore; readonly inbox: DecisionInbox;
  readonly view: MainView;
}

function elapsed(start: number | undefined, now: number): string {
  if (start === undefined) return "";
  const seconds = Math.max(0, Math.round((now - start) / 1000));
  return seconds < 60 ? ` · ${seconds}s` : ` · ${Math.floor(seconds / 60)}m${String(seconds % 60).padStart(2, "0")}s`;
}
function spend(budget: unknown): number | undefined {
  const nanos = budget && typeof budget === "object" ? (budget as { usd_nanos?: unknown }).usd_nanos : undefined;
  return typeof nanos === "number" && Number.isFinite(nanos) && nanos >= 0 ? nanos / 1e9 : undefined;
}

export function navRows(source: NavSource, now = Date.now()): NavRow[] {
  const { snapshot, review, inspection, inbox, view } = source;
  const rows: NavRow[] = [];
  if (source.taskBound) {
    const steps = snapshot?.ledger?.phases.flatMap(phase => phase.steps) ?? [];
    const done = steps.filter(step => step.status.state === "done").length;
    const current = steps.find(step => step.status.state === "in_progress") ?? steps.find(step => step.status.state === "pending");
    const waiting = current && snapshot?.ledger?.waits?.some(wait => wait.step === current.id);
    rows.push({ view: "task", key: "F5", label: "Task", badge: steps.length ? PANEL_TEXT + `☰ ${done}/${steps.length}` : "",
      ...(current ? { sub: PANEL_BLUE + "● " + PANEL_MUTED + plainLabel(current.label) + (waiting ? PANEL_WARNING + " ⏸" : "") } : {}) });
  }
  rows.push({ view: "conversation", key: "F9", label: "Chat", badge: snapshot?.streaming ? PANEL_BLUE + "●" : "" });
  // Untracked and binary files have no line counts: never show them as +0−0.
  let added = 0, removed = 0, counted = 0;
  for (const stat of review.stats.values()) { if (stat.added === null) continue; counted++; added += stat.added; removed += stat.removed ?? 0; }
  const files = review.checkedAt ? review.files.length : 0;
  rows.push({ view: "changes", key: "F7", label: "Changes",
    badge: review.scopeRequired ? PANEL_WARNING + "?" : files ? counted ? PANEL_SUCCESS + `+${added}` + PANEL_ERROR + `−${removed}` : PANEL_TEXT + `Δ${files}` : "",
    ...(review.scopeRequired ? { sub: "choose project" } : review.error ? { sub: PANEL_WARNING + plainLabel(review.error) }
      : files ? { sub: `${files} file${files === 1 ? "" : "s"} · ${plainLabel(basename(review.files[0]?.path ?? ""))}` } : {}) });
  const jobs = (snapshot?.jobs ?? []).filter(job => job.state === "running");
  const helpers = (snapshot?.helpers ?? []).filter(helperActive);
  const active = jobs.length + helpers.length, job = jobs[0], helper = helpers[0];
  rows.push({ view: "activity", key: "F8", label: "Activity", badge: active ? PANEL_BLUE + `● ${active}` : "",
    ...(job ? { sub: plainLabel(job.detail?.command ?? job.job) + elapsed(job.detail?.startedAt, now) }
      : helper ? { sub: plainLabel(helper.prompt) + elapsed(helper.startedAt, now) } : {}) });
  if (source.proof) {
    const data = inspection.current, gate = data?.gate, verdict = data?.review;
    // Glyphs differ by state, so colour is never the only signal.
    const gateMark = !gate ? PANEL_MUTED + "–" : gate.result === "pass" && gate.fresh ? PANEL_SUCCESS + "✓"
      : gate.result === "fail" ? PANEL_ERROR + "✗" : gate.result === "running" ? PANEL_BLUE + "●" : PANEL_WARNING + "…";
    const reviewMark = !verdict ? PANEL_MUTED + "–" : !verdict.fresh || !verdict.verdict ? PANEL_WARNING + "…" : PANEL_TEXT + "R";
    const tested = data?.criteria.filter(c => c.observation?.status === "pass").length ?? 0;
    rows.push({ view: "evidence", key: "F10", label: "Proof", badge: data ? gateMark + reviewMark : "",
      sub: data ? `${tested}/${data.criteria.length} tested · review ${plainLabel(verdict?.verdict ?? "pending")}` + (inspection.stale ? PANEL_WARNING + " · stale" : "")
        : inspection.error ? PANEL_WARNING + "unavailable" : "not read" });
  }
  const pending = inbox.snapshot?.items.filter(item => item.pending).length ?? 0;
  if (pending || view === "inbox") rows.push({ view: "inbox", key: "F11", label: "Inbox", badge: pending ? PANEL_WARNING + `⚑ ${pending}` : "", footer: true });
  const usd = spend(snapshot?.budget);
  if (usd !== undefined || view === "usage") rows.push({ view: "usage", key: "", label: "Usage", badge: usd === undefined ? "" : PANEL_TEXT + `$${usd.toFixed(2)}`, footer: true });
  return rows;
}

export interface NavFrame { readonly lines: string[]; readonly hits: ReadonlyMap<number, MainView> }
/** Never scrolls: sub-lines drop first (and below 30 columns), then the footer folds into one row. */
export function renderNav(rows: readonly NavRow[], active: (view: MainView) => boolean, width: number, height: number, heading = "GO TO"): NavFrame {
  const main = rows.filter(row => !row.footer), footer = rows.filter(row => row.footer);
  const base = 1 + main.length + (footer.length ? 1 : 0);
  const fold = footer.length > 1 && base + footer.length > height;
  const footerRows = fold ? 1 : footer.length;
  const withSubs = width >= 30 && base + footerRows + main.filter(row => row.sub).length <= height;
  const lines: string[] = [], hits = new Map<number, MainView>();
  const line = (text: string, selected = false) => (selected ? PANEL_SELECTED : PANEL_BG) + pad(text, width) + RESET;
  lines.push(line(" " + PANEL_MUTED + "\x1b[1m" + heading + "\x1b[22m"));
  const item = (row: NavRow, label = row.label, badge = row.badge) => {
    const on = active(row.view);
    const left = (on ? PANEL_BLUE + "▌\x1b[1m" : " ") + (on ? PANEL_BLUE : PANEL_MUTED) + row.key.padEnd(4)
      + (on ? PANEL_BLUE : PANEL_TEXT) + label + "\x1b[22m";
    hits.set(lines.length, row.view);
    lines.push(line(splitRow(left, badge, width - 1), on));
    if (withSubs && row.sub) { hits.set(lines.length, row.view); lines.push(line("      " + PANEL_MUTED + row.sub)); }
  };
  for (const row of main) item(row);
  if (footer.length) {
    lines.push(line(" " + PANEL_RULE + "─".repeat(Math.max(0, width - 2))));
    const first = footer[0];
    if (fold && first) item({ ...first, key: "" }, "More", footer.map(row => row.badge).filter(Boolean).join(PANEL_MUTED + " · "));
    else for (const row of footer) item(row);
  }
  while (lines.length < height - 1) lines.push(line(""));
  if (lines.length < height) lines.push(line(" " + PANEL_MUTED + (width >= 30 ? "F6 inspect · Ctrl+B hide" : "F6 inspect")));
  return { lines: lines.slice(0, Math.max(0, height)), hits };
}
