// F8 agents view: one scrollable page of the agents table, background jobs, the selected row's detail and a
// timeline. Selection is keyed ("helper:<request>", "job:<id>"), so rows inserted above never move it. Keys
// live in the footer; the page itself only offers clickable action labels.
import { ScrollView, matchesKey, truncateToWidth, visibleWidth, wrapTextWithAnsi, type TuiMouseEvent } from "@earendil-works/pi-tui";
import {
  duration, money, statusLabel, toneColor, type AgentRow, type AgentSpan, type AgentsModel, type JobRow, type Tone,
} from "./agents-model.ts";
import { jobDetailLines, type JobFreeze } from "./job-detail.ts";
import { PANEL_BG, PANEL_BLUE, PANEL_MUTED, PANEL_RULE, PANEL_SELECTED, PANEL_TEXT, PANEL_WARNING, RESET } from "./panels-view.ts";

export type AgentsAction = "open" | "transcript" | "steer" | "cancel" | "model"
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

/** Table density by main-area columns: every column at 112+, two-row entries from 60, the fewest below. */
export function agentDensity(width: number): "full" | "med" | "min" {
  return width >= 112 ? "full" : width >= 60 ? "med" : "min";
}

const BOLD = "\x1b[1m", UNBOLD = "\x1b[22m";
const NAME = 11, TIME = 11, COST = 6, CTX = 4, FILES = 5, PULSE_FULL = 14, PULSE_MED = 12, PROFILE = 27, GAP = "  ";
/** Cells before the activity column at full density: bar, glyph and space, seven fixed columns, their gaps, a right inset. */
const FULL_FIXED = 3 + NAME + PROFILE + TIME + COST + CTX + FILES + PULSE_FULL + GAP.length * 7 + 1;
const BARS = " ▁▂▃▄▅▆▇";
const AGENT_ACTIONS = ["open", "transcript", "steer", "cancel", "model"] as const;
const AGENT_LABELS: Readonly<Record<(typeof AGENT_ACTIONS)[number], string>> = {
  open: "open conversation", transcript: "transcript", steer: "steer", cancel: "cancel", model: "model",
};
const JOB_ACTIONS = ["output", "search", "follow", "peek", "background", "stop"] as const;

/** Styled text clipped to exactly `width` cells; resets inside the clip restore `base` so the row keeps its surface. */
function cell(text: string, width: number, base: string, right = false): string {
  if (width <= 0) return "";
  const clipped = truncateToWidth(text, width, "…"), room = " ".repeat(Math.max(0, width - visibleWidth(clipped)));
  const kept = clipped.replaceAll(RESET, RESET + base);
  return right ? room + kept : kept + room;
}
function fill(text: string, width: number, surface = PANEL_BG): string {
  const base = surface + PANEL_TEXT;
  return base + cell(text, width, base) + RESET;
}
function entriesOf(model: AgentsModel): AgentsEntry[] {
  return [...model.agents.map(row => ({ kind: "agent" as const, row })), ...model.jobs.map(row => ({ kind: "job" as const, row }))];
}
function shownEntry(model: AgentsModel, selected: string | undefined): AgentsEntry | undefined {
  const entries = entriesOf(model);
  return selected === undefined ? entries[0] : entries.find(entry => entry.row.key === selected);
}
function timeTone(tone: Tone): Tone { return tone === "accent" ? "text" : tone; }
function pulse(row: AgentRow, cells: number, color: string): string {
  if (!row.pulse?.length || (row.state !== "running" && row.state !== "needs")) return "";
  const bars = row.pulse.slice(-cells).map(value => BARS[Math.max(0, Math.min(7, Math.round(value)))] ?? " ").join("");
  return color + bars.padStart(cells, " ");
}
function activityColor(row: AgentRow): string {
  return row.status.tone === "error" || row.status.tone === "warning" ? toneColor(row.status.tone) : PANEL_MUTED;
}

function agentLines(row: AgentRow, selected: boolean, density: "full" | "med" | "min", width: number, color: (hue: number) => string): string[] {
  const surface = selected ? PANEL_SELECTED : PANEL_BG, base = surface + PANEL_TEXT;
  const lead = (selected ? PANEL_BLUE + "▌" : " ") + toneColor(row.status.tone) + row.status.glyph + " ";
  const hue = color(row.hue), name = BOLD + hue + row.name + UNBOLD;
  const time = cell(toneColor(timeTone(row.status.tone)) + statusLabel(row.status), TIME, base, true);
  const cost = cell(PANEL_TEXT + money(row.usage?.cost), COST, base, true);
  const profile = PANEL_MUTED + row.profile + " · " + row.model + (row.pendingModel ? " → " + row.pendingModel : "");
  if (density === "full") {
    const ctx = cell(PANEL_TEXT + (row.usage?.contextPercent === undefined ? "–" : `${Math.round(row.usage.contextPercent)}%`), CTX, base, true);
    const files = cell((row.outside ? PANEL_WARNING + "!" : "") + PANEL_TEXT + (row.files === undefined ? "–" : String(row.files)), FILES, base, true);
    const text = lead + cell(name, NAME, base) + GAP + cell(profile, PROFILE, base) + GAP + time + GAP + cost + GAP + ctx + GAP + files
      + GAP + cell(pulse(row, PULSE_FULL, hue), PULSE_FULL, base) + GAP + cell(activityColor(row) + row.activity, width - FULL_FIXED, base);
    return [fill(text, width, surface)];
  }
  const flex = density === "med" ? width - (3 + NAME + TIME + COST + GAP.length * 3 + 1) : width - (3 + TIME + COST + GAP.length * 2 + 1);
  const first = density === "med"
    ? lead + cell(name, NAME, base) + GAP + cell(profile, flex, base) + GAP + time + GAP + cost
    : lead + cell(name, flex, base) + GAP + time + GAP + cost;
  const beat = density === "med" ? pulse(row, PULSE_MED, hue) : "";
  const room = width - 4 - (beat ? PULSE_MED + GAP.length : 0);
  const second = "   " + cell(activityColor(row) + row.activity, room, base) + (beat ? GAP + cell(beat, PULSE_MED, base) : "");
  return [fill(first, width, surface), fill(second, width, surface)];
}
function jobLine(row: JobRow, selected: boolean, density: "full" | "med" | "min", width: number): string {
  const surface = selected ? PANEL_SELECTED : PANEL_BG, base = surface + PANEL_TEXT;
  const lead = (selected ? PANEL_BLUE + "▌" : " ") + toneColor(row.status.tone) + row.status.glyph + " ";
  const state = row.status.time ? `${row.status.word} ${row.status.time}` : row.status.word;
  const stateWidth = Math.min(20, visibleWidth(state)), id = density === "min" ? "" : row.job;
  const idWidth = id ? Math.min(14, visibleWidth(id)) : 0;
  const flex = width - 3 - stateWidth - GAP.length - (idWidth ? idWidth + GAP.length : 0) - 1;
  return fill(lead + cell(PANEL_TEXT + row.command, flex, base) + (idWidth ? GAP + cell(PANEL_MUTED + id, idWidth, base) : "")
    + GAP + cell(toneColor(timeTone(row.status.tone)) + state, stateWidth, base, true), width, surface);
}
function headerLine(width: number): string {
  const base = PANEL_BG + PANEL_MUTED;
  return fill(PANEL_MUTED + "   " + cell("agent", NAME, base) + GAP + cell("profile · model", PROFILE, base) + GAP + cell("time", TIME, base, true)
    + GAP + cell("cost", COST, base, true) + GAP + cell("ctx", CTX, base, true) + GAP + cell("files", FILES, base, true)
    + GAP + cell("activity", PULSE_FULL, base) + GAP + cell("now", width - FULL_FIXED, base), width);
}

interface Action { readonly action: AgentsAction; readonly label: string; readonly enabled: boolean; readonly reason?: string }
/** Clickable action labels, wrapped to `width`; returns the rows and the hit ranges of each row. */
function actionRows(key: string, actions: readonly Action[], width: number): { readonly lines: string[]; readonly hits: AgentsHit[][] } {
  const lines: string[] = [], hits: AgentsHit[][] = [];
  let text = " ", used = 1, row: AgentsHit[] = [];
  const reasons = [...new Set(actions.flatMap(action => action.enabled || !action.reason ? [] : [action.reason]))];
  const items = [...actions.map(action => ({ text: action.label, color: action.enabled ? PANEL_BLUE : PANEL_MUTED, action })),
    ...(reasons.length ? [{ text: "· " + reasons.join(", "), color: PANEL_MUTED, action: undefined }] : [])];
  for (const item of items) {
    const size = visibleWidth(item.text), gap = used > 1 ? 3 : 0;
    if (used > 1 && used + gap + size > width) { lines.push(fill(text, width)); hits.push(row); text = " "; used = 1; row = []; }
    const from = used + (used > 1 ? 3 : 0);
    text += (used > 1 ? "   " : "") + item.color + item.text;
    if (item.action?.enabled) row.push({ key, action: item.action.action, from, to: from + size });
    used = from + size;
  }
  lines.push(fill(text, width)); hits.push(row);
  return { lines, hits };
}
function agentActions(row: AgentRow): Action[] {
  return AGENT_ACTIONS.flatMap((action): Action[] => {
    if (row.capabilities[action] !== true) return [];
    const reason = row.disabled?.[action];
    return [reason === undefined ? { action, label: AGENT_LABELS[action], enabled: true } : { action, label: AGENT_LABELS[action], enabled: false, reason }];
  });
}
function jobActions(row: JobRow, freeze: JobFreeze | undefined): Action[] {
  const frozen = freeze?.job === row.job;
  const labels: Readonly<Record<(typeof JOB_ACTIONS)[number], string>> = {
    output: "output", search: "search output", follow: frozen ? "resume follow" : "pause follow", peek: "peek", background: "background", stop: "stop",
  };
  return JOB_ACTIONS.filter(action => row.capabilities[action] || action === "follow" && frozen)
    .map(action => ({ action, label: labels[action], enabled: true }));
}
/** The actions the shown entry offers right now; disabled and absent capabilities are excluded. */
function offered(entry: AgentsEntry | undefined, freeze: JobFreeze | undefined): AgentsAction[] {
  if (!entry) return [];
  return (entry.kind === "agent" ? agentActions(entry.row) : jobActions(entry.row, freeze)).filter(action => action.enabled).map(action => action.action);
}
function wrapped(text: string, color: string, width: number, cap: number): string[] {
  const rows = wrapTextWithAnsi(text, Math.max(1, width - 1));
  const shown = rows.slice(0, cap);
  if (rows.length > cap && shown.length) shown[shown.length - 1] = truncateToWidth(shown[shown.length - 1] ?? "", Math.max(1, width - 2), "") + "…";
  return shown.map(line => fill(" " + color + line, width));
}

/**
 * Timeline bars, one row per agent: `┄` waiting, `━` running in the agent's hue, `━` in warning tone while
 * asking, then `✓`, `✗` or `!` where it ended. The last row is the axis from `0s` to the elapsed time.
 * `names` false draws a bare `●` per row for narrow panes.
 */
export function timelineLines(rows: readonly AgentRow[], width: number, now: number, origin: number,
  names: boolean, color: (hue: number) => string): string[] {
  const cols = names ? Math.max(10, Math.min(96, width - 14)) : Math.max(1, width - 3);
  const end = Math.max(now, ...rows.flatMap(row => [row.end?.at ?? origin, ...row.spans.map(span => span.to ?? origin)]));
  const total = Math.max(1000, end - origin), step = total / cols;
  const column = (time: number) => Math.max(0, Math.min(cols - 1, Math.floor((time - origin) / step)));
  const lead = (row: AgentRow) => " " + (names ? cell(color(row.hue) + row.name, NAME, PANEL_TEXT) : color(row.hue) + "●") + " ";
  const result = rows.map(row => {
    const kinds: (AgentSpan["kind"] | "end" | undefined)[] = Array.from({ length: cols }, () => undefined);
    for (const kind of ["wait", "run", "ask"] as const) {
      for (const span of row.spans.filter(item => item.kind === kind)) {
        const first = column(span.from), last = span.to === undefined ? column(now) : Math.max(first, Math.ceil((span.to - origin) / step) - 1);
        for (let index = first; index <= Math.min(cols - 1, last); index++) kinds[index] = kind;
      }
    }
    let mark = -1;
    if (row.end) { mark = column(row.end.at); kinds[mark] = "end"; kinds.fill(undefined, mark + 1); }
    const glyph = row.end?.outcome === "done" ? "✓" : row.end?.outcome === "uncertain" ? "!" : "✗";
    const tone: Tone = row.end?.outcome === "done" ? "success" : row.end?.outcome === "uncertain" ? "warning" : "error";
    let bar = "";
    kinds.forEach(kind => {
      bar += kind === "run" ? color(row.hue) + "━" : kind === "ask" ? PANEL_WARNING + "━" : kind === "wait" ? PANEL_RULE + "┄"
        : kind === "end" ? toneColor(tone) + glyph : " ";
    });
    return truncateToWidth(lead(row) + bar + PANEL_TEXT, width, "");
  });
  const label = duration(end - origin), gutter = names ? NAME + 1 : 2;
  const axis = "0s".padEnd(Math.max(3, cols - label.length)) + (cols > label.length + 3 ? label : "");
  return [...result, truncateToWidth(" " + " ".repeat(gutter) + PANEL_MUTED + axis + PANEL_TEXT, width, "")];
}
/** The whole agents page for `width` columns; `hits[row]` lists the click targets on that rendered row. */
export function renderAgents(model: AgentsModel, selected: string | undefined, width: number, options: AgentsOptions):
  { readonly lines: string[]; readonly hits: ReadonlyMap<number, AgentsHit[]>; readonly selectedRow: number } {
  const density = agentDensity(width), lines: string[] = [], hits = new Map<number, AgentsHit[]>();
  const push = (line: string, rowHits?: readonly AgentsHit[]) => { if (rowHits?.length) hits.set(lines.length, [...rowHits]); lines.push(line); };
  const muted = (text: string) => push(fill(" " + PANEL_MUTED + text, width));
  const rule = () => push(fill(PANEL_RULE + "─".repeat(width), width));
  const entry = shownEntry(model, selected);
  let selectedRow = -1;

  muted(density === "min" ? "host observations" : "This host's helper calls and jobs · observations, not task settlement");
  if (!model.bound) push(fill(" " + PANEL_WARNING + "! Host unbound · these observations may be stale", width));

  if (model.agents.length && density === "full") push(headerLine(width));
  for (const row of model.agents) {
    const isSelected = entry?.row.key === row.key;
    if (isSelected) selectedRow = lines.length;
    for (const line of agentLines(row, isSelected, density, width, options.color)) push(line, [{ key: row.key }]);
  }
  if (!model.sources.helpers) muted("Helper calls unavailable from this host · unknown, not none");
  else if (!model.agents.length) muted(model.jobs.length || !model.sources.jobs ? "No helper calls yet" : "No helper calls or background jobs yet · they appear here when the agent starts them");

  if (!model.sources.jobs) muted("Background jobs unavailable from this host · unknown, not none");
  else if (model.jobs.length) {
    push(fill(" " + BOLD + PANEL_TEXT + "Jobs" + UNBOLD + PANEL_MUTED + ` · ${model.jobs.filter(job => job.live).length} running`, width));
    for (const row of model.jobs) {
      const isSelected = entry?.row.key === row.key;
      if (isSelected) selectedRow = lines.length;
      push(jobLine(row, isSelected, density, width), [{ key: row.key }]);
    }
  }

  if (selected !== undefined && !entry) {
    rule();
    push(fill(" " + PANEL_WARNING + `The selected ${selected.startsWith("job:") ? "job" : "agent"} is no longer retained`, width));
  } else if (entry?.kind === "agent") {
    const row = entry.row;
    rule();
    push(fill(" " + BOLD + options.color(row.hue) + row.name + UNBOLD + PANEL_TEXT + "  " + row.title, width));
    if (!options.short) {
      if (row.meta.length) lines.push(...wrapped(row.meta.join(" · "), PANEL_MUTED, width, 3));
      if (row.pendingModel) push(fill(" " + PANEL_BLUE + `Next request uses ${row.pendingModel}`, width));
      if (row.owns !== undefined || row.after?.length) {
        const owns = row.owns?.length ? row.owns.join(", ") : "nothing (read-only)";
        lines.push(...wrapped(`Owns: ${owns}${row.after?.length ? " · starts after " + row.after.join(", ") : ""}`, PANEL_MUTED, width, 3));
      }
      if (row.steers?.length) lines.push(...wrapped("Steered: " + row.steers.join(" · "), PANEL_BLUE, width, 3));
      if (row.question) lines.push(...wrapped("▲ " + row.question, PANEL_WARNING, width, 3));
      for (const note of row.notes) lines.push(...wrapped(note.text, toneColor(note.tone), width, 4));
    }
    const actions = actionRows(row.key, agentActions(row), width);
    actions.lines.forEach((line, index) => push(line, actions.hits[index]));
  } else if (entry?.kind === "job") {
    const row = entry.row;
    rule();
    const height = options.short ? 10 : 22;
    for (const line of jobDetailLines(row.source, { bound: model.bound, now: options.now, legend: options.legend, freeze: options.freeze }, Math.max(4, width - 1), height)) {
      push(fill(" " + line, width));
    }
    const actions = actionRows(row.key, jobActions(row, options.freeze), width);
    if (actions.hits.some(rowHits => rowHits.length)) actions.lines.forEach((line, index) => push(line, actions.hits[index]));
  }

  if (model.agents.length) {
    rule();
    push(fill(" " + BOLD + PANEL_TEXT + "Timeline" + UNBOLD + (model.maxParallel === undefined ? "" : PANEL_MUTED + ` · up to ${model.maxParallel} at once`), width));
    const bars = timelineLines(model.agents, width, options.now, model.origin, density !== "min", options.color);
    bars.forEach((line, index) => {
      const row = model.agents[index];
      push(fill(line, width), row ? [row.capabilities.open ? { key: row.key, action: "open" } : { key: row.key }] : undefined);
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
      ? enter ? "open" : data === "t" ? "transcript" : data === "s" ? "steer" : data === "x" ? "cancel" : data === "m" ? "model" : undefined
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
