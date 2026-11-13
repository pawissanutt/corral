// The working indicator: what the main agent is doing and since when, observed from Pi's extension events,
// and the one shared redraw clock. The clock runs only while the agent is busy or a live row holds it, so an
// idle session schedules nothing. Presentation only: nothing here changes what the agent sends or receives.
import { spinner } from "./frame.ts";
import { NOTICE_MS } from "./header.ts";
import { PANEL_BLUE, PANEL_ERROR, PANEL_MUTED, PANEL_SUCCESS, PANEL_TEXT, PANEL_WARNING } from "./panels-view.ts";

/** Rows that must keep redrawing (a running job, a detached cell) hold the shared clock instead of starting a timer. */
export interface Ticker {
  /** Keeps the shared redraw clock running until the returned release is called. One hold per id; holding an id twice returns the same release. */
  hold(id: string): () => void;
  /** Time source for spinners and elapsed labels, in epoch ms. */
  now(): number;
}

/** The time and interval source; tests inject one so no real time passes. */
export interface Scheduler {
  now(): number;
  /** Calls `tick` every `ms` milliseconds until the returned cancel runs. */
  every(ms: number, tick: () => void): () => void;
}
export const SYSTEM_SCHEDULER: Scheduler = {
  now: () => Date.now(),
  every: (ms, tick) => {
    const timer = setInterval(tick, ms);
    timer.unref();
    return () => clearInterval(timer);
  },
};

/** The clock period: one spinner frame while animating, one second while motion is still. */
export const TICK_MS = 110, STILL_TICK_MS = 1000;
/** Without model data this long, the indicator warns that nothing is arriving. */
export const STALL_MS = 60_000;

/** The shared redraw clock. Each tick requests one redraw; the clock stops itself once nothing is busy or held. */
export class ActivityClock implements Ticker {
  private readonly holds = new Map<string, () => void>();
  private cancel: (() => void) | undefined;
  private period = 0;
  constructor(private readonly scheduler: Scheduler, private readonly busy: () => boolean,
    private readonly tick: () => void, private readonly still: () => boolean) {}
  now(): number { return this.scheduler.now(); }
  hold(id: string): () => void {
    const held = this.holds.get(id);
    if (held) return held;
    const release = () => {
      if (this.holds.get(id) !== release) return;
      this.holds.delete(id);
      this.sync();
    };
    this.holds.set(id, release);
    this.sync();
    return release;
  }
  /** True while an interval is scheduled. */
  get running(): boolean { return this.cancel !== undefined; }
  /** Starts the clock while busy or held, stops it otherwise, and restarts it at the other period after a motion change. */
  sync(): void {
    const wanted = this.busy() || this.holds.size > 0, period = this.still() ? STILL_TICK_MS : TICK_MS;
    if (this.cancel && (!wanted || period !== this.period)) { this.cancel(); this.cancel = undefined; }
    if (!wanted || this.cancel) return;
    this.period = period;
    this.cancel = this.scheduler.every(period, () => { this.tick(); this.sync(); });
  }
  dispose(): void {
    this.holds.clear();
    this.cancel?.(); this.cancel = undefined;
  }
}

/**
 * One cache entry per synchronous render pass: the first read builds, later reads in the same pass reuse it, and
 * the entry is dropped on the next microtask (a render pass never awaits) or by `clear()` after a state change.
 */
export class PassCache<T> {
  private entry: { readonly value: T } | undefined;
  constructor(private readonly build: () => T) {}
  get(): T {
    if (this.entry) return this.entry.value;
    const entry = { value: this.build() };
    this.entry = entry;
    queueMicrotask(() => { if (this.entry === entry) this.entry = undefined; });
    return entry.value;
  }
  clear(): void { this.entry = undefined; }
}

export type Phase = "idle" | "model" | "thinking" | "writing" | "tool" | "retry" | "compacting" | "owner";
interface RunningTool { readonly name: string; readonly start: number; readonly parent?: string }
/** The turn that just ended, shown for the notice time. */
export interface TurnEnd { readonly at: number; readonly elapsed: number; readonly aborted: boolean }

/** `42s`, `1m 04s`, `1h 02m`; `compact` drops the inner space (`1m04s`). */
export function elapsedLabel(ms: number, compact = false): string {
  const seconds = Math.max(0, Math.floor((Number.isFinite(ms) ? ms : 0) / 1000));
  if (seconds < 60) return `${seconds}s`;
  const minutes = Math.floor(seconds / 60), gap = compact ? "" : " ";
  if (minutes < 60) return `${minutes}m${gap}${String(seconds % 60).padStart(2, "0")}s`;
  return `${Math.floor(minutes / 60)}h${gap}${String(minutes % 60).padStart(2, "0")}m`;
}

/** A tool's name as words: `job_wait` reads `job wait`, `corral_next` reads `corral next`. */
function toolWords(name: string): string { return name.replaceAll("_", " "); }

/**
 * What the main agent is doing, fed by `NativePanels` from Pi's extension events. Phases, highest first: waiting
 * for the owner (a blocking dialog the agent opened), compacting, retrying after a provider error, running tools,
 * then the model stream (waiting for the model, thinking, writing). Idle once the run settles.
 */
export class Activity {
  private running = false;
  private turnStart = 0;
  private stream: "model" | "thinking" | "writing" = "model";
  private streamTool: string | undefined;
  private streamStart = 0;
  private lastData = 0;
  private lastEvent = 0;
  private readonly tools = new Map<string, RunningTool>();
  /** Open blocking prompts, oldest first: `true` for one the owner opened (palette, keys list, pager). */
  private readonly prompts: { readonly own: boolean; readonly start: number }[] = [];
  private compacting: number | undefined;
  private retrying: number | undefined;
  private lastStopError = false;
  private ended: TurnEnd | undefined;

  constructor(private readonly now: () => number) {}

  private touch(): number { return this.lastEvent = this.now(); }
  /** `agent_start`: a run starts, or continues after a retry within the same turn. */
  agentStart(): void {
    const now = this.touch();
    if (!this.running) { this.running = true; this.turnStart = now; this.ended = undefined; }
    this.retrying = undefined; this.lastStopError = false;
    this.toModel(now);
  }
  /** `before_provider_request`: waiting for the model's first data. */
  request(): void { if (this.running) this.toModel(this.touch()); }
  /** `provider_stream_event`: data arrived; nothing else changes. */
  data(): void { this.lastData = this.now(); }
  /** `message_update`: `kind` is the assistant event type; `tool` names a tool call whose arguments stream. */
  update(kind: string, tool?: string): void {
    const now = this.touch();
    this.lastData = now;
    const next = kind.startsWith("thinking") ? "thinking" : kind.startsWith("text") || kind.startsWith("toolcall") ? "writing" : undefined;
    if (!next) return;
    const streamTool = kind.startsWith("toolcall") ? tool : undefined;
    if (next !== this.stream || streamTool !== this.streamTool) { this.stream = next; this.streamTool = streamTool; this.streamStart = now; }
  }
  /** `message_end` of an assistant message: the next wait is for tools or the next request. */
  messageEnd(stopReason: string | undefined): void {
    this.lastStopError = stopReason === "error";
    this.toModel(this.touch());
  }
  /** `tool_execution_start`; `parent` is set for a call another tool made (a Code Mode cell). */
  toolStart(id: string, name: string, parent?: string): void {
    const start = this.touch();
    this.tools.set(id, { name, start, ...(parent === undefined ? {} : { parent }) });
  }
  /** `tool_execution_end`. Once no top-level tool runs, the agent waits for the model again. */
  toolEnd(id: string): void {
    const now = this.touch();
    const tool = this.tools.get(id);
    this.tools.delete(id);
    if (tool && tool.parent === undefined) {
      for (const [key, nested] of this.tools) if (nested.parent === id) this.tools.delete(key);
      if (!this.topTools().length) this.toModel(now);
    }
  }
  /** `ui_prompt_start`; `own` when the owner opened it (the palette, the keys list, a pager). */
  promptStart(own: boolean): void { this.prompts.push({ own, start: this.touch() }); }
  promptEnd(): void { this.touch(); this.prompts.pop(); }
  compactStart(): void { this.compacting = this.touch(); }
  compactEnd(): void { this.touch(); this.compacting = undefined; }
  /** `agent_end`: a run whose last response failed is retried by Pi until the run settles. */
  agentEnd(): void {
    const now = this.touch();
    this.tools.clear();
    if (this.running && this.lastStopError) this.retrying = now;
  }
  /** `agent_settled`: the turn is over. Returns how it ended, or undefined when no run was observed. */
  settled(aborted: boolean): TurnEnd | undefined {
    const now = this.touch();
    if (!this.running) return undefined;
    this.running = false; this.tools.clear(); this.retrying = undefined; this.compacting = undefined; this.streamTool = undefined;
    this.ended = { at: now, elapsed: now - this.turnStart, aborted };
    return this.ended;
  }
  /**
   * Squares the model with the session's own streaming flag, for a run whose start or end event this observer
   * missed (installed mid-run, or a run that ended without settling). A quiet second of grace avoids racing the events.
   */
  reconcile(streaming: boolean): void {
    const now = this.now();
    if (streaming && !this.running) { this.running = true; this.turnStart = now; this.ended = undefined; this.toModel(now); this.lastEvent = now; }
    else if (!streaming && this.running && this.retrying === undefined && this.compacting === undefined && now - this.lastEvent > 1000) {
      this.running = false; this.tools.clear(); this.streamTool = undefined;
    }
  }
  private toModel(now: number): void { this.stream = "model"; this.streamTool = undefined; this.streamStart = now; this.lastData = now; }
  private topTools(): RunningTool[] { return [...this.tools.values()].filter(tool => tool.parent === undefined); }
  private agentPrompt(): { readonly start: number } | undefined { return this.running ? this.prompts.find(prompt => !prompt.own) : undefined; }

  phase(): Phase {
    if (this.agentPrompt()) return "owner";
    if (this.compacting !== undefined) return "compacting";
    if (!this.running) return "idle";
    if (this.retrying !== undefined) return "retry";
    if (this.topTools().length) return "tool";
    return this.stream;
  }
  /** True while the indicator changes on its own: a run, a compaction, or the turn-end label. */
  busy(now = this.now()): boolean {
    return this.running || this.compacting !== undefined || (this.ended !== undefined && now - this.ended.at < NOTICE_MS);
  }
  /** True while a run is active. */
  get active(): boolean { return this.running; }
  /** No model data for `STALL_MS` while the turn waits on the model. */
  stalled(now = this.now()): boolean {
    const phase = this.phase();
    return (phase === "model" || phase === "thinking" || phase === "writing") && now - this.lastData >= STALL_MS;
  }

  /** When the current phase began. */
  private phaseStart(phase: Phase): number {
    switch (phase) {
      case "owner": return this.agentPrompt()?.start ?? this.lastEvent;
      case "compacting": return this.compacting ?? this.lastEvent;
      case "retry": return this.retrying ?? this.lastEvent;
      case "tool": return Math.min(...this.topTools().map(tool => tool.start));
      default: return this.streamStart;
    }
  }
  /** What the agent is doing, long and short: `running bash` / `bash`. */
  private detail(phase: Phase): { readonly long: string; readonly short: string } {
    switch (phase) {
      case "owner": return { long: "waiting for you", short: "you" };
      case "compacting": return { long: "compacting context", short: "compacting" };
      case "retry": return { long: "retrying after an error", short: "retrying" };
      case "tool": {
        const top = this.topTools();
        if (top.length > 1) return { long: `running ${top.length} tools`, short: `${top.length} tools` };
        const tool = top[0]!, nested = [...this.tools.values()].filter(item => item.parent !== undefined).at(-1);
        const name = toolWords(tool.name) + (nested ? ` › ${toolWords(nested.name)}` : "");
        return { long: `running ${name}`, short: toolWords(nested?.name ?? tool.name) };
      }
      case "writing": return this.streamTool ? { long: `writing ${toolWords(this.streamTool)}`, short: "writing" } : { long: "writing", short: "writing" };
      case "thinking": return { long: "thinking", short: "thinking" };
      default: return { long: "waiting for the model", short: "waiting" };
    }
  }

  /**
   * The indicator in the composer's top border, as variants from the longest to the shortest; the composer shows
   * the first that fits. Empty when there is nothing to say. `columns` is the terminal width: from 105 the word
   * `Working` leads, from 92 the stop key follows (`stopKey`, e.g. `esc`), below 64 times are compact. Every state
   * carries a glyph and a word, never colour alone.
   */
  indicator(columns: number, now = this.now(), stopKey?: string): readonly string[] {
    const sep = PANEL_MUTED + " · ", compact = columns < 64;
    const phase = this.phase();
    if (phase === "idle") {
      const ended = this.ended;
      if (!ended || now - ended.at >= NOTICE_MS) return [];
      const time = PANEL_MUTED + elapsedLabel(ended.elapsed, compact);
      const head = ended.aborted ? PANEL_ERROR + "✗ " + PANEL_TEXT + "stopped" : PANEL_SUCCESS + "✓ " + PANEL_TEXT + "done";
      return [head + sep + time, (ended.aborted ? PANEL_ERROR + "✗ " : PANEL_SUCCESS + "✓ ") + time];
    }
    const stop = stopKey && columns >= 92 && this.running && phase !== "owner" ? sep + PANEL_MUTED + `${stopKey} stop` : "";
    if (phase === "owner") {
      const time = PANEL_MUTED + elapsedLabel(now - this.phaseStart(phase), compact);
      return [PANEL_WARNING + "▲ waiting for you" + sep + time, PANEL_WARNING + "▲ you" + sep + time, PANEL_WARNING + "▲ " + time];
    }
    if (this.stalled(now)) {
      const time = elapsedLabel(now - this.lastData, compact), warn = PANEL_WARNING;
      return [warn + `! no data from the model for ${time}` + stop, warn + `! no data from the model for ${time}`,
        warn + `! no data for ${time}`, warn + `! ${time}`];
    }
    const glyph = PANEL_BLUE + spinner(now) + " ", time = PANEL_MUTED + elapsedLabel(now - this.phaseStart(phase), compact);
    const { long, short } = this.detail(phase), tone = phase === "retry" ? PANEL_WARNING : PANEL_MUTED;
    const working = columns >= 105 ? [glyph + PANEL_TEXT + "Working" + sep + tone + long + sep + time + stop] : [];
    return [...working, glyph + tone + long + sep + time + stop, glyph + tone + long + sep + time, glyph + tone + short + sep + time, glyph + time];
  }
  /** The header state word's mark while a run is active: the spinner (or `▲`, `!`) and the turn's elapsed time. */
  headerMark(now = this.now()): { readonly glyph: string; readonly elapsed: string } | undefined {
    if (!this.running) return undefined;
    const phase = this.phase();
    const glyph = phase === "owner" ? PANEL_WARNING + "▲" : this.stalled(now) ? PANEL_WARNING + "!" : PANEL_BLUE + spinner(now);
    return { glyph, elapsed: elapsedLabel(now - this.turnStart) };
  }
}
