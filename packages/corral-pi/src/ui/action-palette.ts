import { Input, matchesKey, truncateToWidth, type Component, type Focusable } from "@earendil-works/pi-tui";
import type { Theme } from "@earendil-works/pi-coding-agent";

export interface HostAction {
  readonly id: string;
  readonly label: string;
  readonly shortcut: string;
  readonly scope: string;
  /** Evaluated again after the overlay closes; presentation is not an authorization grant. */
  readonly disabled: () => string | undefined;
  readonly run: () => void | Promise<void>;
}

/** Only explicit host handlers, never strings sent to an editor, shell, model or CLI. */
export class ActionPalette implements Component, Focusable {
  private readonly input = new Input({ prompt: "Search: " });
  private selected = 0;
  get focused(): boolean { return this.input.focused; }
  set focused(value: boolean) { this.input.focused = value; }
  constructor(private readonly actions: readonly HostAction[], private readonly theme: Pick<Theme, "fg" | "bold">,
    private readonly done: (action: HostAction | undefined) => void, private readonly changed: () => void,
    private readonly capacity = 8) {}
  get matches(): readonly HostAction[] {
    const words = this.input.getValue().toLowerCase().split(/\s+/).filter(Boolean);
    return this.actions.filter(a => words.every(word => `${a.label} ${a.id} ${a.scope} ${a.shortcut} ${a.disabled() ?? ""}`.toLowerCase().includes(word)));
  }
  handleInput(data: string): void {
    if (matchesKey(data, "escape")) { this.done(undefined); return; }
    const actions = this.matches;
    if (matchesKey(data, "enter")) {
      const action = actions[this.selected];
      if (action && !action.disabled()) this.done(action);
      return;
    }
    if (matchesKey(data, "up")) this.selected = Math.max(0, this.selected - 1);
    else if (matchesKey(data, "down")) this.selected = Math.min(Math.max(0, actions.length - 1), this.selected + 1);
    else { this.input.handleInput(data); this.selected = 0; }
    this.changed();
  }
  render(width: number): string[] {
    const actions = this.matches;
    this.selected = Math.min(this.selected, Math.max(0, actions.length - 1));
    const start = Math.max(0, this.selected - this.capacity + 1);
    const lines = [this.theme.bold("Corral actions · session-local preferences"), ...this.input.render(width)];
    for (const [index, action] of actions.slice(start, start + this.capacity).entries()) {
      const reason = action.disabled();
      const label = `${start + index === this.selected ? ">" : " "} ${action.label} · ${action.shortcut || "no shortcut"} · ${action.scope}`;
      lines.push(this.theme.fg(reason ? "muted" : start + index === this.selected ? "accent" : "text", label));
      lines.push(this.theme.fg(reason ? "warning" : "muted", reason ? `  Disabled: ${reason}` : `  ${action.id}`));
    }
    if (!actions.length) lines.push("No matching actions");
    lines.push("↑/↓ choose · Enter select · Esc cancel");
    return lines.map(line => truncateToWidth(line, Math.max(0, width), "…"));
  }
  invalidate(): void { this.input.invalidate(); }
}
