// Inbox (F11) split layout: item list beside the selected item's detail, matching Proof.
// Below SPLIT_MIN main-view columns the list collapses to a one-line picker above the same detail. The workbench
// draws this view VIEW_INSET cells narrower than the main view, so the threshold adds them back.
import type { ExtensionUIContext } from "@earendil-works/pi-coding-agent";
import { matchesKey, stripTerminalSequences, truncateToWidth, visibleWidth, type Component, type TuiMouseEvent } from "@earendil-works/pi-tui";
import type { Role } from "../roles/types.ts";
import { DecisionInbox, inboxIdentity, type InboxItem } from "./decision-inbox.ts";
import { bold, chip, chipHeader, chipRows, field, glyphRows, listRow, sectionRule, stateBanner, stateRow, toneColor, wrap, type Tone } from "./panel-parts.ts";
import { splitRow } from "./panels-chrome.ts";
import { pad, plainLabel, PANEL_BG, PANEL_BLUE, PANEL_MUTED, PANEL_RULE, PANEL_TEXT, PANEL_WARNING, RESET } from "./panels-view.ts";
import { LIST_WIDTH, SPLIT_MIN } from "./proof-view.ts";
import { VIEW_INSET } from "./frame.ts";
import { openPager } from "./pager.ts";

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
  /**
   * `notice` shows a refusal (closed, stale or out-of-scope item) where the panels show their feedback; `focused`
   * says whether the view has the keys, which draws the selection bar in accent instead of muted.
   */
  constructor(readonly store: DecisionInbox, private readonly role: Role, private readonly project: string,
    private readonly canInteract: () => boolean, private readonly changed: () => void,
    private readonly focus: () => void, private readonly legend: () => boolean = () => false,
    private readonly notice: (text: string) => void = () => {}, private readonly focused: () => boolean = () => true) {}
  bind(ui: ExtensionUIContext): void { this.ui = ui; }
  render(width: number): string[] {
    const lines: string[] = []; this.hits.clear(); this.rows.clear();
    const full = (text: string) => lines.push(...wrap(PANEL_TEXT + text, width));
    const store = this.store, state = store.state, items = store.visible, snapshot = store.snapshot;
    if (state.phase === "not_applicable") return stateRow(state, "inbox", width, "no canonical inbox reader is attached to this session.");
    // A count is drawn only from a snapshot: with nothing read the number of waiting items is unknown, not zero.
    const chips = [chip("read only"), ...(snapshot ? [chip(`${store.history ? "all states" : "pending"} ${items.length}`, !store.history && items.length ? "warn" : "none")] : []),
      ...(store.query ? [chip(`filter ${store.query}`)] : [])];
    lines.push(...chipHeader(chips, width, state, Date.now()), ...stateBanner(state, "inbox", width));
    if (this.legend()) {
      full(`${PANEL_MUTED}Canonical attention and decisions${snapshot ? ` for ${plainLabel(snapshot.workspace)}` : ""}. ⚑ pending · · closed or resolved.${PANEL_TEXT}`);
      full(`${PANEL_MUTED}Enter opens read-only detail; a rereads, revalidates and opens the existing answer-flow instructions. No panel action sends an answer or grants authority.${PANEL_TEXT}`);
    }
    // Without a snapshot the state row is the whole body: reading, not read yet, or nothing (the problem line said it).
    if (!snapshot) { lines.push(...stateRow(state, "canonical inbox", width)); return lines; }
    if (store.selected && !store.current) full(`${PANEL_WARNING}! Selected item no longer matches this view; choose an item with j/k.${PANEL_TEXT}`);
    if (!items.length) {
      const base = snapshot.items.some(item => store.history || item.pending);
      lines.push(...glyphRows(store.query && base ? `○ No item matches “${store.query}”.`
        : store.history ? "○ No items in this snapshot." : `○ Nothing is waiting for a decision in ${snapshot.workspace}.`, width));
      return lines;
    }
    const current = store.current, index = current ? items.indexOf(current) : -1;

    if (width + VIEW_INSET < SPLIT_MIN) {
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
      const [glyph, tone] = stateOf(item), status = `${toneColor(tone)}${glyph} ${fit(item.state, 12)}${PANEL_TEXT}`;
      const label = `${PANEL_TEXT}${fit(item.task, 12)} ${PANEL_MUTED}${fit(item.kind, LIST_WIDTH - 3 - 15 - visibleWidth(status))}${PANEL_TEXT}`;
      const row = listRow(label, status, item === current, LIST_WIDTH);
      // listRow draws the bar in accent; an unfocused view keeps the bar but mutes it.
      left.push({ text: item === current && !this.focused() ? row.replace(PANEL_BLUE + "▌", PANEL_MUTED + "▌") : row, id: inboxIdentity(item) });
    }
    const detailWidth = Math.max(20, width - LIST_WIDTH - 2);
    const right = current ? detail(current, detailWidth) : wrap(`${PANEL_MUTED}Choose an item with j/k or a click.${PANEL_TEXT}`, detailWidth);
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
  /**
   * Selects the first pending item in canonical order (the reader lists pending items first; items carry no time,
   * so this is the oldest the data can tell), clearing a filter that hides it. False when nothing is pending.
   */
  selectFirstPending(): boolean {
    const item = this.store.snapshot?.items.find(entry => entry.pending);
    if (!item) return false;
    if (!this.store.visible.includes(item)) this.store.query = "";
    this.store.select(inboxIdentity(item));
    return true;
  }
  handleInput(data: string): boolean {
    if (data === "h") { this.store.history = !this.store.history; this.changed(); return true; }
    if (data === "r") { void this.store.refresh(); return true; }
    if (matchesKey(data, "f3")) { void this.search(); return true; }
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
    try { await openPager(ui, { title: flow ? "Established answer flow · NOT SENT" : "Decision inspection · READ ONLY", lines: source, wrap: true }); }
    finally { this.sheetOpen = false; }
    this.changed();
  }
  invalidate(): void {}
  dispose(): void { this.disposed = true; this.ui = undefined; this.store.dispose(); }
}
