// Changes (F7) split layout: one chip row, a directory tree beside the diff, key hints pinned below.
// Below SPLIT_MIN columns the tree collapses to a one-line file picker above the diff.
// Presentation only: every value comes from the ReviewStore read and the displayed patch.
import { dirname } from "node:path";
import { HStack, ScrollView, VStack, visibleWidth, type Component, type TuiMouseEvent } from "@earendil-works/pi-tui";
import { relevantFile, type Comparison } from "./panels-git.ts";
import { comparisonLabel, fileStatus, type ReviewStore } from "./review-store.ts";
import { plainLabel, type PanelFile, PANEL_BG, PANEL_TEXT, PANEL_MUTED, PANEL_BLUE, PANEL_RULE, PANEL_SUCCESS, PANEL_WARNING, PANEL_ERROR, PANEL_SELECTED, PANEL_TINT, RESET } from "./panels-view.ts";

/** Main-area width at which the tree sits beside the diff; below it a one-line picker is used. */
export const SPLIT_MIN = 105;
export const LIST_WIDTH = 42;
const BAR_CELLS = 6;
const BG_OFF = "\x1b[49m", BOLD = "\x1b[1m", NORMAL = "\x1b[22m";

export type Tone = "ok" | "warn" | "error" | "accent" | "muted";
export interface ChangeRow { readonly path: string; readonly letter: string; readonly added: number | null | undefined; readonly removed: number | null | undefined }
export type ChangesDiff =
  | { readonly kind: "patch"; readonly path: string; readonly status: string; readonly text: string; readonly notice?: string }
  | { readonly kind: "message"; readonly lines: readonly string[] };
export interface ChangesModel {
  readonly comparison: string;
  readonly range: string;
  readonly state: { readonly text: string; readonly tone: Tone };
  readonly scope: string;
  readonly updated: string;
  readonly files: readonly ChangeRow[];
  readonly selected: string | undefined;
  readonly query: string;
  readonly others: readonly { readonly label: string; readonly count: number | undefined }[];
  readonly notes: readonly string[];
  readonly diff: ChangesDiff;
  readonly hunk: number;
}
/** The patch the workbench last accepted; it stays on screen until a matching newer read is applied. */
export interface DisplayedPatch { readonly root: string | undefined; readonly path: string | undefined; readonly comparison: string; readonly text: string; readonly head: string | undefined; readonly base: string | undefined; readonly file: PanelFile | undefined }

const NAMES: Record<Comparison, string> = { candidate: "candidate", session: "since session", all: "working tree", unstaged: "unstaged", staged: "staged" };
const LETTERS: Record<string, string> = { Conflict: "U", New: "?", Renamed: "R", Deleted: "D", Added: "A", Modified: "M" };
const WORDS: Record<string, readonly [string, Tone]> = { U: ["conflict", "error"], "?": ["new", "ok"], R: ["renamed", "accent"], D: ["deleted", "error"], A: ["added", "ok"], M: ["modified", "warn"] };
const fg = (tone: Tone) => tone === "ok" ? PANEL_SUCCESS : tone === "warn" ? PANEL_WARNING : tone === "error" ? PANEL_ERROR : tone === "accent" ? PANEL_BLUE : PANEL_MUTED;
const bg = (tone: Tone) => tone === "ok" ? PANEL_TINT.success : tone === "warn" ? PANEL_TINT.warning : tone === "error" ? PANEL_TINT.error : tone === "accent" ? PANEL_TINT.accent : PANEL_TINT.raised;
const short = (sha: string | undefined) => sha ? sha.slice(0, 8) : "unborn";

export function ago(ms: number): string {
  const s = Math.max(0, Math.round(ms / 1000));
  return s < 60 ? `${s}s` : s < 3600 ? `${Math.floor(s / 60)}m` : s < 172800 ? `${Math.floor(s / 3600)}h` : `${Math.floor(s / 86400)}d`;
}

/** Everything the view shows, read once from the store and the displayed patch. */
export function changesModel(r: ReviewStore, shown: DisplayedPatch, scopeLabel: string, hunk: number, now: number): ChangesModel {
  const label = comparisonLabel(r.comparison);
  const sameScope = shown.root === r.root && shown.path === r.selected && shown.comparison === label;
  const head = sameScope ? shown.head : r.head, base = sameScope ? shown.base : r.comparison === "session" ? r.sessionBase : r.candidateBase;
  const range = r.comparison === "candidate" ? `${short(base)} → ${short(head)}` : r.comparison === "session" ? `${short(base)} → worktree` : r.comparison === "all" ? `${short(head)} → worktree`
    : r.comparison === "unstaged" ? "index → worktree" : `${short(head)} → index`;
  const state: ChangesModel["state"] = r.scopeRequired ? { text: "no project", tone: "warn" } : r.error ? { text: "stale", tone: "error" }
    : !r.checkedAt ? { text: r.refreshing ? "checking…" : "not loaded", tone: "muted" }
    : r.comparison === "session" ? r.baseRewritten ? { text: "base rewritten · from merge-base", tone: "warn" } : { text: "commits + worktree", tone: "muted" }
    : r.comparison !== "candidate" ? { text: "uncommitted", tone: "muted" }
    : r.worktree === undefined ? { text: "committed", tone: "muted" }
    : r.worktree.length ? { text: "committed · worktree dirty", tone: "warn" } : { text: "committed · clean", tone: "ok" };
  const files = r.visible.map(file => {
    const stat = r.stats.get(file.path);
    return { path: file.path, letter: LETTERS[fileStatus(file)] ?? "M", added: stat?.added, removed: stat?.removed };
  });
  const others = r.comparisons.filter(value => value !== r.comparison).map(value => ({ label: NAMES[value],
    count: value === "candidate" || value === "session" ? undefined : r.worktree?.filter(file => relevantFile(file, value)).length }));
  const notes = [...(r.error ? ["STALE / unavailable: " + plainLabel(r.error)] : []), ...(r.limitNotice ? [r.limitNotice] : [])];
  const model = { comparison: NAMES[r.comparison], range, state, scope: `${plainLabel(scopeLabel)} @ ${plainLabel(r.branch)}`,
    updated: r.checkedAt ? `updated ${ago(now - r.checkedAt)} ago` : r.refreshing ? "checking Git…" : "not loaded",
    files, selected: r.selected, query: r.query, others, notes, hunk };
  const message = (...lines: string[]): ChangesModel => ({ ...model, diff: { kind: "message", lines } });
  if (r.scopeRequired) return message("Choose a project", "Workspace-wide changes are not shown.",
    "Press o to choose a project or your scratch worktree.", "This is a read-only view; your session stays bound to Corral.");
  const file = r.current;
  if (!file) return message(r.refreshing ? "Loading comparison…" : r.error ?? (r.selected ? "Selected file is no longer in this comparison." : "No changes in this comparison."));
  if (r.patchPath !== r.selected || r.patchComparison !== r.comparison) return message("Loading selected diff…");
  const unavailable = r.patch.startsWith("Diff unavailable:");
  if (!sameScope) return message(unavailable ? plainLabel(r.patch) : "Waiting for a captured diff matching this scope…");
  if (!r.patchSnapshot && unavailable) return message(plainLabel(r.patch));
  const stale = r.patch !== shown.text || r.patchSnapshot?.head !== shown.head || r.patchSnapshot?.base !== shown.base;
  return { ...model, diff: { kind: "patch", path: file.path, status: LETTERS[fileStatus(shown.file ?? file)] ?? "M", text: shown.text,
    ...(stale ? { notice: "New diff available (content or identity) · r to apply; current hunk/scroll preserved" } : {}) } };
}

/** Files grouped by directory, in Git's path order; root files group under `./`. */
export function changeTree(files: readonly ChangeRow[]): { readonly dir: string; readonly files: readonly ChangeRow[] }[] {
  const groups = new Map<string, ChangeRow[]>();
  for (const file of files) {
    const dir = dirname(file.path) === "." ? "./" : dirname(file.path) + "/";
    const group = groups.get(dir) ?? []; group.push(file); groups.set(dir, group);
  }
  return [...groups].map(([dir, rows]) => ({ dir, files: rows }));
}
/** Cells of the +/− mini bar, scaled to the largest file; any non-zero side keeps at least one cell. */
export function miniBar(added: number, removed: number, largest: number, cells = BAR_CELLS): { readonly plus: number; readonly minus: number } {
  const scale = (n: number) => n > 0 ? Math.max(1, Math.round(n / Math.max(1, largest) * cells)) : 0;
  let plus = scale(added), minus = scale(removed);
  while (plus + minus > cells) { if (plus >= minus) plus--; else minus--; }
  return { plus, minus };
}

type Seg = readonly [style: string, text: string];
/** Plain text cut to `width` cells with an ellipsis, keeping the head or (for paths) the tail. */
export function clip(text: string, width: number, keep: "head" | "tail" = "head"): string {
  if (width <= 0) return "";
  if (visibleWidth(text) <= width) return text;
  const chars = Array.from(text); if (keep === "tail") chars.reverse();
  const out: string[] = []; let used = 0;
  for (const char of chars) { const w = visibleWidth(char); if (used + w > width - 1) break; out.push(char); used += w; }
  return keep === "tail" ? "…" + out.reverse().join("") : out.join("") + "…";
}
/** One row exactly `width` cells wide: left segments clipped to fit, right segments right-aligned. */
function line(left: readonly Seg[], right: readonly Seg[], width: number, base: string): string {
  const rightWidth = right.reduce((n, [, text]) => n + visibleWidth(text), 0);
  const showRight = rightWidth > 0 && rightWidth + 1 <= width;
  const room = showRight ? width - rightWidth - 1 : width;
  let out = base, used = 0;
  for (const [style, text] of left) {
    const cut = clip(text, room - used); if (!cut) break;
    out += style + cut + base; used += visibleWidth(cut); if (cut !== text) break;
  }
  if (showRight) { out += " ".repeat(width - used - rightWidth) + right.map(([style, text]) => style + text + base).join(""); used = width; }
  return out + " ".repeat(Math.max(0, width - used));
}
function wrap(text: string, width: number): string[] {
  const rows: string[] = []; let row = "", used = 0;
  for (const char of Array.from(text)) {
    const w = visibleWidth(char);
    if (used + w > Math.max(1, width) && row) { rows.push(row); row = ""; used = 0; }
    row += char; used += w;
  }
  return [...rows, row];
}
const code = (text: string) => text.replace(/\t/g, "    ").replace(/[\u0000-\u001f\u007f-\u009f]/g, " ");
const counts = (file: ChangeRow) => file.added === null ? "bin" : file.added === undefined ? "" : `+${file.added} −${file.removed ?? 0}`;
const totals = (files: readonly ChangeRow[]): Seg[] => {
  const add = files.reduce((n, f) => n + (f.added ?? 0), 0), del = files.reduce((n, f) => n + (f.removed ?? 0), 0);
  return [[PANEL_SUCCESS, `+${add}`], [PANEL_TEXT, " "], [PANEL_ERROR, `−${del}`]];
};
const filesLabel = (m: ChangesModel) => `${m.files.length} file${m.files.length === 1 ? "" : "s"}${m.query ? ` · /${plainLabel(m.query)}` : ""}`;
function barSegs(file: ChangeRow, largest: number): Seg[] {
  const bar = miniBar(file.added ?? 0, file.removed ?? 0, largest);
  return [[PANEL_SUCCESS, " ".repeat(BAR_CELLS - bar.plus - bar.minus) + "▮".repeat(bar.plus)], [PANEL_ERROR, "▮".repeat(bar.minus)]];
}

/** `candidate ▾  aece0a19 → 7c41d0e2  committed · clean  corral @ task/T-796 … updated 12s ago`. */
export function chipRow(m: ChangesModel, width: number): string {
  const chip = (text: string, back: string, color: string): Seg => [back + color, ` ${text} `];
  const gap: Seg = [PANEL_TEXT, " "];
  const all = { comparison: chip(m.comparison + " ▾", PANEL_SELECTED, PANEL_BLUE), range: chip(m.range, PANEL_TINT.raised, PANEL_TEXT),
    state: chip(m.state.text, bg(m.state.tone), fg(m.state.tone === "muted" ? "muted" : m.state.tone)), scope: chip(m.scope, PANEL_TINT.raised, PANEL_TEXT) };
  const updated: Seg = [PANEL_MUTED, m.updated + " "];
  // Least important chips leave first; the comparison and its state always stay.
  const orders: (readonly (keyof typeof all)[])[] = [["comparison", "range", "state", "scope"], ["comparison", "range", "state"], ["comparison", "state"]];
  for (const [i, order] of orders.entries()) {
    const left = order.flatMap(key => [all[key], gap]);
    const need = left.reduce((n, [, t]) => n + visibleWidth(t), 0) + visibleWidth(updated[1]);
    if (need <= width || i === orders.length - 1) return line(left, need <= width ? [updated] : [], width, NORMAL + PANEL_BG) + RESET;
  }
  return "";
}
const HINTS: readonly (readonly [key: string, full: string, brief: string])[] = [["j/k", "file", "file"], ["n/p", "hunk", "hunk"],
  ["a", "ask about hunk (adds to draft, not sent)", "ask"], ["/", "filter", "filter"], ["c", "comparison", "compare"], ["o", "repository", "repo"], ["Esc", "chat", "chat"]];
/** Key hints for the bottom row, shortening from full labels to bare keys before anything is cut. */
export function hintLine(width: number): string {
  const forms = [HINTS.map(([key, full]) => `${key} ${full}`).join(" · "), HINTS.map(([key, , brief]) => `${key} ${brief}`).join(" · "), HINTS.map(([key]) => key).join(" ")];
  const text = forms.find(form => visibleWidth(form) + 2 <= width) ?? forms[forms.length - 1] ?? "";
  return line([[PANEL_MUTED, "  " + text]], [], width, NORMAL + PANEL_BG) + RESET;
}

export interface ListFrame { readonly lines: string[]; readonly hits: ReadonlyMap<number, string>; readonly selectedRow: number }
/** The left pane: totals, the directory tree with mini bars, then the other comparisons. */
export function listLines(m: ChangesModel, width: number): ListFrame {
  const lines: string[] = [], hits = new Map<number, string>(); let selectedRow = -1;
  const body = width - 1, edge = PANEL_BG + PANEL_RULE + "│" + RESET;
  const push = (left: readonly Seg[], right: readonly Seg[] = [], base = PANEL_BG) => lines.push(line(left, right, body, NORMAL + base) + edge);
  push([[PANEL_TEXT, " " + filesLabel(m)]], m.files.length ? [...totals(m.files), [PANEL_TEXT, " "]] : []);
  push([]);
  const largest = Math.max(1, ...m.files.map(f => (f.added ?? 0) + (f.removed ?? 0)));
  const countWidth = Math.max(0, ...m.files.map(f => visibleWidth(counts(f))));
  for (const group of changeTree(m.files)) {
    push([[PANEL_MUTED, " " + clip(plainLabel(group.dir), body - 2, "tail")]]);
    for (const file of group.files) {
      const selected = file.path === m.selected, tone = WORDS[file.letter]?.[1] ?? "warn";
      if (selected) selectedRow = lines.length;
      hits.set(lines.length, file.path);
      push([[PANEL_BLUE, selected ? "▌" : " "], [fg(tone), file.letter + " "], [selected ? BOLD + PANEL_TEXT : PANEL_TEXT, plainLabel(file.path.slice(file.path.lastIndexOf("/") + 1))]],
        [...barSegs(file, largest), [PANEL_MUTED, " " + counts(file).padStart(countWidth) + " "]], selected ? PANEL_SELECTED : PANEL_BG);
    }
  }
  if (!m.files.length) push([[m.state.tone === "error" ? PANEL_ERROR : PANEL_MUTED, " " + (m.state.tone === "error" ? "Git unavailable · r to retry" : m.query ? "No matching files · / to search" : "No changes")]]);
  if (m.others.length) {
    push([]); push([[PANEL_MUTED, " other comparisons  "], [PANEL_TEXT, "c"]]);
    for (const other of m.others) push([[PANEL_MUTED, " " + other.label]], [[PANEL_MUTED, (other.count === undefined ? "—" : String(other.count)) + " "]]);
  }
  for (const note of m.notes) for (const row of wrap(note, body - 2)) push([[PANEL_MUTED, " " + row]]);
  return { lines, hits, selectedRow };
}

/** Below SPLIT_MIN: `‹ 1/2 › M receipts_tests.rs ▮▮▮▮▮▮ +15 −3 … 2 files +18 −4`. */
export function pickerLine(m: ChangesModel, width: number): string {
  const index = m.files.findIndex(f => f.path === m.selected), file = m.files[index];
  if (!file) return line([[PANEL_MUTED, " " + filesLabel(m) + " · j/k choose"]], [], width, NORMAL + PANEL_BG) + RESET;
  const [, tone] = WORDS[file.letter] ?? ["modified", "warn"];
  const largest = Math.max(1, ...m.files.map(f => (f.added ?? 0) + (f.removed ?? 0)));
  const right: Seg[] = width >= 72 ? [[PANEL_MUTED, filesLabel(m) + " "], ...totals(m.files), [PANEL_TEXT, " "]] : [];
  return line([[PANEL_BLUE, " ‹ "], [PANEL_MUTED, `${index + 1}/${m.files.length}`], [PANEL_BLUE, " › "], [fg(tone), file.letter + " "],
    [BOLD + PANEL_TEXT, plainLabel(file.path.slice(file.path.lastIndexOf("/") + 1))], [PANEL_TEXT, "  "],
    ...barSegs(file, largest), [PANEL_MUTED, " " + counts(file)]], right, width, NORMAL + PANEL_BG) + RESET;
}

const HUNK = /^@@ -(\d+)(?:,(\d+))? \+(\d+)(?:,(\d+))? @@ ?(.*)$/;
export interface DiffFrame { readonly lines: string[]; readonly hunkRows: number[]; readonly hunkSources: number[] }
/** The right pane: file header, hunk separator bars, old/new gutter and tinted add/remove rows. */
export function diffLines(m: ChangesModel, width: number): DiffFrame {
  const lines: string[] = [], hunkRows: number[] = [], hunkSources: number[] = [];
  const plain = NORMAL + BG_OFF, inner = Math.max(1, width - 1);
  const say = (color: string, text: string) => { for (const row of wrap(plainLabel(text), inner - 1)) lines.push(line([[color, " " + row]], [], width, plain) + RESET); };
  for (const note of m.notes) say(PANEL_ERROR, note);
  const d = m.diff;
  if (d.kind === "message") { for (const [i, text] of d.lines.entries()) say(i ? PANEL_MUTED : PANEL_TEXT, text); return { lines, hunkRows, hunkSources }; }
  const source = d.text.split("\n"); if (source.at(-1) === "") source.pop();
  const first = source.findIndex(row => row.startsWith("@@"));
  const body = first < 0 ? [] : source.slice(first);
  const added = body.filter(row => row.startsWith("+")).length, removed = body.filter(row => row.startsWith("-")).length;
  const total = source.filter(row => HUNK.test(row)).length;
  const highest = Math.max(0, ...source.map(row => HUNK.exec(row)).map(h => h ? Math.max(Number(h[1]) + Number(h[2] ?? 1), Number(h[3]) + Number(h[4] ?? 1)) : 0));
  const digits = Math.max(4, String(highest).length), current = Math.max(0, Math.min(total - 1, m.hunk));
  const [word, tone] = WORDS[d.status] ?? ["modified", "warn"];
  lines.push(line([[BOLD + PANEL_BLUE, " " + clip(plainLabel(d.path), Math.max(8, inner - 34), "tail")], [PANEL_TEXT, "  "], [bg(tone) + fg(tone), ` ${word} `],
    [PANEL_TEXT, " "], [PANEL_SUCCESS, `+${added}`], [PANEL_TEXT, " "], [PANEL_ERROR, `−${removed}`], [PANEL_MUTED, `  · ${total} hunk${total === 1 ? "" : "s"}`]], [], width, plain) + RESET);
  if (d.notice) say(PANEL_BLUE, d.notice);
  let oldNo: number | undefined, newNo: number | undefined, hunk = -1;
  const gutter = (a: string, b: string) => `${a.padStart(digits)} ${b.padStart(digits)} `;
  const edge = (on: boolean) => BG_OFF + (on ? PANEL_BLUE + "▏" : " ");
  source.forEach((raw, index) => {
    const h = HUNK.exec(raw);
    if (raw.startsWith("diff ")) { oldNo = newNo = undefined; return; }
    if (h) {
      hunk++; hunkRows.push(lines.length); hunkSources.push(index); oldNo = Number(h[1]); newNo = Number(h[3]);
      const context = code(h[5] ?? "").trim().replace(/\s*\(.*$|\s*\{$/, "");
      const on = hunk === current;
      lines.push(BG_OFF + (on ? PANEL_BLUE + "▌" : " ") + line([[PANEL_BLUE, ` @@ ${oldNo || newNo}${context ? " · " + context : ""}`]],
        [[PANEL_MUTED, `hunk ${hunk + 1}/${total} `]], inner, NORMAL + PANEL_TINT.accent) + RESET);
      return;
    }
    if (oldNo === undefined || newNo === undefined) {
      if (!/^(index |--- |\+\+\+ )/.test(raw)) say(PANEL_MUTED, raw);
      return;
    }
    const sign = raw[0], kind = sign === "+" ? "add" : sign === "-" ? "del" : sign === " " ? "ctx" : "meta";
    const back = kind === "add" ? PANEL_TINT.success : kind === "del" ? PANEL_TINT.error : BG_OFF;
    const mark = kind === "add" ? [PANEL_SUCCESS, "+"] as const : kind === "del" ? [PANEL_ERROR, "−"] as const : [PANEL_MUTED, " "] as const;
    const numbers = kind === "meta" ? gutter("", "") : gutter(kind === "add" ? "" : String(oldNo++), kind === "del" ? "" : String(newNo++));
    const text = code(kind === "meta" ? raw : raw.slice(1)), room = Math.max(1, inner - visibleWidth(numbers) - 4);
    wrap(text, room).forEach((chunk, k) => lines.push(edge(hunk === current) + line([[PANEL_MUTED, k ? " ".repeat(numbers.length) : numbers], [PANEL_RULE, "│"],
      [mark[0], k ? "   " : ` ${mark[1]} `], [kind === "meta" ? PANEL_MUTED : PANEL_TEXT, chunk]], [], inner, NORMAL + back) + RESET));
  });
  if (!total) say(PANEL_MUTED, first < 0 && source.length ? "No text hunks in this diff." : "Empty diff.");
  return { lines, hunkRows, hunkSources };
}

export interface ChangesHost {
  model(): ChangesModel;
  /** A plain left click the session may act on (no modifiers, not a drag, interaction allowed). */
  click(event: TuiMouseEvent): boolean;
  focus(): void;
  select(path: string): void;
  step(delta: number): void;
  pickHunk(index: number): void;
}
/** Owns the Changes components; the workbench keeps selection, patch acceptance and key handling. */
export class ChangesView {
  readonly diff: ScrollView; readonly list: ScrollView; readonly root: Component;
  hunkRows: number[] = []; hunkSources: number[] = [];
  private listHits: ReadonlyMap<number, string> = new Map();
  private shownSelection: string | undefined;
  private pickerNext = 0;
  constructor(private readonly host: ChangesHost) {
    this.diff = new ScrollView({ render: width => {
      const frame = diffLines(host.model(), width);
      this.hunkRows = frame.hunkRows; this.hunkSources = frame.hunkSources;
      return frame.lines;
    }, handleMouse: event => {
      if (!host.click(event)) return undefined;
      host.focus();
      let index = -1; for (const [i, row] of this.hunkRows.entries()) if (row <= event.y) index = i;
      if (index >= 0) host.pickHunk(index);
      return { handled: true };
    }, invalidate() {} }, { primary: true, follow: "none", scrollbar: "auto" });
    this.list = new ScrollView({ render: width => this.renderList(width), handleMouse: event => {
      if (!host.click(event)) return undefined;
      host.focus(); const path = this.listHits.get(event.y); if (path) host.select(path);
      return { handled: true };
    }, invalidate() {} }, { follow: "none", scrollbar: "auto" });
    const chips: Component = { render: width => [chipRow(host.model(), width)], invalidate() {} };
    const picker: Component = { render: width => {
      const model = host.model();
      this.pickerNext = 4 + `${model.files.findIndex(f => f.path === model.selected) + 1}/${model.files.length}`.length;
      return [pickerLine(model, width)];
    }, handleMouse: event => {
      if (!host.click(event)) return undefined;
      host.focus(); if (event.x <= 2) host.step(-1); else if (Math.abs(event.x - this.pickerNext) <= 1) host.step(1);
      return { handled: true };
    }, invalidate() {} };
    const hints: Component = { render: width => [hintLine(width)], invalidate() {} };
    const split = new HStack([
      { component: this.list, basis: LIST_WIDTH, shrink: 0, visible: viewport => viewport.width >= SPLIT_MIN },
      { component: this.diff, basis: 0, grow: 1, minSize: 1 },
    ]);
    this.root = new VStack([
      { component: chips, basis: 1, shrink: 0 },
      { component: picker, basis: 1, shrink: 0, visible: viewport => viewport.width < SPLIT_MIN },
      { component: split, basis: 0, grow: 1, minSize: 1 },
      { component: hints, basis: 1, shrink: 0 },
    ]);
  }
  private renderList(width: number): string[] {
    const model = this.host.model(), frame = listLines(model, width);
    this.listHits = frame.hits;
    // Keep a newly selected file in view without fighting a reader who scrolled the list.
    if (model.selected !== this.shownSelection && frame.selectedRow >= 0) {
      this.shownSelection = model.selected;
      const top = this.list.scrollTop, height = this.list.viewportHeight;
      if (height > 0 && (frame.selectedRow < top || frame.selectedRow >= top + height)) {
        this.list.updateLayout(frame.lines.length, height, () => {});
        this.list.scrollTo(Math.max(0, frame.selectedRow - Math.floor(height / 2)), { disableFollow: true });
      }
    }
    // The pane rule runs the full height even when the tree is shorter than the viewport.
    const blank = PANEL_BG + " ".repeat(Math.max(0, width - 1)) + PANEL_RULE + "│" + RESET;
    return [...frame.lines, ...Array.from({ length: Math.max(0, this.list.viewportHeight - frame.lines.length) }, () => blank)];
  }
}
