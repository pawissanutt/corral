import { Input, matchesKey, truncateToWidth, visibleWidth, type Component, type Focusable, type TuiMouseEvent, type TuiMouseEventResult } from "@earendil-works/pi-tui";
import type { Theme } from "@earendil-works/pi-coding-agent";
import { keyCap } from "./header.ts";
import { pad, plainLabel, PANEL_BG, PANEL_SELECTED, RESET } from "./panels-view.ts";

export interface HostAction {
  readonly id: string;
  readonly label: string;
  readonly shortcut: string;
  readonly scope: string;
  /** Evaluated again after the overlay closes; presentation is not an authorization grant. */
  readonly disabled: () => string | undefined;
  readonly run: () => void | Promise<void>;
}

/**
 * Only explicit host handlers, never strings sent to an editor, shell, model or CLI. A filter row
 * (`› query`, with the session scope on the right), then one row per action: label, the reason it is
 * unavailable, and its key cap. Words match the label, id, scope, shortcut and the unavailable reason.
 */
export class ActionPalette implements Component, Focusable {
  private readonly input: Input;
  private selected = 0;
  private start = 0;
  get focused(): boolean { return this.input.focused; }
  set focused(value: boolean) { this.input.focused = value; }
  constructor(private readonly actions: readonly HostAction[], private readonly theme: Pick<Theme, "fg" | "bold">,
    private readonly done: (action: HostAction | undefined) => void, private readonly changed: () => void,
    private readonly capacity = 8) {
    this.input = new Input({ prompt: theme.fg("accent", "›") + " ", placeholder: "type to filter actions",
      placeholderStyle: text => theme.fg("muted", text) });
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
    if (event.type !== "click" || event.button !== "left" || event.shift || event.alt || event.ctrl || event.y < 1) return { handled: true };
    const index = this.start + event.y - 1;
    if (index === this.selected) this.choose(this.matches[index]);
    else if (index < this.matches.length) { this.selected = index; this.changed(); }
    return { handled: true };
  }
  private choose(action: HostAction | undefined): void {
    if (action && !action.disabled()) this.done(action);
  }
  render(width: number): string[] {
    const room = Math.max(0, width);
    const actions = this.matches;
    const height = Math.max(1, this.capacity);
    this.selected = Math.min(this.selected, Math.max(0, actions.length - 1));
    if (this.selected < this.start) this.start = this.selected;
    if (this.selected >= this.start + height) this.start = this.selected - height + 1;
    this.start = Math.max(0, Math.min(this.start, actions.length - height));
    const scope = plainLabel(this.actions[0]?.scope ?? "").replace(/\s*·\s*$/, "");
    const scopeWidth = scope && room >= visibleWidth(scope) + 24 ? visibleWidth(scope) : 0;
    const field = this.input.render(Math.max(1, room - (scopeWidth ? scopeWidth + 2 : 0)))[0] ?? "";
    const lines = [truncateToWidth(field + (scopeWidth ? "  " + this.theme.fg("muted", scope) : ""), room, "…")];
    for (const [offset, action] of actions.slice(this.start, this.start + height).entries()) {
      const on = this.start + offset === this.selected;
      const reason = action.disabled();
      const bg = on ? PANEL_SELECTED : PANEL_BG;
      const shortcut = plainLabel(action.shortcut);
      const cap = shortcut ? keyCap(shortcut, { restore: bg }) : "";
      const capWidth = shortcut ? visibleWidth(shortcut) + 2 : 0;
      const label = this.theme.fg(reason ? "muted" : "text", plainLabel(action.label))
        + (reason ? " " + this.theme.fg("warning", `· unavailable: ${plainLabel(reason)}`) : "");
      const bar = on ? this.theme.fg("accent", "▌") : " ";
      const body = room >= capWidth + 12 ? pad(bar + " " + label, room - capWidth - 1) + " " + cap : pad(bar + " " + label, room);
      lines.push(bg + pad(body, room).replaceAll(RESET, RESET + bg) + RESET);
    }
    if (!actions.length) lines.push(truncateToWidth("  " + this.theme.fg("muted", "no action matches"), room, "…"));
    return lines;
  }
  invalidate(): void { this.input.invalidate(); }
}
