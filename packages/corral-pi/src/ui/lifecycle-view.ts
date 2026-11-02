import { z } from "zod";
import { Text, matchesKey, type Component, type TuiMouseEvent } from "@earendil-works/pi-tui";
import type { HostIdentity } from "../roles/types.ts";
import type { InspectorSnapshot } from "./inspector.ts";
import type { AgentObservation } from "./agent-view.ts";
import { plainLabel } from "./panels-view.ts";

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
function eventRun(detail: unknown): string | undefined {
  return typeof detail === "object" && detail !== null && "run" in detail && typeof detail.run === "string" ? detail.run : undefined;
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
  private hits = new Map<number, number>();
  constructor(private readonly identity: HostIdentity, private readonly load: (() => Promise<unknown>) | undefined,
    private readonly agents: () => readonly AgentObservation[] | undefined, private readonly bound: () => boolean,
    private readonly changed: () => void, private readonly canInteract: () => boolean,
    private readonly host: () => InspectorSnapshot | undefined = () => undefined) {}
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
      this.current = data; this.stale = false; this.error = "";
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
    if (index === undefined) return undefined;
    this.selected = index;
    if (this.mode === "timeline" && this.source === "host") this.hostSeq = this.host()?.events.events.slice(-80)[index]?.seq;
    this.changed(); return { handled: true };
  }
  render(width: number): string[] {
    const rows: string[] = []; this.hits.clear();
    const add = (text: string, index?: number) => {
      const wrapped = new Text(plainLabel(text), 0, 0).render(Math.max(1, width));
      if (index !== undefined) for (let i = 0; i < wrapped.length; i++) this.hits.set(rows.length + i, index);
      rows.push(...wrapped);
    };
    add(`${this.mode === "graph" ? "Dependencies" : "Lifecycle timeline"} · read-only · j/k/↑/↓ or click inspect · r reconcile`);
    add(`${this.identity.task ?? "unbound"} / ${this.identity.run ?? "unbound"} · No queue, move or dispatch controls`);
    if (this.mode === "timeline") add("c canonical task events · h scoped host/session events · sources never merged or timestamp-sorted");
    if (this.mode === "timeline" && this.source === "host") {
      const host = this.host(), page = host?.events;
      add(this.bound() ? "Bound host observations · NOT canonical task settlement" : "STALE / disconnected host observations");
      if (!host || !page) { add("Session event source unavailable"); return rows; }
      add(`Session ${host.session} · bound session ${host.boundSession ?? "unavailable"} · g${page.generation} · last #${page.last}`);
      if (page.generation !== this.identity.generation) { add("STALE generation mismatch · host events unavailable"); return rows; }
      const events = page.events.slice(-80);
      if (page.gap) add(`GAP: ${page.gap.missing} host events missing; resumed #${page.gap.resumed}`);
      if (page.more || page.events.length > 80) add("GAP: host event window capped / incomplete; older or additional events unavailable");
      if (this.hostSeq !== undefined) this.selected = events.findIndex(event => event.seq === this.hostSeq);
      const event = events[this.selected];
      if (event) {
        this.hostSeq = event.seq;
        add(`Inspect session ${host.session} / g${event.generation} / #${event.seq}`);
        add(JSON.stringify(event).slice(0, 8192));
      } else add("Selected session event unavailable / evicted · j/k select existing event");
      const start = Math.max(0, this.selected - 2);
      events.slice(start, start + 8).forEach((event, local) => add(`${local + start === this.selected ? ">" : " "} g${event.generation} #${event.seq} · ${event.at} · ${event.kind}${event.omitted ? " · payload omitted" : ""}`, local + start));
      add("Append sequence within this host generation only; earlier sessions unavailable. No success inferred.");
      return rows;
    }
    add(this.stale || !this.bound() ? `STALE / disconnected · ${this.error || "retained canonical window; reconcile"}` : "Canonical snapshot at last reconcile · not a live subscription");
    if (this.refreshing) add("Reading… retained rows are historical");
    const data = this.current;
    if (!data) { add("Unavailable · no empty graph or successful history inferred"); return rows; }
    const count = this.mode === "graph" ? data.nodes.length : data.events.length;
    this.selected = Math.max(-1, Math.min(this.selected, count - 1));
    if (this.selected < 0) add("Selected identity no longer in this window · j/k or click to select an existing record");
    if (this.mode === "graph") {
      add(width >= 90 ? "Task graph: task ─depends_on→ prerequisite · task ┄task-parent→ parent" : "Dependency list · same canonical edges and details");
      if (data.graph_gap) add("GAP: graph capped at 24 nodes/edges per task; omitted targets remain unknown");
      const node = data.nodes[this.selected];
      if (node) { add(`Inspect canonical task ${node.id} · ${node.status ?? "unavailable"}`); add(node.title); add(`depends_on: ${node.depends_on.join(", ") || "none"} · task-parent: ${node.parent ?? "none"}`); }
      add(`Nodes ${this.selected + 1}/${data.nodes.length} · j/k navigates the full bounded window`);
      const start = Math.max(0, this.selected - 2);
      data.nodes.slice(start, start + 5).forEach((node, local) => {
        const i = start + local;
        add(`${i === this.selected ? ">" : " "} ${node.id} [${node.status ?? "unavailable"}] ${node.title}`, i);
        for (const dep of i === this.selected ? node.depends_on : []) {
          const target = data.nodes.find(n => n.id === dep);
          add(`  ${width >= 90 ? "├─depends_on→" : "requires"} ${dep} [${!target || target.missing ? "missing / unavailable" : target.status}]${target && !target.missing && target.status !== "done" ? " · unmet prerequisite" : ""}`, i);
        }
        if (node.parent) add(`  ${width >= 90 ? "└┄task-parent→" : "task-parent"} ${node.parent}`, i);
      });
      add("Agent-parent provenance (host observations, NOT task dependency edges):");
      const agents = this.agents();
      if (!agents) add("Agent/session source unavailable");
      for (const agent of agents?.slice(0, 100) ?? []) add(`${agent.id} ┄agent-parent→ ${agent.parent ?? "host root"} · session ${agent.session} · ${agent.state}`);
      add("F8 then a: inspect host agent/session details; no parent inferred from conversation order");
    } else {
      add(`Source: canonical database events · append sequence (not timestamp-sorted) · end sequence ${data.end}`);
      add("Bound task/run events and task-wide events only · h opens separate host/session source");
      add(data.event_gap ? "GAP: bounded page / omitted, malformed or oversized events" : "Complete bounded task page at read boundary; other scopes are not included");
      const entry = data.events[this.selected];
      if (entry) {
        add(`Inspect event ${entry.source}@${entry.sequence} · task ${entry.event.task} · run ${entry.event.run ?? eventRun(entry.event.detail) ?? "task-wide"}`);
        add(`Actor ${entry.event.actor ?? "unavailable"} · via ${entry.event.via ?? "unavailable"}`);
        const detail = JSON.stringify(entry.event.detail) ?? "null";
        add(detail.slice(0, 8192) + (detail.length > 8192 ? "… [detail excerpt]" : ""));
      } else add("No matching events in this bounded window · not proof of no history");
      const start = Math.max(0, this.selected - 2);
      add(`Events ${this.selected + 1}/${data.events.length} · j/k navigates the full bounded window`);
      data.events.slice(start, start + 8).forEach((entry, local) => add(`${start + local === this.selected ? ">" : " "} ${entry.source}@${entry.sequence} · ${entry.event.ts} · ${entry.event.kind}`, start + local));
    }
    return rows;
  }
  invalidate(): void {}
  dispose(): void { this.disposed = true; }
}
