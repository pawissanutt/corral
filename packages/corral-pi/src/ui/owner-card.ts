// The owner card's controller: selection, filter, scrolling, the open guard and settling once. Each render draws a
// frame with owner-card-view.ts and keeps the facts of it its keys and pointer act on.
import type { PresentedDialog } from "@earendil-works/pi-coding-agent";
import { decodeKittyPrintable, getKeybindings, Input } from "@earendil-works/pi-tui";
import type { TuiMouseEvent, TuiMouseEventResult } from "@earendil-works/pi-tui";
import { CHOICE_GUARD_MS, PASTE_START } from "./build/choice-dock.ts";
import { piPressed } from "./keymap.ts";
import { cardFrame, cardGeometry, type RowHit } from "./owner-card-view.ts";
import { muted, type CardSpec, type DialogHintHit, type Entry } from "./owner-dialog-parts.ts";
import { PANEL_BLUE, PANEL_TEXT } from "./panels-view.ts";

/**
 * The card. Keys: ↑↓ select (wrapping), Enter chooses, digits pick in the choice form, typing filters the list form
 * or edits the input, Esc answers undefined at once (as Pi's own selectors cancel, filter or not), PgUp/PgDn (and the
 * wheel) move through text that does not fit (the body, then the selected option's own rows) or else page the list,
 * Pi's tool-output key folds the options to the selected one so the whole text has the rows. It settles once;
 * afterwards every key and click is ignored.
 */
export class OwnerCard implements PresentedDialog {
  private readonly input: Input | undefined;
  private readonly opened = Date.now();
  private settled = false;
  private focus = false;
  /** The selected entry's index in `entries()`, -1 with none. */
  private selected = 0;
  private visible: readonly number[] = [];
  /** The rows of the last `entries()` call, which the selection's value is followed from. */
  private entries: readonly Entry[] | undefined;
  private optionOffset = 0;
  private bodyOffset = 0;
  private expanded = false;
  private seconds: number | undefined;
  // Facts of the last frame the keys act on.
  private bodyRows = 0;
  private bodyMax = 0;
  /** The body shows text lines and not all of them: the page keys and the wheel scroll it. */
  private scrollable = false;
  private expandable = false;
  private optionRows = 0;
  /** The selected option's rows scrolled out of a window too short for them; reset when the selection moves. */
  private label = { index: -1, offset: 0 };
  private labelMax = 0;
  private rowHits: ReadonlyMap<number, RowHit> = new Map();
  private hintRow = -1;
  private hintHits: readonly DialogHintHit[] = [];
  private inputX = 0;

  constructor(private readonly spec: CardSpec) {
    this.seconds = spec.seconds;
    if (spec.form !== "choice") {
      this.input = new Input({ prompt: PANEL_BLUE + "›" + PANEL_TEXT + " ", placeholder: spec.placeholder, placeholderStyle: muted });
      if (spec.filter) this.input.setValue(spec.filter);
    }
    if (spec.initial) {
      const entries = spec.entries();
      if (entries.length) this.selected = Math.max(0, Math.min(entries.length - 1, Math.trunc(spec.initial(entries)) || 0));
    }
    this.current();
  }

  get focused(): boolean { return this.focus; }
  set focused(value: boolean) {
    this.focus = value;
    if (this.input) this.input.focused = value;
  }
  /** While its text does not fit, the page keys scroll the card's text instead of the transcript. */
  get capturesViewportInput(): boolean { return this.scrollable || this.labelMax > 0; }

  setCountdown(seconds: number): void {
    this.seconds = seconds;
    this.spec.changed();
  }

  dispose(): void { this.settled = true; }

  invalidate(): void { this.input?.invalidate(); }

  handleInput(data: string): void {
    if (this.settled) return;
    this.key(data);
    this.spec.changed();
  }

  /**
   * Tells the spec the owner moved the selection away from `before` by navigating (keys, wheel, click). A filter
   * change or a refreshed list moves it too, but is not navigation, so nothing is previewed for it.
   */
  private moved(before: number): void {
    if (!this.settled && this.selected !== before) this.spec.moved?.(this.selected >= 0 ? this.selected : undefined);
  }

  private key(data: string): void {
    const spec = this.spec, keys = getKeybindings(), input = this.input;
    this.current();
    if (spec.form === "choice" && data.startsWith(PASTE_START)) return;
    if (keys.matches(data, "tui.select.cancel")) {
      this.settle(() => spec.cancel());
      return;
    }
    if (piPressed(data, "app.tools.expand")) {
      spec.expandTools?.();
      if (this.expanded || this.expandable) { this.expanded = !this.expanded; this.bodyOffset = 0; }
      return;
    }
    const page = keys.matches(data, "tui.select.pageDown") ? 1 : keys.matches(data, "tui.select.pageUp") ? -1 : 0;
    if (page) {
      if (!this.scroll(page, true) && spec.form !== "input") this.move(page * Math.max(1, this.optionRows - 1), false);
      return;
    }
    const submit = keys.matches(data, spec.form === "input" ? "tui.input.submit" : "tui.select.confirm") || data === "\n";
    if (submit) {
      if (!this.armed()) return;
      if (spec.form === "input") { const text = input?.getValue() ?? ""; this.settle(() => spec.submit(text)); }
      else this.activate(this.selected);
      return;
    }
    if (spec.form !== "input") {
      if (keys.matches(data, "tui.select.up") || (spec.form === "choice" && data === "k")) { this.move(-1, true); return; }
      if (keys.matches(data, "tui.select.down") || (spec.form === "choice" && data === "j")) { this.move(1, true); return; }
      if (spec.key?.(data, this.selected >= 0 ? this.selected : undefined, input?.getValue() ?? "")) { this.current(); return; }
    }
    if (spec.form === "choice") {
      // Every digit is the card's: one that names no option does nothing.
      const ch = decodeKittyPrintable(data) ?? data;
      if (/^[1-9]$/.test(ch) && this.armed()) this.activate(Number(ch) - 1);
      return;
    }
    const before = input?.getValue();
    input?.handleInput(data);
    if (spec.form === "list" && input?.getValue() !== before) this.refilter();
  }

  /** The filter changed: the list starts again at its first match. */
  private refilter(): void {
    this.optionOffset = 0;
    this.selected = this.current()[0] ?? -1;
  }

  private armed(): boolean { return !this.spec.guard || Date.now() - this.opened >= CHOICE_GUARD_MS; }

  /** Entry indexes the filter keeps, in their order; the selection stays on one of them. */
  private current(): readonly number[] {
    const entries = this.spec.entries();
    // New rows (a refreshed list): the selection stays on the value it had, wherever that row now is.
    const was = this.entries?.[this.selected]?.value;
    if (entries !== this.entries && was !== undefined && entries[this.selected]?.value !== was) {
      const at = entries.findIndex(entry => entry.value === was);
      if (at >= 0) this.selected = at;
    }
    this.entries = entries;
    const terms = this.spec.form === "list" ? (this.input?.getValue() ?? "").toLowerCase().split(/\s+/).filter(Boolean) : [];
    const visible: number[] = [];
    entries.forEach((entry, index) => {
      const match = entry.match.toLowerCase();
      if (terms.every(term => match.includes(term))) visible.push(index);
    });
    if (!visible.includes(this.selected)) this.selected = visible[0] ?? -1;
    this.visible = visible;
    return visible;
  }

  /** The owner's navigation: ↑↓ (wrapping), a page of the list, the wheel. */
  private move(delta: number, wrap: boolean): void {
    const visible = this.visible, count = visible.length, before = this.selected;
    if (!count) return;
    const at = Math.max(0, visible.indexOf(this.selected));
    const next = wrap ? (at + delta + count) % count : Math.max(0, Math.min(count - 1, at + delta));
    this.selected = visible[next]!;
    this.moved(before);
  }

  /**
   * Moves through text that does not fit: down through the body to its end, then through the selected option's
   * own rows; up in reverse. A page moves a window less two rows (one for an option), the wheel one row. False when
   * nothing is cut off, so the keys may page the list instead; never chooses or changes the selection.
   */
  private scroll(delta: number, page: boolean): boolean {
    const bodyStep = page ? Math.max(1, this.bodyRows - 2) : 1, labelStep = page ? Math.max(1, this.optionRows - 1) : 1;
    if (delta > 0 && this.scrollable && this.bodyOffset < this.bodyMax) this.bodyOffset = Math.min(this.bodyMax, this.bodyOffset + bodyStep);
    else if (delta > 0 && this.labelMax > 0) this.label.offset = Math.min(this.labelMax, this.label.offset + labelStep);
    else if (delta < 0 && this.labelMax > 0 && this.label.offset > 0) this.label.offset = Math.max(0, this.label.offset - labelStep);
    else if (delta < 0 && this.scrollable) this.bodyOffset = Math.max(0, this.bodyOffset - bodyStep);
    return this.scrollable || this.labelMax > 0;
  }

  /** Chooses a shown, enabled entry; anything else does nothing. */
  private activate(index: number): void {
    const entry = this.spec.entries()[index];
    if (!entry || entry.disabled !== undefined || !this.visible.includes(index)) return;
    if (this.spec.act?.(index)) return;
    this.settle(() => this.spec.choose(index));
  }

  private settle(answer: () => void): void {
    if (this.settled) return;
    this.settled = true;
    answer();
  }

  render(width: number): string[] {
    this.rowHits = new Map(); this.hintRow = -1; this.hintHits = [];
    const geometry = cardGeometry(width);
    if (!geometry) return [];
    const visible = this.current();
    const frame = cardFrame(this.spec, {
      selected: this.selected, visible, expanded: this.expanded, seconds: this.seconds,
      bodyOffset: this.bodyOffset, optionOffset: this.optionOffset, label: this.label,
    }, width, geometry, this.input);
    this.bodyOffset = frame.bodyOffset; this.bodyRows = frame.bodyRows; this.bodyMax = frame.bodyMax;
    this.scrollable = frame.scrollable; this.expandable = frame.expandable;
    this.optionOffset = frame.optionOffset; this.optionRows = frame.optionRows;
    this.label = { ...frame.label }; this.labelMax = frame.labelMax;
    this.rowHits = frame.rowHits; this.hintRow = frame.hintRow; this.hintHits = frame.hintHits; this.inputX = frame.inputX;
    return frame.lines;
  }

  /**
   * A click on an option selects it, a click on the selected one chooses it (after the guard); a click on a hint
   * replays its key; the input row places the caret; the wheel scrolls text that does not fit, else the selection.
   */
  handleMouse(event: TuiMouseEvent): TuiMouseEventResult | undefined {
    if (this.settled) return { handled: true };
    if (event.type === "wheel") {
      const delta = Math.sign(event.wheelDelta ?? 0);
      if (delta && !this.scroll(delta, false) && this.spec.form !== "input") this.move(delta, false);
      this.spec.changed();
      return { handled: true };
    }
    if (event.y === this.hintRow) {
      const hit = this.hintHits.find(item => event.x >= item.from && event.x < item.to);
      if (hit && event.type === "click" && event.button === "left") this.handleInput(hit.data);
      return { handled: true };
    }
    const row = this.rowHits.get(event.y);
    if (row?.kind === "input") {
      // The caret moves; the keys stay with the card, which forwards typing to its input.
      this.input?.handleMouse({ ...event, x: event.x - this.inputX, y: 0 });
      this.spec.changed();
      return { handled: true };
    }
    if (row?.kind === "option" && event.type === "click" && event.button === "left") {
      if (row.index !== this.selected) { const before = this.selected; this.selected = row.index; this.moved(before); }
      else if (this.armed()) this.activate(row.index);
      this.spec.changed();
    }
    return { handled: true };
  }
}
