import { Input, matchesKey, visibleWidth, type Component, type Focusable, type TuiMouseEvent, type TuiMouseEventResult } from "@earendil-works/pi-tui";
import { keyCap, popup } from "./frame.ts";
import { pad, plainLabel, PANEL_BLUE, PANEL_MUTED, PANEL_TEXT, PANEL_WARNING } from "./panels-view.ts";

export interface HostAction {
  readonly id: string;
  readonly label: string;
  readonly shortcut: string;
  readonly scope: string;
  /** Evaluated again after the overlay closes; presentation is not an authorization grant. */
  readonly disabled: () => string | undefined;
  readonly run: () => void | Promise<void>;
}
/** Rows above the list: the top border, the filter row and its divider. */
const HEAD_ROWS = 3;

/**
 * Only explicit host handlers, never strings sent to an editor, shell, model or CLI. A popup on the raised
 * surface: the filter row (`› query`, with the session scope on the right) over a divider, then one row per
 * action: label, the reason it is unavailable, and its key cap. Words match the label, id, scope, shortcut
 * and the unavailable reason.
 */
export class ActionPalette implements Component, Focusable {
  private readonly input: Input;
  private selected = 0;
  private start = 0;
  get focused(): boolean { return this.input.focused; }
  set focused(value: boolean) { this.input.focused = value; }
  constructor(private readonly actions: readonly HostAction[], private readonly done: (action: HostAction | undefined) => void,
    private readonly changed: () => void, private readonly capacity = 8) {
    this.input = new Input({ prompt: PANEL_BLUE + "›" + PANEL_TEXT + " ", placeholder: "type to filter actions",
      placeholderStyle: text => PANEL_MUTED + text + PANEL_TEXT });
  }
  get matches(): readonly HostAction[] {
    const words = this.input.getValue().toLowerCase().split(/\s+/).filter(Boolean);
    return this.actions.filter(a => words.every(word => `${a.label} ${a.id} ${a.scope} ${a.shortcut} ${a.disabled() ?? ""}`.toLowerCase().includes(word)));
  }
  handleInput(data: string): void {
    if (matchesKey(data, "escape")) { this.done(undefined); return; }
    const actions = this.matches;
    if (matchesKey(data, "enter")) { this.choose(actions[this.selected]); return; }
    if (matchesKey(data, "up")) this.selected = Math.max(0, this.selected - 1);
    else if (matchesKey(data, "down")) this.selected = Math.min(Math.max(0, actions.length - 1), this.selected + 1);
    else { this.input.handleInput(data); this.selected = 0; }
    this.changed();
  }
  handleMouse(event: TuiMouseEvent): TuiMouseEventResult | undefined {
    if (event.type !== "click" || event.button !== "left" || event.shift || event.alt || event.ctrl || event.y < HEAD_ROWS) return { handled: true };
    const index = this.start + event.y - HEAD_ROWS;
    if (index === this.selected) this.choose(this.matches[index]);
    else if (index < this.matches.length) { this.selected = index; this.changed(); }
    return { handled: true };
  }
  private choose(action: HostAction | undefined): void {
    if (action && !action.disabled()) this.done(action);
  }
  render(width: number): string[] {
    const inner = Math.max(0, width - 3);
    const actions = this.matches;
    const height = Math.max(1, this.capacity);
    this.selected = Math.min(this.selected, Math.max(0, actions.length - 1));
    if (this.selected < this.start) this.start = this.selected;
    if (this.selected >= this.start + height) this.start = this.selected - height + 1;
    this.start = Math.max(0, Math.min(this.start, actions.length - height));
    const scope = plainLabel(this.actions[0]?.scope ?? "").replace(/\s*·\s*$/, "");
    const scopeWidth = scope && inner >= visibleWidth(scope) + 24 ? visibleWidth(scope) : 0;
    const field = this.input.render(Math.max(1, inner - 1 - (scopeWidth ? scopeWidth + 1 : 0)))[0] ?? "";
    const rows = actions.slice(this.start, this.start + height).map(action => {
      const reason = action.disabled();
      const shortcut = plainLabel(action.shortcut);
      const capWidth = shortcut ? visibleWidth(shortcut) + 2 : 0;
      const label = (reason ? PANEL_MUTED : PANEL_TEXT) + plainLabel(action.label)
        + (reason ? " " + PANEL_WARNING + `· unavailable: ${plainLabel(reason)}` : "") + PANEL_TEXT;
      return shortcut && inner >= capWidth + 12 ? pad(label, inner - capWidth - 1) + " " + keyCap(shortcut) : label;
    });
    if (!actions.length) rows.push(PANEL_MUTED + "no action matches");
    return popup(field, scopeWidth ? PANEL_MUTED + scope : "", rows, width, actions.length ? this.selected - this.start : undefined);
  }
  invalidate(): void { this.input.invalidate(); }
}
