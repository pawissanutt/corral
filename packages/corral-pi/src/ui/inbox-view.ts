import type { ExtensionUIContext } from "@earendil-works/pi-coding-agent";
import { Text, matchesKey, type Component, type TuiMouseEvent } from "@earendil-works/pi-tui";
import type { Role } from "../roles/types.ts";
import { DecisionInbox, inboxIdentity, type InboxItem } from "./decision-inbox.ts";
import { box, PANEL_BLUE, PANEL_MUTED, plainLabel } from "./panels-view.ts";

/** A read-only inspector with a separately opened, revalidated established-answer-flow sheet. */
export class InboxView implements Component {
  private ui: ExtensionUIContext | undefined; private busy = false; private disposed = false;
  private hits = new Map<number, string>(); readonly rows = new Map<string, number>();
  constructor(readonly store: DecisionInbox, private readonly role: Role, private readonly project: string,
    private readonly canInteract: () => boolean, private readonly changed: () => void,
    private readonly focus: () => void) {}
  bind(ui: ExtensionUIContext): void { this.ui = ui; }
  render(width: number): string[] {
    const rows: string[] = []; this.hits.clear(); this.rows.clear();
    const add = (text: string, accent = false) => rows.push(...new Text((accent ? PANEL_BLUE : PANEL_MUTED)
      + text.replace(/[\u0000-\u001f\u007f-\u009f]/g, " "), 0, 0).render(width));
    add("Canonical attention & decisions · READ ONLY", true);
    add(this.store.status);
    add(`Workspace: ${this.store.snapshot?.workspace ?? "unavailable"}`);
    add(`${this.store.history ? "All states" : "Pending only"} · h history · / filter · r refresh · Enter inspect · a answer flow`);
    add("No panel action sends an answer or grants authority.");
    if (this.store.query) add("Filter: " + this.store.query);
    add("");
    for (const item of this.store.visible) {
      const id = inboxIdentity(item), start = rows.length; this.rows.set(id, start);
      add(`${id === this.store.selected ? ">" : " "} ${item.task} · ${item.state} · ${item.kind}`, id === this.store.selected);
      add(`  ${item.key} @${item.revision} (${item.revision_kind})`);
      add(`  ${item.container ?? "No container"} · scope ${item.scope} · ${item.responsible}`);
      add("  " + item.reason);
      for (let row = start; row < rows.length; row++) this.hits.set(row, id);
      add("");
    }
    if (!this.store.visible.length) add(this.store.snapshot ? this.store.error ? "Retained snapshot has no matching items; current state unavailable."
      : "No matching items in this canonical snapshot." : "Canonical inbox unavailable; r to retry.");
    if (this.store.selected && !this.store.current) add("Selected item no longer matches this view; choose an item with j/k.");
    return rows;
  }
  handleMouse(event: TuiMouseEvent): { handled: true } | undefined {
    if (!this.canInteract() || event.type !== "click" || event.button !== "left" || event.shift || event.alt || event.ctrl || (event.clickCount ?? 1) !== 1) return undefined;
    const key = this.hits.get(event.y);
    if (!key) return undefined;
    this.focus();
    this.store.select(key);
    return { handled: true };
  }
  handleInput(data: string): boolean {
    if (data === "h") { this.store.history = !this.store.history; this.changed(); return true; }
    if (data === "r") { void this.store.refresh(); return true; }
    if (data === "/") { void this.search(); return true; }
    if (data === "a") { void this.inspect(true); return true; }
    if (matchesKey(data, "enter")) { void this.inspect(false); return true; }
    return false;
  }
  private async search(): Promise<void> {
    const value = await this.ui?.input("Filter canonical inbox (task, scope, reason, key)", this.store.query);
    if (value !== undefined && !this.disposed) { this.store.query = value; this.changed(); }
  }
  async inspect(answerFlow: boolean): Promise<void> {
    const captured = this.store.current, ui = this.ui;
    if (!captured || !ui || this.busy || !this.canInteract()) return;
    this.busy = true;
    try {
      const flow = answerFlow ? await this.store.answerFlow(captured, this.role, this.project) : undefined;
      if (this.disposed) return;
      await this.sheet(ui, captured, flow);
    } catch (error) { if (!this.disposed) ui.notify(plainLabel(error instanceof Error ? error.message : String(error)), "warning"); }
    finally { this.busy = false; }
  }
  private async sheet(ui: ExtensionUIContext, item: InboxItem, flow?: string): Promise<void> {
    const source = [item.task + " · " + item.state + " · " + item.kind, item.key,
      `Revision: ${item.revision} (${item.revision_kind})`, `Container: ${item.container ?? "none"}`,
      `Scope: ${item.scope} · responsible: ${item.responsible}`, "", item.reason, ...item.detail, "",
      "Inspect: " + item.inspect, "", ...(flow ? [flow] : [item.response ? "An existing answer route is available. Close this sheet, then press a to revalidate and open its instructions."
        : "No answer route: closed, already resolved or inspection-only condition."]), "", "Nothing was sent or authorized."];
    await ui.custom<void>((tui, _theme, _keys, done) => {
      let offset = 0, max = 0;
      const scroll = (delta: number) => { offset = Math.max(0, Math.min(max, offset + delta)); tui.requestRender(); };
      return {
        render(width) {
          const height = Math.max(5, Math.floor(tui.terminal.rows * 0.8));
          const rows = source.flatMap(line => new Text(line.replace(/[\u0000-\u001f\u007f-\u009f]/g, " "), 0, 0).render(Math.max(1, width - 4)));
          max = Math.max(0, rows.length - height + 3); offset = Math.min(offset, max);
          return box(flow ? "Established answer flow · NOT SENT" : "Decision inspection · READ ONLY", [...rows.slice(offset, offset + height - 3), "↑/↓ PgUp/PgDn scroll · Esc close"], width, height, true);
        },
        handleInput(data) {
          if (matchesKey(data, "escape") || data === "q") { done(); return; }
          if (matchesKey(data, "up")) scroll(-1);
          if (matchesKey(data, "down")) scroll(1);
          if (matchesKey(data, "pageUp")) scroll(-8);
          if (matchesKey(data, "pageDown")) scroll(8);
        },
        handleMouse(event) { if (event.type === "wheel") { scroll(event.wheelDelta ?? 0); return { handled: true }; } return undefined; },
        invalidate() {},
      };
    }, { overlay: true, overlayOptions: { width: "90%", maxHeight: "85%", anchor: "center" } });
    this.changed();
  }
  invalidate(): void {}
  dispose(): void { this.disposed = true; this.ui = undefined; this.store.dispose(); }
}
