// Changes (F7): one comparison chip row, an author filter row when the host supplies authors, then the body.
// At SPLIT_MIN columns and above the flat file list sits beside the diff; below it a capped list stacks above
// a summary rule and the diff. Presentation only: every value comes from the ReviewStore read and the displayed patch.
import { HStack, ScrollView, VStack, visibleWidth, type Component, type TuiMouseEvent } from "@earendil-works/pi-tui";
import { relevantFile, type Comparison } from "./panels-git.ts";
import { comparisonLabel, fileStatus, type ReviewStore } from "./review-store.ts";
import { plainLabel, type PanelFile, PANEL_BG, PANEL_TEXT, PANEL_MUTED, PANEL_BLUE, PANEL_RULE, PANEL_SUCCESS, PANEL_WARNING, PANEL_ERROR, PANEL_SELECTED, PANEL_TINT, RESET } from "./panels-view.ts";

/** Main-area width at which the file list sits beside the diff; below it the list stacks above the diff. */
export const SPLIT_MIN = 90;
/** Width of the side-by-side file list, including its right rule. */
export const LIST_WIDTH = 42;
/** Diff pane width at which old and new line numbers both show; below it only one gutter. */
const TWO_GUTTERS = 80;
/** Stacked list width below which paths keep only their last two segments. */
const SHORT_PATHS = 60;
const BG_OFF = "\x1b[49m", BOLD = "\x1b[1m", NORMAL = "\x1b[22m", UNDERLINE = "\x1b[4m", NO_UNDERLINE = "\x1b[24m";

/** Stacked list rows for the terminal height: 9, 5 when short (< 28 rows), 3 when very short (< 20 rows). */
const stackRows = (rows: number) => rows < 20 ? 3 : rows < 28 ? 5 : 9;

export type Tone = "ok" | "warn" | "error" | "accent" | "muted";
/** One changed file. `who` lists the authors that edited it; `outside` those whose edit is outside their own paths. */
export interface ChangeRow {
  readonly path: string; readonly letter: string; readonly added: number | null | undefined; readonly removed: number | null | undefined;
  readonly who?: readonly string[]; readonly outside?: readonly string[];
}
/** An agent that edits files in this worktree; `paths` maps each edited path to true when it lies outside the agent's own files. */
export interface ChangeAuthor { readonly id: string; readonly label: string; readonly hue: number; readonly paths: ReadonlyMap<string, boolean> }
export type ChangesDiff =
  | { readonly kind: "patch"; readonly path: string; readonly status: string; readonly text: string; readonly notice?: string }
  | { readonly kind: "message"; readonly lines: readonly string[] };
export interface ChangesModel {
  readonly comparison: string;
  readonly range: string;
  readonly state: { readonly text: string; readonly tone: Tone };
  readonly scope: string;
  readonly updated: string;
  /** The files shown: the comparison's files, narrowed by the author filter. */
  readonly files: readonly ChangeRow[];
  /** The comparison's file count before the author filter. */
  readonly total: number;
  readonly selected: string | undefined;
  readonly query: string;
  readonly others: readonly { readonly label: string; readonly count: number | undefined }[];
  readonly notes: readonly string[];
  readonly diff: ChangesDiff;
  readonly hunk: number;
  /** Authors with at least one file in this comparison (or the active filter), in host order. */
  readonly authors: readonly { readonly id: string; readonly label: string; readonly hue: number; readonly count: number }[];
  /** `all`, or the id of the author whose files are shown. */
  readonly filter: string;
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

/** Everything the view shows, read once from the store and the displayed patch; `filter` narrows the files to one author's. */
export function changesModel(r: ReviewStore, shown: DisplayedPatch, scopeLabel: string, hunk: number, now: number,
  authors: readonly ChangeAuthor[] = [], filter = "all"): ChangesModel {
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
  const all: ChangeRow[] = r.visible.map(file => {
    const stat = r.stats.get(file.path);
    const who = authors.filter(author => author.paths.has(file.path)).map(author => author.id);
    const outside = authors.filter(author => author.paths.get(file.path) === true).map(author => author.id);
    return { path: file.path, letter: LETTERS[fileStatus(file)] ?? "M", added: stat?.added, removed: stat?.removed,
      ...(who.length ? { who } : {}), ...(outside.length ? { outside } : {}) };
  });
  const active = authors.some(author => author.id === filter) ? filter : "all";
  const writers = authors.map(author => ({ id: author.id, label: plainLabel(author.label), hue: author.hue, count: all.filter(file => file.who?.includes(author.id)).length }))
    .filter(author => author.count > 0 || author.id === active);
  const files = active === "all" ? all : all.filter(file => file.who?.includes(active));
  const others = r.comparisons.filter(value => value !== r.comparison).map(value => ({ label: NAMES[value],
    count: value === "candidate" || value === "session" ? undefined : r.worktree?.filter(file => relevantFile(file, value)).length }));
  const notes = [...(r.error ? ["STALE / unavailable: " + plainLabel(r.error)] : []), ...(r.limitNotice ? [r.limitNotice] : [])];
  const model = { comparison: NAMES[r.comparison], range, state, scope: `${plainLabel(scopeLabel)} @ ${plainLabel(r.branch)}`,
    updated: r.checkedAt ? `updated ${ago(now - r.checkedAt)} ago` : r.refreshing ? "checking Git…" : "not loaded",
    files, total: all.length, selected: r.selected, query: r.query, others, notes, hunk, authors: writers, filter: active };
  const message = (...lines: string[]): ChangesModel => ({ ...model, diff: { kind: "message", lines } });
  if (r.scopeRequired) return message("Choose a project", "Workspace-wide changes are not shown.",
    "Pick a project or your scratch worktree to see its changes.", "This is a read-only view; your session stays bound to Corral.");
  const file = r.current;
  if (!file) return message(r.refreshing ? "Loading comparison…" : r.error ?? (r.selected ? "Selected file is no longer in this comparison." : "No changes in this comparison."));
  if (r.patchPath !== r.selected || r.patchComparison !== r.comparison) return message("Loading selected diff…");
  const unavailable = r.patch.startsWith("Diff unavailable:");
  if (!sameScope) return message(unavailable ? plainLabel(r.patch) : "Waiting for a captured diff matching this scope…");
  if (!r.patchSnapshot && unavailable) return message(plainLabel(r.patch));
  const stale = r.patch !== shown.text || r.patchSnapshot?.head !== shown.head || r.patchSnapshot?.base !== shown.base;
  return { ...model, diff: { kind: "patch", path: file.path, status: LETTERS[fileStatus(shown.file ?? file)] ?? "M", text: shown.text,
    ...(stale ? { notice: "New diff available (content or identity) · the current hunk and scroll stay until it is applied" } : {}) } };
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
const segWidth = (segs: readonly Seg[]) => segs.reduce((n, [, text]) => n + visibleWidth(text), 0);
/** One row exactly `width` cells wide: left segments clipped to fit, right segments right-aligned. */
function line(left: readonly Seg[], right: readonly Seg[], width: number, base: string): string {
  const rightWidth = segWidth(right);
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
/** `─ 3 files +18 −4 ─────── 4–8/12 ─`: a rule carrying a summary, the right part dropped first when space runs out. */
function ruleLine(left: readonly Seg[], right: readonly Seg[], width: number): string {
  const base = NORMAL + PANEL_BG, lead: Seg = [PANEL_RULE, "─ "];
  const fill = (showRight: boolean) => width - 3 - segWidth(left) - (showRight ? segWidth(right) + 3 : 0);
  const showRight = right.length > 0 && fill(true) >= 1;
  const n = fill(showRight);
  if (n < 1) return line([lead, ...left], [], width, base) + RESET;
  const segs: Seg[] = [lead, ...left, [PANEL_RULE, " " + "─".repeat(n)], ...(showRight ? [[PANEL_RULE, " "] as Seg, ...right, [PANEL_RULE, " ─"] as Seg] : [])];
  return base + segs.map(([style, text]) => style + text + base).join("") + RESET;
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
const totals = (files: readonly ChangeRow[]): Seg[] => {
  const add = files.reduce((n, f) => n + (f.added ?? 0), 0), del = files.reduce((n, f) => n + (f.removed ?? 0), 0);
  return [[PANEL_SUCCESS, `+${add}`], [PANEL_TEXT, " "], [PANEL_ERROR, `−${del}`]];
};
const filesLabel = (m: ChangesModel) => `${m.files.length} file${m.files.length === 1 ? "" : "s"}${m.query ? ` · /${plainLabel(m.query)}` : ""}`;
const authorOf = (m: ChangesModel, id: string) => m.authors.find(author => author.id === id);

/** The comparison chips: `candidate ▾  aece0a19 → 7c41d0e2  committed · clean  corral @ task/T-796 … updated 12s ago`. */
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

/** A filter chip's columns on the filter row, `to` exclusive. */
export interface FilterHit { readonly from: number; readonly to: number; readonly id: string }
/**
 * `all 9 · host-slots 2 · picker-ui 3`: one chip per author in its hue, the active one bold and underlined.
 * Counts drop before chips do; when chips still overflow, `all` and the active chip stay and the rest fold into `+n`.
 */
export function filterRow(m: ChangesModel, width: number, color: (hue: number) => string): { line: string; hits: FilterHit[] } {
  const chips = [{ id: "all", label: "all", count: m.total, color: PANEL_TEXT },
    ...m.authors.map(author => ({ id: author.id, label: author.label, count: author.count, color: color(author.hue) }))];
  const SEP = " · ", lead = 1;
  const text = (chip: (typeof chips)[number], counts: boolean) => counts ? `${chip.label} ${chip.count}` : chip.label;
  const span = (list: readonly (typeof chips)[number][], counts: boolean, more: number) => lead
    + list.reduce((n, chip, i) => n + (i ? SEP.length : 0) + visibleWidth(text(chip, counts)), 0) + (more ? SEP.length + `+${more}`.length : 0);
  let shown = chips, counts = true;
  if (span(chips, true, 0) > width) {
    counts = false;
    if (span(chips, false, 0) > width) {
      const keep = new Set(chips.filter(chip => chip.id === "all" || chip.id === m.filter));
      for (const chip of chips) {
        if (keep.has(chip)) continue;
        const next = chips.filter(c => keep.has(c) || c === chip);
        if (span(next, false, chips.length - next.length) <= width) keep.add(chip);
      }
      shown = chips.filter(chip => keep.has(chip));
    }
  }
  const more = chips.length - shown.length, base = NORMAL + PANEL_BG, hits: FilterHit[] = [];
  let out = base + " ", used = lead;
  for (const [i, chip] of shown.entries()) {
    if (i) { const sep = clip(SEP, width - used); out += PANEL_MUTED + sep + base; used += visibleWidth(sep); }
    const label = clip(text(chip, counts), width - used); if (!label) break;
    const on = chip.id === m.filter;
    out += chip.color + (on ? BOLD + UNDERLINE : "") + label + NO_UNDERLINE + base;
    hits.push({ from: used, to: used + visibleWidth(label), id: chip.id }); used += visibleWidth(label);
  }
  if (more) { const tail = clip(`${SEP}+${more}`, width - used); out += PANEL_MUTED + tail + base; used += visibleWidth(tail); }
  return { line: out + " ".repeat(Math.max(0, width - used)) + RESET, hits };
}

export interface ListFrame {
  readonly lines: string[]; readonly hits: ReadonlyMap<number, string>;
  /** First row of the selected file, or -1; with `selectedSpan` its rows (the file row and any flag row). */
  readonly selectedRow: number; readonly selectedSpan: number;
}
export interface ListOptions {
  /** Stacked above the diff: file rows only, the full width, no right rule, header or other comparisons. */
  readonly stacked?: boolean;
  readonly color?: (hue: number) => string;
}
/**
 * The flat file list: `▌ M ●● ! src/a/one.rs   +3 −2`, with the status letter, author dots in their hue,
 * and `!` plus a flag row when an author edited the file outside its own paths (`also edited by …, outside
 * its files`) or when two or more authors edited it (`edited by a and b`). As a side pane it adds the file
 * count and totals above and the other comparisons below.
 */
export function listLines(m: ChangesModel, width: number, options: ListOptions = {}): ListFrame {
  const lines: string[] = [], hits = new Map<number, string>(); let selectedRow = -1, selectedSpan = 0;
  const stacked = options.stacked ?? false, color = options.color ?? (() => PANEL_TEXT);
  const body = stacked ? width : width - 1, edge = stacked ? "" : PANEL_BG + PANEL_RULE + "│" + RESET;
  const push = (left: readonly Seg[], right: readonly Seg[] = [], base = PANEL_BG) => lines.push(line(left, right, body, NORMAL + base) + (stacked ? RESET : edge));
  if (!stacked) { push([[PANEL_TEXT, " " + filesLabel(m)]], m.files.length ? [...totals(m.files), [PANEL_TEXT, " "]] : []); push([]); }
  const plus = (f: ChangeRow) => `+${f.added ?? 0}`, minus = (f: ChangeRow) => `−${f.removed ?? 0}`;
  const numeric = m.files.filter(f => typeof f.added === "number");
  const plusWidth = Math.max(0, ...numeric.map(f => plus(f).length)), minusWidth = Math.max(0, ...numeric.map(f => minus(f).length));
  const countWidth = Math.max(3, numeric.length ? plusWidth + 1 + minusWidth : 0);
  const counts = (f: ChangeRow): Seg[] => {
    if (typeof f.added === "number") return [[PANEL_TEXT, " ".repeat(countWidth - plusWidth - 1 - minusWidth)], [PANEL_SUCCESS, plus(f).padStart(plusWidth)], [PANEL_TEXT, " "], [PANEL_ERROR, minus(f).padStart(minusWidth)], [PANEL_TEXT, " "]];
    const word = f.added === null ? "bin" : f.letter === "?" || f.letter === "A" ? "new" : "";
    return [[PANEL_MUTED, word.padStart(countWidth) + " "]];
  };
  const shortPaths = stacked && width < SHORT_PATHS;
  const names = (ids: readonly string[]) => {
    const labels = ids.map(id => plainLabel(authorOf(m, id)?.label ?? id));
    return labels.length < 2 ? labels.join("") : labels.slice(0, -1).join(", ") + " and " + labels.at(-1);
  };
  for (const file of m.files) {
    const selected = file.path === m.selected, tone = WORDS[file.letter]?.[1] ?? "warn", base = selected ? PANEL_SELECTED : PANEL_BG;
    if (selected) selectedRow = lines.length;
    const dots: Seg[] = (file.who ?? []).map(id => { const author = authorOf(m, id); return [author ? color(author.hue) : PANEL_MUTED, "●"] as Seg; });
    const flag = file.outside?.length ? `also edited by ${names(file.outside)}, outside ${file.outside.length === 1 ? "its" : "their"} files`
      : (file.who?.length ?? 0) >= 2 ? `edited by ${names(file.who ?? [])}` : undefined;
    const prefix: Seg[] = [[PANEL_BLUE, selected ? "▌" : " "], [PANEL_TEXT, " "], [fg(tone), file.letter], [PANEL_TEXT, " "],
      ...(dots.length ? [...dots, [PANEL_TEXT, " "] as Seg] : []), ...(flag ? [[PANEL_WARNING, "!"] as Seg, [PANEL_TEXT, " "] as Seg] : [])];
    const right = counts(file), room = body - segWidth(prefix) - segWidth(right) - 1;
    const path = plainLabel(shortPaths ? file.path.split("/").slice(-2).join("/") : file.path);
    hits.set(lines.length, file.path);
    push([...prefix, [selected ? BOLD + PANEL_TEXT : PANEL_TEXT, clip(path, room, "tail")]], right, base);
    if (flag) {
      hits.set(lines.length, file.path);
      push([[PANEL_TEXT, " ".repeat(Math.min(segWidth(prefix), Math.max(0, body - 8)))], [PANEL_WARNING, flag]], [], base);
    }
    if (selected) selectedSpan = lines.length - selectedRow;
  }
  if (!m.files.length) {
    const empty = m.state.tone === "error" ? "Git unavailable" : m.filter !== "all" ? "No files from this author in this comparison" : m.query ? "No matching files" : "No changes";
    push([[m.state.tone === "error" ? PANEL_ERROR : PANEL_MUTED, " " + empty]]);
  }
  if (!stacked) {
    if (m.others.length) {
      push([]); push([[PANEL_MUTED, " other comparisons"]]);
      for (const other of m.others) push([[PANEL_MUTED, " " + other.label]], [[PANEL_MUTED, (other.count === undefined ? "—" : String(other.count)) + " "]]);
    }
    for (const note of m.notes) for (const row of wrap(note, body - 2)) push([[PANEL_MUTED, " " + row]]);
  }
  return { lines, hits, selectedRow, selectedSpan };
}

const HUNK = /^@@ -(\d+)(?:,(\d+))? \+(\d+)(?:,(\d+))? @@ ?(.*)$/;
export interface DiffFrame { readonly lines: string[]; readonly hunkRows: number[]; readonly hunkSources: number[] }
/**
 * The diff: file header (with its authors when known), hunk separator bars and tinted add/remove rows.
 * At TWO_GUTTERS columns and above each row carries old and new line numbers; below it one gutter holds
 * the new line number, and removed lines show their old number.
 */
export function diffLines(m: ChangesModel, width: number, color: (hue: number) => string = () => PANEL_TEXT): DiffFrame {
  const lines: string[] = [], hunkRows: number[] = [], hunkSources: number[] = [];
  const plain = NORMAL + BG_OFF, inner = Math.max(1, width - 1);
  const say = (tint: string, text: string) => { for (const row of wrap(plainLabel(text), inner - 1)) lines.push(line([[tint, " " + row]], [], width, plain) + RESET); };
  for (const note of m.notes) say(PANEL_ERROR, note);
  const d = m.diff;
  if (d.kind === "message") { for (const [i, text] of d.lines.entries()) say(i ? PANEL_MUTED : PANEL_TEXT, text); return { lines, hunkRows, hunkSources }; }
  const source = d.text.split("\n"); if (source.at(-1) === "") source.pop();
  const first = source.findIndex(row => row.startsWith("@@"));
  const body = first < 0 ? [] : source.slice(first);
  const added = body.filter(row => row.startsWith("+")).length, removed = body.filter(row => row.startsWith("-")).length;
  const total = source.filter(row => HUNK.test(row)).length;
  const highest = Math.max(0, ...source.map(row => HUNK.exec(row)).map(h => h ? Math.max(Number(h[1]) + Number(h[2] ?? 1), Number(h[3]) + Number(h[4] ?? 1)) : 0));
  const two = width >= TWO_GUTTERS, digits = Math.max(two ? 4 : 3, String(highest).length), current = Math.max(0, Math.min(total - 1, m.hunk));
  const [word, tone] = WORDS[d.status] ?? ["modified", "warn"];
  const who: Seg[] = (m.files.find(file => file.path === d.path)?.who ?? []).flatMap(id => {
    const author = authorOf(m, id); return author ? [[PANEL_TEXT, "  "] as Seg, [color(author.hue), "● " + author.label] as Seg] : [];
  });
  lines.push(line([[BOLD + PANEL_BLUE, " " + clip(plainLabel(d.path), Math.max(8, inner - 34), "tail")], [PANEL_TEXT, "  "], [bg(tone) + fg(tone), ` ${word} `],
    [PANEL_TEXT, " "], [PANEL_SUCCESS, `+${added}`], [PANEL_TEXT, " "], [PANEL_ERROR, `−${removed}`], [PANEL_MUTED, `  · ${total} hunk${total === 1 ? "" : "s"}`], ...who], [], width, plain) + RESET);
  if (d.notice) say(PANEL_BLUE, d.notice);
  let oldNo: number | undefined, newNo: number | undefined, hunk = -1;
  const gutter = (old: string, now: string) => two ? `${old.padStart(digits)} ${now.padStart(digits)} ` : `${(now || old).padStart(digits)} `;
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
  /** Moves the file selection by `delta` files. */
  step(delta: number): void;
  pickHunk(index: number): void;
  /** Agents that edit this worktree; the author filter row shows only when this returns at least one. */
  authors?(): readonly ChangeAuthor[];
  /** ANSI foreground for an author hue. */
  color?(hue: number): string;
  /** Terminal rows, which cap the stacked file list; the layout viewport height is used without it. */
  rows?(): number;
}
/** Owns the Changes components; the workbench keeps selection, patch acceptance and key handling. */
export class ChangesView {
  readonly diff: ScrollView; readonly list: ScrollView; readonly root: Component;
  hunkRows: number[] = []; hunkSources: number[] = [];
  /** `all`, or the author id whose files the host should show (passed to `changesModel`). */
  filter = "all";
  private listHits: ReadonlyMap<number, string> = new Map();
  private stackHits: ReadonlyMap<number, string> = new Map();
  private filterHits: readonly FilterHit[] = [];
  private shownSelection: string | undefined;
  private stackTop = 0;
  private stackCap = stackRows(Number.MAX_SAFE_INTEGER);
  private readonly color: (hue: number) => string;
  constructor(private readonly host: ChangesHost) {
    this.color = hue => host.color?.(hue) ?? PANEL_TEXT;
    this.diff = new ScrollView({ render: width => {
      const frame = diffLines(host.model(), width, this.color);
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
    const filters: Component = { render: width => {
      const row = filterRow(host.model(), width, this.color);
      this.filterHits = row.hits;
      return [row.line];
    }, handleMouse: event => {
      if (!host.click(event)) return undefined;
      host.focus(); const hit = this.filterHits.find(h => event.x >= h.from && event.x < h.to); if (hit) this.setFilter(hit.id);
      return { handled: true };
    }, invalidate() {} };
    const stacked: Component = { render: width => this.renderStacked(width), handleMouse: event => {
      if (!host.click(event)) return undefined;
      host.focus(); const path = this.stackHits.get(event.y); if (path) host.select(path);
      return { handled: true };
    }, invalidate() {} };
    const column = new VStack([
      { component: stacked, basis: "auto", shrink: 0, visible: viewport => {
        if (viewport.width >= SPLIT_MIN) return false;
        this.stackCap = stackRows(host.rows?.() ?? viewport.height);
        return true;
      } },
      { component: this.diff, basis: 0, grow: 1, minSize: 1 },
    ]);
    const body = new HStack([
      { component: this.list, basis: LIST_WIDTH, shrink: 0, visible: viewport => viewport.width >= SPLIT_MIN },
      { component: column, basis: 0, grow: 1, minSize: 1 },
    ]);
    this.root = new VStack([
      { component: chips, basis: 1, shrink: 0 },
      { component: filters, basis: 1, shrink: 0, visible: () => (host.authors?.().length ?? 0) > 0 },
      { component: body, basis: 0, grow: 1, minSize: 1 },
    ]);
  }
  /** `f`: the next author filter after the current one, back to `all` after the last author. */
  cycleFilter(): void {
    const ids = ["all", ...this.host.model().authors.map(author => author.id)];
    this.setFilter(ids[(ids.indexOf(this.filter) + 1) % ids.length] ?? "all");
  }
  /** Shows one author's files (or `all`); a selection the filter hides moves to the first shown file. */
  setFilter(id: string): void {
    this.filter = id;
    const m = this.host.model(), first = m.files[0];
    if (first && !m.files.some(file => file.path === m.selected)) this.host.select(first.path);
  }
  private renderList(width: number): string[] {
    const model = this.host.model(), frame = listLines(model, width, { color: this.color });
    this.listHits = frame.hits;
    // Keep a newly selected file in view without fighting a reader who scrolled the list.
    if (model.selected !== this.shownSelection && frame.selectedRow >= 0) {
      this.shownSelection = model.selected;
      const top = this.list.scrollTop, height = this.list.viewportHeight;
      if (height > 0 && (frame.selectedRow < top || frame.selectedRow + frame.selectedSpan > top + height)) {
        this.list.updateLayout(frame.lines.length, height, () => {});
        this.list.scrollTo(Math.max(0, frame.selectedRow - Math.floor(height / 2)), { disableFollow: true });
      }
    }
    // The pane rule runs the full height even when the list is shorter than the viewport.
    const blank = PANEL_BG + " ".repeat(Math.max(0, width - 1)) + PANEL_RULE + "│" + RESET;
    return [...frame.lines, ...Array.from({ length: Math.max(0, this.list.viewportHeight - frame.lines.length) }, () => blank)];
  }
  /** The stacked list: at most `stackCap` rows, moved the least distance that keeps the selected file whole, then the summary rule. */
  private renderStacked(width: number): string[] {
    const m = this.host.model(), frame = listLines(m, width, { stacked: true, color: this.color });
    const cap = Math.max(1, this.stackCap), total = frame.lines.length, last = Math.max(0, total - cap);
    let top = Math.min(this.stackTop, last);
    if (frame.selectedRow >= 0) {
      const end = frame.selectedRow + Math.max(1, frame.selectedSpan);
      if (frame.selectedRow < top) top = frame.selectedRow;
      else if (end > top + cap) top = Math.min(frame.selectedRow, end - cap);
    }
    top = Math.max(0, Math.min(top, last));
    this.stackTop = top;
    const hits = new Map<number, string>();
    for (const [row, path] of frame.hits) if (row >= top && row < top + cap) hits.set(row - top, path);
    this.stackHits = hits;
    const shown = [...new Set(hits.values())].map(path => m.files.findIndex(file => file.path === path)).filter(index => index >= 0);
    const range: Seg[] = total > cap && shown.length ? [[PANEL_MUTED, `${Math.min(...shown) + 1}–${Math.max(...shown) + 1}/${m.files.length}`]] : [];
    return [...frame.lines.slice(top, top + cap), ruleLine([[PANEL_MUTED, filesLabel(m)], [PANEL_TEXT, " "], ...totals(m.files)], range, width)];
  }
}
