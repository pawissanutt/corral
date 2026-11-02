// Navigation over a bounded canonical read. No mutation, send-message or owner-event capability.
import type { Role } from "../roles/types.ts";
export interface InboxItem {
  readonly kind: string; readonly task: string; readonly container: string | null;
  readonly key: string; readonly revision: string; readonly revision_kind: string;
  readonly state: string; readonly pending: boolean; readonly scope: string;
  readonly reason: string; readonly responsible: string; readonly detail: readonly string[];
  readonly inspect: string; readonly response: string | null; readonly pm_eligible: boolean;
}
export interface InboxSnapshot {
  readonly version: 1; readonly workspace: string; readonly observed_at: string;
  readonly items: readonly InboxItem[];
}
function parse(value: unknown): InboxSnapshot {
  const v = value as Partial<InboxSnapshot> | null;
  if (!v || v.version !== 1 || typeof v.workspace !== "string" || typeof v.observed_at !== "string" || !Array.isArray(v.items)
    || !v.items.every(item => item && [item.kind, item.task, item.key, item.revision, item.revision_kind, item.state, item.scope, item.reason, item.responsible, item.inspect].every(v => typeof v === "string")
      && (item.container === null || typeof item.container === "string") && typeof item.pending === "boolean" && typeof item.pm_eligible === "boolean"
      && (item.response === null || typeof item.response === "string") && Array.isArray(item.detail) && item.detail.every((v: unknown) => typeof v === "string"))) {
    throw new Error("Unsupported canonical inbox schema");
  }
  return v as InboxSnapshot;
}
/** Source namespaces can legally reuse a key (e.g. portable decision id Q1). */
export function inboxIdentity(item: InboxItem): string {
  return JSON.stringify([item.kind, item.task, item.container, item.key]);
}
export class DecisionInbox {
  snapshot: InboxSnapshot | undefined; selected: string | undefined;
  history = false; query = ""; error: string | undefined; refreshing = false;
  private active: Promise<void> | undefined; private disposed = false;
  private readonly abort = new AbortController();
  constructor(private readonly read: ((signal: AbortSignal) => Promise<unknown>) | undefined, private readonly changed: () => void) {}
  get status(): string {
    return this.error ? "STALE / unavailable: " + this.error : this.refreshing ? "Reading canonical inbox…"
      : this.snapshot ? "Observed " + this.snapshot.observed_at : "Not loaded / unavailable";
  }
  get visible(): readonly InboxItem[] {
    const query = this.query.toLowerCase();
    return (this.snapshot?.items ?? []).filter(item => (this.history || item.pending)
      && (!query || [item.task, item.container, item.key, item.reason, item.scope, item.state].join(" ").toLowerCase().includes(query)));
  }
  get current(): InboxItem | undefined { return this.visible.find(item => inboxIdentity(item) === this.selected); }
  select(key: string): void { this.selected = key; this.changed(); }
  move(delta: number): void {
    const rows = this.visible, from = rows.findIndex(item => inboxIdentity(item) === this.selected);
    const item = rows[Math.max(0, Math.min(rows.length - 1, from + delta))];
    if (item) this.select(inboxIdentity(item));
  }
  refresh(): Promise<void> {
    if (this.disposed) return Promise.resolve();
    if (this.active) return this.active;
    const read = this.load(); this.active = read;
    void read.finally(() => { if (this.active === read) this.active = undefined; });
    return read;
  }
  private async load(): Promise<void> {
    this.refreshing = true; this.changed();
    try {
      if (!this.read) throw new Error("Canonical reader unavailable in this host");
      const snapshot = parse(await this.read(this.abort.signal));
      if (this.disposed) return;
      this.snapshot = snapshot; this.error = undefined;
      // Never silently replace a selection removed by a refresh with another task.
      if (this.selected === undefined && this.visible[0]) this.selected = inboxIdentity(this.visible[0]);
    } catch (error) { if (!this.disposed) this.error = error instanceof Error ? error.message : String(error); }
    finally { this.refreshing = false; if (!this.disposed) this.changed(); }
  }
  /** Opens instructions only; caller must not execute them or submit them as owner input. */
  async answerFlow(captured: InboxItem, role: Role, project?: string): Promise<string> {
    // An in-flight read may predate this click. Always do a new read afterward.
    if (this.active) await this.active;
    await this.refresh();
    if (this.disposed) throw new Error("Inbox closed");
    if (this.error) throw new Error(this.error);
    const current = this.snapshot?.items.find(item => inboxIdentity(item) === inboxIdentity(captured));
    if (!current || current.revision !== captured.revision) throw new Error("stale_revision: item changed; inspect the current revision");
    if (!current.pending || !current.response) throw new Error("closed/ineligible: no current answer route for this item");
    if (role === "pm" && (!current.pm_eligible || current.scope !== project)) throw new Error("owner-only/out_of_scope: use the established trusted owner answer flow");
    if (role !== "pm" && role !== "assistant") throw new Error("This role cannot answer; use the established trusted owner answer flow");
    return current.response + "\n\nNo answer has been sent. Panel inspection is not owner input, permission, acceptance or a restart. The canonical operation rechecks eligibility when an actual answer is submitted.";
  }
  dispose(): void { this.disposed = true; this.abort.abort(); }
}
