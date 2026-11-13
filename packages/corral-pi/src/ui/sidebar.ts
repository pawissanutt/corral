// The session sidebar: Context, Files and Plan over structural inputs. One body model feeds the
// rail (34 columns from 105), the collapsed navigation row above the composer and the drawer that
// row opens. Selection is keyed per tab and the page window keeps it in view with the least
// movement, so new items never move it. Every state carries a glyph or word next to its colour. The
// only key cap is `Ctrl+B` beside the toggle; nothing here fills a background except the selected row.
import { matchesKey, visibleWidth } from "@earendil-works/pi-tui";
import type { TodoProgress } from "./todo-widget.ts";
import { keyCap, rule, spinner } from "./frame.ts";
import { COMPACT_COLUMNS } from "./header.ts";
import { splitRow } from "./panels-chrome.ts";
import {
  pad, plainLabel, PANEL_BG, PANEL_BLUE, PANEL_ERROR, PANEL_MUTED, PANEL_SELECTED, PANEL_SUCCESS, PANEL_TEXT,
  PANEL_WARNING, RESET,
} from "./panels-view.ts";

export type SideTab = "context" | "files" | "plan";
export type SideTone = "text" | "muted" | "accent" | "success" | "warning" | "error";
export interface SideAgent { readonly key: string; readonly name: string; readonly hue: number; readonly profile: string;
  readonly glyph: string; readonly tone: SideTone; readonly activity: string; readonly settled?: "ok" | "failed" }
export interface SideJob { readonly key: string; readonly command: string; readonly glyph: string; readonly tone: SideTone;
  readonly time: string; readonly settled?: "ok" | "failed" }
export interface SideFile { readonly path: string; readonly letter: string; readonly added: number | null | undefined;
  readonly removed: number | null | undefined; readonly who?: readonly number[]; readonly outside?: boolean }
export interface SideSource {
  /** null: not available in this session. */
  readonly agents: readonly SideAgent[] | null;
  readonly jobs: readonly SideJob[] | null;
  readonly files: { readonly scope: string; readonly state: "ok" | "loading" | "error" | "choose";
    readonly error?: string; readonly list: readonly SideFile[] };
  readonly plan: { readonly progress: TodoProgress | null; readonly stale: boolean };
  readonly bound: boolean;
  /** ANSI foreground for an agent hue index. */
  readonly color: (hue: number) => string;
  /** Key of the agent the Agents view selects while its list has the focus; its row is drawn selected. */
  readonly current?: string;
}
export type SideTarget = { readonly kind: "agent" | "job"; readonly key: string } | { readonly kind: "file"; readonly path: string }
  | { readonly kind: "step"; readonly id: string } | { readonly kind: "scope" };
/** A pointer target on one frame line; `from` inclusive, `to` exclusive, in cells from the frame's left edge. */
export interface SideHit { readonly from: number; readonly to: number;
  readonly tab?: SideTab; readonly target?: SideTarget; readonly page?: -1 | 1; readonly toggle?: true }
export interface SideFrame { readonly lines: string[]; readonly hits: ReadonlyMap<number, readonly SideHit[]> }

/** Body rows of the drawer for a terminal of `terminalRows` rows: 2 at 80x14, at most 5 when short, 10 otherwise. */
export function drawerRows(terminalRows: number): number {
  return Math.max(2, Math.min(terminalRows < 28 ? 5 : 10, terminalRows - 12));
}
/** Below this many terminal rows the rule above the collapsed row goes first. */
const RULE_ROWS = 16;

const TABS: readonly SideTab[] = ["context", "files", "plan"];
const LABELS: Readonly<Record<SideTab, string>> = { context: "Context", files: "Files", plan: "Plan" };
const BOLD = "\x1b[1m", NORMAL = "\x1b[22m", UNDERLINE = "\x1b[4m", NO_UNDERLINE = "\x1b[24m";

function toneColor(tone: SideTone): string {
  switch (tone) {
    case "text": return PANEL_TEXT;
    case "muted": return PANEL_MUTED;
    case "accent": return PANEL_BLUE;
    case "success": return PANEL_SUCCESS;
    case "warning": return PANEL_WARNING;
    case "error": return PANEL_ERROR;
  }
}

/**
 * The tab summaries the collapsed row shows. Live agents count by tone (accent running, warning
 * asking, anything else waiting); unsettled jobs count as running. Tiny keeps one count of what is at
 * work (running and asking). Files shows only `?` (no project) or `!` (Git failed); its count is the
 * Changes tab's.
 */
export function sideSummary(source: SideSource, tiny: boolean): Readonly<Record<SideTab, string>> {
  const agents = source.agents ?? [], jobs = source.jobs ?? [];
  const live = agents.filter(agent => !agent.settled), liveJobs = jobs.filter(job => !job.settled).length;
  const asking = live.filter(agent => agent.tone === "warning").length;
  const waiting = live.filter(agent => agent.tone !== "warning" && agent.tone !== "accent").length;
  const running = live.length - asking - waiting + liveJobs;
  const failed = agents.filter(agent => agent.settled === "failed").length + jobs.filter(job => job.settled === "failed").length;
  const context = tiny ? (running + asking ? String(running + asking) : "")
    : [running ? `${running} running` : "", asking ? `${asking} asking` : "", waiting ? `${waiting} waiting` : "",
      failed ? `${failed} failed` : ""].filter(Boolean).join(" · ");
  const { files } = source;
  const steps = source.plan.progress?.phases.flatMap(phase => phase.steps) ?? [];
  return {
    context,
    files: files.state === "error" ? "!" : files.state === "choose" ? "?" : "",
    plan: steps.length ? `${steps.filter(step => step.status.state === "done").length}/${steps.length}` : "",
  };
}

// One body line: `left` starts at the glyph column (column 1); a function receives the cells it may use.
interface Row { readonly left: string | ((room: number) => string); readonly right?: string }
interface Item { readonly key: string; readonly target: SideTarget }
interface Block { readonly rows: readonly Row[]; readonly item?: Item; readonly gap?: true }

const segmenter = new Intl.Segmenter();
/** The end of `text` within `width` cells, led by "…": a clipped path keeps its file name. */
function tail(text: string, width: number): string {
  if (visibleWidth(text) <= width) return text;
  if (width <= 1) return width === 1 ? "…" : "";
  const parts = Array.from(segmenter.segment(text), part => part.segment);
  let kept = "", used = 0;
  for (let i = parts.length - 1; i >= 0; i--) {
    const part = parts[i] ?? "", size = visibleWidth(part);
    if (used + size > width - 1) break;
    kept = part + kept; used += size;
  }
  return "…" + kept;
}
/** Exactly `width` cells. Truncation ends with a full reset, so the row background is restored after each one. */
function paint(text: string, width: number, bg: string): string {
  return bg + pad(text, width).replaceAll(RESET, RESET + bg) + RESET;
}
function targetKey(target: SideTarget): string {
  switch (target.kind) {
    case "agent": case "job": return `${target.kind}:${target.key}`;
    case "file": return `file:${target.path}`;
    case "step": return `step:${target.id}`;
    case "scope": return "scope";
  }
}

const GAP: Block = { rows: [{ left: "" }], gap: true };
const heading = (text: string): Block => ({ rows: [{ left: PANEL_MUTED + text }] });
/** A muted line; `indent` 2 aligns it with item text under a heading. */
const note = (text: string, indent = 2): Block => ({ rows: [{ left: " ".repeat(indent) + PANEL_MUTED + text }] });
const counts = (added: number, removed: number) => PANEL_SUCCESS + `+${added}` + " " + PANEL_ERROR + `−${removed}`;
/** A running state draws the braille spinner, staggered by row; every other state its own glyph. */
const stateGlyph = (glyph: string, tone: SideTone, running: boolean, phase: number) =>
  toneColor(tone) + (running ? spinner(Date.now(), phase) : plainLabel(glyph) || "·");

function contextBlocks(source: SideSource): Block[] {
  const blocks: Block[] = [];
  // Sections are separated by one blank row, never a rule.
  const section = (block: Block) => { if (blocks.length) blocks.push(GAP); blocks.push(block); };
  const agents = source.agents ?? [], jobs = source.jobs ?? [];
  if (!agents.length && !jobs.length) return [note("no agents or jobs yet")];
  // Successful completions fold into one line; failed and uncertain ones stay listed.
  const done = [...agents.filter(agent => agent.settled === "ok").map(agent => plainLabel(agent.name)),
    ...jobs.filter(job => job.settled === "ok").map(job => plainLabel(job.command))];
  if (source.agents !== null) {
    section(heading("AGENTS"));
    const listed = agents.filter(agent => agent.settled !== "ok");
    if (!listed.length) blocks.push(note("none at work"));
    listed.forEach((agent, index) => {
      const name = plainLabel(agent.name), profile = plainLabel(agent.profile), activity = plainLabel(agent.activity);
      // A helper's name already is its kind; the profile shows only when it adds something.
      const left = stateGlyph(agent.glyph, agent.tone, !agent.settled && agent.tone === "accent", index) + " "
        + source.color(agent.hue) + BOLD + name + NORMAL + (profile && !name.startsWith(profile) ? " " + PANEL_MUTED + profile : "");
      blocks.push({ rows: activity ? [{ left }, { left: "  " + PANEL_MUTED + activity }] : [{ left }],
        item: { key: targetKey({ kind: "agent", key: agent.key }), target: { kind: "agent", key: agent.key } } });
    });
  }
  // A Jobs section appears only while a job is listed.
  const listedJobs = jobs.filter(job => job.settled !== "ok");
  if (listedJobs.length) section(heading("JOBS"));
  listedJobs.forEach((job, index) => {
    const time = plainLabel(job.time);
    blocks.push({ rows: [{ left: stateGlyph(job.glyph, job.tone, !job.settled && job.tone === "accent", index + agents.length) + " "
      + PANEL_TEXT + plainLabel(job.command), ...(time ? { right: PANEL_MUTED + time } : {}) }],
    item: { key: targetKey({ kind: "job", key: job.key }), target: { kind: "job", key: job.key } } });
  });
  if (done.length) {
    section(heading(`COMPLETED · ${done.length}`));
    blocks.push(note(done.join(" · ")));
  }
  return blocks;
}

/** The last two segments of a path: the file and its folder. */
const shortPath = (path: string) => path.split("/").slice(-2).join("/");

function fileBlocks(source: SideSource): Block[] {
  const { files } = source;
  const choose: Block = { rows: [{ left: PANEL_BLUE + "› " + PANEL_TEXT + "Choose project" }], item: { key: "scope", target: { kind: "scope" } } };
  if (files.state === "choose") return [{ rows: [{ left: PANEL_WARNING + "? " + PANEL_MUTED + "No project chosen" }] }, choose];
  if (files.state === "error") {
    const detail = plainLabel(files.error?.split("\n")[0] ?? "");
    return [{ rows: [{ left: PANEL_WARNING + "! " + PANEL_TEXT + "Git unavailable" }] }, ...(detail ? [note(detail)] : []), choose];
  }
  if (!files.list.length) return [note(files.state === "loading" ? "reading changes…" : "no changes yet")];
  return files.list.map(file => {
    // The two-cell mark column: the dots of the agents that touched the file, else its Git status letter.
    const who = (file.who ?? []).slice(0, 2);
    const letter = plainLabel(file.letter).slice(0, 1) || "·";
    const tone = letter === "A" || letter === "?" ? PANEL_SUCCESS : letter === "D" ? PANEL_ERROR : letter === "U" ? PANEL_WARNING
      : letter === "R" ? PANEL_BLUE : PANEL_MUTED;
    const mark = who.length ? who.map(hue => source.color(hue) + "●").join("") + (who.length < 2 ? " " : "") : tone + letter + " ";
    const flag = file.outside ? PANEL_WARNING + "! " : "";
    const lead = 2 + (file.outside ? 2 : 0), path = shortPath(plainLabel(file.path));
    const right = file.added === null ? PANEL_MUTED + (letter === "?" || letter === "A" ? "new" : "bin")
      : file.added === undefined ? "" : counts(file.added, file.removed ?? 0);
    return { rows: [{ left: (room: number) => mark + flag + PANEL_TEXT + tail(path, Math.max(0, room - lead)),
      ...(right ? { right } : {}) }], item: { key: targetKey({ kind: "file", path: file.path }), target: { kind: "file", path: file.path } } };
  });
}

function planBlocks(source: SideSource, fold: boolean): Block[] {
  const { progress, stale } = source.plan;
  const phases = progress?.phases ?? [], steps = phases.flatMap(phase => phase.steps);
  if (!steps.length) return [note("no plan yet")];
  const waits = new Map(progress?.waits?.map(wait => [wait.step, wait.on.kind] as const) ?? []);
  const done = steps.filter(step => step.status.state === "done").length;
  const blocks: Block[] = [{ rows: [{ left: PANEL_MUTED + `${done}/${steps.length} done` + (stale ? PANEL_WARNING + " · stale" : "") }] }];
  // Under pressure finished steps fold into one line so the open ones stay in view.
  if (fold && done) blocks.push({ rows: [{ left: PANEL_SUCCESS + "✓" + PANEL_MUTED + ` ${done} done` }] });
  let running = 0;
  for (const phase of phases) {
    const shown = phase.steps.filter(step => !fold || step.status.state !== "done");
    if (!shown.length) continue;
    if (phases.length > 1) blocks.push(heading(plainLabel(phase.label)));
    for (const step of shown) {
      const state = step.status.state, wait = waits.get(step.id);
      const [mark, markColor, textColor]: readonly [string, string, string] = state === "done" ? ["✓", PANEL_SUCCESS, PANEL_MUTED]
        : state === "in_progress" ? [spinner(Date.now(), running++), PANEL_BLUE, PANEL_TEXT] : state === "dropped" ? ["–", PANEL_MUTED, PANEL_MUTED]
        : ["○", PANEL_MUTED, PANEL_TEXT];
      const right = wait === "owner" ? PANEL_WARNING + "?" : wait === "event" ? PANEL_MUTED + "…" : "";
      blocks.push({ rows: [{ left: markColor + mark + " " + textColor + plainLabel(step.label), ...(right ? { right } : {}) }],
        item: { key: targetKey({ kind: "step", id: step.id }), target: { kind: "step", id: step.id } } });
    }
  }
  return blocks;
}

const lineCount = (blocks: readonly Block[]) => blocks.reduce((n, block) => n + block.rows.length, 0);

/** The blocks a page starting at item `start` shows within `rows`, and the last item it shows whole. */
function reach(blocks: readonly Block[], items: readonly number[], start: number, rows: number): { from: number; to: number; last: number } {
  const first = items[Math.max(0, start)] ?? blocks.length;
  let from = start <= 0 ? 0 : first;
  // A page keeps the heading directly above its first item, never a leading blank.
  if (start > 0) while (from > 0 && !blocks[from - 1]?.item) from--;
  // Leading lines give way until at least the first line of the page's first item fits.
  let above = 0;
  for (let at = from; at < first; at++) above += blocks[at]?.rows.length ?? 0;
  while (from < first && (blocks[from]?.gap || above + 1 > rows)) { above -= blocks[from]?.rows.length ?? 0; from++; }
  // Item indices run in block order from the page's first item.
  let used = 0, to = from, last = -1, item = Math.max(0, start);
  for (let at = from; at < blocks.length; at++) {
    const block = blocks[at], size = block?.rows.length ?? 0;
    if (used + size > rows) {
      // The page's first item always shows, clipped, so a short pane never shows only headings.
      if (last < 0 && block?.item) { to = at + 1; last = item; }
      break;
    }
    used += size; to = at + 1;
    if (block?.item) last = item++;
  }
  return { from, to, last };
}

interface TabState {
  key: string | undefined; index: number; offset: number; align: "end" | undefined;
  first: number; last: number; firstKey: string | undefined; lastKey: string | undefined; capacity: number;
}
interface Pager { readonly first: number; readonly last: number; readonly count: number }
interface Body { readonly lines: string[]; readonly hits: Map<number, SideHit[]>; readonly pager: Pager | undefined }
interface Segment { readonly text: string; readonly width: number; readonly hit?: Omit<SideHit, "from" | "to"> }

const seg = (text: string, visible: string, hit?: Omit<SideHit, "from" | "to">): Segment => ({ text, width: visibleWidth(visible), ...(hit ? { hit } : {}) });
const width = (list: readonly Segment[]) => list.reduce((n, part) => n + part.width, 0);
/** `left` from column 1, `right` against a one-cell margin; the last right segment's hit runs to the edge. */
function compose(left: readonly Segment[], right: readonly Segment[], cells: number): { line: string; hits: SideHit[] } {
  const rightWidth = width(right), room = Math.max(0, cells - 2 - rightWidth - (rightWidth ? 1 : 0));
  const hits: SideHit[] = [];
  let column = 1;
  for (const part of left) {
    if (part.hit && column + part.width <= room + 1) hits.push({ ...part.hit, from: column, to: column + part.width });
    column += part.width;
  }
  column = cells - 1 - rightWidth;
  right.forEach((part, at) => {
    if (part.hit && column >= 0) hits.push({ ...part.hit, from: column, to: at === right.length - 1 ? cells : column + part.width });
    column += part.width;
  });
  const text = " " + pad(left.map(part => part.text).join(""), room) + (rightWidth ? PANEL_BG + " " : "") + right.map(part => part.text).join("") + " ";
  return { line: paint(text, cells, PANEL_BG), hits };
}
const space = (cells: number): Segment => ({ text: " ".repeat(cells), width: cells });
/** The toggle at a row's right edge: `▸`, `▴` or `▾`, after the `Ctrl+B` key cap when `capped`; all of it clicks. */
function toggle(arrow: string, capped: boolean): Segment[] {
  return [...(capped ? [seg(keyCap("Ctrl+B"), " Ctrl+B ", { toggle: true })] : []), seg(PANEL_MUTED + " " + arrow, " " + arrow, { toggle: true })];
}
/** The first `[left, right]` pair that fits `cells` (one-cell margins, one cell between them), else the last. */
function choose(variants: readonly (readonly [readonly Segment[], readonly Segment[]])[], cells: number): { line: string; hits: SideHit[] } {
  const [left, right] = variants.find(([l, r]) => width(l) + width(r) + (width(r) ? 1 : 0) <= cells - 2) ?? variants[variants.length - 1] ?? [[], []] as const;
  return compose(left, right, cells);
}
/** `lines` under a rule in the line colour, unless the terminal has fewer than 16 rows; hits move down with their lines. */
function ruled(lines: readonly string[], hits: ReadonlyMap<number, readonly SideHit[]>, cells: number, rows: number | undefined): SideFrame {
  if (rows !== undefined && rows < RULE_ROWS) return { lines: [...lines], hits };
  return { lines: [paint(rule(cells), cells, PANEL_BG), ...lines], hits: new Map([...hits].map(([row, list]) => [row + 1, list])) };
}
function pagerSegments(pager: Pager): Segment[] {
  const back = pager.first > 0, next = pager.last < pager.count - 1;
  const range = pager.first === pager.last ? `${pager.first + 1}` : `${pager.first + 1}–${pager.last + 1}`;
  return [seg((back ? PANEL_TEXT : PANEL_MUTED) + "‹ ", "‹ ", back ? { page: -1 } : undefined),
    seg(PANEL_MUTED + `${range}/${pager.count}`, `${range}/${pager.count}`),
    seg((next ? PANEL_TEXT : PANEL_MUTED) + " ›", " ›", next ? { page: 1 } : undefined)];
}

export class Sidebar {
  tab: SideTab = "context";
  focused = false;
  drawer = false;
  private readonly states: Record<SideTab, TabState> = { context: Sidebar.state(), files: Sidebar.state(), plan: Sidebar.state() };
  /** Tab hits drawn by the collapsed row (a click opens the drawer) and by the open drawer (the active tab closes it). */
  private readonly opening = new WeakSet<SideHit>();
  private readonly closing = new WeakSet<SideHit>();

  constructor(private readonly changed: () => void) {}

  private static state(): TabState {
    return { key: undefined, index: 0, offset: 0, align: undefined, first: 0, last: 0, firstKey: undefined, lastKey: undefined, capacity: Infinity };
  }

  /**
   * The rail, from the row under the tab rule down: tabs and `[Ctrl+B] ▸` on row 0, the active tab's body
   * directly below, a pager on the last row when it overflows. A narrow rail drops the key cap, then
   * shortens the tabs to initials.
   */
  rail(source: SideSource, cells: number, height: number, resizing?: number): SideFrame {
    const lines: string[] = [], hits = new Map<number, SideHit[]>();
    if (height <= 0) return { lines, hits };
    const head = resizing === undefined
      ? choose([[this.rowSegments(undefined, 1), toggle("▸", true)], [this.rowSegments(undefined, 1), toggle("▸", false)],
        [this.initials(true), toggle("▸", false)]], cells)
      : compose([seg(PANEL_BLUE + `⇔ ${resizing} cols`, `⇔ ${resizing} cols`)], toggle("▸", false), cells);
    lines.push(head.line); hits.set(0, head.hits);
    const blank = paint("", cells, PANEL_BG);
    const capacity = height - lines.length;
    if (capacity > 0) {
      const body = this.body(source, cells, capacity, false), top = lines.length;
      for (const [row, list] of body.hits) hits.set(top + row, list);
      lines.push(...body.lines);
      if (body.pager) {
        while (lines.length < height - 1) lines.push(blank);
        const pager = compose(pagerSegments(body.pager), [], cells);
        hits.set(lines.length, pager.hits); lines.push(pager.line);
      }
    }
    while (lines.length < height) lines.push(blank);
    return { lines, hits };
  }

  /**
   * The collapsed navigation row under its rule: muted labels with their summaries in text, and
   * `[Ctrl+B] ▴` at the right (`▴` alone when tiny). The rule goes when the terminal has fewer than
   * 16 `rows`.
   */
  row(source: SideSource, cells: number, tiny: boolean, rows?: number): SideFrame {
    const summary = sideSummary(source, tiny), short = sideSummary(source, true), plain = toggle("▴", false);
    const variants: [readonly Segment[], readonly Segment[]][] = tiny
      ? [[this.rowSegments(summary, 2), plain], [this.rowSegments(undefined, 2), plain], [this.initials(false), plain]]
      : [[this.rowSegments(summary, 2), toggle("▴", true)], [this.rowSegments(summary, 2), plain], [this.rowSegments(short, 2), plain],
        [this.rowSegments(undefined, 2), plain], [this.initials(false), plain]];
    const row = choose(variants, cells);
    for (const hit of row.hits) if (hit.tab) this.opening.add(hit);
    return ruled([row.line], new Map([[0, row.hits]]), cells, rows);
  }

  /**
   * The open drawer under its rule: the tabs with the active one marked, the pager and `[Ctrl+B] ▾` at the
   * right (`▾` alone below 64 columns), then the active tab's body for `drawerRows(rows)` rows at most.
   */
  drawerFrame(source: SideSource, cells: number, rows: number): SideFrame {
    const capacity = drawerRows(rows), body = this.body(source, cells, capacity, true);
    const right = (capped: boolean) => body.pager ? [...pagerSegments(body.pager), space(1), ...toggle("▾", capped)] : toggle("▾", capped);
    const variants: [readonly Segment[], readonly Segment[]][] = [
      ...(cells >= COMPACT_COLUMNS ? [[this.rowSegments(undefined, 2), right(true)] as [Segment[], Segment[]]] : []),
      [this.rowSegments(undefined, 2), right(false)], [this.initials(true), right(false)]];
    const head = choose(variants, cells);
    for (const hit of head.hits) if (hit.tab) this.closing.add(hit);
    const lines = [head.line, ...body.lines], hits = new Map<number, SideHit[]>([[0, head.hits]]);
    for (const [row, list] of body.hits) hits.set(row + 1, list);
    // A paged drawer keeps its height while turning pages.
    if (body.pager) while (lines.length < capacity + 1) lines.push(paint("", cells, PANEL_BG));
    return ruled(lines, hits, cells, rows);
  }

  nextTab(): void { this.shift(1); }

  handleKey(data: string, source: SideSource): { readonly handled: boolean; readonly open?: SideTarget; readonly leave?: boolean } {
    if (matchesKey(data, "left") || data === "h") { this.shift(-1); return { handled: true }; }
    if (matchesKey(data, "right") || data === "l" || matchesKey(data, "f6")) { this.shift(1); return { handled: true }; }
    if (matchesKey(data, "escape")) { this.focused = false; this.changed(); return { handled: true, leave: true }; }
    const items = this.items(source);
    if (matchesKey(data, "up") || data === "k") { this.move(-1, items); return { handled: true }; }
    if (matchesKey(data, "down") || data === "j") { this.move(1, items); return { handled: true }; }
    if (matchesKey(data, "pageUp")) { this.turn(-1, items); return { handled: true }; }
    if (matchesKey(data, "pageDown")) { this.turn(1, items); return { handled: true }; }
    if (matchesKey(data, "enter")) {
      const at = this.resolve(this.states[this.tab], items, true), item = items[at];
      return item ? { handled: true, open: item.target } : { handled: true };
    }
    return { handled: false };
  }

  /** Tabs, pager and items; the toggle is the caller's. An item click selects it and returns it to open. */
  click(hit: SideHit, source: SideSource): SideTarget | undefined {
    if (hit.tab) {
      if (this.opening.has(hit)) { this.tab = hit.tab; this.drawer = true; }
      else if (this.closing.has(hit) && hit.tab === this.tab) this.drawer = false;
      else this.tab = hit.tab;
      this.changed();
      return undefined;
    }
    if (hit.page) { this.turn(hit.page, this.items(source)); return undefined; }
    if (!hit.target) return undefined;
    const state = this.states[this.tab], key = targetKey(hit.target), at = this.items(source).findIndex(item => item.key === key);
    state.key = key;
    if (at >= 0) state.index = at;
    this.changed();
    return hit.target;
  }

  private shift(delta: number): void {
    this.tab = TABS[(TABS.indexOf(this.tab) + delta + TABS.length) % TABS.length] ?? "context";
    this.changed();
  }
  private blocks(source: SideSource, capacity: number): Block[] {
    if (this.tab === "context") return contextBlocks(source);
    if (this.tab === "files") return fileBlocks(source);
    const full = planBlocks(source, false);
    return lineCount(full) > capacity ? planBlocks(source, true) : full;
  }
  private items(source: SideSource): Item[] {
    return this.blocks(source, this.states[this.tab].capacity).flatMap(block => block.item ? [block.item] : []);
  }
  /** The shown selection: the kept key, else the item now at the old position. Focus pins it to a key. */
  private resolve(state: TabState, items: readonly Item[], pin = this.focused): number {
    if (!items.length) return -1;
    let at = state.key === undefined ? -1 : items.findIndex(item => item.key === state.key);
    if (at < 0) at = Math.min(state.index, items.length - 1);
    state.index = at;
    if (state.key !== undefined || pin) state.key = items[at]?.key;
    return at;
  }
  private move(delta: number, items: readonly Item[]): void {
    const state = this.states[this.tab], at = this.resolve(state, items, true);
    if (at < 0) return;
    const next = Math.max(0, Math.min(items.length - 1, at + delta));
    state.key = items[next]?.key; state.index = next;
    this.changed();
  }
  /** Next page: select the first item past it and start there. Previous page: select the item before it and end there. */
  private turn(direction: -1 | 1, items: readonly Item[]): void {
    const state = this.states[this.tab], find = (key: string | undefined, fallback: number) => {
      const at = key === undefined ? -1 : items.findIndex(item => item.key === key);
      return at < 0 ? fallback : at;
    };
    if (direction > 0) {
      const last = find(state.lastKey, state.last);
      if (last >= items.length - 1) return;
      state.key = items[last + 1]?.key; state.index = last + 1; state.offset = last + 1; state.align = undefined;
    } else {
      const first = find(state.firstKey, state.first);
      if (first <= 0) return;
      state.key = items[first - 1]?.key; state.index = first - 1; state.align = "end";
    }
    this.changed();
  }

  /** The active tab's body within `capacity` rows; when it overflows, a page window that holds the selection. */
  private body(source: SideSource, cells: number, capacity: number, inlinePager: boolean): Body {
    const state = this.states[this.tab];
    state.capacity = capacity;
    const blocks = this.blocks(source, capacity);
    const items: number[] = [];
    blocks.forEach((block, at) => { if (block.item) items.push(at); });
    const list = items.map(at => blocks[at]?.item).filter((item): item is Item => item !== undefined);
    const selected = this.resolve(state, list);
    const paged = items.length > 0 && lineCount(blocks) > capacity;
    const rows = Math.max(0, paged && !inlinePager ? capacity - 1 : capacity);
    let start = 0;
    if (paged) {
      const last = (from: number) => reach(blocks, items, from, rows).last;
      start = Math.max(0, Math.min(state.offset, items.length - 1));
      if (state.align === "end") { start = selected; while (start > 0 && last(start - 1) >= selected) start--; }
      else if (selected < start) start = selected;
      else while (start < selected && last(start) < selected) start++;
      // A short last page pulls earlier items back in rather than leaving rows empty.
      while (start > 0 && last(start - 1) >= Math.max(selected, last(start))) start--;
    }
    const view = reach(blocks, items, start, paged ? rows : Infinity);
    const current = source.current === undefined ? undefined : targetKey({ kind: "agent", key: source.current });
    state.offset = start; state.align = undefined;
    state.first = start; state.last = Math.max(start, view.last);
    state.firstKey = list[state.first]?.key; state.lastKey = list[state.last]?.key;
    const lines: string[] = [], hits = new Map<number, SideHit[]>();
    for (let at = view.from; at < view.to && lines.length < rows; at++) {
      const block = blocks[at];
      if (!block) continue;
      // The sidebar's own selection while it has the focus, and the agent the Agents view selects.
      const chosen = block.item !== undefined && (this.focused && block.item.key === list[selected]?.key || block.item.key === current);
      block.rows.forEach((row, index) => {
        if (lines.length >= rows) return;
        if (block.item) hits.set(lines.length, [{ from: 0, to: cells, target: block.item.target }]);
        lines.push(this.line(row, cells, chosen, chosen && index === 0));
      });
    }
    return { lines, hits, pager: paged ? { first: state.first, last: state.last, count: items.length } : undefined };
  }
  private line(row: Row, cells: number, selected: boolean, bar: boolean): string {
    const inner = Math.max(0, cells - 2), right = row.right ?? "";
    const room = Math.max(0, inner - (right ? visibleWidth(right) + 2 : 0));
    const left = typeof row.left === "function" ? row.left(room) : row.left;
    return paint((bar ? PANEL_BLUE + "▌" : " ") + PANEL_TEXT + splitRow(left, right, inner) + " ", cells, selected ? PANEL_SELECTED : PANEL_BG);
  }

  /** The active tab is bold, underlined and accent, on the selected surface while the sidebar has focus. */
  private label(text: string, active: boolean): string {
    return active ? (this.focused ? PANEL_SELECTED : "") + PANEL_BLUE + BOLD + UNDERLINE + text + NO_UNDERLINE + NORMAL + PANEL_BG
      : PANEL_MUTED + text;
  }
  /** Collapsed (`summary` given): every label muted with its summary. Rail and drawer (`summary` undefined): the active tab marked. */
  private rowSegments(summary: Readonly<Record<SideTab, string>> | undefined, gapCells: number): Segment[] {
    return TABS.flatMap((tab, at) => {
      const value = summary?.[tab] ?? "", tone = value === "!" || value === "?" ? PANEL_WARNING : PANEL_TEXT;
      const text = summary ? PANEL_MUTED + LABELS[tab] + (value ? " " + tone + value : "") : this.label(LABELS[tab], tab === this.tab);
      return [...(at ? [space(gapCells)] : []), seg(text, LABELS[tab] + (value ? " " + value : ""), { tab })];
    });
  }
  /** The narrowest tab row: one letter per tab. */
  private initials(marked: boolean): Segment[] {
    return TABS.flatMap((tab, at) => {
      const letter = LABELS[tab].slice(0, 1);
      return [...(at ? [space(2)] : []), seg(marked ? this.label(letter, tab === this.tab) : PANEL_MUTED + letter, letter, { tab })];
    });
  }
}
