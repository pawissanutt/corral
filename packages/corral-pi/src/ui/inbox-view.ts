// Inbox (F11) split layout: item list beside the selected item's detail, matching Proof.
// Below SPLIT_MIN columns the list collapses to a one-line picker above the same detail.
import type { ExtensionUIContext } from "@earendil-works/pi-coding-agent";
import { Text, matchesKey, stripTerminalSequences, truncateToWidth, visibleWidth, type Component, type TuiMouseEvent } from "@earendil-works/pi-tui";
import type { Role } from "../roles/types.ts";
import { DecisionInbox, inboxIdentity, type InboxItem } from "./decision-inbox.ts";
import { ago, bold, chip, chipRows, clock, field, listRow, sectionRule, toneColor, wrap, type Tone } from "./panel-parts.ts";
import { splitRow } from "./panels-chrome.ts";
import { box, pad, plainLabel, PANEL_BG, PANEL_BLUE, PANEL_MUTED, PANEL_RULE, PANEL_TEXT, PANEL_WARNING, RESET } from "./panels-view.ts";
import { LIST_WIDTH, SPLIT_MIN } from "./proof-view.ts";

const fit = (text: string, width: number) => stripTerminalSequences(truncateToWidth(plainLabel(text), Math.max(0, width), "…"));
const stateOf = (item: InboxItem): readonly [glyph: string, tone: Tone] => item.pending ? ["⚑", "warn"] : ["·", "none"];

function detail(item: InboxItem, width: number): string[] {
  const [glyph, tone] = stateOf(item);
  const tags = [chip(`${glyph} ${item.state}`, tone), chip(item.pm_eligible ? "PM may answer" : "owner only")];
  const out = [`${PANEL_TEXT}${bold(fit(`${item.task} · ${item.kind}`, width - 2))}`, ...chipRows(tags, width)];
  out.push(...field("Key", `${item.key} @${item.revision} (${item.revision_kind})`, width, 11),
    ...field("Container", item.container ?? "none", width, 11), ...field("Scope", item.scope, width, 11),
    ...field("Responsible", item.responsible, width, 11), ...field("Reason", item.reason, width, 11));
  out.push(sectionRule("Detail", width));
  if (!item.detail.length) out.push(`${PANEL_MUTED}· none recorded${PANEL_TEXT}`);
  for (const line of item.detail) out.push(...wrap(PANEL_TEXT + plainLabel(line), width));
  out.push(sectionRule("Inspect", width), ...wrap(PANEL_BLUE + plainLabel(item.inspect) + PANEL_TEXT, width));
  out.push(sectionRule("Answer", width), ...wrap(item.response
    ? `${PANEL_TEXT}a revalidates and opens the existing answer route${PANEL_MUTED} · nothing is sent${PANEL_TEXT}`
    : `${PANEL_MUTED}· no answer route: closed, already resolved or inspection-only${PANEL_TEXT}`, width));
  return out;
}

/** A read-only inspector with a separately opened, revalidated established-answer-flow sheet. */
export class InboxView implements Component {
  private ui: ExtensionUIContext | undefined; private busy = false; private disposed = false;
  /** True while the read-only detail sheet is open; the footer then lists the pager's keys. */
  sheetOpen = false;
  private hits = new Map<number, string>(); readonly rows = new Map<string, number>();
  /** `notice` shows a refusal (closed, stale or out-of-scope item) where the panels show their feedback. */
  constructor(readonly store: DecisionInbox, private readonly role: Role, private readonly project: string,
    private readonly canInteract: () => boolean, private readonly changed: () => void,
    private readonly focus: () => void, private readonly legend: () => boolean = () => false,
    private readonly notice: (text: string) => void = () => {}) {}
  bind(ui: ExtensionUIContext): void { this.ui = ui; }
  render(width: number): string[] {
    const lines: string[] = []; this.hits.clear(); this.rows.clear();
    const full = (text: string) => lines.push(...wrap(PANEL_TEXT + text, width));
    const store = this.store, items = store.visible, snapshot = store.snapshot;
    const chips = [chip("read only"), chip(`${store.history ? "all states" : "pending"} ${items.length}`, !store.history && items.length ? "warn" : "none"),
      ...(store.query ? [chip(`filter ${store.query}`)] : [])];
    const at = snapshot ? Date.parse(snapshot.observed_at) : Number.NaN, since = ago(at, Date.now());
    const reads = since ? [`read ${clock(at)} (${since}) · r reread`, `read ${since} · r reread`, "r reread"] : ["not read · r reread", "r reread"];
    const rows = chipRows(chips, width), last = rows.at(-1) ?? "";
    const read = reads.find(r => visibleWidth(last) + 2 + visibleWidth(r) <= width);
    lines.push(...rows.slice(0, -1), read ? splitRow(last, PANEL_MUTED + read + PANEL_TEXT, width) : last);
    if (store.error) full(`${PANEL_WARNING}~ STALE / unavailable: ${plainLabel(store.error)}${PANEL_TEXT}`);
    if (store.refreshing) full(`${PANEL_MUTED}Reading canonical inbox…${PANEL_TEXT}`);
    if (this.legend()) {
      full(`${PANEL_MUTED}Canonical attention and decisions for ${plainLabel(snapshot?.workspace ?? "an unavailable workspace")}. ⚑ pending · · closed or resolved.${PANEL_TEXT}`);
      full(`${PANEL_MUTED}Enter opens read-only detail; a rereads, revalidates and opens the existing answer-flow instructions. No panel action sends an answer or grants authority.${PANEL_TEXT}`);
    }
    if (store.selected && !store.current) full(`${PANEL_WARNING}! Selected item no longer matches this view; choose an item with j/k.${PANEL_TEXT}`);
    const empty = snapshot ? store.error ? "Retained snapshot has no matching items; current state unavailable."
      : "No matching items in this canonical snapshot." : "Canonical inbox unavailable; r to retry.";
    const current = store.current, index = current ? items.indexOf(current) : -1;

    if (width < SPLIT_MIN) {
      if (!items.length) { full(PANEL_MUTED + empty + PANEL_TEXT); return lines; }
      const shown = current ?? items[0];
      if (!shown) return lines;
      const [glyph, tone] = stateOf(shown), right = `${toneColor(tone)}${glyph} ${fit(shown.state, 14)}${PANEL_TEXT} `;
      const head = `${PANEL_BLUE} ‹ ${PANEL_TEXT}${bold(fit(shown.task, 12))} ${PANEL_MUTED}${index < 0 ? "–" : index + 1}/${items.length}${PANEL_BLUE} › `;
      this.rows.set(inboxIdentity(shown), lines.length); this.hits.set(lines.length, inboxIdentity(shown));
      lines.push(`${PANEL_BG}${splitRow(`${head}${PANEL_TEXT}${fit(shown.kind, width)}`, right, width)}${RESET}`, `${PANEL_RULE}${"─".repeat(width)}${PANEL_TEXT}`);
      lines.push(...detail(shown, width));
      return lines;
    }

    lines.push(`${PANEL_RULE}${"─".repeat(LIST_WIDTH)}┬${"─".repeat(Math.max(0, width - LIST_WIDTH - 1))}${PANEL_TEXT}`);
    const left: { text: string; id?: string }[] = [{ text: listRow(`${PANEL_MUTED}${store.history ? "ALL" : "PENDING"} ${items.length}`, `state${PANEL_TEXT}`, false, LIST_WIDTH) }];
    for (const item of items) {
      const [glyph, tone] = stateOf(item), state = `${toneColor(tone)}${glyph} ${fit(item.state, 12)}${PANEL_TEXT}`;
      const label = `${PANEL_TEXT}${fit(item.task, 12)} ${PANEL_MUTED}${fit(item.kind, LIST_WIDTH - 3 - 15 - visibleWidth(state))}${PANEL_TEXT}`;
      left.push({ text: listRow(label, state, item === current, LIST_WIDTH), id: inboxIdentity(item) });
    }
    if (!items.length) left.push({ text: listRow(`${PANEL_MUTED}none${PANEL_TEXT}`, "", false, LIST_WIDTH) });
    const detailWidth = Math.max(20, width - LIST_WIDTH - 2);
    const right = current ? detail(current, detailWidth) : wrap(PANEL_MUTED + (items.length ? "Choose an item with j/k or a click." : empty) + PANEL_TEXT, detailWidth);
    const start = lines.length;
    for (let i = 0; i < Math.max(left.length, right.length); i++) {
      const l = left[i];
      if (l?.id !== undefined) { this.hits.set(start + i, l.id); this.rows.set(l.id, start + i); }
      lines.push(`${l?.text ?? " ".repeat(LIST_WIDTH)}${PANEL_RULE}│${PANEL_TEXT} ${pad(right[i] ?? "", detailWidth)}`);
    }
    return lines;
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
    } catch (error) { if (!this.disposed) this.notice(plainLabel(error instanceof Error ? error.message : String(error))); }
    finally { this.busy = false; }
  }
  private async sheet(ui: ExtensionUIContext, item: InboxItem, flow?: string): Promise<void> {
    const source = [item.task + " · " + item.state + " · " + item.kind, item.key,
      `Revision: ${item.revision} (${item.revision_kind})`, `Container: ${item.container ?? "none"}`,
      `Scope: ${item.scope} · responsible: ${item.responsible}`, "", item.reason, ...item.detail, "",
      "Inspect: " + item.inspect, "", ...(flow ? [flow] : [item.response ? "An existing answer route is available. Close this sheet, then press a to revalidate and open its instructions."
        : "No answer route: closed, already resolved or inspection-only condition."]), "", "Nothing was sent or authorized."];
    this.sheetOpen = true; this.changed();
    try {
      await ui.custom<void>((tui, _theme, _keys, done) => {
      let offset = 0, max = 0;
      const scroll = (delta: number) => { offset = Math.max(0, Math.min(max, offset + delta)); tui.requestRender(); };
      return {
        render(width) {
          const height = Math.max(5, Math.floor(tui.terminal.rows * 0.8));
          const rows = source.flatMap(line => new Text(line.replace(/[\u0000-\u001f\u007f-\u009f]/g, " "), 0, 0).render(Math.max(1, width - 4)));
          max = Math.max(0, rows.length - height + 2); offset = Math.min(offset, max);
          return box(flow ? "Established answer flow · NOT SENT" : "Decision inspection · READ ONLY", rows.slice(offset, offset + height - 2), width, height, true);
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
    } finally { this.sheetOpen = false; }
    this.changed();
  }
  invalidate(): void {}
  dispose(): void { this.disposed = true; this.ui = undefined; this.store.dispose(); }
}
