import { stripTerminalSequences, type ProgramStatus, type Terminal } from "@earendil-works/pi-tui";
import { within } from "./wait.ts";

/** A real InteractiveMode's public terminal boundary, without touching process stdin/stdout. */
export class RecordingTerminal implements Terminal {
  columns = 140;
  rows = 46;
  kittyProtocolActive = false;
  readonly writes: string[] = [];
  private input: ((data: string) => void) | undefined;
  private resizeInput: (() => void) | undefined;
  private readonly listeners = new Set<() => void>();
  start(input: (data: string) => void, resize: () => void): void { this.input = input; this.resizeInput = resize; }
  stop(): void { this.input = undefined; this.resizeInput = undefined; }
  async drainInput(): Promise<void> {}
  write(data: string): void { this.writes.push(data); for (const listener of this.listeners) listener(); }
  send(data: string): void { this.input?.(data); }
  resize(columns: number, rows: number): void { this.columns = columns; this.rows = rows; this.resizeInput?.(); }
  until(text: string, after = 0): Promise<void> {
    return this.whenWritten(() => this.writes.slice(after).join("").includes(text), JSON.stringify(text) + " on the native terminal");
  }
  /** Like `until`, but matches visible text: styled segments split by colour codes still match. */
  untilVisible(text: string, after = 0): Promise<void> {
    return this.whenWritten(() => stripTerminalSequences(this.writes.slice(after).join("")).includes(text), JSON.stringify(text) + " visible on the native terminal");
  }
  /** Rechecks `matches` after every terminal write until it holds. */
  async whenWritten(matches: () => boolean, what: string): Promise<void> {
    if (matches()) return;
    let listener: () => void = () => {};
    const seen = new Promise<void>(resolve => {
      listener = () => { if (matches()) resolve(); };
      this.listeners.add(listener);
    });
    try { await within(seen, 10_000, what); }
    finally { this.listeners.delete(listener); }
  }
  moveBy(lines: number): void { this.write(`\x1b[${Math.abs(lines)}${lines < 0 ? "A" : "B"}`); }
  hideCursor(): void { this.write("\x1b[?25l"); }
  showCursor(): void { this.write("\x1b[?25h"); }
  clearLine(): void { this.write("\x1b[2K"); }
  clearFromCursor(): void { this.write("\x1b[J"); }
  clearScreen(): void { this.write("\x1b[2J\x1b[H"); }
  setTitle(_title: string): void {}
  setProgress(_active: boolean): void {}
  setProgramStatus(_status: ProgramStatus): void {}
}
