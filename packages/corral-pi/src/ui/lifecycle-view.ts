// Timeline (`l` from Task): the house list-plus-detail layout over this host's session event log, observed live.
// Row 0 is the back line `‹ Task · Events`, then one chip row with source, position, last sequence and gaps, one muted
// caveat line and, behind the shared `?` legend, every other caveat. From SPLIT_MIN main-view columns the list sits left
// of the selected event's detail; below, the detail stacks under the list. The list fills the rows the view is given.
import { matchesKey, truncateToWidth, visibleWidth, type Component, type TuiMouseEvent } from "@earendil-works/pi-tui";
import type { HostIdentity } from "../roles/types.ts";
import type { BridgeEvent } from "../bridge/events.ts";
import type { InspectorSnapshot } from "./inspector.ts";
import { pad, plainLabel, PANEL_MUTED, PANEL_RULE, PANEL_TEXT, PANEL_WARNING } from "./panels-view.ts";
import { VIEW_INSET } from "./frame.ts";
import {
  backLine, bold, cells, chip, chipHeader, clock, field, fitColumns, headerRow, listRow, sectionRule, stateRow, wrap,
  type ColumnLayout,
} from "./panel-parts.ts";
import type { ReadState } from "./read-state.ts";
import { LIST_WIDTH, SPLIT_MIN } from "./proof-view.ts";

/**
 * Workbench wiring: the shared `?` legend, the rows the view is shown in (0 or absent: draw the whole list) and whether
 * the view has the keys (the selection bar is accent then, muted otherwise).
 */
export interface LifecycleViewOptions { readonly legend?: () => boolean; readonly rows?: () => number; readonly focused?: () => boolean }

/** The view's content before layout. */
interface Frame {
  readonly chips: readonly string[];
  readonly legend: readonly string[];
  readonly layouts: readonly ColumnLayout[];
  readonly rows: readonly (readonly string[])[];
  readonly selected: number;
  readonly empty: string;
  /** Drawn in place of the list and detail while there is nothing to list. */
  readonly body: ((width: number) => string[]) | undefined;
  readonly detail: (width: number) => string[];
}

const VALUE_CAP = 2048, PAIR_CAP = 60, DEPTH_CAP = 6, WINDOW = 80;
/** Host events are observed live in this process, never read: no read phase, no freshness note, no reread. */
const OBSERVED: ReadState = { phase: "not_applicable", readAt: undefined, refreshing: false };
const LINE = "Host observations · NOT canonical task settlement";

const muted = (text: string) => PANEL_MUTED + text + PANEL_TEXT;
const clip = (text: string, width: number) => truncateToWidth(text, Math.max(0, width), "…");
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
/** Event payload as label/value pairs: nested objects as `a.b`, arrays of objects as `a[0].b`, bounded in depth and count. */
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

/** This host's session events, newest window only. Render never reads a log or invokes mutations. */
export class LifecycleView implements Component {
  private selected = 0;
  /** The selected event's sequence: the selection follows its event while the window slides. */
  private seq: number | undefined;
  /** First list row shown while the list is windowed; follows the selection. */
  private top = 0;
  private backTo = 0;
  /** List rows by content row; a click selects only within the list's columns `[0, hitWidth)`. */
  private hits = new Map<number, number>(); private hitWidth = 0;
  constructor(private readonly identity: HostIdentity, private readonly bound: () => boolean,
    private readonly changed: () => void, private readonly canInteract: () => boolean,
    private readonly host: () => InspectorSnapshot | undefined, private readonly options: LifecycleViewOptions = {}) {}
  /** The `‹ Task` back link: view-local columns `[from, to)` on content row `row`. The view itself ignores clicks there. */
  get back(): { readonly row: number; readonly from: number; readonly to: number } {
    return { row: 0, from: 0, to: this.backTo };
  }
  private window(): readonly BridgeEvent[] {
    return this.host()?.events.events.slice(-WINDOW) ?? [];
  }
  handleInput(data: string): boolean {
    if (!this.canInteract()) return false;
    if (data === "j" || data === "k" || matchesKey(data, "down") || matchesKey(data, "up")) {
      const events = this.window();
      if (this.seq !== undefined) this.selected = events.findIndex(e => e.seq === this.seq);
      this.selected = Math.max(0, Math.min(events.length - 1, this.selected + (data === "j" || matchesKey(data, "down") ? 1 : -1)));
      this.seq = events[this.selected]?.seq;
      this.changed(); return true;
    }
    return false;
  }
  handleMouse(event: TuiMouseEvent) {
    if (!this.canInteract() || event.type !== "click" || event.button !== "left" || event.shift || event.alt || event.ctrl) return undefined;
    const index = this.hits.get(event.y);
    if (index === undefined || event.x >= this.hitWidth) return undefined;
    this.selected = index;
    this.seq = this.window()[index]?.seq;
    this.changed(); return { handled: true };
  }
  render(width: number): string[] {
    const w = Math.max(1, width);
    this.hits.clear();
    const right = `${this.identity.task ?? "no task"} / ${this.identity.run ?? "no run"} · read-only`;
    const back = backLine("Task", "Events", right, w);
    this.backTo = back.to;
    return this.layout(this.frame(), back.line, w);
  }

  private position(count: number): string {
    if (!count) return chip("0 events");
    return chip(`${this.selected >= 0 && this.selected < count ? this.selected + 1 : "–"}/${count} events`);
  }

  private frame(): Frame {
    const host = this.host(), page = host?.events, bound = this.bound();
    const legend = [`Read-only · ${this.identity.task ?? "no task"} / ${this.identity.run ?? "no run"} · No queue, move or dispatch controls.`,
      "Scoped host/session events · in append order, never timestamp-sorted.",
      bound ? "Bound host observations · NOT canonical task settlement." : "Host unbound · these observations may be stale · NOT canonical task settlement.",
      ...(host && page ? [`Session ${host.session} · bound session ${host.boundSession ?? "unavailable"} · last #${page.last}.`] : []),
      ...(page?.gap ? [`GAP: ${page.gap.missing} host events missing; resumed #${page.gap.resumed}.`] : []),
      ...(page && (page.more || page.events.length > WINDOW) ? ["GAP: host event window capped / incomplete; older or additional events unavailable."] : []),
      "Append sequence within this host process only; earlier sessions unavailable. No success inferred."];
    if (!host || !page) {
      return { chips: [], legend, layouts: [], rows: [], selected: -1, empty: "", detail: () => [],
        body: bodyWidth => stateRow(OBSERVED, "session event log", bodyWidth, "this host does not expose its session event log.") };
    }
    const events = this.window();
    if (this.seq !== undefined) this.selected = events.findIndex(event => event.seq === this.seq);
    const selected = this.selected, current = events[selected];
    if (current) this.seq = current.seq;
    const seq = Math.max(1, ...events.map(event => String(event.seq).length));
    return {
      chips: [chip("host log · not canonical"), this.position(events.length), chip(`last #${page.last}`),
        ...(page.gap ? [chip(`gap · ${page.gap.missing} missing, resumed #${page.gap.resumed}`, "warn")] : []),
        ...(page.more || page.events.length > WINDOW ? [chip("gap · window capped", "warn")] : [])],
      legend, body: undefined,
      layouts: [
        { columns: [{ label: "#", width: seq, align: "right" }, { label: "time", width: 8 }, { label: "kind", width: 8 }], flex: 2 },
        { columns: [{ label: "#", width: seq, align: "right" }, { label: "kind", width: 1 }], flex: 1 },
      ],
      rows: events.map(event => [muted(String(event.seq)), muted(eventTime(event.at)),
        PANEL_TEXT + plainLabel(event.kind) + (event.omitted ? muted(" · payload omitted") : "")]),
      selected, empty: "No host event in this process's window",
      detail: detailWidth => this.detail(host, current, detailWidth, events.length > 0),
    };
  }
  private detail(host: InspectorSnapshot, event: BridgeEvent | undefined, width: number, any: boolean): string[] {
    if (!event) return any ? wrap(PANEL_WARNING + "! Selected session event unavailable / evicted · j/k select existing event" + PANEL_TEXT, width) : [];
    const L = 8, payload = Object.fromEntries(Object.entries(event).filter(([key]) => !["seq", "at", "kind"].includes(key)));
    const out = [clip(`${PANEL_TEXT}${bold(plainLabel(event.kind))}${PANEL_MUTED} · #${event.seq}${PANEL_TEXT}`, width),
      ...field("Time", stampOf(event.at), width, L),
      ...field("Session", `${host.session} / #${event.seq}`, width, L),
      ...field("Bound", host.boundSession ?? "unavailable", width, L)];
    if (Object.keys(payload).length) out.push(sectionRule("Payload", width), ...pairRows(payload, width));
    return out;
  }

  /**
   * Back line, the chip row with the host's binding, the caveat row, the legend, then the list beside (or above) the
   * detail; while nothing is listed, the frame's body stands in their place.
   */
  private layout(frame: Frame, back: string, width: number): string[] {
    const lines = [back], legend = this.options.legend?.() ?? false;
    lines.push(...chipHeader(frame.chips, width, OBSERVED, Date.now(), { unbound: !this.bound() }));
    lines.push(muted(clip(LINE, width)));
    if (legend) for (const text of frame.legend) lines.push(...wrap(muted(plainLabel(text)), width));
    if (frame.body) return [...lines, ...frame.body(width)];

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
    return lines;
  }
  invalidate(): void {}
}
