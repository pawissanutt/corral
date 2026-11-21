// Preparation and the owner's eventual fast-forward report the same ordered steps. Land itself
// only completes the fast-forward; earlier steps identify the preparation and evidence it reused.
import { closeSync, fstatSync, openSync, readSync } from "node:fs";
import type { LandGate, LandProgress, LandStep } from "./types.ts";

/** The lines of the gate's log kept while it runs, and the longer tail kept once it failed. */
export const TAIL_RUNNING = 6, TAIL_FAILED = 12;
const TAIL_BYTES = 16 * 1024, LINE_MAX = 400;
// CSI and OSC sequences, then the single-character escapes.
const TERMINAL = /\x1b\[[0-?]*[ -/]*[@-~]|\x1b\][^\x07\x1b]*(?:\x07|\x1b\\)|\x1b[@-Z\\-_]/g;

/** The last `lines` non-empty lines of `path` as plain text: no terminal sequences, a redrawn line as it last read. */
export function logTail(path: string, lines: number): string[] {
  let fd: number;
  try { fd = openSync(path, "r"); } catch { return []; }
  try {
    const size = fstatSync(fd).size, length = Math.min(size, TAIL_BYTES);
    const buffer = Buffer.alloc(length);
    readSync(fd, buffer, 0, length, size - length);
    const text = buffer.toString("utf8").split("\n");
    // A read that starts mid-file starts mid-line.
    if (size > length) text.shift();
    return text.map(line => (line.split("\r").findLast(part => part.trim()) ?? "").replace(TERMINAL, "")
      .replace(/[\u0000-\u001f\u007f-\u009f]/g, " ").trimEnd().slice(0, LINE_MAX))
      .filter(line => line.trim()).slice(-lines);
  } catch { return []; }
  finally { closeSync(fd); }
}

export class LandTracker {
  private steps: LandStep[];
  private gate: LandGate | undefined;

  constructor(private readonly base: string, gated: boolean, private readonly now: () => number) {
    this.steps = (gated ? ["rebase", "gate", "land"] as const : ["rebase", "land"] as const).map(id => ({ id, state: "waiting" }));
  }
  begin(id: LandStep["id"]): void { this.set(id, { state: "running", startedAt: this.now() }); }
  done(id: LandStep["id"], note?: string): void { this.set(id, { state: "done", endedAt: this.now(), ...(note ? { note } : {}) }); }
  /** Step `id`, else the running step, failed. A step that already failed keeps its note. */
  fail(note?: string, id?: LandStep["id"]): void {
    const step = this.steps.find(step => id ? step.id === id : step.state === "running");
    if (step && step.state !== "failed") this.set(step.id, { state: "failed", endedAt: this.now(), ...(note ? { note } : {}) });
  }
  setGate(gate: Partial<LandGate>): void { this.gate = { tail: [], ...this.gate, ...gate }; }
  clearTail(): void { if (this.gate) this.gate = { ...this.gate, tail: [] }; }
  refreshTail(lines = TAIL_RUNNING): boolean {
    if (!this.gate?.log) return false;
    const tail = logTail(this.gate.log, lines);
    if (tail.length === this.gate.tail.length && tail.every((line, index) => line === this.gate!.tail[index])) return false;
    this.gate = { ...this.gate, tail };
    return true;
  }
  view(): LandProgress { return { base: this.base, steps: this.steps, ...(this.gate ? { gate: this.gate } : {}) }; }

  private set(id: LandStep["id"], patch: Partial<LandStep>): void {
    this.steps = this.steps.map(step => step.id === id ? { ...step, ...patch } : step);
  }
}
