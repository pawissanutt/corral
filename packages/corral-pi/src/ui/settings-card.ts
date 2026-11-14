// Pi's settings in the owner card's list form, in the composer's place: the top-level settings and Pi's nested lists
// (Warnings, Automatic Theme) alike, every item Pi passes with its live value and description. Pi keeps what a change
// does: a value cycles through the request's own `change`, an item's submenu is Pi's own component (drawn by the same
// hooks) hosted in the card's place until it calls back, and Esc is Pi's own close or back.
import type { SelectorComponent, SettingsSelectorRequest } from "@earendil-works/pi-coding-agent";
import { decodeKittyPrintable, isFocusable, visibleWidth } from "@earendil-works/pi-tui";
import type { Component, SettingItem, TuiMouseEvent, TuiMouseEventResult } from "@earendil-works/pi-tui";
import { ownerListCard, type OwnerDialogHost, type OwnerListItem } from "./owner-dialog.ts";
import { PANEL_BLUE, PANEL_TEXT, plainLabel } from "./panels-view.ts";
import { cell, detailRows, GAP } from "./picker-rows.ts";

/** The label column's cells (UID §14); a narrower card gives the value its room first. */
const LABEL_COLUMN = 28;
/** Fewest cells a value keeps before the label gives way. */
const VALUE_MIN = 6;
/** What opens Pi's submenu, after the value. */
const OPENS = " ›";

/** Pi's boolean values in words. */
function shownValue(item: SettingItem): string {
  const value = item.currentValue === "true" ? "on" : item.currentValue === "false" ? "off" : plainLabel(item.currentValue);
  return item.submenu ? value + OPENS : value;
}

/**
 * One setting: the label in its column, the value in accent after it; selected, the label bold and, under it, whatever
 * the row clipped and Pi's description, whole.
 */
function settingItem(item: SettingItem, column: number): OwnerListItem<string> {
  const label = plainLabel(item.label), description = plainLabel(item.description ?? "");
  return {
    value: item.id,
    match: label,
    lines: (width, selected) => {
      const value = shownValue(item);
      const labelWidth = Math.max(1, Math.min(column, width - GAP.length - Math.min(VALUE_MIN, visibleWidth(value))));
      const room = Math.max(0, width - labelWidth - GAP.length);
      const row = cell(label, labelWidth, selected) + GAP + PANEL_BLUE + cell(value, room) + PANEL_TEXT;
      if (!selected) return [row];
      const clipped = visibleWidth(label) > labelWidth || visibleWidth(value) > room;
      return [row, ...(clipped ? detailRows(`${label}: ${value}`, width, 2, PANEL_TEXT) : []), ...detailRows(description, width)];
    },
  };
}

/** The card, or the submenu Pi opened from it, which has the keys, focus and rows until it calls Pi's `done`. */
class SettingsCard implements SelectorComponent {
  private card: SelectorComponent;
  private submenu: Component | undefined;
  private live = true;
  private focus = false;

  constructor(private readonly request: SettingsSelectorRequest, private readonly host: OwnerDialogHost) {
    this.card = this.list(undefined);
  }

  /** The list card, selecting the item at `initial` (Pi's navigate-after-close) or the first. */
  private list(initial: number | undefined): SelectorComponent {
    const request = this.request, nested = request.title !== undefined;
    return ownerListCard<string>({
      title: [request.title ?? "Pi settings", ...(request.description ? [request.description] : [])].join("\n"),
      placeholder: "search settings",
      enter: "change", escape: nested ? "back" : "close",
      ...(initial === undefined ? {} : { initial: () => initial }),
      items: () => {
        const column = Math.min(LABEL_COLUMN, Math.max(1, ...request.items.map(item => visibleWidth(plainLabel(item.label)))));
        return request.items.map(item => settingItem(item, column));
      },
      // Space changes a value too, as in Pi's list, while no search is typed.
      key: (data, id, filter) => {
        if ((decodeKittyPrintable(data) ?? data) !== " " || filter || id === undefined) return false;
        this.activate(id);
        return true;
      },
      act: id => { this.activate(id); return true; },
      // Only Esc answers: every item acts in place.
      done: () => this.close(() => request.cancel()),
    }, this.host);
  }

  get focused(): boolean { return this.focus; }
  set focused(value: boolean) {
    this.focus = value;
    this.card.focused = value && !this.submenu;
    if (this.submenu && isFocusable(this.submenu)) this.submenu.focused = value;
  }
  get capturesViewportInput(): boolean {
    return (this.submenu ? this.submenu.capturesViewportInput : this.card.capturesViewportInput) ?? false;
  }

  invalidate(): void {
    this.card.invalidate();
    this.submenu?.invalidate();
  }

  render(width: number): string[] {
    return (this.submenu ?? this.card).render(width);
  }

  handleInput(data: string): void {
    if (!this.live) return;
    if (!this.submenu) { this.card.handleInput(data); return; }
    this.submenu.handleInput?.(data);
    this.host.changed();
  }

  handleMouse(event: TuiMouseEvent): TuiMouseEventResult | undefined {
    if (!this.live) return { handled: true };
    if (!this.submenu) return this.card.handleMouse?.(event);
    const result = this.submenu.handleMouse?.(event);
    this.host.changed();
    return result ?? { handled: true };
  }

  dispose(): void {
    this.live = false;
    this.card.dispose?.();
    const submenu = this.submenu;
    this.submenu = undefined;
    if (submenu && "dispose" in submenu && typeof submenu.dispose === "function") submenu.dispose();
  }

  private close(answer: () => void): void {
    if (!this.live) return;
    this.live = false;
    answer();
  }

  /** Enter on an item: Pi's submenu opens in the card's place, or the value moves to Pi's next one. */
  private activate(id: string): void {
    const item = this.request.items.find(candidate => candidate.id === id);
    if (!this.live || !item) return;
    if (item.submenu) {
      let opened: Component | undefined;
      opened = item.submenu(item.currentValue, (value, options) => this.closed(opened, item, value, options?.navigateTo));
      this.submenu = opened;
      this.focused = this.focus;
      return;
    }
    const values = item.values ?? [];
    const next = values[(values.indexOf(item.currentValue) + 1) % Math.max(1, values.length)];
    if (next !== undefined) this.request.change(item.id, next);
  }

  /** Pi's submenu called `done`: its value goes to Pi's `change`; the card returns on its item, or on `navigateTo`'s. */
  private closed(submenu: Component | undefined, item: SettingItem, value: string | undefined, navigateTo: string | undefined): void {
    if (!this.live || !submenu || this.submenu !== submenu) return;
    this.submenu = undefined;
    if (value !== undefined) this.request.change(item.id, value);
    if (!this.live) return;
    const target = navigateTo === undefined ? undefined : this.request.items.find(candidate => candidate.id === navigateTo);
    if (target) {
      this.card.dispose?.();
      this.card = this.list(this.request.items.indexOf(target));
    }
    this.focused = this.focus;
    if (target) this.activate(target.id);
    this.host.changed();
  }
}

/** Pi's settings, or one of its nested lists, in the owner card. */
export function settingsCard(request: SettingsSelectorRequest, host: OwnerDialogHost): SelectorComponent {
  return new SettingsCard(request, host);
}
