// The owner decision card: every Pi `select`, `confirm` and `input` in rich panels, drawn in the composer's place as
// the docked choice's accent card, with its keys in one hint row under it. Pi keeps the promise, abort signal,
// countdown timer, blocked status, dialog stack and the `Yes` comparison; the card only presents what Pi passes and
// answers with an offered option, the typed text, or undefined. Its list form is reused by the SDK pickers.
// The card itself is owner-card.ts (controller) and owner-card-view.ts (frame); its pure parts owner-dialog-parts.ts.
import type { DialogPresentation, PresentedDialog, SelectorComponent } from "@earendil-works/pi-coding-agent";
import { choiceRows } from "./build/choice-dock.ts";
import { OwnerCard } from "./owner-card.ts";
import { optionLines, textLines, type CardSpec, type DialogHint, type Entry } from "./owner-dialog-parts.ts";
import { plainLabel } from "./panels-view.ts";

/** What the card reads from the panels for its row budget, and how it asks for a redraw. */
export interface OwnerDialogHost {
  /** Terminal rows. */
  rows(): number;
  /** Rows the rest of the screen keeps while the card holds the editor slot: everything but the card and its hint row. */
  chromeRows(rows: number): number;
  /** The card changed outside a delivered key (a countdown tick, a click): redraw. */
  changed(): void;
}

/** One row of the list form: `match` is what the filter searches; a `disabled` item is shown but never chosen. */
export interface OwnerListItem<T> {
  readonly value: T;
  readonly match: string;
  /** The item's rows at `width` cells (the card's content width); the card adds the selection bar and surface. */
  lines(width: number, selected: boolean): readonly string[];
  readonly disabled?: string;
}
export interface OwnerListOptions<T> {
  /** The question; lines after the first are body text above the list. */
  readonly title: string;
  /** A muted note at the right of the header. */
  readonly right?: () => string;
  /** Body lines after the title's, read at each render (a picker's refresh status); fitted and scrolled as the body. */
  readonly body?: () => readonly string[];
  /** The filter row's placeholder (never a value). */
  readonly placeholder: string;
  /** The filter's text when the card opens, as if typed (Pi's initial search). */
  readonly filter?: string;
  items(): readonly OwnerListItem<T>[];
  /**
   * The index in `items` selected when the card opens (Pi's current value); clamped. When the opening filter hides
   * it, the first match is selected. Nothing is chosen or previewed by it.
   */
  initial?(items: readonly OwnerListItem<T>[]): number;
  /** Extra keys before the filter sees them (scope toggle, save as default), with the filter's text; true = handled. */
  key?(data: string, selected: T | undefined, filter: string): boolean;
  /** Hints for the keys `key` handles, after choose, cancel and select; each replays its data through the card. */
  hints?(selected: T | undefined): readonly DialogHint[];
  /** What Enter and Esc do, as the hint row names them; `choose` and `cancel` when absent. */
  readonly enter?: string;
  readonly escape?: string;
  /**
   * Enter (or a click on the selected item) acting in place: true when it handled the item and the card stays open
   * (a settings value cycles); otherwise `done` answers with it.
   */
  act?(value: T): boolean;
  /** The owner navigated the selection (↑↓, a page, the wheel, a click); never on open, a filter change or an answer. */
  moved?(selected: T | undefined): void;
  /** Called once: the chosen item's value, or undefined when the owner cancelled. */
  done(value: T | undefined): void;
}

/** Digits pick options directly up to this many; a longer list is filtered by typing instead. */
const DIGITS = 9;

/** The card's row budget from the panels (its hint row included) and its redraw. */
function hosted(host: OwnerDialogHost): Pick<CardSpec, "budget" | "changed"> {
  return {
    budget: () => { const rows = host.rows(); return Math.max(2, choiceRows(rows, host.chromeRows(rows))); },
    changed: () => host.changed(),
  };
}

/** The rich host's dialogs: one accent card for Pi's `select` and `confirm` (options) and `input` (a text row). */
export function ownerDialogPresentation(host: OwnerDialogHost): DialogPresentation {
  return {
    select: (request, done): PresentedDialog => {
      const lines = textLines(request.title);
      const offered = request.options, list = offered.length > DIGITS;
      const entries: Entry[] = offered.map((label, index) => ({
        match: plainLabel(label),
        lines: (width, selected) => optionLines(label, list ? undefined : index + 1, width, selected),
      }));
      return new OwnerCard({
        ...hosted(host),
        question: lines[0] ?? "",
        body: [...lines.slice(1), ...(request.message === undefined ? [] : textLines(request.message))],
        form: list ? "list" : "choice", placeholder: "type to filter",
        seconds: request.timeoutMs === undefined ? undefined : request.timeoutMs / 1000,
        // Esc answers undefined: a confirm reads it as No; what it means to a select is the caller's.
        enter: "choose", escape: request.kind === "confirm" ? "no" : "cancel", guard: true,
        entries: () => entries,
        expandTools: () => request.toggleToolsExpanded(),
        choose: index => done(offered[index]),
        submit: () => {},
        cancel: () => done(undefined),
      });
    },
    input: (request, done): PresentedDialog => {
      const lines = textLines(request.title);
      return new OwnerCard({
        ...hosted(host),
        question: lines[0] ?? "", body: lines.slice(1),
        form: "input", placeholder: plainLabel(request.placeholder ?? ""),
        seconds: request.timeoutMs === undefined ? undefined : request.timeoutMs / 1000,
        enter: "use", escape: "cancel", guard: true,
        entries: () => [],
        choose: () => {},
        submit: text => done(text),
        cancel: () => done(undefined),
      });
    },
  };
}

/** The card's filterable, scrollable list form for the SDK's pickers (model, effort, settings options). */
export function ownerListCard<T>(options: OwnerListOptions<T>, host: OwnerDialogHost): SelectorComponent {
  const lines = textLines(options.title);
  let items: readonly OwnerListItem<T>[] = [];
  const value = (index: number | undefined) => index === undefined ? undefined : items[index]?.value;
  return new OwnerCard({
    ...hosted(host),
    question: lines[0] ?? "", body: lines.slice(1),
    ...(options.body ? { more: () => options.body?.() ?? [] } : {}),
    form: "list", placeholder: options.placeholder, seconds: undefined, lead: true,
    ...(options.filter ? { filter: options.filter } : {}),
    // `entries()` has just stored the items `initial` reads.
    ...(options.initial ? { initial: () => options.initial?.(items) ?? 0 } : {}),
    // The owner opened the picker: no guard against keys in flight.
    enter: options.enter ?? "choose", escape: options.escape ?? "cancel", guard: false,
    entries: () => (items = options.items()),
    ...(options.right ? { right: options.right } : {}),
    key: (data, index, filter) => options.key?.(data, value(index), filter) ?? false,
    hints: index => options.hints?.(value(index)) ?? [],
    ...(options.act ? { act: (index: number) => { const item = items[index]; return item !== undefined && (options.act?.(item.value) ?? false); } } : {}),
    ...(options.moved ? { moved: (index: number | undefined) => options.moved?.(value(index)) } : {}),
    choose: index => options.done(value(index)),
    submit: () => {},
    cancel: () => options.done(undefined),
  });
}
