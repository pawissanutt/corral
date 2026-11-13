import { mkdir, writeFile } from "node:fs/promises";
import { join } from "node:path";
import { checked, json, type Run, type TaskReadback } from "./corral-cli.ts";
import type { BoardControl, BoardPhase, BoardPlan, BoardPreview, BoardTaskStatus, BuildContext } from "./types.ts";

export function validatePlan(plan: BoardPlan): void {
  if (!plan.title.trim() || !plan.tasks.length) throw new Error("A board plan needs a title and at least one task.");
  if (plan.tasks.length > 20) throw new Error("A board plan may contain at most 20 tasks.");
  const tasks = new Map(plan.tasks.map(task => [task.key, task]));
  if (tasks.size !== plan.tasks.length) throw new Error("Board task keys must be unique.");
  for (const task of plan.tasks) {
    if (!task.key.trim() || !task.title.trim()) throw new Error("Each board task needs a key and title.");
    for (const dependency of task.depends) if (!tasks.has(dependency) && !/^T-\d+$/.test(dependency)) {
      throw new Error(`Unknown dependency ${dependency} for ${task.key}.`);
    }
  }
  const visiting = new Set<string>(), visited = new Set<string>();
  function visit(key: string) {
    if (visiting.has(key)) throw new Error(`Board dependencies contain a cycle at ${key}.`);
    if (visited.has(key)) return;
    visiting.add(key);
    for (const dependency of tasks.get(key)!.depends) if (tasks.has(dependency)) visit(dependency);
    visiting.delete(key); visited.add(key);
  }
  for (const key of tasks.keys()) visit(key);
}

export function planFile(plan: BoardPlan): string {
  const quote = (value: string | readonly string[]) => JSON.stringify(value).replace(/[\u0085\u2028\u2029]/g, char => `\\u${char.charCodeAt(0).toString(16).padStart(4, "0")}`);
  const lines = ["---", `title: ${quote(plan.title)}`, ...(plan.project === undefined ? [] : [`project: ${quote(plan.project)}`]), "tasks:"];
  for (const task of plan.tasks) {
    lines.push(`  - key: ${quote(task.key)}`, `    title: ${quote(task.title)}`);
    for (const key of ["kind", "size", "profile"] as const) {
      const value = task[key];
      if (value !== undefined) lines.push(`    ${key}: ${quote(value)}`);
    }
    lines.push(`    depends: ${quote(task.depends)}`, `    body: ${quote(task.body)}`);
  }
  return `${lines.join("\n")}\n---\n${plan.why}`;
}

interface ImportResult { tracker: TaskReadback; tasks: TaskReadback[]; dry_run: boolean }

export function boardTask(value: TaskReadback, known: ReadonlyMap<string, TaskReadback>, now: number, refusal?: string): BoardTaskStatus {
  const status = value.status.toLowerCase();
  const current = value.runs?.at(-1);
  const dependencies = value.depends_on ?? [];
  const waiting = dependencies.filter(id => {
    const task = known.get(id);
    return !task?.landed && task?.status.toLowerCase() !== "done";
  });
  let phase: BoardPhase;
  let detail: string | undefined = refusal;
  if (value.landed) phase = "landed";
  else if (current?.gate?.result === "fail") { phase = "failed"; detail ??= current.gate.failed_cmd ?? "The gate failed."; }
  else if (value.automation_blocker || value.hold || status === "needs_me") {
    phase = "blocked"; detail ??= value.automation_blocker ?? value.hold?.reason ?? current?.autopilot?.reason ?? "The task is blocked.";
  } else if (current?.autopilot?.stage === "gating") phase = "gating";
  else if (current?.autopilot?.stage === "reviewing") phase = "reviewing";
  else if (status === "running") phase = "running";
  else if (status === "ready" && waiting.length) { phase = "waiting"; detail ??= `Waiting for ${waiting.join(", ")}.`; }
  else if (status === "ready") phase = "queued";
  else if (status === "review") phase = "reviewing";
  else phase = "backlog";
  return { id: value.id, title: value.title, status: value.status, phase, depends: dependencies, detail, updatedAt: now };
}

export function createBoard(options: {
  context: BuildContext; run: Run; tmpDir: string; changed: () => void; now?: () => number; pollMs?: number;
}): BoardControl & { propose(plan: BoardPlan): void; close(): void } {
  const root = options.context.session.workspaceRoot;
  const now = options.now ?? Date.now;
  const listeners = new Set<() => void>();
  const known = new Map<string, TaskReadback>();
  const refusals = new Map<string, string>();
  let statuses: readonly BoardTaskStatus[] = [];
  let proposed: BoardPlan | undefined;
  let closed = false;
  let timer: ReturnType<typeof setTimeout> | undefined;
  let refreshing: Promise<void> | undefined;
  let filing: Promise<readonly BoardTaskStatus[]> | undefined;
  function available(): string {
    if (!root) throw new Error("Board plans require a corral workspace.");
    return root;
  }
  function changed() { options.changed(); for (const listener of listeners) listener(); }
  async function writePlan(plan: BoardPlan) {
    available(); validatePlan(plan);
    await mkdir(options.tmpDir, { recursive: true, mode: 0o700 });
    const path = join(options.tmpDir, `board-${now()}-${crypto.randomUUID()}.md`);
    await writeFile(path, planFile({ ...plan, project: plan.project ?? options.context.session.project }), { mode: 0o600, flag: "wx" });
    return path;
  }
  function schedule() {
    clearTimeout(timer);
    if (!closed && statuses.some(task => task.phase !== "landed")) timer = setTimeout(() => { void refresh(); }, options.pollMs ?? 5000);
  }
  async function doRefresh() {
    const cwd = available();
    clearTimeout(timer);
    const errors = new Map<string, string>();
    for (const task of statuses) {
      try { known.set(task.id, await json<TaskReadback>(options.run, ["show", task.id, "--json"], cwd)); }
      catch (error) { errors.set(task.id, error instanceof Error ? error.message : String(error)); }
      if (closed) return;
    }
    const dependencies = new Set(statuses.flatMap(task => known.get(task.id)?.depends_on ?? []));
    const filed = new Set(statuses.map(task => task.id));
    for (const id of dependencies) if (!filed.has(id)) {
      try { known.set(id, await json<TaskReadback>(options.run, ["show", id, "--json"], cwd)); }
      catch { known.delete(id); }
      if (closed) return;
    }
    statuses = statuses.map(task => {
      const error = errors.get(task.id);
      return error ? { ...task, phase: "failed", detail: error, updatedAt: now() } : boardTask(known.get(task.id)!, known, now(), refusals.get(task.id));
    });
    changed(); schedule();
  }
  function refresh(): Promise<void> {
    if (!root) return Promise.resolve();
    if (refreshing) return refreshing;
    refreshing = doRefresh().finally(() => { refreshing = undefined; });
    return refreshing;
  }
  async function file(plan: BoardPlan, start: boolean): Promise<readonly BoardTaskStatus[]> {
    const cwd = available();
    const path = await writePlan(plan);
    const result = await json<ImportResult>(options.run, ["import", path, "--origin", "owner", "--json"], cwd);
    proposed = undefined;
    for (const task of result.tasks) known.set(task.id, task);
    const prior = new Map(statuses.map(task => [task.id, task]));
    for (const task of result.tasks) prior.set(task.id, boardTask(task, known, now()));
    statuses = [...prior.values()]; changed();
    if (start) {
      const pending = new Map(result.tasks.map(task => [task.id, task]));
      const ordered: TaskReadback[] = [];
      function visit(task: TaskReadback) {
        if (!pending.delete(task.id)) return;
        for (const dependency of task.depends_on ?? []) {
          const parent = pending.get(dependency);
          if (parent) visit(parent);
        }
        ordered.push(task);
      }
      for (const task of result.tasks) visit(task);
      for (const task of ordered) {
        try {
          await checked(options.run, ["queue", "add", task.id], cwd);
          known.set(task.id, { ...task, status: "ready" });
        } catch (error) { refusals.set(task.id, error instanceof Error ? error.message : String(error)); }
        statuses = statuses.map(status => status.id === task.id ? boardTask(known.get(task.id)!, known, now(), refusals.get(task.id)) : status);
        changed();
      }
    }
    schedule(); return statuses;
  }
  return {
    tasks() { return statuses; },
    proposed() { return proposed; },
    propose(plan) { if (!root) return; validatePlan(plan); proposed = structuredClone(plan); changed(); },
    async preview(plan): Promise<BoardPreview> {
      const cwd = available();
      const result = await json<ImportResult>(options.run, ["import", await writePlan(plan), "--dry-run", "--json"], cwd);
      return { tracker: result.tracker.id, tasks: result.tasks.map((task, index) => ({ key: plan.tasks[index]!.key, id: task.id, title: task.title, depends: task.depends_on ?? [] })) };
    },
    file(plan, start) {
      if (filing) return filing;
      filing = file(plan, start).finally(() => { filing = undefined; });
      return filing;
    },
    dismiss() { if (!root) return; proposed = undefined; changed(); },
    refresh,
    close() { if (!root) return; closed = true; clearTimeout(timer); },
    subscribe(listener) { listeners.add(listener); return () => { listeners.delete(listener); }; },
  };
}
