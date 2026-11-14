import { Input, matchesKey, visibleWidth, type Component, type Focusable, type TuiMouseEvent, type TuiMouseEventResult } from "@earendil-works/pi-tui";
import { keyCap, popup } from "./frame.ts";
import { pad, plainLabel, PANEL_BLUE, PANEL_HEAD, PANEL_MUTED, PANEL_TEXT, PANEL_WARNING } from "./panels-view.ts";

export type ActionGroup = "views" | "inspect" | "message" | "model" | "workspace" | "display" | "help";
/** Group titles in their fixed order; a group shows only while it has rows. */
const GROUP_TITLES: Readonly<Record<ActionGroup, string>> = {
  views: "Views", inspect: "Inspect", message: "Message", model: "Model", workspace: "Workspace", display: "Display", help: "Help",
};
const GROUP_ORDER = Object.keys(GROUP_TITLES) as ActionGroup[];

export interface HostAction {
  readonly id: string;
  readonly label: string;
  readonly shortcut: string;
  readonly scope: string;
  readonly group: ActionGroup;
  /** The setting value in force (Display): marked `current` at the right of its row. */
  readonly current?: boolean;
  /** Evaluated again after the overlay closes; presentation is not an authorization grant. */
  readonly disabled: () => string | undefined;
  readonly run: () => void | Promise<void>;
}
/** Rows above the list: the top border, the filter row and its divider. */
const HEAD_ROWS = 3;
/** The current-value marker; its cells at the right of a row. */
const CURRENT = "current";
/** Label cells a row keeps before its right-hand marker and cap are dropped. */
const LABEL_MIN = 12;

type Line = { readonly kind: "group"; readonly title: string } | { readonly kind: "action"; readonly action: HostAction; readonly index: number };

/**
 * Only explicit host handlers, never strings sent to an editor, shell, model or CLI. A popup on the raised
 * surface: the filter row (`› query`, with the session scope on the right) over a divider, then the actions under
 * their group titles (bold in the head colour, never selectable): label, the reason it is unavailable, the
 * `current` marker of a setting in force and its key cap. Words match the group, label, id, scope, shortcut and
 * the unavailable reason; a group title stays above its matching rows. Group titles take window rows like any
 * other line: ↑↓ move through the actions (wrapping) and bring the selected row into the window with its group
 * title when both fit, the wheel scrolls the window, and a click selects a row and a second click chooses it.
 */
export class ActionPalette implements Component, Focusable {
  private readonly input: Input;
  /** Index into `matches`. */
  private selected = 0;
  /** First line of the window, group titles included. */
  private start = 0;
  get focused(): boolean { return this.input.focused; }
  set focused(value: boolean) { this.input.focused = value; }
  constructor(private readonly actions: readonly HostAction[], private readonly done: (action: HostAction | undefined) => void,
    private readonly changed: () => void, private readonly capacity = 8) {
    this.input = new Input({ prompt: PANEL_BLUE + "›" + PANEL_TEXT + " ", placeholder: "type to filter actions",
      placeholderStyle: text => PANEL_MUTED + text + PANEL_TEXT });
  }
  /** The matching actions in drawn order: by group, then as given. */
  get matches(): readonly HostAction[] {
    const words = this.input.getValue().toLowerCase().split(/\s+/).filter(Boolean);
    const found = this.actions.filter(a => words.every(word =>
      `${GROUP_TITLES[a.group]} ${a.label} ${a.id} ${a.scope} ${a.shortcut} ${a.disabled() ?? ""}`.toLowerCase().includes(word)));
    return GROUP_ORDER.flatMap(group => found.filter(a => a.group === group));
  }
  private get height(): number { return Math.max(1, this.capacity); }
  private layout(actions: readonly HostAction[]): Line[] {
    const lines: Line[] = [];
    actions.forEach((action, index) => {
      if (actions[index - 1]?.group !== action.group) lines.push({ kind: "group", title: GROUP_TITLES[action.group] });
      lines.push({ kind: "action", action, index });
    });
    return lines;
  }
  private lineOf(lines: readonly Line[], index: number): number {
    return lines.findIndex(line => line.kind === "action" && line.index === index);
  }
  private shown(lines: readonly Line[], line: number): boolean { return line >= this.start && line < this.start + this.height; }
  private clamp(lines: readonly Line[]): void { this.start = Math.max(0, Math.min(this.start, lines.length - this.height)); }
  /** Brings the selected row into the window, with its group title when both fit. */
  private reveal(lines: readonly Line[]): void {
    const at = this.lineOf(lines, this.selected);
    if (at < 0) { this.start = 0; return; }
    if (at < this.start) this.start = at;
    else if (at >= this.start + this.height) this.start = at - this.height + 1;
    let title = at;
    while (title > 0 && lines[title]!.kind !== "group") title--;
    if (title < this.start && at - title < this.height) this.start = title;
    this.clamp(lines);
  }
  /** Moves the window; a selection it leaves moves to the nearest action still in it, when there is one. */
  private scroll(delta: number): void {
    const lines = this.layout(this.matches);
    this.start += delta;
    this.clamp(lines);
    if (this.shown(lines, this.lineOf(lines, this.selected))) return;
    const inside = lines.filter((line, at): line is Extract<Line, { kind: "action" }> => line.kind === "action" && this.shown(lines, at));
    const nearest = delta > 0 ? inside[0] : inside.at(-1);
    if (nearest) this.selected = nearest.index;
  }
  handleInput(data: string): void {
    if (matchesKey(data, "escape")) { this.done(undefined); return; }
    const actions = this.matches, lines = this.layout(actions), count = actions.length;
    if (matchesKey(data, "enter")) {
      if (this.shown(lines, this.lineOf(lines, this.selected))) this.choose(actions[this.selected]);
      return;
    }
    if (matchesKey(data, "up")) { this.selected = count ? (this.selected - 1 + count) % count : 0; this.reveal(lines); }
    else if (matchesKey(data, "down")) { this.selected = count ? (this.selected + 1) % count : 0; this.reveal(lines); }
    else { this.input.handleInput(data); this.selected = 0; this.start = 0; this.reveal(this.layout(this.matches)); }
    this.changed();
  }
  handleMouse(event: TuiMouseEvent): TuiMouseEventResult | undefined {
    if (event.type === "wheel") {
      if (event.wheelDelta) { this.scroll(event.wheelDelta); this.changed(); }
      return { handled: true };
    }
    if (event.type !== "click" || event.button !== "left" || event.shift || event.alt || event.ctrl) return { handled: true };
    const row = event.y - HEAD_ROWS;
    if (row < 0 || row >= this.height) return { handled: true };
    const line = this.layout(this.matches)[this.start + row];
    if (line?.kind !== "action") return { handled: true };
    if (line.index === this.selected) this.choose(line.action);
    else { this.selected = line.index; this.changed(); }
    return { handled: true };
  }
  private choose(action: HostAction | undefined): void {
    if (action && !action.disabled()) this.done(action);
  }
  /**
   * One action row, `inner` cells: the label (muted with its reason in warning when unavailable), then `current` and
   * the key. A cap is drawn only for an action that can run; an unavailable one keeps its key as muted text in the
   * cap's cells, so the key column stays aligned.
   */
  private row(action: HostAction, inner: number): string {
    const reason = action.disabled();
    const shortcut = plainLabel(action.shortcut);
    const label = (reason ? PANEL_MUTED : PANEL_TEXT) + plainLabel(action.label)
      + (reason ? " " + PANEL_WARNING + `· unavailable: ${plainLabel(reason)}` : "") + PANEL_TEXT;
    const rightWidth = (action.current ? visibleWidth(CURRENT) : 0) + (action.current && shortcut ? 1 : 0)
      + (shortcut ? visibleWidth(shortcut) + 2 : 0);
    if (!rightWidth) return label;
    if (inner < rightWidth + 1 + LABEL_MIN) return label + (action.current ? " " + PANEL_BLUE + `· ${CURRENT}` + PANEL_TEXT : "");
    const right = (action.current ? PANEL_BLUE + CURRENT + PANEL_TEXT : "") + (action.current && shortcut ? " " : "")
      + (!shortcut ? "" : reason ? PANEL_MUTED + ` ${shortcut} ` + PANEL_TEXT : keyCap(shortcut));
    return pad(label, inner - rightWidth - 1) + " " + right;
  }
  render(width: number): string[] {
    const inner = Math.max(0, width - 3);
    const actions = this.matches, lines = this.layout(actions);
    const last = Math.max(0, actions.length - 1);
    if (this.selected > last) { this.selected = last; this.reveal(lines); } else this.clamp(lines);
    const scope = plainLabel(this.actions[0]?.scope ?? "").replace(/\s*·\s*$/, "");
    const scopeWidth = scope && inner >= visibleWidth(scope) + 24 ? visibleWidth(scope) : 0;
    const field = this.input.render(Math.max(1, inner - 1 - (scopeWidth ? scopeWidth + 1 : 0)))[0] ?? "";
    const rows = lines.slice(this.start, this.start + this.height).map(line => line.kind === "group"
      ? PANEL_HEAD + "\x1b[1m" + plainLabel(line.title) + "\x1b[22m" + PANEL_TEXT
      : this.row(line.action, inner));
    if (!actions.length) rows.push(PANEL_MUTED + "no action matches");
    const at = this.lineOf(lines, this.selected) - this.start;
    return popup(field, scopeWidth ? PANEL_MUTED + scope : "", rows, width, actions.length && at >= 0 && at < rows.length ? at : undefined);
  }
  invalidate(): void { this.input.invalidate(); }
}
