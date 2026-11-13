// F8 agents view: one scrollable page with the agents table (the mock's `.fleet` grid at three densities),
// background jobs when there are any, the selected row's detail with its actions as key caps, and the
// timeline. Selection is keyed ("helper:<request>", "child:<id>", "job:<id>"), so rows inserted above never
// move it. Every row keeps one blank cell each side; only the selected row has a surface of its own.
import { ScrollView, matchesKey, truncateToWidth, visibleWidth, wrapTextWithAnsi, type TuiMouseEvent } from "@earendil-works/pi-tui";
import {
  duration, money, sparkline, timeLabel, toneColor, type AgentRow, type AgentSpan, type AgentsModel, type JobRow, type Status, type Tone,
} from "./agents-model.ts";
import { keyCap, rule, spinner } from "./frame.ts";
import { jobDetailLines, type JobFreeze } from "./job-detail.ts";
import { PANEL_BG, PANEL_BLUE, PANEL_HEAD, PANEL_MUTED, PANEL_RULE, PANEL_SELECTED, PANEL_TEXT, PANEL_WARNING, RESET } from "./panels-view.ts";

export type AgentsAction = "open" | "answer" | "transcript" | "steer" | "cancel" | "model"
  | "output" | "search" | "follow" | "peek" | "background" | "stop";
export interface AgentsViewHost {
  model(): AgentsModel; now(): number; short(): boolean; legend(): boolean;
  freeze(): JobFreeze | undefined; color(hue: number): string;
  click(event: TuiMouseEvent): boolean;       // plain click allowed (workbench plainClick + canInteract)
  focus(): void;
  act(action: AgentsAction, key: string): void;
}
export type AgentsEntry = { readonly kind: "agent"; readonly row: AgentRow } | { readonly kind: "job"; readonly row: JobRow };
export interface AgentsHit { readonly key: string; readonly action?: AgentsAction; readonly from?: number; readonly to?: number }
export interface AgentsOptions {
  readonly now: number; readonly short: boolean; readonly legend: boolean;
  readonly freeze: JobFreeze | undefined; readonly color: (hue: number) => string;
}

/**
 * Table density by main-area columns, measured as the mock does (the area minus four cells of padding): every
 * column from 112 content cells, two-row entries with profile and model from 60, below that name, time and cost only.
 */
export function agentDensity(width: number): "full" | "med" | "min" {
  const content = width - 4;
  return content >= 112 ? "full" : content >= 60 ? "med" : "min";
}

const BOLD = "\x1b[1m", UNBOLD = "\x1b[22m", GAP = "  ";
/** `.fleet` column widths in cells; columns are two cells apart. */
const NAME = 11, PROFILE = 27, TIME = 9, COST = 6, CTX = 4, FILES = 5, PULSE_FULL = 14, PULSE_MED = 12, JOB_ID = 14, JOB_STATE = 20;
/** Bar, gap, state glyph, gap: the name column starts this many cells into a row. */
const LEAD = 6;
/** One timeline column per fleet history bucket until the bars fill their room. */
const TICK = 5000;
const AGENT_ACTIONS = ["open", "answer", "steer", "cancel", "model", "transcript"] as const;
const AGENT_KEYS: Readonly<Record<(typeof AGENT_ACTIONS)[number], readonly [string, string]>> = {
  open: ["⏎", "open conversation"], answer: ["a", "answer"], steer: ["s", "steer"], cancel: ["x", "cancel"], model: ["m", "model"],
  transcript: ["t", "transcript"],
};
const JOB_ACTIONS = ["output", "search", "follow", "peek", "background", "stop"] as const;

/** Styled text clipped to exactly `width` cells; resets inside the clip restore `base` so the row keeps its surface. */
function cell(text: string, width: number, base: string, right = false): string {
  if (width <= 0) return "";
  const clipped = truncateToWidth(text, width, "…"), room = " ".repeat(Math.max(0, width - visibleWidth(clipped)));
  const kept = clipped.replaceAll(RESET, RESET + base);
  return right ? room + kept : kept + room;
}
/** `text` on the canvas, exactly `width` cells. */
function fill(text: string, width: number): string {
  const base = PANEL_BG + PANEL_TEXT;
  return base + cell(text, width, base) + RESET;
}
/** One page row: a blank cell each side and `text` fitted to the cells between, on `surface`. */
function row(text: string, width: number, surface = PANEL_BG): string {
  if (width <= 2) return PANEL_BG + " ".repeat(Math.max(0, width)) + RESET;
  const base = surface + PANEL_TEXT;
  return PANEL_BG + " " + base + cell(text, width - 2, base) + PANEL_BG + " " + RESET;
}
function entriesOf(model: AgentsModel): AgentsEntry[] {
  return [...model.agents.map(row => ({ kind: "agent" as const, row })), ...model.jobs.map(row => ({ kind: "job" as const, row }))];
}
function shownEntry(model: AgentsModel, selected: string | undefined): AgentsEntry | undefined {
  const entries = entriesOf(model);
  return selected === undefined ? entries[0] : entries.find(entry => entry.row.key === selected);
}
function timeTone(tone: Tone): Tone { return tone === "accent" ? "text" : tone; }
/** The state cell: the spinner while running (staggered by `phase`), otherwise the state glyph in its tone. */
function glyph(status: Status, running: boolean, now: number, phase: number): string {
  return running ? PANEL_BLUE + spinner(now, phase) : toneColor(status.tone) + status.glyph;
}
/** The activity sparkline, `cells` wide in the agent's hue, while it runs or asks. */
function pulse(entry: AgentRow, cells: number, color: string): string {
  if (!entry.pulse?.length || (entry.state !== "running" && entry.state !== "needs")) return "";
  return color + sparkline(entry.pulse, cells);
}

/** The cells a table row spends on its grid: the page minus its two blank cells and the row's right inset. */
function gridOf(width: number): number { return width - 3; }

function agentLines(entry: AgentRow, phase: number, selected: boolean, density: "full" | "med" | "min", width: number,
  color: (hue: number) => string, now: number): string[] {
  const surface = selected ? PANEL_SELECTED : PANEL_BG, base = surface + PANEL_TEXT, grid = gridOf(width);
  const hue = color(entry.hue), name = BOLD + hue + entry.name + UNBOLD;
  const lead = (selected ? PANEL_BLUE + "▌" : " ") + GAP + glyph(entry.status, entry.state === "running", now, phase) + GAP;
  const time = cell(toneColor(timeTone(entry.status.tone)) + timeLabel(entry.status), TIME, base, true);
  const cost = cell(PANEL_TEXT + money(entry.usage?.cost), COST, base, true);
  const profile = PANEL_MUTED + entry.profile + " · " + entry.model + (entry.pendingModel ? " → " + entry.pendingModel : "");
  const activity = PANEL_MUTED + entry.activity;
  if (density === "full") {
    const ctx = cell(PANEL_TEXT + (entry.usage?.contextPercent === undefined ? "–" : `${Math.round(entry.usage.contextPercent)}%`), CTX, base, true);
    const files = cell((entry.outside ? PANEL_WARNING + "!" : "") + PANEL_TEXT + (entry.files === undefined ? "–" : String(entry.files)), FILES, base, true);
    const rest = grid - (LEAD + NAME + PROFILE + TIME + COST + CTX + FILES + PULSE_FULL + GAP.length * 7);
    return [row(lead + cell(name, NAME, base) + GAP + cell(profile, PROFILE, base) + GAP + time + GAP + cost + GAP + ctx + GAP + files
      + GAP + cell(pulse(entry, PULSE_FULL, hue), PULSE_FULL, base) + GAP + cell(activity, rest, base), width, surface)];
  }
  const first = density === "med"
    ? lead + cell(name, NAME, base) + GAP + cell(profile, grid - (LEAD + NAME + TIME + COST + GAP.length * 3), base) + GAP + time + GAP + cost
    : lead + cell(name, grid - (LEAD + TIME + COST + GAP.length * 2), base) + GAP + time + GAP + cost;
  const beat = density === "med" ? pulse(entry, PULSE_MED, hue) : "";
  const second = " ".repeat(LEAD) + cell(activity, grid - LEAD - (beat ? PULSE_MED + GAP.length : 0), base) + (beat ? GAP + beat : "");
  return [row(first, width, surface), row(second, width, surface)];
}
function jobLine(entry: JobRow, phase: number, selected: boolean, density: "full" | "med" | "min", width: number, now: number): string {
  const surface = selected ? PANEL_SELECTED : PANEL_BG, base = surface + PANEL_TEXT, grid = gridOf(width);
  const lead = (selected ? PANEL_BLUE + "▌" : " ") + GAP + glyph(entry.status, entry.live, now, phase) + GAP;
  const state = entry.status.time ? `${entry.status.word} ${entry.status.time}` : entry.status.word;
  const stateWidth = Math.min(JOB_STATE, visibleWidth(state)), idWidth = density === "min" ? 0 : Math.min(JOB_ID, visibleWidth(entry.job));
  const command = grid - LEAD - stateWidth - GAP.length - (idWidth ? idWidth + GAP.length : 0);
  return row(lead + cell(PANEL_TEXT + entry.command, command, base) + (idWidth ? GAP + cell(PANEL_MUTED + entry.job, idWidth, base) : "")
    + GAP + cell(toneColor(timeTone(entry.status.tone)) + state, stateWidth, base, true), width, surface);
}
function headerLine(width: number): string {
  const base = PANEL_BG + PANEL_MUTED, rest = gridOf(width) - (LEAD + NAME + PROFILE + TIME + COST + CTX + FILES + PULSE_FULL + GAP.length * 7);
  return row(PANEL_MUTED + " ".repeat(LEAD) + cell("agent", NAME, base) + GAP + cell("profile · model", PROFILE, base) + GAP + cell("time", TIME, base, true)
    + GAP + cell("cost", COST, base, true) + GAP + cell("ctx", CTX, base, true) + GAP + cell("files", FILES, base, true)
    + GAP + cell("activity", PULSE_FULL, base) + GAP + cell("now", rest, base), width);
}

interface Action { readonly action: AgentsAction; readonly key: string; readonly label: string; readonly enabled: boolean }
/**
 * The actions as key caps, `[s] steer`, two cells apart and wrapped to `width`; an unavailable one is its muted label
 * without a cap. Returns the rows and, per row, the hit range of each available action (cap and label).
 */
function actionRows(key: string, actions: readonly Action[], width: number): { readonly lines: string[]; readonly hits: AgentsHit[][] } {
  const room = width - 2, lines: string[] = [], hits: AgentsHit[][] = [];
  let text = "", used = 0, rowHits: AgentsHit[] = [];
  for (const item of actions) {
    const piece = item.enabled ? keyCap(item.key) + " " + PANEL_TEXT + item.label : PANEL_MUTED + item.label;
    const size = visibleWidth(piece);
    if (used > 0 && used + GAP.length + size > room) { lines.push(row(text, width)); hits.push(rowHits); text = ""; used = 0; rowHits = []; }
    const at = used > 0 ? used + GAP.length : 0;
    text += (used > 0 ? GAP : "") + piece;
    if (item.enabled) rowHits.push({ key, action: item.action, from: 1 + at, to: 1 + at + size });
    used = at + size;
  }
  if (text) { lines.push(row(text, width)); hits.push(rowHits); }
  return { lines, hits };
}
function agentActions(entry: AgentRow): Action[] {
  return AGENT_ACTIONS.flatMap((action): Action[] => {
    if (entry.capabilities[action] !== true) return [];
    const [key, label] = AGENT_KEYS[action];
    return [{ action, key, label, enabled: entry.disabled?.[action] === undefined }];
  });
}
function jobActions(entry: JobRow, freeze: JobFreeze | undefined): Action[] {
  const frozen = freeze?.job === entry.job;
  const keys: Readonly<Record<(typeof JOB_ACTIONS)[number], readonly [string, string]>> = {
    output: ["⏎", "output"], search: ["/", "search output"], follow: ["f", frozen ? "resume follow" : "pause follow"], peek: ["p", "peek"],
    background: ["b", "background"], stop: ["x", "stop"],
  };
  return JOB_ACTIONS.filter(action => entry.capabilities[action] || action === "follow" && frozen)
    .map(action => ({ action, key: keys[action][0], label: keys[action][1], enabled: true }));
}
/** The actions the shown entry offers right now; disabled and absent capabilities are excluded. */
function offered(entry: AgentsEntry | undefined, freeze: JobFreeze | undefined): AgentsAction[] {
  if (!entry) return [];
  return (entry.kind === "agent" ? agentActions(entry.row) : jobActions(entry.row, freeze)).filter(action => action.enabled).map(action => action.action);
}
function wrapped(text: string, color: string, width: number, cap: number): string[] {
  const room = Math.max(1, width - 2), rows = wrapTextWithAnsi(text, room), shown = rows.slice(0, cap);
  if (rows.length > cap && shown.length) shown[shown.length - 1] = truncateToWidth(shown[shown.length - 1] ?? "", Math.max(1, room - 1), "") + "…";
  return shown.map(line => row(color + line, width));
}
/**
 * The selected agent's detail under its title: what a build child owns and starts after, the owner's steers, its
 * open question and its notes (pending model, edits outside its files, errors). A queued child's wait already
 * reads in its `now` cell. Short terminals keep only the question, the one thing the owner must act on.
 */
function agentDetail(entry: AgentRow, width: number, short: boolean): string[] {
  const asks = entry.question === undefined ? [] : wrapped("▲ Asks: " + entry.question, PANEL_WARNING, width, short ? 2 : 4);
  if (short) return asks;
  const lines: string[] = [];
  if (entry.source === "child") {
    const after = entry.after?.length ? " · starts after " + entry.after.join(", ") : "";
    lines.push(...wrapped("Owns: " + (entry.owns?.length ? entry.owns.join(", ") : "nothing (read-only)") + after, PANEL_MUTED, width, 3));
  }
  if (entry.steers?.length) lines.push(...wrapped("Steered: " + entry.steers.join(" · "), PANEL_BLUE, width, 3));
  lines.push(...asks);
  for (const note of entry.notes) if (note.tone !== "muted") lines.push(...wrapped(note.text, toneColor(note.tone), width, 4));
  return lines;
}

/**
 * Timeline bars, one row per agent: `┄` waiting, `━` running in the agent's hue, `━` in warning tone while
 * asking, then `✓`, `✗` or `!` where it ended. One column per 5 s of the session until the bars fill their room
 * (at most 96 cells), and the last row is the axis from `0s` to the elapsed time under the bars only.
 * `names` false draws a bare `●` per row for narrow panes.
 */
export function timelineLines(rows: readonly AgentRow[], width: number, now: number, origin: number,
  names: boolean, color: (hue: number) => string): string[] {
  const end = Math.max(now, ...rows.flatMap(entry => [entry.end?.at ?? origin, ...entry.spans.map(span => span.to ?? origin)]));
  const elapsed = Math.max(0, end - origin), room = Math.max(10, Math.min(96, width - (names ? 14 : 4)));
  const cols = Math.max(1, Math.min(room, Math.ceil(elapsed / TICK))), step = Math.max(1, elapsed) / cols;
  const column = (time: number) => Math.max(0, Math.min(cols - 1, Math.floor((time - origin) / step)));
  const lead = (entry: AgentRow) => " " + (names ? cell(color(entry.hue) + entry.name, NAME, PANEL_BG + PANEL_TEXT) : color(entry.hue) + "●") + " ";
  const result = rows.map(entry => {
    const kinds: (AgentSpan["kind"] | "end" | undefined)[] = Array.from({ length: cols }, () => undefined);
    for (const kind of ["wait", "run", "ask"] as const) {
      for (const span of entry.spans.filter(item => item.kind === kind)) {
        const first = column(span.from), last = span.to === undefined ? column(now) : Math.max(first, Math.ceil((span.to - origin) / step) - 1);
        for (let index = first; index <= Math.min(cols - 1, last); index++) kinds[index] = kind;
      }
    }
    if (entry.end) { const mark = column(entry.end.at); kinds[mark] = "end"; kinds.fill(undefined, mark + 1); }
    const sign = entry.end?.outcome === "done" ? "✓" : entry.end?.outcome === "uncertain" ? "!" : "✗";
    const tone: Tone = entry.end?.outcome === "done" ? "success" : entry.end?.outcome === "uncertain" ? "warning" : "error";
    let bar = "";
    kinds.forEach(kind => {
      bar += kind === "run" ? color(entry.hue) + "━" : kind === "ask" ? PANEL_WARNING + "━" : kind === "wait" ? PANEL_RULE + "┄"
        : kind === "end" ? toneColor(tone) + sign : " ";
    });
    return truncateToWidth(lead(entry) + bar + PANEL_TEXT, width, "");
  });
  const label = duration(elapsed), gutter = names ? NAME + 1 : 2;
  const axis = "0s".padEnd(Math.max(3, cols - label.length)) + (cols > label.length + 3 ? label : "");
  return [...result, truncateToWidth(" " + " ".repeat(gutter) + PANEL_MUTED + axis + PANEL_TEXT, width, "")];
}
/** The whole agents page for `width` columns; `hits[row]` lists the click targets on that rendered row. */
export function renderAgents(model: AgentsModel, selected: string | undefined, width: number, options: AgentsOptions):
  { readonly lines: string[]; readonly hits: ReadonlyMap<number, AgentsHit[]>; readonly selectedRow: number } {
  const density = agentDensity(width), lines: string[] = [], hits = new Map<number, AgentsHit[]>();
  const push = (line: string, rowHits?: readonly AgentsHit[]) => { if (rowHits?.length) hits.set(lines.length, [...rowHits]); lines.push(line); };
  const divider = () => push(row(rule(width - 2), width));
  const heading = (title: string, note: string) => push(row(BOLD + PANEL_HEAD + title + UNBOLD + PANEL_MUTED + note, width));
  const entry = shownEntry(model, selected);
  let selectedRow = -1;

  if (!model.bound && (model.jobs.length > 0 || model.agents.some(agent => agent.source === "helper"))) {
    push(row(PANEL_WARNING + "! Host unbound · these observations may be stale", width));
  }
  if (!model.agents.length && !model.jobs.length) {
    lines.push(...wrapped(BOLD + PANEL_TEXT + "No agents yet." + UNBOLD + PANEL_MUTED + " Agents and background jobs appear here when the session starts them.", "", width, 3));
  }

  if (model.agents.length && density === "full") push(headerLine(width));
  model.agents.forEach((agent, index) => {
    const on = entry?.row.key === agent.key;
    if (on) selectedRow = lines.length;
    for (const line of agentLines(agent, index, on, density, width, options.color, options.now)) push(line, [{ key: agent.key }]);
  });
  if (model.jobs.length) {
    heading("Jobs", ` · ${model.jobs.filter(job => job.live).length} running`);
    model.jobs.forEach((job, index) => {
      const on = entry?.row.key === job.key;
      if (on) selectedRow = lines.length;
      push(jobLine(job, model.agents.length + index, on, density, width, options.now), [{ key: job.key }]);
    });
  }

  if (selected !== undefined && !entry) {
    divider();
    push(row(PANEL_WARNING + `The selected ${selected.startsWith("job:") ? "job" : "agent"} is no longer retained`, width));
  } else if (entry?.kind === "agent") {
    const shown = entry.row;
    divider();
    push(row(BOLD + options.color(shown.hue) + shown.name + UNBOLD + " " + PANEL_TEXT + shown.title, width));
    lines.push(...agentDetail(shown, width, options.short));
    const actions = actionRows(shown.key, agentActions(shown), width);
    actions.lines.forEach((line, index) => push(line, actions.hits[index]));
  } else if (entry?.kind === "job") {
    const shown = entry.row;
    divider();
    const height = options.short ? 10 : 22;
    for (const line of jobDetailLines(shown.source, { bound: model.bound, now: options.now, legend: options.legend, freeze: options.freeze }, Math.max(4, width - 2), height)) {
      push(row(line, width));
    }
    const actions = actionRows(shown.key, jobActions(shown, options.freeze), width);
    actions.lines.forEach((line, index) => push(line, actions.hits[index]));
  }

  if (model.agents.length) {
    divider();
    heading("Timeline", model.maxParallel === undefined ? "" : ` · up to ${model.maxParallel} at once`);
    const bars = timelineLines(model.agents, width, options.now, model.origin, density !== "min", options.color);
    bars.forEach((line, index) => {
      const agent = model.agents[index];
      push(fill(line, width), agent ? [agent.capabilities.open ? { key: agent.key, action: "open" } : { key: agent.key }] : undefined);
    });
  }
  return { lines, hits, selectedRow };
}

/** The F8 page as a scrollable component with keyed selection, keys and pointer targets. */
export class AgentsView {
  selected: string | undefined;
  readonly scroll: ScrollView;
  private hits: ReadonlyMap<number, AgentsHit[]> = new Map();
  private reveal = false;
  constructor(private readonly host: AgentsViewHost) {
    this.scroll = new ScrollView({
      render: width => this.render(width),
      handleMouse: event => this.mouse(event),
      invalidate() {},
    }, { primary: true, follow: "none", scrollbar: "auto" });
  }
  /** The shown selection; the first entry when unset, none when the chosen key is no longer retained. */
  entry(): AgentsEntry | undefined { return shownEntry(this.host.model(), this.selected); }
  /** Agents then jobs, by key. */
  move(delta: number): void {
    const entries = entriesOf(this.host.model()), shown = this.entry();
    const index = shown ? entries.findIndex(entry => entry.row.key === shown.row.key) + delta : 0;
    const next = entries[Math.max(0, Math.min(entries.length - 1, index))];
    if (next) this.select(next.row.key);
  }
  select(key: string): void { this.selected = key; this.reveal = true; }
  handleKey(data: string): boolean {
    if (matchesKey(data, "up") || data === "k") { this.move(-1); return true; }
    if (matchesKey(data, "down") || data === "j") { this.move(1); return true; }
    if (matchesKey(data, "pageUp") || matchesKey(data, "pageDown")) {
      const page = Math.max(1, this.scroll.viewportHeight - 2);
      this.scroll.scrollBy(matchesKey(data, "pageUp") ? -page : page); return true;
    }
    const entry = this.entry();
    if (!entry) return false;
    const enter = matchesKey(data, "enter");
    const action: AgentsAction | undefined = entry.kind === "agent"
      ? enter ? "open" : data === "a" ? "answer" : data === "t" ? "transcript" : data === "s" ? "steer" : data === "x" ? "cancel" : data === "m" ? "model" : undefined
      : enter ? "output" : data === "/" ? "search" : data === "f" ? "follow" : data === "p" ? "peek" : data === "b" ? "background" : data === "x" ? "stop" : undefined;
    if (!action) return false;
    if (this.actions().includes(action)) { this.host.act(action, entry.row.key); return true; }
    // A present but disabled capability is shown dim with its reason; its key does nothing else.
    const reasons: Readonly<Record<string, string | undefined>> = entry.kind === "agent" ? entry.row.disabled ?? {} : {};
    return reasons[action] !== undefined;
  }
  /** Offered for the shown entry, for the footer and the F1 list. */
  actions(): readonly AgentsAction[] { return offered(this.entry(), this.host.freeze()); }

  private render(width: number): string[] {
    const frame = renderAgents(this.host.model(), this.selected, width, {
      now: this.host.now(), short: this.host.short(), legend: this.host.legend(), freeze: this.host.freeze(), color: hue => this.host.color(hue),
    });
    this.hits = frame.hits;
    const height = this.scroll.viewportHeight, top = this.scroll.scrollTop, row = frame.selectedRow;
    if (this.reveal && row >= 0 && height > 0) {
      this.reveal = false;
      const target = row < top ? row : row + 2 > top + height ? row + 2 - height : top;
      if (target !== top) {
        this.scroll.updateLayout(frame.lines.length, height, () => {});
        this.scroll.scrollTo(target, { disableFollow: true });
      }
    }
    return frame.lines;
  }
  private mouse(event: TuiMouseEvent): { readonly handled: boolean } | undefined {
    if (!this.host.click(event)) return undefined;
    this.host.focus();
    const rowHits = this.hits.get(event.y) ?? [];
    const hit = rowHits.find(item => item.from !== undefined && item.to !== undefined && event.x >= item.from && event.x < item.to)
      ?? rowHits.find(item => item.from === undefined);
    if (!hit) return { handled: true };
    const shown = this.entry()?.row.key;
    if (hit.key !== shown) {
      this.select(hit.key);
      if (hit.action === undefined) return { handled: true };
    }
    const action = hit.action ?? (this.entry()?.kind === "job" ? "output" : "open");
    if (this.actions().includes(action)) this.host.act(action, hit.key);
    return { handled: true };
  }
}
