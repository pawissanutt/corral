import { readFileSync } from "node:fs";
import { SESSION_PART_MAX } from "../../bridge/protocol.ts";
import { panelGit, repositoryRoot } from "../../ui/panels-git.ts";
import type { ContinuedState, TodoProgress } from "../../ui/todo.ts";
import type { RemoteRegistry } from "./registry.ts";
import { clipText, cursorOffset, fingerprintPart, fitRows, jsonBytes, optionalText, parseCursor, partCursor } from "./state.ts";

interface McpServer { name: string; state: string }
export interface WorkspaceInputs {
  cwd: string; worktree?: string; branch?: string; context_files: readonly string[];
  skills: readonly string[]; prompts: readonly string[]; tools: readonly string[]; mcp?: Record<string, unknown>;
}

export function contextPart(input: WorkspaceInputs) {
  const strings = (items: readonly string[], count: number, budget: number) =>
    fitRows(items.slice(0, count).map(value => clipText(value, 4096)), 0, count, budget);
  const mcp: McpServer[] = [];
  if (Array.isArray(input.mcp?.servers)) for (const row of input.mcp.servers as { name?: unknown; connected?: unknown; error?: unknown }[]) {
    if (typeof row.name === "string") mcp.push({ name: clipText(row.name, 256),
      state: row.connected === true ? "connected" : row.error ? "unavailable" : "disconnected" });
  }
  return { cwd: clipText(input.cwd, 4096), worktree: optionalText(input.worktree, 4096), branch: optionalText(input.branch, 512),
    context_files: strings(input.context_files, 64, 12 * 1024), skills: strings(input.skills, 200, 5 * 1024),
    prompts: strings(input.prompts, 200, 5 * 1024), tools: strings(input.tools, 200, 8 * 1024),
    mcp: fitRows(mcp, 0, 100, 5 * 1024) };
}

export class WorkspaceGit {
  private readonly abort = new AbortController();
  private pending: Promise<void> | undefined;
  worktree: string | undefined;
  branch: string | undefined;

  constructor(private readonly cwd: string, private readonly changed: () => void) {}

  refresh(): Promise<void> {
    return this.pending ??= this.read().finally(() => { this.pending = undefined; });
  }

  private async read(): Promise<void> {
    const signal = this.abort.signal;
    const worktree = await repositoryRoot(this.cwd, signal).catch(() => undefined);
    const branch = worktree ? (await panelGit(worktree, ["symbolic-ref", "--quiet", "--short", "HEAD"], signal).catch(() => "")).trimEnd() || undefined : undefined;
    if (signal.aborted || this.worktree === worktree && this.branch === branch) return;
    this.worktree = worktree;
    this.branch = branch;
    this.changed();
  }

  close(): void { this.abort.abort(); }
}

interface PlanStep { id: string; text: string; state: string; detail?: string; wait?: string }
interface PlanChange { at: string; revision: number; summary: string }
export interface LedgerView { current: TodoProgress | null; unavailable: boolean; continued: ContinuedState | null }

export function planHistory(path: string): PlanChange[] {
  try {
    // The local ledger keeps completed plans, not a timestamped log of each step mutation.
    const raw = JSON.parse(readFileSync(path, "utf8")) as { history?: { plan: string; revision: number; closed: string; done: number; steps: number; dropped: number }[] };
    return (raw.history ?? []).slice(-16).reverse().map(({ plan, revision, closed, done, steps, dropped }) => {
      if (typeof plan !== "string" || !Number.isSafeInteger(revision) || revision < 0 ||
          typeof closed !== "string" || !Number.isFinite(Date.parse(closed))) throw new Error("Invalid retained plan history");
      return { at: new Date(closed).toISOString(), revision, summary: clipText(`${plan}: ${done}/${steps} done, ${dropped} dropped`, 512) };
    });
  } catch (error) {
    if ((error as NodeJS.ErrnoException).code === "ENOENT") return [];
    throw error;
  }
}

export function planPage(ledger: LedgerView, history: readonly PlanChange[], after?: string) {
  const progress = ledger.current;
  const continued = ledger.continued;
  const steps: PlanStep[] = [];
  if (progress) {
    const waits = new Map(progress.waits?.map(({ step, on }) => [step, on.kind === "event" ? on.event : on.question]));
    for (const phase of progress.phases) for (const step of phase.steps) {
      const detail = step.status.state === "dropped" ? step.status.reason : step.criteria.join(", ");
      steps.push({ id: clipText(step.id, 256), text: clipText(step.label, 500), state: step.status.state,
        detail: detail ? clipText(detail, 2048) : undefined, wait: optionalText(waits.get(step.id), 256) });
    }
  }
  const first = { stale: ledger.unavailable, continued: continued ? clipText(`${continued.plan} · attempt ${continued.attempt}${continued.stopped ? ` · ${continued.stopped}` : ""}`, 512) : undefined,
    plan: progress ? { title: clipText(progress.plan ?? progress.phases[0]?.label ?? "Plan", 512), revision: progress.revision, steps: [] as PlanStep[] } : undefined,
    history: [] as PlanChange[] };
  const key = fingerprintPart(["plan", first, steps, history]);
  const start = after === undefined ? 0 : cursorOffset(after, key);
  const budget = SESSION_PART_MAX - jsonBytes(first) - 64;
  const visible = fitRows(steps, start, 100, budget);
  if (first.plan) first.plan.steps = visible;
  const end = start + visible.length;
  if (end < steps.length) return { ...first, next: partCursor(key, end) };
  const historyStart = Math.max(0, start - steps.length);
  first.history = fitRows(history, historyStart, 50, SESSION_PART_MAX - jsonBytes(first) - 64);
  const consumed = historyStart + first.history.length;
  return { ...first, next: consumed < history.length ? partCursor(key, steps.length + consumed) : undefined };
}

export function registerWorkspace(registry: RemoteRegistry, input: {
  context(): WorkspaceInputs; git: WorkspaceGit; ledger?: () => LedgerView; history(): PlanChange[];
}): void {
  registry.register({ name: "context", parse: () => undefined, run: async () => { await input.git.refresh(); return contextPart(input.context()); } });
  if (!input.ledger) return;
  const read = input.ledger;
  registry.register({ name: "plan", parse: ({ after }) => parseCursor(after), run: after => {
    const ledger = read();
    let history: PlanChange[];
    try { history = input.history(); }
    catch { return planPage({ ...ledger, unavailable: true }, [], after); }
    return planPage(ledger, history, after);
  } });
}
