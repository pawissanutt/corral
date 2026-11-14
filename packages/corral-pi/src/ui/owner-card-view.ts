// One frame of the owner card, drawn from its spec and the state its controller keeps: the accent card with the
// question and countdown in the header, the fitted body, the input or filter row and the option window, then the
// hint row. Returns the lines and the facts the controller's keys and pointer act on until the next frame.
import { visibleWidth, wrapTextWithAnsi } from "@earendil-works/pi-tui";
import type { Input } from "@earendil-works/pi-tui";
import { card } from "./frame.ts";
import { keyData, keyLabel, piKey } from "./keymap.ts";
import { BOLD, bodyView, clock, dialogHintRow, muted, NORMAL, trimBlank } from "./owner-dialog-parts.ts";
import type { CardSpec, DialogHint, DialogHintHit } from "./owner-dialog-parts.ts";
import { PANEL_BG, PANEL_MUTED, PANEL_TEXT, PANEL_WARNING, plainLabel } from "./panels-view.ts";

/** From this card width the countdown reads `answer in 4:58`, below it `4:58`. */
const LONG_COUNTDOWN = 70;
/** The countdown turns to the warning colour at this many seconds. */
const WARN_SECONDS = 30;
/** Below this terminal width the card drops its one-cell margins. */
const MARGIN_FROM = 12;

export type RowHit = { readonly kind: "option"; readonly index: number } | { readonly kind: "input" };
/** The selected option's rows scrolled out of a window too short for them; reset when the selection moves. */
export interface LabelScroll { readonly index: number; readonly offset: number }

/** What the controller keeps between frames. */
export interface CardState {
  /** The selected entry's index in `entries()`, -1 with none. */
  readonly selected: number;
  /** Entry indexes the filter keeps, in their order. */
  readonly visible: readonly number[];
  readonly expanded: boolean;
  readonly seconds: number | undefined;
  readonly bodyOffset: number;
  readonly optionOffset: number;
  readonly label: LabelScroll;
}

/** One frame's lines and the facts of it the keys and the pointer act on. */
export interface CardFrame {
  readonly lines: string[];
  readonly bodyOffset: number;
  readonly bodyRows: number;
  readonly bodyMax: number;
  /** The body shows text lines and not all of them: the page keys and the wheel scroll it. */
  readonly scrollable: boolean;
  readonly expandable: boolean;
  readonly optionOffset: number;
  readonly optionRows: number;
  readonly label: LabelScroll;
  readonly labelMax: number;
  readonly rowHits: ReadonlyMap<number, RowHit>;
  readonly hintRow: number;
  readonly hintHits: readonly DialogHintHit[];
  /** The input row's first cell, for the caret a click places. */
  readonly inputX: number;
}

/** The countdown while Pi runs a timeout, else the spec's note. */
function headerNote(spec: CardSpec, seconds: number | undefined, cardWidth: number): string | undefined {
  if (seconds !== undefined) {
    const time = clock(seconds);
    return (seconds <= WARN_SECONDS ? PANEL_WARNING : PANEL_MUTED) + (cardWidth >= LONG_COUNTDOWN ? `answer in ${time}` : time);
  }
  const note = spec.right?.();
  return note ? PANEL_MUTED + plainLabel(note) : undefined;
}

/** The hint row's keys: choose, cancel, select, the spec's own, then whole text and scrolling when they would act. */
function cardHints(spec: CardSpec, selected: number,
  facts: { readonly expandable: boolean; readonly expanded: boolean; readonly scrollable: boolean; readonly labelMax: number }): DialogHint[] {
  const out: DialogHint[] = [];
  const add = (binding: string | undefined, label: string, key?: string) => {
    const data = binding ? keyData(binding) : undefined;
    if (binding && data) out.push({ key: key ?? keyLabel(binding), label, data });
  };
  add(piKey(spec.form === "input" ? "tui.input.submit" : "tui.select.confirm"), spec.enter);
  add(piKey("tui.select.cancel"), spec.escape);
  if (spec.form !== "input") {
    const up = piKey("tui.select.up"), down = piKey("tui.select.down");
    if (up && down) add(down, "select", up === "up" && down === "down" ? "↑↓" : `${keyLabel(up)}/${keyLabel(down)}`);
    out.push(...(spec.hints?.(selected >= 0 ? selected : undefined) ?? []));
  }
  if (facts.expandable || facts.expanded) add(piKey("app.tools.expand"), facts.expanded ? "options" : "whole text");
  if (facts.scrollable || facts.labelMax > 0) {
    const up = piKey("tui.select.pageUp"), down = piKey("tui.select.pageDown");
    if (up && down) add(down, "scroll text", `${keyLabel(up)}/${keyLabel(down)}`);
  }
  return out;
}

/** The card's outer margin and width at `width` terminal cells, or undefined when it has no room to draw. */
export function cardGeometry(width: number): { readonly outer: number; readonly cardWidth: number } | undefined {
  const outer = width >= MARGIN_FROM ? 1 : 0, cardWidth = width - outer * 2;
  return cardWidth < 8 ? undefined : { outer, cardWidth };
}

/** The frame at `width` cells, in the `geometry` `cardGeometry` gave for it. */
export function cardFrame(spec: CardSpec, state: CardState, width: number,
  geometry: { readonly outer: number; readonly cardWidth: number }, input: Input | undefined): CardFrame {
  const { outer, cardWidth } = geometry;
  const inner = cardWidth - 4, selected = state.selected, visible = state.visible;
  const budget = spec.budget();
  const hintRows = budget >= 4 ? 1 : 0;

  // The question is the header; when it does not fit there it also opens the body, so none of it is lost.
  const right = headerNote(spec, state.seconds, cardWidth);
  const headerRoom = inner - (right ? Math.min(visibleWidth(right), Math.max(0, inner - 4)) + 1 : 0);
  const question = plainLabel(spec.question);
  const text = trimBlank([...(visibleWidth(question) > headerRoom ? [question] : []), ...spec.body, ...(spec.more?.() ?? [])]);
  const wrapped = text.flatMap(line => line ? wrapTextWithAnsi(line, inner) : [""]);

  const entries = spec.entries();
  const options: { readonly text: string; readonly index: number }[] = [];
  for (const index of visible) for (const row of entries[index]!.lines(inner, index === selected)) options.push({ text: row, index });
  if (spec.form !== "input" && !visible.length) {
    options.push({ text: muted(entries.length ? "no matches" : "nothing to choose"), index: -1 });
  }
  const first = options.findIndex(row => row.index === selected && row.index >= 0);
  let last = first;
  while (first >= 0 && options[last + 1]?.index === selected) last++;
  const selectedRows = first >= 0 ? last - first + 1 : Math.min(1, options.length);

  // Rows: header and the input row always; every option before borders while they all fit, else the list
  // scrolls and the borders stay; the body keeps one row while it has text, then takes what is left.
  let rest = budget - hintRows - 1 - (spec.form === "choice" ? 0 : 1);
  const want = state.expanded ? selectedRows : options.length, bodyMin = wrapped.length ? 1 : 0;
  const shown = want + bodyMin <= rest ? want : Math.min(want, Math.max(want ? 1 : 0, rest - bodyMin - 2));
  rest -= shown;
  let bodyRows = Math.min(bodyMin, Math.max(0, rest)); rest -= bodyRows;
  const bottom = rest >= 1; if (bottom) rest--;
  const top = rest >= 1; if (top) rest--;
  const more = Math.min(wrapped.length - bodyRows, Math.max(0, rest)); bodyRows += more; rest -= more;
  const spacer = bodyRows > 0 && (shown > 0 || spec.form !== "choice") && rest >= 1;

  const expandable = !state.expanded && (wrapped.length > bodyRows || selectedRows > shown) && options.length > selectedRows;
  // A one-row body says its line count (the whole text is one key away); a picker's shows its first row, which pages.
  const summary = expandable && !spec.lead;
  const view = bodyView(wrapped, bodyRows, state.bodyOffset, summary);
  const scrollable = bodyRows > 0 && wrapped.length > bodyRows && !(bodyRows === 1 && summary);

  // An option too long for the window shows a window of its own rows, which the page keys and the wheel move.
  const labelMax = first >= 0 ? Math.max(0, selectedRows - shown) : 0;
  const kept = state.label.index === selected ? state.label.offset : 0;
  const label = { index: selected, offset: Math.min(kept, labelMax) };

  let offset = state.optionOffset;
  if (labelMax > 0) offset = first + label.offset;
  else if (first >= 0) {
    if (first < offset) offset = first;
    if (last >= offset + shown) offset = Math.min(first, last - shown + 1);
  }
  offset = Math.max(0, Math.min(offset, options.length - shown));
  const window = options.slice(offset, offset + shown);

  const body: string[] = view.rows.map(row => PANEL_TEXT + row);
  if (spacer) body.push("");
  let inputAt = -1;
  if (input) {
    inputAt = body.length;
    const count = spec.form === "list" && input.getValue() ? `${visible.length} / ${entries.length}` : "";
    const room = inner - (count ? visibleWidth(count) + 1 : 0);
    body.push((room >= 3 ? input.render(room)[0] ?? "" : "") + (count ? " " + muted(count) : ""));
  }
  const optionsAt = body.length;
  body.push(...window.map(row => row.text));
  const from = window.findIndex(row => row.index === selected && row.index >= 0);
  let to = from;
  while (from >= 0 && window[to + 1]?.index === selected) to++;
  const framed = card(BOLD + question + NORMAL, body, cardWidth, {
    tone: "accent", ...(right ? { right } : {}),
    ...(from >= 0 ? { selected: { from: optionsAt + from, to: optionsAt + to } } : {}),
  });
  const lines = framed.slice(top ? 0 : 1, bottom ? framed.length : framed.length - 1);
  const bodyStart = top ? 2 : 1;
  const rowHits = new Map<number, RowHit>();
  window.forEach((row, at) => { if (row.index >= 0) rowHits.set(bodyStart + optionsAt + at, { kind: "option", index: row.index }); });
  if (inputAt >= 0) rowHits.set(bodyStart + inputAt, { kind: "input" });
  const out = outer ? lines.map(line => PANEL_BG + " " + line + PANEL_BG + " " + PANEL_TEXT) : lines;

  let hintRow = -1, hintHits: readonly DialogHintHit[] = [];
  if (hintRows) {
    // Where the list window stands, when it does not show every option, sits at the right of the keys.
    const firstShown = visible.indexOf(window.find(row => row.index >= 0)?.index ?? -1) + 1;
    const lastShown = visible.indexOf([...window].reverse().find(row => row.index >= 0)?.index ?? -1) + 1;
    const partial = firstShown > 0 && (firstShown > 1 || lastShown < visible.length);
    const position = partial ? `${firstShown === lastShown ? firstShown : `${firstShown}–${lastShown}`} / ${visible.length}` : "";
    const reserve = position && width - visibleWidth(position) - 1 >= 12 ? visibleWidth(position) + 1 : 0;
    const row = dialogHintRow(cardHints(spec, selected, { expandable, expanded: state.expanded, scrollable, labelMax }), width - reserve);
    hintRow = out.length; hintHits = row.hits;
    out.push(row.line + (reserve ? muted(position) + PANEL_BG + " " : ""));
  }
  return {
    lines: out, bodyOffset: view.offset, bodyRows, bodyMax: view.max, scrollable, expandable,
    optionOffset: offset, optionRows: shown, label, labelMax, rowHits, hintRow, hintHits, inputX: outer + 2,
  };
}
