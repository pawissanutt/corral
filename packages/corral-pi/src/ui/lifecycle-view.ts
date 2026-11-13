// Graph (`g` from Task) and Timeline (`l`): the house list-plus-detail layout over the cached lifecycle projection.
// Row 0 is the back line `‹ Task · Graph`, then one chip row with source, freshness, counts and gaps, one muted caveat
// line and, behind the shared `?` legend, every other caveat. From SPLIT_MIN main-view columns the list sits left of
// the selected record's detail; below, the detail stacks under the list. The list fills the rows the view is given.
import { z } from "zod";
import { matchesKey, truncateToWidth, visibleWidth, type Component, type TuiMouseEvent } from "@earendil-works/pi-tui";
import type { HostIdentity } from "../roles/types.ts";
import type { BridgeEvent } from "../bridge/events.ts";
import type { InspectorSnapshot } from "./inspector.ts";
import type { AgentObservation } from "./agents-model.ts";
import { pad, plainLabel, PANEL_BLUE, PANEL_MUTED, PANEL_RULE, PANEL_TEXT, PANEL_WARNING } from "./panels-view.ts";
import { VIEW_INSET } from "./frame.ts";
import {
  backLine, bold, cells, chip, chipHeader, clock, field, fitColumns, headerRow, listRow, sectionRule, toneColor, wrap,
  type ColumnLayout,
} from "./panel-parts.ts";
import { LIST_WIDTH, SPLIT_MIN } from "./proof-view.ts";

/**
 * Main-view columns from which the graph detail draws its edges as arrows; below, the same edges read as words. The
 * workbench draws this view VIEW_INSET cells narrower than the main view, so the comparison adds them back.
 */
export const GRAPH_MIN = 90;

const Label = z.string().max(4096);
const Schema = z.object({
  version: z.literal(1), task: Label, run: Label,
  nodes: z.array(z.object({ id: Label, title: Label, status: Label.nullable(), depends_on: z.array(Label).max(24), parent: Label.nullable(), missing: z.boolean() })).max(24),
  graph_gap: z.boolean(), event_gap: z.boolean(), end: z.number().nonnegative(),
  events: z.array(z.object({ source: Label, sequence: z.number().nonnegative(), event: z.object({
    ts: Label, task: Label.nullable().optional(), run: Label.nullable().optional(), kind: Label, actor: Label.nullable().optional(), via: Label.nullable().optional(), detail: z.unknown(),
  }) })).max(80),
});
export type LifecycleSnapshot = z.infer<typeof Schema>;
type GraphNode = LifecycleSnapshot["nodes"][number];
type EventEntry = LifecycleSnapshot["events"][number];
function eventRun(detail: unknown): string | undefined {
  return typeof detail === "object" && detail !== null && "run" in detail && typeof detail.run === "string" ? detail.run : undefined;
}

/**
 * Workbench wiring: the shared `?` legend, the rows the view is shown in (0 or absent: draw the whole list) and whether
 * the view has the keys (the selection bar is accent then, muted otherwise).
 */
export interface LifecycleViewOptions { readonly legend?: () => boolean; readonly rows?: () => number; readonly focused?: () => boolean }

/** One view's content before layout. */
interface Frame {
  readonly chips: readonly string[];
  /** Last successful canonical read; `note` replaces the read text for a source that is not reread. */
  readonly at: number | undefined; readonly note: string | undefined;
  /** Warning sentences, one clipped row each. */
  readonly alerts: readonly string[];
  /** The one muted caveat row. */
  readonly line: string;
  readonly legend: readonly string[];
  readonly layouts: readonly ColumnLayout[];
  readonly rows: readonly (readonly string[])[];
  readonly selected: number;
  readonly empty: string;
  readonly detail: (width: number) => string[];
  readonly after: ((width: number) => string[]) | undefined;
}

const VALUE_CAP = 2048, PAIR_CAP = 60, DEPTH_CAP = 6;
const UNAVAILABLE = "Unavailable · no empty graph or successful history inferred";
const READ_ONLY = (identity: HostIdentity) => `Read-only · ${identity.task ?? "unbound"} / ${identity.run ?? "unbound"} · No queue, move or dispatch controls.`;
const SNAPSHOT = "Canonical snapshot at last reconcile · not a live subscription.";

const muted = (text: string) => PANEL_MUTED + text + PANEL_TEXT;
const clip = (text: string, width: number) => truncateToWidth(text, Math.max(0, width), "…");
/** An error in plain words: no exception class prefix, and a schema dump becomes a sentence. */
function plainError(text: string): string {
  const words = plainLabel(text).replace(/^\w*Error:\s*/, "");
  return /^[[{]/.test(words) ? "the read returned data of an unexpected shape" : words;
}
function eventTime(ts: string): string {
  const at = Date.parse(ts);
  return Number.isFinite(at) ? clock(at, true) : plainLabel(ts);
}
/** The detail's time: local `HH:MM:SS` beside the recorded timestamp, or the recorded text alone when it does not parse. */
function stampOf(ts: string): string {
  const at = Date.parse(ts);
  return Number.isFinite(at) ? `${clock(at, true)} local · ${ts}` : ts;
}
function excerpt(text: string): string {
  return text.length > VALUE_CAP ? `${text.slice(0, VALUE_CAP)}… [detail excerpt]` : text;
}

interface Pair { readonly key: string; readonly value: string }
/** Event detail as label/value pairs: nested objects as `a.b`, arrays of objects as `a[0].b`, bounded in depth and count. */
function flatten(value: unknown): { readonly pairs: Pair[]; readonly more: number } {
  const pairs: Pair[] = [];
  let more = 0;
  const put = (key: string, text: string) => { if (pairs.length < PAIR_CAP) pairs.push({ key: plainLabel(key) || "value", value: text }); else more++; };
  const walk = (item: unknown, key: string, depth: number): void => {
    if (pairs.length >= PAIR_CAP && more > 500) return;
    if (item === null || item === undefined) return put(key, item === null ? "null" : "absent");
    if (typeof item !== "object") return put(key, excerpt(String(item)));
    if (depth >= DEPTH_CAP) return put(key, "nested deeper than this view shows");
    if (Array.isArray(item)) {
      if (!item.length) return put(key, "none");
      if (item.every(entry => entry === null || typeof entry !== "object")) return put(key, excerpt(item.map(entry => String(entry)).join(", ")));
      item.forEach((entry, index) => walk(entry, `${key}[${index}]`, depth + 1));
      return;
    }
    const entries = Object.entries(item);
    if (!entries.length) return put(key, "empty");
    for (const [name, entry] of entries) walk(entry, key ? `${key}.${name}` : name, depth + 1);
  };
  walk(value, "", 0);
  return { pairs, more };
}
/** Pairs as label/value rows; a key longer than the label column takes its own row above its value. */
function pairRows(value: unknown, width: number): string[] {
  const { pairs, more } = flatten(value);
  const cap = Math.max(6, Math.min(18, Math.floor(width * 0.35)));
  const labelWidth = Math.min(cap, Math.max(5, ...pairs.map(pair => visibleWidth(pair.key))));
  const rows = pairs.flatMap(pair => visibleWidth(pair.key) <= labelWidth ? field(pair.key, pair.value || "(empty)", width, labelWidth)
    : [muted(clip(pair.key, width)), ...field("", pair.value || "(empty)", width, 2)]);
  if (more) rows.push(muted(`… ${more} more fields`));
  return rows;
}
/** A label/value row like `field`, whose value keeps its own colours. */
function styledField(label: string, value: string, width: number, labelWidth: number): string[] {
  return wrap(PANEL_TEXT + value, width - labelWidth - 1).map((row, i) => `${PANEL_MUTED}${pad(i ? "" : label, labelWidth)}${PANEL_TEXT} ${row}`);
}

/** Glyph, word and colour of a task status; the word always shows beside the colour. */
function nodeState(node: GraphNode): readonly [glyph: string, word: string, color: string] {
  if (node.missing) return ["!", "missing", PANEL_WARNING];
  const status = node.status;
  if (status === null) return ["!", "unavailable", PANEL_WARNING];
  const key = status.toLowerCase();
  if (/^(done|merged|landed|closed|accepted)$/.test(key)) return ["✓", status, toneColor("ok")];
  if (/fail|cancel|reject|abandon/.test(key)) return ["✗", status, toneColor("bad")];
  if (/block/.test(key)) return ["!", status, toneColor("warn")];
  if (/running|progress|active|gate|review/.test(key)) return ["●", status, PANEL_BLUE];
  return ["○", status, PANEL_MUTED];
}
/** How `node` relates to the bound task, in the canonical edges' own terms. */
function relationOf(data: LifecycleSnapshot, node: GraphNode): string {
  const root = data.nodes.find(item => item.id === data.task), out: string[] = [];
  if (node.id === data.task) out.push("this task");
  if (root?.depends_on.includes(node.id)) out.push("prerequisite");
  if (root?.parent === node.id) out.push("parent");
  if (node.depends_on.includes(data.task)) out.push("dependent");
  if (node.parent === data.task) out.push("subtask");
  return out.join(", ") || "indirect";
}

/** Cached bounded projection. Render never reads a log, traverses a history or invokes mutations. */
export class LifecycleView implements Component {
  mode: "graph" | "timeline" = "graph";
  source: "task" | "host" = "task";
  private hostSeq: number | undefined;
  current: LifecycleSnapshot | undefined;
  error = "Canonical data not read"; stale = true; refreshing = false;
  private active: Promise<void> | undefined;
  private disposed = false;
  private selected = 0;
  /** First list row shown while the list is windowed; follows the selection. */
  private top = 0;
  private readAt: number | undefined;
  private backTo = 0;
  /** List rows by content row; a click selects only within the list's columns `[0, hitWidth)`. */
  private hits = new Map<number, number>(); private hitWidth = 0;
  constructor(private readonly identity: HostIdentity, private readonly load: (() => Promise<unknown>) | undefined,
    private readonly agents: () => readonly AgentObservation[] | undefined, private readonly bound: () => boolean,
    private readonly changed: () => void, private readonly canInteract: () => boolean,
    private readonly host: () => InspectorSnapshot | undefined = () => undefined,
    private readonly options: LifecycleViewOptions = {}) {}
  /** The `‹ Task` back link: view-local columns `[from, to)` on content row `row`. The view itself ignores clicks there. */
  get back(): { readonly row: number; readonly from: number; readonly to: number } {
    return { row: 0, from: 0, to: this.backTo };
  }
  refresh(): Promise<void> {
    if (this.active || this.disposed) return this.active ?? Promise.resolve();
    this.active = this.read().finally(() => { this.active = undefined; });
    return this.active;
  }
  private async read(): Promise<void> {
    this.refreshing = true; this.changed();
    try {
      if (!this.load || !this.identity.task || !this.identity.run) throw new Error("No canonical task/run bound to this host");
      const data = Schema.parse(await this.load());
      if (data.task !== this.identity.task || data.run !== this.identity.run) throw new Error("Lifecycle identity mismatch");
      if (data.events.some(e => e.event.task !== data.task || ((e.event.run ?? eventRun(e.event.detail)) !== undefined && (e.event.run ?? eventRun(e.event.detail)) !== data.run))) throw new Error("Foreign lifecycle event");
      if (this.disposed) return;
      const previous = this.current;
      if (previous) {
        if (this.mode === "graph") {
          const id = previous.nodes[this.selected]?.id;
          if (id) this.selected = data.nodes.findIndex(node => node.id === id);
        } else {
          const entry = previous.events[this.selected];
          if (entry) this.selected = data.events.findIndex(item => item.source === entry.source && item.sequence === entry.sequence);
        }
      }
      this.current = data; this.stale = false; this.error = ""; this.readAt = Date.now();
    } catch (error) { if (!this.disposed) { this.stale = true; this.error = String(error); } }
    finally { this.refreshing = false; if (!this.disposed) this.changed(); }
  }
  handleInput(data: string): boolean {
    if (!this.canInteract()) return false;
    if (this.mode === "timeline" && (data === "h" || data === "c")) { this.source = data === "h" ? "host" : "task"; this.selected = 0; this.hostSeq = undefined; this.changed(); return true; }
    if (data === "r") { void this.refresh(); return true; }
    if (data === "j" || data === "k" || matchesKey(data, "down") || matchesKey(data, "up")) {
      const host = this.mode === "timeline" && this.source === "host" ? this.host()?.events.events.slice(-80) : undefined;
      if (host && this.hostSeq !== undefined) this.selected = host.findIndex(e => e.seq === this.hostSeq);
      const count = host ? host.length : this.mode === "graph" ? this.current?.nodes.length : this.current?.events.length;
      this.selected = Math.max(0, Math.min((count ?? 1) - 1, this.selected + (data === "j" || matchesKey(data, "down") ? 1 : -1)));
      if (host) this.hostSeq = host[this.selected]?.seq;
      this.changed(); return true;
    }
    if (matchesKey(data, "enter")) { this.changed(); return true; }
    return false;
  }
  handleMouse(event: TuiMouseEvent) {
    if (!this.canInteract() || event.type !== "click" || event.button !== "left" || event.shift || event.alt || event.ctrl) return undefined;
    const index = this.hits.get(event.y);
    if (index === undefined || event.x >= this.hitWidth) return undefined;
    this.selected = index;
    if (this.mode === "timeline" && this.source === "host") this.hostSeq = this.host()?.events.events.slice(-80)[index]?.seq;
    this.changed(); return { handled: true };
  }
  render(width: number): string[] {
    const w = Math.max(1, width);
    this.hits.clear();
    const right = `${this.identity.task ?? "unbound"} / ${this.identity.run ?? "unbound"} · read-only`;
    const back = backLine("Task", this.mode === "graph" ? "Graph" : "Events", right, w);
    this.backTo = back.to;
    const frame = this.mode === "graph" ? this.graphFrame(w) : this.source === "host" ? this.hostFrame() : this.eventFrame();
    return this.layout(frame, back.line, w);
  }

  /** The source chip and warning rows of the canonical read, shared by Graph and the canonical timeline. */
  private canonical(): { readonly chip: string; readonly alerts: string[] } {
    const error = plainError(this.error);
    if (!this.bound()) return { chip: chip("STALE · disconnected", "warn"), alerts: [`Disconnected · ${error || "retained canonical window; reconcile"}`] };
    if (!this.current) return this.refreshing ? { chip: chip("reading…"), alerts: [] }
      : { chip: chip("unavailable", "warn"), alerts: [this.error === "Canonical data not read" ? "Canonical data not read" : `Read failed: ${error || "unknown error"}`] };
    if (this.stale) return { chip: chip("STALE · read failed", "warn"), alerts: [`Read failed, retained rows are historical: ${error || "unknown error"}`] };
    return { chip: chip("canonical"), alerts: [] };
  }
  private clampSelection(count: number): number {
    this.selected = Math.max(-1, Math.min(this.selected, count - 1));
    return this.selected;
  }
  private position(count: number, noun: string): string {
    if (!count) return chip(`0 ${noun}`);
    return chip(`${this.selected >= 0 && this.selected < count ? this.selected + 1 : "–"}/${count} ${noun}`);
  }

  private graphFrame(width: number): Frame {
    const data = this.current, state = this.canonical(), arrows = width + VIEW_INSET >= GRAPH_MIN;
    const nodes = data?.nodes ?? [], selected = data ? this.clampSelection(nodes.length) : this.selected;
    const id = Math.min(12, Math.max(4, ...nodes.map(node => visibleWidth(plainLabel(node.id)))));
    const legend = [READ_ONLY(this.identity),
      "Task graph: task ─depends_on→ prerequisite · task ┄task-parent→ parent. Below the graph width the same canonical edges read as a dependency list.",
      SNAPSHOT,
      ...(data?.graph_gap ? ["GAP: graph capped at 24 nodes/edges per task; omitted targets remain unknown."] : []),
      "Agent-parent provenance comes from host observations, NOT task dependency edges. No agent parent is inferred from conversation order.",
      "✓ done · ● running or in review · ○ other state · ! blocked, missing or unavailable · ✗ failed or cancelled"];
    return {
      chips: [state.chip, ...(data ? [this.position(nodes.length, "tasks"), ...(data.graph_gap ? [chip("gap · capped at 24", "warn")] : [])] : [])],
      at: this.readAt, note: undefined, alerts: state.alerts,
      line: this.refreshing ? (data ? "Reading… retained rows are historical" : "Reading canonical records…")
        : arrows ? "Task graph: task ─depends_on→ prerequisite · task ┄task-parent→ parent" : "Dependency list · same canonical edges and details",
      legend,
      layouts: [
        { columns: [{ label: "task", width: id }, { label: "state", width: 13 }, { label: "relation", width: 20 }, { label: "title", width: 10 }], flex: 3 },
        { columns: [{ label: "task", width: id }, { label: "state", width: 13 }, { label: "relation", width: 8 }], flex: 2 },
        { columns: [{ label: "task", width: id }, { label: "state", width: 4 }], flex: 1 },
      ],
      rows: data ? data.nodes.map(node => {
        const [glyph, word, color] = nodeState(node);
        return [PANEL_TEXT + plainLabel(node.id), `${color}${glyph} ${plainLabel(word)}${PANEL_TEXT}`, muted(relationOf(data, node)), PANEL_TEXT + plainLabel(node.title)];
      }) : [],
      selected, empty: data ? "No task in this bounded window · not proof of no dependencies" : UNAVAILABLE,
      detail: detailWidth => data ? this.nodeDetail(data, nodes[selected], detailWidth, arrows) : [],
      after: detailWidth => this.agentRows(detailWidth),
    };
  }
  private nodeDetail(data: LifecycleSnapshot, node: GraphNode | undefined, width: number, arrows: boolean): string[] {
    if (!node) return data.nodes.length ? wrap(PANEL_WARNING + "! Selected identity no longer in this window · j/k or click to select an existing record" + PANEL_TEXT, width) : [];
    const [glyph, word, color] = nodeState(node), L = 10;
    const out = [clip(`${PANEL_TEXT}${bold(plainLabel(node.id))} ${color}${glyph} ${plainLabel(word)}${PANEL_TEXT}`, width)];
    out.push(...field("Title", node.title, width, L), ...field("Relation", relationOf(data, node), width, L));
    if (node.missing) out.push(...styledField("Record", `${PANEL_WARNING}! missing / unavailable${PANEL_TEXT}`, width, L));
    const dep = (target: string) => {
      const found = data.nodes.find(item => item.id === target);
      const status = !found || found.missing ? `${PANEL_WARNING}missing / unavailable${PANEL_TEXT}` : muted(plainLabel(found.status ?? "unavailable"));
      const unmet = found && !found.missing && found.status !== "done" ? `${PANEL_WARNING} · unmet prerequisite${PANEL_TEXT}` : "";
      return `${plainLabel(target)} ${status}${unmet}`;
    };
    if (arrows) {
      const edges = [...node.depends_on.map(target => `─depends_on→ ${dep(target)}`), ...(node.parent ? [`┄task-parent→ ${plainLabel(node.parent)}`] : [])];
      if (!edges.length) out.push(...field("Edges", "none", width, L));
      edges.forEach((edge, i) => out.push(...styledField(i ? "" : "Edges", `${muted(i === edges.length - 1 ? "└" : "├")}${edge}`, width, L)));
    } else {
      if (!node.depends_on.length) out.push(...field("Requires", "none", width, L));
      node.depends_on.forEach((target, i) => out.push(...styledField(i ? "" : "Requires", dep(target), width, L)));
      out.push(...field("Parent", node.parent ?? "none", width, L));
    }
    const needed = data.nodes.filter(item => item.depends_on.includes(node.id)).map(item => item.id);
    if (needed.length) out.push(...field("Needed by", needed.join(", "), width, L));
    return out;
  }
  private agentRows(width: number): string[] {
    const agents = this.agents(), out = ["", sectionRule("Agent parents · host observations", width)];
    if (!agents) out.push(muted("Agent/session source unavailable"));
    else if (!agents.length) out.push(muted("No agent observed in this host"));
    for (const agent of agents?.slice(0, 100) ?? []) {
      out.push(clip(`${PANEL_TEXT}${plainLabel(agent.id)} ${PANEL_MUTED}┄agent-parent→ ${PANEL_TEXT}${plainLabel(agent.parent ?? "host root")}`
        + `${PANEL_MUTED} · session ${plainLabel(agent.session)} · ${plainLabel(agent.state)}${PANEL_TEXT}`, width));
    }
    return out;
  }

  private eventFrame(): Frame {
    const data = this.current, state = this.canonical(), events = data?.events ?? [];
    const selected = data ? this.clampSelection(events.length) : this.selected;
    const runOf = (entry: EventEntry) => plainLabel(entry.event.run ?? eventRun(entry.event.detail) ?? "–");
    const seq = Math.max(1, ...events.map(entry => String(entry.sequence).length));
    const run = Math.min(10, Math.max(3, ...events.map(entry => visibleWidth(runOf(entry)))));
    const legend = [READ_ONLY(this.identity),
      "Canonical task events · a separate source from host/session events, never merged or timestamp-sorted.",
      `Source: canonical database events · append sequence (not timestamp-sorted) · end sequence ${data?.end ?? "unavailable"}.`,
      "Bound task/run events and task-wide events only · host/session events are a separate source.",
      data?.event_gap ? "GAP: bounded page / omitted, malformed or oversized events." : "Complete bounded task page at read boundary; other scopes are not included.",
      SNAPSHOT,
      `Times are local; the detail adds the recorded timestamp. – in the run column marks a task-wide event. Values over ${VALUE_CAP} characters end in "… [detail excerpt]".`];
    return {
      chips: [state.chip, ...(data ? [this.position(events.length, "events"), ...(data.event_gap ? [chip("gap · bounded page", "warn")] : []), chip(`end #${data.end}`)] : [])],
      at: this.readAt, note: undefined, alerts: state.alerts,
      line: this.refreshing ? (data ? "Reading… retained rows are historical" : "Reading canonical records…")
        : "Canonical task events in append order · never merged with host events or timestamp-sorted",
      legend,
      layouts: [
        { columns: [{ label: "#", width: seq, align: "right" }, { label: "time", width: 8 }, { label: "kind", width: 8 }, { label: "run", width: run }], flex: 2 },
        { columns: [{ label: "#", width: seq, align: "right" }, { label: "kind", width: 6 }, { label: "run", width: run }], flex: 1 },
        { columns: [{ label: "#", width: seq, align: "right" }, { label: "kind", width: 1 }], flex: 1 },
      ],
      rows: events.map(entry => [muted(String(entry.sequence)), muted(eventTime(entry.event.ts)), PANEL_TEXT + plainLabel(entry.event.kind), muted(runOf(entry))]),
      selected, empty: data ? "No matching events in this bounded window · not proof of no history" : UNAVAILABLE,
      detail: detailWidth => {
        const entry = events[selected];
        if (!data || !events.length) return [];
        if (!entry) return wrap(PANEL_WARNING + "! Selected identity no longer in this window · j/k or click to select an existing record" + PANEL_TEXT, detailWidth);
        const e = entry.event, L = 7;
        return [clip(`${PANEL_TEXT}${bold(plainLabel(e.kind))}${PANEL_MUTED} · #${entry.sequence}${PANEL_TEXT}`, detailWidth),
          ...field("Time", stampOf(e.ts), detailWidth, L),
          ...field("Task", `${e.task ?? "task-wide"} · run ${e.run ?? eventRun(e.detail) ?? "task-wide"}`, detailWidth, L),
          ...field("Actor", `${e.actor ?? "unavailable"} · via ${e.via ?? "unavailable"}`, detailWidth, L),
          ...field("Source", `${entry.source}@${entry.sequence}`, detailWidth, L),
          sectionRule("Detail", detailWidth), ...pairRows(e.detail, detailWidth)];
      },
      after: undefined,
    };
  }

  private hostFrame(): Frame {
    const host = this.host(), page = host?.events, bound = this.bound();
    const source = bound ? chip("host log · not canonical") : chip("STALE · disconnected", "warn");
    const legend = [READ_ONLY(this.identity),
      "Scoped host/session events · a separate source from canonical task events, never merged or timestamp-sorted.",
      bound ? "Bound host observations · NOT canonical task settlement." : "STALE / disconnected host observations.",
      ...(host && page ? [`Session ${host.session} · bound session ${host.boundSession ?? "unavailable"} · g${page.generation} · last #${page.last}.`] : []),
      ...(page?.gap ? [`GAP: ${page.gap.missing} host events missing; resumed #${page.gap.resumed}.`] : []),
      ...(page && (page.more || page.events.length > 80) ? ["GAP: host event window capped / incomplete; older or additional events unavailable."] : []),
      "Append sequence within this host generation only; earlier sessions unavailable. No success inferred."];
    const line = bound ? "Bound host observations · NOT canonical task settlement" : "STALE / disconnected host observations";
    const empty = (chips: string[], alert: string, text: string): Frame => ({ chips: [source, ...chips], at: undefined, note: "observed in this host",
      alerts: [alert], line, legend, layouts: [{ columns: [{ label: "#", width: 1 }, { label: "kind", width: 1 }], flex: 1 }], rows: [], selected: -1,
      empty: text, detail: () => [], after: undefined });
    if (!host || !page) return empty([chip("unavailable", "warn")], "Session event source unavailable", "Session event source unavailable");
    if (page.generation !== this.identity.generation) {
      return empty([chip("STALE · generation mismatch", "warn")], `Host events unavailable · the log is g${page.generation}, this session is g${this.identity.generation}`,
        "STALE generation mismatch · host events unavailable");
    }
    const events = page.events.slice(-80);
    if (this.hostSeq !== undefined) this.selected = events.findIndex(event => event.seq === this.hostSeq);
    const selected = this.selected, current = events[selected];
    if (current) this.hostSeq = current.seq;
    const seq = Math.max(1, ...events.map(event => String(event.seq).length));
    return {
      chips: [source, this.position(events.length, "events"), chip(`g${page.generation} · last #${page.last}`),
        ...(page.gap ? [chip(`gap · ${page.gap.missing} missing, resumed #${page.gap.resumed}`, "warn")] : []),
        ...(page.more || page.events.length > 80 ? [chip("gap · window capped", "warn")] : [])],
      at: undefined, note: "observed in this host", alerts: [], line, legend,
      layouts: [
        { columns: [{ label: "#", width: seq, align: "right" }, { label: "time", width: 8 }, { label: "kind", width: 8 }], flex: 2 },
        { columns: [{ label: "#", width: seq, align: "right" }, { label: "kind", width: 1 }], flex: 1 },
      ],
      rows: events.map(event => [muted(String(event.seq)), muted(eventTime(event.at)),
        PANEL_TEXT + plainLabel(event.kind) + (event.omitted ? muted(" · payload omitted") : "")]),
      selected, empty: "No host event in this generation's window",
      detail: detailWidth => this.hostDetail(host, current, detailWidth, events.length > 0),
      after: undefined,
    };
  }
  private hostDetail(host: InspectorSnapshot, event: BridgeEvent | undefined, width: number, any: boolean): string[] {
    if (!event) return any ? wrap(PANEL_WARNING + "! Selected session event unavailable / evicted · j/k select existing event" + PANEL_TEXT, width) : [];
    const L = 8, payload = Object.fromEntries(Object.entries(event).filter(([key]) => !["seq", "generation", "at", "kind"].includes(key)));
    const out = [clip(`${PANEL_TEXT}${bold(plainLabel(event.kind))}${PANEL_MUTED} · #${event.seq} · g${event.generation}${PANEL_TEXT}`, width),
      ...field("Time", stampOf(event.at), width, L),
      ...field("Session", `${host.session} / g${event.generation} / #${event.seq}`, width, L),
      ...field("Bound", host.boundSession ?? "unavailable", width, L)];
    if (Object.keys(payload).length) out.push(sectionRule("Payload", width), ...pairRows(payload, width));
    return out;
  }

  /** Back line, chips, alerts, the caveat row, the legend, then the list beside (or above) the detail. */
  private layout(frame: Frame, back: string, width: number): string[] {
    const lines = [back];
    lines.push(...chipHeader(frame.chips, width, frame.at, Date.now(), frame.note));
    for (const alert of frame.alerts) lines.push(PANEL_WARNING + clip(`! ${plainLabel(alert)}`, width) + PANEL_TEXT);
    if (frame.line) lines.push(muted(clip(plainLabel(frame.line), width)));
    if (this.options.legend?.()) for (const text of frame.legend) lines.push(...wrap(muted(plainLabel(text)), width));

    const wide = width + VIEW_INSET >= SPLIT_MIN, listWidth = wide ? LIST_WIDTH : width;
    this.hitWidth = listWidth;
    const detailWidth = wide ? Math.max(20, width - LIST_WIDTH - 2) : width;
    const columns = fitColumns(frame.layouts, listWidth - 3), detail = frame.detail(detailWidth);
    const count = frame.rows.length, viewport = this.options.rows?.() ?? 0;
    let shown = count;
    if (viewport > 0) {
      const free = viewport - lines.length - 2; // the rule and the list's header row
      shown = Math.min(count, wide ? Math.max(3, free) : Math.max(3, free - 1 - detail.length, Math.floor((free - 1) / 2)));
    }
    const selected = frame.selected;
    if (selected >= 0 && selected < count) {
      if (selected < this.top) this.top = selected;
      else if (selected >= this.top + shown) this.top = selected - shown + 1;
    }
    this.top = Math.max(0, Math.min(this.top, count - shown));
    const left: { readonly text: string; readonly index?: number }[] = [{ text: headerRow(columns, listWidth) }];
    const focused = this.options.focused?.() ?? true;
    for (let i = this.top; i < this.top + shown; i++) left.push({ text: listRow(cells(columns, frame.rows[i] ?? []), "", i === selected, listWidth, focused), index: i });
    if (!count) for (const row of wrap(muted(plainLabel(frame.empty)), listWidth - 3)) left.push({ text: `  ${pad(row, listWidth - 3)} ` });

    if (wide) {
      lines.push(`${PANEL_RULE}${"─".repeat(LIST_WIDTH)}┬${"─".repeat(Math.max(0, width - LIST_WIDTH - 1))}${PANEL_TEXT}`);
      const start = lines.length;
      for (let i = 0; i < Math.max(left.length, detail.length); i++) {
        const row = left[i];
        if (row?.index !== undefined) this.hits.set(start + i, row.index);
        lines.push(`${row?.text ?? " ".repeat(LIST_WIDTH)}${PANEL_RULE}│${PANEL_TEXT} ${detail[i] ?? ""}`);
      }
    } else {
      const rule = `${PANEL_RULE}${"─".repeat(width)}${PANEL_TEXT}`;
      lines.push(rule);
      for (const row of left) { if (row.index !== undefined) this.hits.set(lines.length, row.index); lines.push(row.text); }
      if (detail.length) lines.push(rule, ...detail);
    }
    if (frame.after) lines.push(...frame.after(width));
    return lines;
  }
  invalidate(): void {}
  dispose(): void { this.disposed = true; }
}
