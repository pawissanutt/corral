// Changes (F7), after the mock's `.chg`: from SPLIT_MIN main columns the file column (author chips, then one row
// per file) sits beside the diff with a `│` rule between them; below it the file block (at most 9 rows) stacks
// above a rule and the diff. A row naming the comparison shows for a candidate (its exact ids) or while the comparison
// is not the session's default, and a view without changes is one empty-state paragraph. Every row keeps one blank
// cell at each side of the main area.
// Presentation only: every value comes from the ReviewStore read and the displayed patch.
import { HStack, ScrollView, VStack, visibleWidth, type Component, type TuiMouseEvent } from "@earendil-works/pi-tui";
import type { Comparison } from "./panels-git.ts";
import { comparisonLabel, fileStatus, type FileStat, type ReviewStore } from "./review-store.ts";
import { ansi, tint, PACKAGED, type Appearance } from "./appearance.ts";
import { plainLabel, type PanelFile, PANEL_BG, PANEL_TEXT, PANEL_MUTED, PANEL_BLUE, PANEL_LINE, PANEL_DIALOG, PANEL_SUCCESS, PANEL_WARNING, PANEL_ERROR, PANEL_SELECTED, RESET } from "./panels-view.ts";

/** Main-area width at which the file column sits beside the diff; below it the files stack above the diff. */
export const SPLIT_MIN = 90;
/** Cells of the side-by-side file column: the view's left margin, 44 cells of rows, one blank cell and the `│` rule. */
export const LIST_WIDTH = 47;
/** Main-area width below which paths keep only their last two segments. */
const SHORT_PATHS = 60;
/** Widest empty-state paragraph, as the mock's `.empty`. */
const MEASURE = 70;
const BOLD = "\x1b[1m", NORMAL = "\x1b[22m", UNDERLINE = "\x1b[4m", NO_UNDERLINE = "\x1b[24m";
const CANVAS = () => NORMAL + PANEL_BG;

/** Stacked file block rows for the terminal height: 9, 5 when short (< 28 rows), 3 when very short (< 20 rows). */
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
  /** True while the comparison is not the session's default (the first one the store offers). */
  readonly custom: boolean;
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
const fg = (tone: Tone) => tone === "ok" ? PANEL_SUCCESS : tone === "warn" ? PANEL_WARNING : tone === "error" ? PANEL_ERROR : tone === "accent" ? PANEL_BLUE : PANEL_MUTED;
const short = (sha: string | undefined) => sha ? sha.slice(0, 8) : "unborn";

/** The diff surfaces (`.d-add`, `.d-del`) and the line-number ink, per appearance. */
interface DiffInk { readonly addBack: string; readonly add: string; readonly delBack: string; readonly del: string; readonly number: string }
function diffInk(appearance: Appearance): DiffInk {
  if (appearance === "dark") return { addBack: ansi("#1f2e27", true), add: ansi("#bfe3c6"), delBack: ansi("#33232a", true), del: ansi("#efb9c1"), number: ansi("#8f98ab") };
  const p = PACKAGED[appearance], amount = appearance === "light" ? 0.1 : 0.2;
  return { addBack: ansi(tint(p.background, p.success, amount), true), add: ansi(p.success),
    delBack: ansi(tint(p.background, p.error, amount), true), del: ansi(p.error), number: ansi(p.muted) };
}
const INKS = (Object.keys(PACKAGED) as Appearance[]).map(appearance => [ansi(PACKAGED[appearance].background, true), diffInk(appearance)] as const);
/** The diff ink of the appearance the panels currently paint, found by its canvas. */
const currentInk = (): DiffInk => (INKS.find(([canvas]) => canvas === PANEL_BG) ?? INKS[0]!)[1];

export function ago(ms: number): string {
  const s = Math.max(0, Math.round(ms / 1000));
  return s < 60 ? `${s}s` : s < 3600 ? `${Math.floor(s / 60)}m` : s < 172800 ? `${Math.floor(s / 3600)}h` : `${Math.floor(s / 86400)}d`;
}
/** Added and removed line counts of a loaded single-file patch; undefined without text hunks (binary, unavailable, not loaded). */
function patchStat(text: string | undefined): FileStat | undefined {
  const rows = text?.split("\n") ?? [], first = rows.findIndex(row => row.startsWith("@@"));
  if (first < 0) return undefined;
  const body = rows.slice(first);
  return { added: body.filter(row => row.startsWith("+")).length, removed: body.filter(row => row.startsWith("-")).length };
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
  // Counts come from the store's read (numstat, and the untracked files it counted itself), so they never depend on
  // the selection. The file whose patch is on screen counts that patch's lines instead, so its row, header ids and
  // hunks stay one identity while a newer read waits to be applied. A file the store has no count for keeps none.
  const all: ChangeRow[] = r.visible.map(file => {
    const counted = r.stats.get(file.path);
    const stat = counted && shown.root === r.root && shown.comparison === label && shown.path === file.path ? patchStat(shown.text) ?? counted : counted;
    const who = authors.filter(author => author.paths.has(file.path)).map(author => author.id);
    const outside = authors.filter(author => author.paths.get(file.path) === true).map(author => author.id);
    return { path: file.path, letter: LETTERS[fileStatus(file)] ?? "M", added: stat?.added, removed: stat?.removed,
      ...(who.length ? { who } : {}), ...(outside.length ? { outside } : {}) };
  });
  const active = authors.some(author => author.id === filter) ? filter : "all";
  const writers = authors.map(author => ({ id: author.id, label: plainLabel(author.label), hue: author.hue, count: all.filter(file => file.who?.includes(author.id)).length }))
    .filter(author => author.count > 0 || author.id === active);
  const files = active === "all" ? all : all.filter(file => file.who?.includes(active));
  const notes = [...(r.error ? ["STALE / unavailable: " + plainLabel(r.error)] : []), ...(r.limitNotice ? [r.limitNotice] : [])];
  const model = { comparison: NAMES[r.comparison], custom: r.comparison !== r.comparisons[0], range, state, scope: `${plainLabel(scopeLabel)} @ ${plainLabel(r.branch)}`,
    updated: r.checkedAt ? `updated ${ago(now - r.checkedAt)} ago` : r.refreshing ? "checking Git…" : "not loaded",
    files, total: all.length, selected: r.selected, query: r.query, notes, hunk, authors: writers, filter: active };
  const message = (...lines: string[]): ChangesModel => ({ ...model, diff: { kind: "message", lines } });
  if (r.scopeRequired) return message("Choose a project.", "Workspace-wide changes are not shown.",
    "Pick a project or your scratch worktree to see its changes.", "This is a read-only view; your session stays bound to Corral.");
  const file = r.current;
  if (!file) {
    if (r.refreshing) return message("Loading comparison…");
    if (r.error !== undefined) return message(r.error);
    if (r.selected) return message("Selected file is no longer in this comparison.");
    return authors.length && !all.length ? message("No changes yet.", "Files appear here as agents edit them, in the colour of who touched them.")
      : message("No changes in this comparison.");
  }
  if (r.patchPath !== r.selected || r.patchComparison !== r.comparison) return message("Loading selected diff…");
  const unavailable = r.patch.startsWith("Diff unavailable:");
  if (!sameScope) return message(unavailable ? plainLabel(r.patch) : "Waiting for a captured diff matching this scope…");
  if (!r.patchSnapshot && unavailable) return message(plainLabel(r.patch));
  const stale = r.patch !== shown.text || r.patchSnapshot?.head !== shown.head || r.patchSnapshot?.base !== shown.base;
  return { ...model, diff: { kind: "patch", path: file.path, status: LETTERS[fileStatus(shown.file ?? file)] ?? "M", text: shown.text,
    ...(stale ? { notice: "New diff available (content or identity) · the current hunk and scroll stay until it is applied" } : {}) } };
}
/** No file at all in this comparison: the view is one empty-state paragraph instead of the columns. */
export const emptyChanges = (m: ChangesModel) => m.total === 0 && !m.query && m.diff.kind === "message";

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
    if (!text) continue;
    const cut = clip(text, room - used); if (!cut) break;
    out += style + cut + base; used += visibleWidth(cut); if (cut !== text) break;
  }
  if (showRight) { out += " ".repeat(width - used - rightWidth) + right.map(([style, text]) => style + text + base).join(""); used = width; }
  return out + " ".repeat(Math.max(0, width - used));
}
/** `row` (already `width - 2` cells) between the view's one-cell margins on the canvas. */
function framed(row: string, width: number): string {
  return width < 3 ? CANVAS() + " ".repeat(Math.max(0, width)) + RESET : CANVAS() + " " + row + CANVAS() + " " + RESET;
}
const innerOf = (width: number) => width < 3 ? 0 : width - 2;
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
const authorOf = (m: ChangesModel, id: string) => m.authors.find(author => author.id === id);
const shorten = (path: string, short: boolean) => plainLabel(short ? path.split("/").slice(-2).join("/") : path);

/** A candidate (whose exact base and head must stay on screen) or a comparison other than the session's default names itself in one row. */
export const showsComparison = (m: ChangesModel) => m.custom || m.comparison === NAMES.candidate;
/** `candidate ▾  aece0a19 → 7c41d0e2  committed · clean  corral @ task/T-796      updated 4s ago`: the label muted, the ids in text. */
export function comparisonRow(m: ChangesModel, width: number): string {
  const inner = innerOf(width), gap: Seg = [PANEL_TEXT, "  "];
  type Part = "comparison" | "range" | "state" | "scope";
  const parts: Record<Part, Seg> = { comparison: [PANEL_MUTED, m.comparison + " ▾"], range: [PANEL_TEXT, m.range],
    state: [fg(m.state.tone), m.state.text], scope: [PANEL_MUTED, m.scope] };
  const updated: Seg[] = [[PANEL_MUTED, m.updated]];
  // When space runs out the time goes first, then the scope (the header also names a scope other than the session's), then the state; the label and the ids stay.
  const orders: readonly (readonly Part[])[] = [["comparison", "range", "state", "scope"], ["comparison", "range", "state"], ["comparison", "range"]];
  for (const [i, order] of orders.entries()) {
    const left = order.flatMap((key, k) => k ? [gap, parts[key]] : [parts[key]]);
    if (!i && segWidth(left) + 2 + segWidth(updated) <= inner) return framed(line(left, updated, inner, CANVAS()), width);
    if (segWidth(left) <= inner || i === orders.length - 1) return framed(line(left, [], inner, CANVAS()), width);
  }
  return framed(" ".repeat(inner), width);
}

/** A chip's cells on its chip row, `to` exclusive, counted from the first cell of the chips' `width`. */
export interface ChipHit { readonly row: number; readonly from: number; readonly to: number; readonly id: string }
/**
 * `all 7  host-slots 2  app-store 3`: one chip per author in its hue (`all` in text), the active one bold and
 * underlined. Chips keep two cells between them, one when that makes them fit, and wrap onto further rows.
 */
export function authorChips(m: ChangesModel, width: number, color: (hue: number) => string): { lines: string[]; hits: ChipHit[] } {
  if (!m.authors.length || width < 1) return { lines: [], hits: [] };
  const chips = [{ id: "all", text: `all ${m.total}`, color: m.filter === "all" ? PANEL_TEXT : PANEL_MUTED },
    ...m.authors.map(author => ({ id: author.id, text: `${author.label} ${author.count}`, color: color(author.hue) }))];
  const gap = chips.reduce((n, chip, i) => n + (i ? 2 : 0) + visibleWidth(chip.text), 0) <= width ? 2 : 1;
  const hits: ChipHit[] = [], rows: string[] = [];
  let row = 0, used = 0;
  for (const chip of chips) {
    if (used && used + gap + visibleWidth(chip.text) > width) { row++; used = 0; }
    const from = used ? used + gap : 0, text = clip(chip.text, width - from);
    hits.push({ row, from, to: from + visibleWidth(text), id: chip.id }); used = from + visibleWidth(text);
  }
  for (let r = 0; r <= row; r++) {
    let out = CANVAS(), at = 0;
    for (const hit of hits.filter(h => h.row === r)) {
      const chip = chips.find(c => c.id === hit.id)!, on = chip.id === m.filter;
      out += " ".repeat(hit.from - at) + chip.color + (on ? BOLD + UNDERLINE : "") + clip(chip.text, hit.to - hit.from) + NO_UNDERLINE + CANVAS();
      at = hit.to;
    }
    rows.push(out + " ".repeat(Math.max(0, width - at)) + RESET);
  }
  return { lines: rows, hits };
}

export interface ListFrame {
  readonly lines: string[]; readonly hits: ReadonlyMap<number, string>;
  /** First row of the selected file, or -1; with `selectedSpan` its rows (the file row and any flag row). */
  readonly selectedRow: number; readonly selectedSpan: number;
}
export interface ListOptions {
  readonly color?: (hue: number) => string;
  /** Paths keep only their last two segments (narrow main areas). */
  readonly short?: boolean;
}
/**
 * The file rows, each exactly `width` cells, as the mock's `.f`: `▌ ●● ! app/src/api/client.ts   +9 −2`. The bar
 * shows on the selected row (on the selected surface); author dots in their hue show when the comparison has
 * authors; `!` and a warn row under the file flag edits outside an author's own paths (`also edited by …`) or by
 * two or more authors (`edited by a and b`). An active path search leads with `/query`.
 */
export function listLines(m: ChangesModel, width: number, options: ListOptions = {}): ListFrame {
  const lines: string[] = [], hits = new Map<number, string>(); let selectedRow = -1, selectedSpan = 0;
  const color = options.color ?? (() => PANEL_TEXT), authored = m.authors.length > 0, canvas = CANVAS();
  const label = (id: string) => plainLabel(authorOf(m, id)?.label ?? id);
  const names = (ids: readonly string[]) => ids.length < 2 ? ids.map(label).join("") : ids.slice(0, -1).map(label).join(", ") + " and " + label(ids.at(-1)!);
  const counts = (f: ChangeRow): Seg[] => typeof f.added === "number" ? [[PANEL_SUCCESS, `+${f.added}`], [PANEL_TEXT, " "], [PANEL_ERROR, `−${f.removed ?? 0}`]]
    : f.added === null ? [[PANEL_MUTED, "bin"]] : f.letter === "?" || f.letter === "A" ? [[PANEL_MUTED, "new"]] : [];
  if (m.query) lines.push(line([[PANEL_MUTED, "/"], [PANEL_TEXT, plainLabel(m.query)]], [], width, canvas) + RESET);
  for (const file of m.files) {
    const selected = file.path === m.selected, base = NORMAL + (selected ? PANEL_SELECTED : PANEL_BG);
    if (selected) selectedRow = lines.length;
    const dots: Seg[] = (file.who ?? []).map(id => { const author = authorOf(m, id); return [author ? color(author.hue) : PANEL_MUTED, "●"] as Seg; });
    const flag = file.outside?.length ? `also edited by ${file.outside.map(label).join(", ")}` : (file.who?.length ?? 0) >= 2 ? `edited by ${names(file.who ?? [])}` : undefined;
    const left: Seg[] = [[PANEL_BLUE, selected ? "▌" : " "], [PANEL_TEXT, " "],
      ...(authored ? [...(dots.length ? dots : [[PANEL_TEXT, " "] as Seg]), [PANEL_TEXT, " "] as Seg] : []),
      ...(flag ? [[PANEL_WARNING, "!"] as Seg, [PANEL_TEXT, " "] as Seg] : [])];
    const right = counts(file), room = width - segWidth(left) - (right.length ? segWidth(right) + 1 : 0);
    hits.set(lines.length, file.path);
    lines.push(line([...left, [PANEL_TEXT, clip(shorten(file.path, options.short ?? false), room, "tail")]], right, width, base) + RESET);
    if (flag) { hits.set(lines.length, file.path); lines.push(line([[PANEL_TEXT, "    "], [PANEL_WARNING, flag]], [], width, canvas) + RESET); }
    if (selected) selectedSpan = lines.length - selectedRow;
  }
  if (!m.files.length) {
    const empty: Seg = m.state.tone === "error" ? [PANEL_ERROR, "Git unavailable"] : m.filter !== "all" ? [PANEL_MUTED, "This agent has not changed a file yet."]
      : m.query ? [PANEL_MUTED, "No matching files"] : [PANEL_MUTED, "No changes"];
    lines.push(line([empty], [], width, canvas) + RESET);
  }
  return { lines, hits, selectedRow, selectedSpan };
}

/** The empty state: the first message line bold in text, the rest muted, as one paragraph of at most MEASURE cells. */
export function emptyLines(m: ChangesModel, width: number): string[] {
  const inner = innerOf(width), measure = Math.min(MEASURE, inner);
  const lines = m.diff.kind === "message" ? m.diff.lines : [];
  const words: Seg[] = lines.flatMap((text, i) => plainLabel(text).split(/\s+/).filter(Boolean).map(word => [i ? PANEL_MUTED : BOLD + PANEL_TEXT, word] as Seg));
  const rows: Seg[][] = []; let row: Seg[] = [], used = 0;
  for (const [style, word] of words) {
    const w = visibleWidth(word);
    if (row.length && used + 1 + w > measure) { rows.push(row); row = []; used = 0; }
    if (row.length) { row.push([PANEL_TEXT, " "]); used++; }
    const cut = clip(word, measure - used); row.push([style, cut]); used += visibleWidth(cut);
  }
  if (row.length) rows.push(row);
  return [...m.notes.flatMap(note => wrap(plainLabel(note), Math.max(1, inner)).map(text => framed(line([[PANEL_ERROR, text]], [], inner, CANVAS()), width))),
    ...rows.map(segs => framed(line(segs, [], inner, CANVAS()), width))];
}

const HUNK = /^@@ -(\d+)(?:,(\d+))? \+(\d+)(?:,(\d+))? @@ ?(.*)$/;
/** Git header lines the diff does not draw; the header row and the hunk rows carry what they say. */
const QUIET = /^(index |--- |\+\+\+ |(new|deleted) file mode |old mode |new mode |similarity index |dissimilarity index )/;
export interface DiffFrame { readonly lines: string[]; readonly hunkRows: number[]; readonly hunkSources: number[] }
/**
 * The diff, each row exactly `width` cells with a one-cell margin each side: the `path ● author` header in bold,
 * each hunk row on the hunk surface (`@@ new file`, `@@ deleted file`, or the hunk line itself; the current hunk bold
 * with `n/m` when there are several), then added and removed rows on their surfaces and context rows muted, each
 * after a right-aligned line number (four cells or more; none on removed rows) and its `+`/`−` sign.
 */
export function diffLines(m: ChangesModel, width: number, color: (hue: number) => string = () => PANEL_TEXT, shortPaths = false): DiffFrame {
  const lines: string[] = [], hunkRows: number[] = [], hunkSources: number[] = [];
  const inner = innerOf(width), canvas = CANVAS(), ink = currentInk();
  const put = (row: string) => lines.push(framed(row, width));
  const say = (style: string, text: string) => { for (const row of wrap(plainLabel(text), inner)) put(line([[style, row]], [], inner, canvas)); };
  for (const note of m.notes) say(PANEL_ERROR, note);
  const d = m.diff;
  if (d.kind === "message") { for (const [i, text] of d.lines.entries()) say(i ? PANEL_MUTED : PANEL_TEXT, text); return { lines, hunkRows, hunkSources }; }
  const source = d.text.split("\n"); if (source.at(-1) === "") source.pop();
  const total = source.filter(row => HUNK.test(row)).length, current = Math.max(0, Math.min(total - 1, m.hunk));
  const highest = Math.max(0, ...source.map(row => HUNK.exec(row)).map(h => h ? Number(h[3]) + Number(h[4] ?? 1) : 0));
  const digits = Math.max(4, String(highest).length);
  const who: Seg[] = (m.files.find(file => file.path === d.path)?.who ?? []).flatMap(id => {
    const author = authorOf(m, id); return author ? [[PANEL_TEXT, " "] as Seg, [color(author.hue), "● " + author.label] as Seg] : [];
  });
  const status: Seg[] = d.status === "U" ? [[PANEL_TEXT, " "], [PANEL_ERROR, "conflict"]] : [];
  const room = Math.max(8, inner - segWidth(who) - segWidth(status));
  put(line([[BOLD + PANEL_TEXT, clip(shorten(d.path, shortPaths), room, "tail")], ...status, ...who], [], inner, canvas));
  if (d.notice) say(PANEL_BLUE, d.notice);
  // `newNo` is the next new-side line number inside a hunk, undefined before the first hunk of a file.
  let newNo: number | undefined, hunk = -1;
  source.forEach((raw, index) => {
    const h = HUNK.exec(raw);
    if (raw.startsWith("diff ")) { newNo = undefined; return; }
    if (h) {
      hunk++; hunkRows.push(lines.length); hunkSources.push(index); newNo = Number(h[3]);
      const created = h[1] === "0" && h[2] === "0", deleted = h[3] === "0" && h[4] === "0";
      const text = created ? "@@ new file" : deleted ? "@@ deleted file" : code(raw).trimEnd();
      const on = hunk === current;
      put(line([[(on ? BOLD : "") + PANEL_BLUE, text]], on && total > 1 ? [[PANEL_MUTED, `${hunk + 1}/${total}`]] : [], inner, NORMAL + PANEL_DIALOG));
      return;
    }
    if (newNo === undefined) {
      if (!QUIET.test(raw)) say(PANEL_MUTED, raw);
      return;
    }
    const sign = raw[0], kind = sign === "+" ? "add" : sign === "-" ? "del" : sign === " " ? "ctx" : "meta";
    const back = kind === "add" ? ink.addBack : kind === "del" ? ink.delBack : PANEL_BG;
    const style = kind === "add" ? ink.add : kind === "del" ? ink.del : PANEL_MUTED;
    const lineNo = kind === "add" || kind === "ctx" ? String(newNo++) : "";
    const mark = kind === "add" ? "+" : kind === "del" ? "−" : kind === "ctx" ? " " : "";
    const text = code(kind === "meta" ? raw : raw.slice(1)), lead = digits + 1;
    wrap(text, Math.max(1, inner - lead - mark.length)).forEach((chunk, k) => put(line([[ink.number, k ? " ".repeat(lead) : lineNo.padStart(digits) + " "],
      [style, k ? " ".repeat(mark.length) : mark], [style, chunk]], [], inner, NORMAL + back)));
  });
  if (!total) say(PANEL_MUTED, source.some(row => !QUIET.test(row) && !row.startsWith("diff ")) ? "No text hunks in this diff." : "Empty diff.");
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
  /** Agents that edit this worktree; author chips and dots show only for the ones with files in the comparison. */
  authors?(): readonly ChangeAuthor[];
  /** ANSI foreground for an author hue. */
  color?(hue: number): string;
  /** Terminal rows, which cap the stacked file block; the layout viewport height is used without it. */
  rows?(): number;
}
/** Owns the Changes components; the workbench keeps selection, patch acceptance and key handling. */
export class ChangesView {
  readonly diff: ScrollView; readonly list: ScrollView; readonly root: Component;
  hunkRows: number[] = []; hunkSources: number[] = [];
  /** `all`, or the author id whose files the host should show (passed to `changesModel`). */
  filter = "all";
  private listHits: ReadonlyMap<number, string> = new Map();
  private listChips: readonly ChipHit[] = [];
  private stackHits: ReadonlyMap<number, string> = new Map();
  private stackChips: readonly ChipHit[] = [];
  private shownSelection: string | undefined;
  private stackTop = 0;
  private stackCap = stackRows(Number.MAX_SAFE_INTEGER);
  private shortPaths = false;
  /** Rows of the layout viewport, which bound the body; the `│` column draws that many and the body clips the rest. */
  private ruleRows = 0;
  private readonly color: (hue: number) => string;
  constructor(private readonly host: ChangesHost) {
    this.color = hue => host.color?.(hue) ?? PANEL_TEXT;
    this.diff = new ScrollView({ render: width => {
      const frame = diffLines(host.model(), width, this.color, this.shortPaths);
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
      host.focus(); this.pick(event, this.listChips, this.listHits);
      return { handled: true };
    }, invalidate() {} }, { follow: "none", scrollbar: "auto" });
    const comparison: Component = { render: width => [comparisonRow(host.model(), width)], invalidate() {} };
    const empty: Component = { render: width => emptyLines(host.model(), width), invalidate() {} };
    const stacked: Component = { render: width => this.renderStacked(width), handleMouse: event => {
      if (!host.click(event)) return undefined;
      host.focus(); this.pick(event, this.stackChips, this.stackHits);
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
    // The rule is its own column so it runs the body's full height whatever the list's length or scroll.
    const rule: Component = { render: () => Array.from({ length: Math.max(1, this.ruleRows) }, () => CANVAS() + PANEL_LINE + "│" + RESET), invalidate() {} };
    const split = (viewport: { width: number }) => viewport.width >= SPLIT_MIN;
    const body = new HStack([
      { component: this.list, basis: LIST_WIDTH - 1, shrink: 0, visible: viewport => {
        this.shortPaths = viewport.width < SHORT_PATHS; this.ruleRows = viewport.height;
        return split(viewport);
      } },
      { component: rule, basis: 1, shrink: 0, visible: split },
      { component: column, basis: 0, grow: 1, minSize: 1 },
    ]);
    this.root = new VStack([
      { component: comparison, basis: 1, shrink: 0, visible: () => showsComparison(host.model()) },
      { component: empty, basis: 0, grow: 1, minSize: 1, visible: () => emptyChanges(host.model()) },
      { component: body, basis: 0, grow: 1, minSize: 1, visible: () => !emptyChanges(host.model()) },
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
  /** A click on a chip sets the author filter; one on a file row (or its flag row) selects the file. Chips start after the margin cell. */
  private pick(event: TuiMouseEvent, chips: readonly ChipHit[], hits: ReadonlyMap<number, string>): void {
    const chip = chips.find(hit => hit.row === event.y && event.x - 1 >= hit.from && event.x - 1 < hit.to);
    if (chip) { this.setFilter(chip.id); return; }
    const path = hits.get(event.y); if (path) this.host.select(path);
  }
  /** The side-by-side file column, left of the `│` rule: margin, chips and file rows, one blank cell. */
  private renderList(width: number): string[] {
    const model = this.host.model(), inner = Math.max(1, width - 2);
    const chips = authorChips(model, inner, this.color), frame = listLines(model, inner, { color: this.color });
    const offset = chips.lines.length;
    this.listChips = chips.hits;
    this.listHits = new Map([...frame.hits].map(([row, path]) => [row + offset, path]));
    const lines = [...chips.lines, ...frame.lines].map(row => CANVAS() + " " + row + CANVAS() + " " + RESET);
    // Keep a newly selected file in view without fighting a reader who scrolled the list.
    if (model.selected !== this.shownSelection && frame.selectedRow >= 0) {
      this.shownSelection = model.selected;
      const top = this.list.scrollTop, height = this.list.viewportHeight, row = frame.selectedRow + offset;
      if (height > 0 && (row < top || row + frame.selectedSpan > top + height)) {
        this.list.updateLayout(lines.length, height, () => {});
        this.list.scrollTo(Math.max(0, row - Math.floor(height / 2)), { disableFollow: true });
      }
    }
    return lines;
  }
  /**
   * The stacked file block: chips, then at most the rows left of the block's cap, moved the least distance that
   * keeps the selected file whole, then a rule; the rule carries `4–8/12` while files lie outside the block.
   */
  private renderStacked(width: number): string[] {
    const m = this.host.model(), inner = innerOf(width);
    const chips = authorChips(m, inner, this.color), frame = listLines(m, inner, { color: this.color, short: this.shortPaths });
    const cap = Math.max(1, this.stackCap - chips.lines.length), total = frame.lines.length, last = Math.max(0, total - cap);
    let top = Math.min(this.stackTop, last);
    if (frame.selectedRow >= 0) {
      const end = frame.selectedRow + Math.max(1, frame.selectedSpan);
      if (frame.selectedRow < top) top = frame.selectedRow;
      else if (end > top + cap) top = Math.min(frame.selectedRow, end - cap);
    }
    top = Math.max(0, Math.min(top, last));
    this.stackTop = top;
    const offset = chips.lines.length, hits = new Map<number, string>();
    for (const [row, path] of frame.hits) if (row >= top && row < top + cap) hits.set(row - top + offset, path);
    this.stackHits = hits; this.stackChips = chips.hits;
    const shown = [...new Set(hits.values())].map(path => m.files.findIndex(file => file.path === path)).filter(index => index >= 0);
    const range = total > cap && shown.length ? ` ${Math.min(...shown) + 1}–${Math.max(...shown) + 1}/${m.files.length} ` : "";
    const fill = inner - visibleWidth(range) - (range ? 1 : 0);
    const rule = range && fill >= 1 ? PANEL_LINE + "─".repeat(fill) + PANEL_MUTED + range + PANEL_LINE + "─" : PANEL_LINE + "─".repeat(inner);
    return [...[...chips.lines, ...frame.lines.slice(top, top + cap)].map(row => framed(row, width)), framed(CANVAS() + rule, width)];
  }
}
