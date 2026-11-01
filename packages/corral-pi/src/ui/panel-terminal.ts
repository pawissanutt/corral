// Public Terminal adapter: Pi still owns every transcript, editor, dialog and input lifecycle.
// In fullscreen its renderer sees a reserved main width; the right rail is painted in the
// remaining physical cells. Normal-screen output and shutdown never receive a rail paint.
import type { ProgramStatus, Terminal } from "@earendil-works/pi-tui";

export const PANEL_BREAKPOINT = 105;
export const PANEL_WIDTH = 34;
const ENTER = "\x1b[?1049h";
const EXIT = "\x1b[?1049l";
const SYNC_END = "\x1b[?2026l";

export interface PanelSurface {
  readonly sidebarEnabled: boolean;
  renderSidebar(width: number, height: number): readonly string[];
  handleInput(data: string): boolean;
  layoutChanged(): void;
}

export class PanelTerminal implements Terminal {
  private active = false;
  private fullscreen = true;
  private started = false;
  private resizeHandler: (() => void) | undefined;
  private autowrap = false;

  constructor(readonly physical: Terminal, private readonly surface: PanelSurface) {}

  get sidebarVisible(): boolean {
    return this.fullscreen && this.surface.sidebarEnabled && this.physical.columns >= PANEL_BREAKPOINT;
  }
  get columns(): number { return this.physical.columns - (this.sidebarVisible ? PANEL_WIDTH + 1 : 0); }
  get rows(): number { return this.physical.rows; }
  get kittyProtocolActive(): boolean { return this.physical.kittyProtocolActive; }

  start(onInput: (data: string) => void, onResize: () => void): void {
    this.started = true;
    this.resizeHandler = onResize;
    this.physical.start(data => { if (!this.surface.handleInput(data)) onInput(data); }, () => {
      this.surface.layoutChanged();
      onResize();
    });
  }
  stop(): void {
    this.started = false;
    this.active = false;
    this.resizeHandler = undefined;
    this.physical.stop();
  }
  drainInput(maxMs?: number, idleMs?: number): Promise<void> { return this.physical.drainInput(maxMs, idleMs); }

  private rail(): string {
    if (!this.active || !this.sidebarVisible) return "";
    const column = this.columns + 2;
    const lines = this.surface.renderSidebar(PANEL_WIDTH, this.rows);
    let data = "\x1b7\x1b[?7l";
    for (let row = 0; row < this.rows; row++) {
      // Never erase a whole physical line: Pi's main screen is on the left.
      data += `\x1b[${row + 1};${column - 1}H\x1b[0m \x1b[${row + 1};${column}H${lines[row] ?? " ".repeat(PANEL_WIDTH)}`;
    }
    return data + "\x1b[0m\x1b8" + (this.autowrap ? "\x1b[?7h" : "");
  }
  write(data: string): void {
    const enter = data.lastIndexOf(ENTER), exit = data.lastIndexOf(EXIT);
    if (enter >= 0 || exit >= 0) this.active = this.fullscreen = enter > exit;
    const wrapOn = data.lastIndexOf("\x1b[?7h"), wrapOff = data.lastIndexOf("\x1b[?7l");
    if (wrapOn >= 0 || wrapOff >= 0) this.autowrap = wrapOn > wrapOff;
    const rail = this.rail();
    if (!rail) { this.physical.write(data); return; }
    // Keep Pi's differential frame and rail in the same synchronized terminal update.
    this.physical.write(data.endsWith(SYNC_END)
      ? data.slice(0, -SYNC_END.length) + rail + SYNC_END
      : data + "\x1b[?2026h" + rail + SYNC_END);
  }
  redrawSidebar(): void {
    if (this.started && this.active) {
      const rail = this.rail();
      if (rail) this.physical.write("\x1b[?2026h" + rail + SYNC_END);
    }
  }
  refreshLayout(): void { this.surface.layoutChanged(); this.resizeHandler?.(); }
  moveBy(lines: number): void { this.physical.moveBy(lines); }
  hideCursor(): void { this.physical.hideCursor(); }
  showCursor(): void { this.physical.showCursor(); }
  clearLine(): void { this.write("\x1b[2K"); }
  clearFromCursor(): void { this.write("\x1b[J"); }
  clearScreen(): void { this.write("\x1b[2J\x1b[H"); }
  setTitle(title: string): void { this.physical.setTitle(title); }
  setProgress(active: boolean): void { this.physical.setProgress(active); }
  setProgramStatus(status: ProgramStatus): void { this.physical.setProgramStatus(status); }
}
