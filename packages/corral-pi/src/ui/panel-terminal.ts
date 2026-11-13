// Public Terminal adapter: Pi still owns every transcript, editor, dialog and input lifecycle and renders
// the whole frame at the physical width. The adapter paints the one canvas background, decides whether the
// sidebar is a rail (the workbench lays it out beside the main view) and routes input to the panels first.
import type { ProgramStatus, Terminal } from "@earendil-works/pi-tui";
import { PanelCanvas } from "./panel-canvas.ts";

export const PANEL_BREAKPOINT = 105;
export const PANEL_WIDTH = 34;
/** A navigation list needs no more than 40 columns; the main pane always keeps 70. */
export const RAIL_MIN = 24;
export const RAIL_MAX = 40;
export const MIN_MAIN = 70;

export interface PanelSurface {
  readonly sidebarEnabled: boolean;
  readonly sidebarWidth?: number;
  handleInput(data: string): boolean;
  layoutChanged(): void;
}

export class PanelTerminal implements Terminal {
  private readonly canvas = new PanelCanvas();
  private fullscreen = true;
  private resizeHandler: (() => void) | undefined;
  private onInput: ((data: string) => void) | undefined;

  constructor(readonly physical: Terminal, private readonly surface: PanelSurface) {}

  /** The requested width, clamped so the main pane keeps MIN_MAIN columns. */
  get railWidth(): number {
    const max = Math.min(RAIL_MAX, this.physical.columns - MIN_MAIN - 1);
    return Math.max(0, Math.min(max, Math.max(RAIL_MIN, this.surface.sidebarWidth ?? PANEL_WIDTH)));
  }
  /** True while the sidebar is a rail: fullscreen, enabled, and at least PANEL_BREAKPOINT columns. */
  get sidebarVisible(): boolean {
    return this.fullscreen && this.surface.sidebarEnabled && this.physical.columns >= PANEL_BREAKPOINT && this.railWidth >= RAIL_MIN;
  }
  /** Columns of the main view: the physical width less the rail and the `│` column on its left. */
  get mainColumns(): number { return this.physical.columns - (this.sidebarVisible ? this.railWidth + 1 : 0); }
  get columns(): number { return this.physical.columns; }
  get rows(): number { return this.physical.rows; }
  get kittyProtocolActive(): boolean { return this.physical.kittyProtocolActive; }

  start(onInput: (data: string) => void, onResize: () => void): void {
    this.resizeHandler = onResize;
    this.onInput = onInput;
    this.physical.start(data => this.replay(data), () => {
      this.surface.layoutChanged();
      onResize();
    });
  }
  /** Input as if typed: the panels first, then Pi. Footer and keys-list clicks replay their key this way. */
  replay(data: string): void {
    if (!this.surface.handleInput(data)) this.onInput?.(data);
  }
  stop(): void {
    this.canvas.reset();
    this.resizeHandler = undefined;
    this.onInput = undefined;
    this.physical.stop();
  }
  drainInput(maxMs?: number, idleMs?: number): Promise<void> { return this.physical.drainInput(maxMs, idleMs); }

  write(data: string): void {
    const frame = this.canvas.write(data);
    if (frame.screen !== undefined) this.fullscreen = frame.screen;
    this.physical.write(frame.data);
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
