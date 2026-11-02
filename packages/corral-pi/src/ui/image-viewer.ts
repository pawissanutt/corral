import { Image, getCapabilities, setImageActivationHandler, type ImageSource, type TuiMouseEvent } from "@earendil-works/pi-tui";
import { pad, plainLabel, PANEL_BG, PANEL_TEXT, PANEL_MUTED, RESET } from "./panels-view.ts";

/** Existing bytes only. Temporary main view avoids compositing graphics over graphics. */
export class NativeImageViewer {
  private unregister: (() => void) | undefined;
  private source: ImageSource | undefined;
  private image: Image | undefined;
  private size = "";
  private viewportHeight = 1;
  setViewportHeight(height: number): void { this.viewportHeight = Math.max(0, Math.min(1000, Math.floor(height))); }
  get isOpen(): boolean { return this.source !== undefined; }
  constructor(private readonly canOpen: () => boolean, private readonly changed: () => void) {}
  bind(): void { this.unregister?.(); this.unregister = setImageActivationHandler(source => this.activate(source)); }
  activate(source: ImageSource): boolean {
    if (!this.canOpen()) return false;
    if (this.isOpen) { this.close(); return true; }
    this.source = source; this.image = undefined; this.size = ""; this.changed(); return true;
  }
  close(): void { if (this.source) { this.source = undefined; this.image = undefined; this.changed(); } }
  handleMouse(event: TuiMouseEvent) {
    if (this.isOpen && this.canOpen() && event.type === "click" && event.button === "left" && !event.shift && !event.ctrl && !event.alt) { this.close(); return { handled: true }; }
    return undefined;
  }
  render(width: number): string[] {
    const height = this.viewportHeight;
    const source = this.source;
    if (!source) return [];
    const title = `${plainLabel(source.filename ?? "Image")} · ${source.dimensions.widthPx}×${source.dimensions.heightPx} · click / Esc close`;
    if (height <= 1) return height ? [PANEL_BG + PANEL_TEXT + pad(title, width) + RESET] : [];
    const size = `${width}x${height}`;
    if (!this.image || size !== this.size) {
      this.size = size;
      this.image = new Image(source.data, source.mimeType, { fallbackColor: text => PANEL_MUTED + text + RESET },
        { maxWidthCells: Math.max(1, width - 2), maxHeightCells: Math.max(1, height - 1), filename: source.filename }, source.dimensions);
    }
    const lines = [PANEL_BG + PANEL_TEXT + pad(title, width) + RESET,
      ...(getCapabilities().images ? this.image.render(width) : [PANEL_MUTED + "Image rendering unavailable in this terminal." + RESET])];
    while (lines.length < height) lines.push(PANEL_BG + " ".repeat(width) + RESET);
    return lines;
  }
  invalidate(): void { this.image?.invalidate(); }
  dispose(): void { this.unregister?.(); this.unregister = undefined; this.source = undefined; this.image = undefined; }
}
