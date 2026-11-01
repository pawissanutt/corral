import { visibleWidth } from "@earendil-works/pi-tui";

const segmenter = new Intl.Segmenter(undefined, { granularity: "grapheme" });
// CSI, OSC/APC/DCS/PM strings (BEL or ST terminated), two-byte escapes, then text runs.
const TOKEN = /\x1b\[([<=>?]?)([\d;:]*)([ -\/]*)([@-~])|\x1b[\]_P^][\s\S]*?(?:\x07|\x1b\\)|\x1b[()][\s\S]|\x1b[\s\S]|[\x00-\x1f\x7f]|[^\x00-\x1f\x7f\x1b]+/g;

/**
 * Minimal model of the alternate-screen frames pi-tui and PanelTerminal emit: absolute cursor
 * moves, line/screen erase, cursor save/restore and grapheme-width text. Writes that would pass the
 * right edge are counted in `overflow` instead of wrapping, so width bugs fail loudly.
 */
export class ScreenModel {
  overflow = 0;
  private cells: string[][] = [];
  private row = 0;
  private col = 0;
  private saved: [number, number] = [0, 0];
  private consumed = 0;

  constructor(public columns: number, public rows: number) { this.clear(); }

  resize(columns: number, rows: number): void {
    this.cells = Array.from({ length: rows }, (_, row) =>
      Array.from({ length: columns }, (_, col) => this.cells[row]?.[col] ?? " "));
    this.columns = columns; this.rows = rows;
  }

  sync(writes: readonly string[]): this {
    for (; this.consumed < writes.length; this.consumed++) this.feed(writes[this.consumed] ?? "");
    return this;
  }

  lines(): string[] { return this.cells.map(row => row.join("").trimEnd()); }
  text(): string { return this.lines().join("\n"); }

  /** Zero-based cell position of the first occurrence of `needle`, by terminal columns. */
  find(needle: string): { row: number; col: number } | undefined {
    for (const [row, cells] of this.cells.entries()) {
      const line = cells.join("");
      const index = line.indexOf(needle);
      if (index >= 0) return { row, col: visibleWidth(line.slice(0, index)) };
    }
    return undefined;
  }

  slice(row: number, from: number, to: number): string { return (this.cells[row] ?? []).slice(from, to).join(""); }

  private clear(): void {
    this.cells = Array.from({ length: this.rows }, () => Array.from({ length: this.columns }, () => " "));
  }

  private feed(data: string): void {
    for (const match of data.matchAll(TOKEN)) {
      const token = match[0];
      if (match[4] !== undefined) this.csi(match[1] ?? "", match[2] ?? "", match[4]);
      else if (token === "\x1b7") this.saved = [this.row, this.col];
      else if (token === "\x1b8") [this.row, this.col] = this.saved;
      else if (token === "\r") this.col = 0;
      else if (token === "\n") this.row = Math.min(this.rows - 1, this.row + 1);
      else if (token === "\b") this.col = Math.max(0, this.col - 1);
      else if (!token.startsWith("\x1b") && !/^[\x00-\x1f\x7f]$/.test(token)) this.print(token);
    }
  }

  private csi(prefix: string, params: string, final: string): void {
    if (prefix) return; // Private modes (alternate screen, autowrap, sync output, cursor) do not move cells.
    const values = params.split(";").map(value => Number(value.split(":")[0]) || 0);
    switch (final) {
      case "H": case "f":
        this.row = Math.max(0, Math.min(this.rows - 1, (values[0] || 1) - 1));
        this.col = Math.max(0, Math.min(this.columns - 1, (values[1] || 1) - 1)); break;
      case "A": this.row = Math.max(0, this.row - (values[0] || 1)); break;
      case "B": this.row = Math.min(this.rows - 1, this.row + (values[0] || 1)); break;
      case "C": this.col = Math.min(this.columns - 1, this.col + (values[0] || 1)); break;
      case "D": this.col = Math.max(0, this.col - (values[0] || 1)); break;
      case "G": this.col = Math.max(0, Math.min(this.columns - 1, (values[0] || 1) - 1)); break;
      case "K": {
        const line = this.cells[this.row];
        if (!line) return;
        const [from, to] = values[0] === 2 ? [0, this.columns] : values[0] === 1 ? [0, this.col + 1] : [this.col, this.columns];
        for (let col = from; col < to; col++) line[col] = " ";
        break;
      }
      case "J": if (values[0] === 2 || values[0] === 3) this.clear(); break;
      default: break;
    }
  }

  private print(text: string): void {
    const line = this.cells[this.row];
    if (!line) return;
    for (const { segment } of segmenter.segment(text)) {
      const width = visibleWidth(segment);
      if (width === 0) { if (this.col > 0) line[this.col - 1] += segment; continue; }
      if (this.col + width > this.columns) { this.overflow++; continue; }
      line[this.col] = segment;
      for (let extra = 1; extra < width; extra++) line[this.col + extra] = "";
      this.col += width;
    }
  }
}
