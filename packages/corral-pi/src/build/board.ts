import { mkdir, writeFile, rm } from "node:fs/promises";
import { join } from "node:path";
import { json, type Run, type TaskReadback } from "./corral-cli.ts";
import type { BoardControl, BoardPhase, BoardPlan, BoardPreview, BoardTaskPlan, BoardTaskStatus, BuildContext } from "./types.ts";

function orderedTasks(plan: BoardPlan): BoardTaskPlan[] {
  if (!plan.title.trim() || !plan.tasks.length) throw new Error("A board plan needs a title and at least one task.");
  if (plan.tasks.length > 20) throw new Error("A board plan may contain at most 20 tasks.");
  const tasks = new Map(plan.tasks.map(task => [task.key, task]));
  if (tasks.size !== plan.tasks.length) throw new Error("Board task keys must be unique.");
  const visiting = new Set<string>(), visited = new Set<string>();
  const ordered: BoardTaskPlan[] = [];
  function visit(task: BoardTaskPlan) {
    if (visiting.has(task.key)) throw new Error(`Board dependencies contain a cycle at ${task.key}.`);
    if (visited.has(task.key)) return;
    if (!task.key.trim() || !task.title.trim()) throw new Error("Each board task needs a key and title.");
    visiting.add(task.key);
    for (const dependency of task.depends) {
      const parent = tasks.get(dependency);
      if (parent) visit(parent);
      else if (!/^T-\d+$/.test(dependency)) throw new Error(`Unknown dependency ${dependency} for ${task.key}.`);
    }
    visiting.delete(task.key); visited.add(task.key); ordered.push(task);
  }
  for (const task of plan.tasks) visit(task);
  return ordered;
}

export function validatePlan(plan: BoardPlan): void { orderedTasks(plan); }

export function boardTask(value: TaskReadback, known: ReadonlyMap<string, TaskReadback>, now: number, refusal?: string): BoardTaskStatus {
  const dependencies = value.depends_on ?? [];
  const waiting = dependencies.filter(id => known.get(id)?.status !== "done");
  let phase: BoardPhase;
  let detail = refusal;
  switch (value.status) {
    case "done": phase = "landed"; break;
    case "dropped": phase = "failed"; detail ??= "The task was dropped."; break;
    case "blocked": phase = "blocked"; detail ??= value.blocked?.reason ?? "The task is blocked."; break;
    case "running": phase = "running"; break;
    case "review": phase = "reviewing"; break;
    case "ready":
      phase = waiting.length ? "waiting" : "queued";
      if (waiting.length) detail ??= `Waiting for ${waiting.join(", ")}.`;
      break;
    default: phase = "backlog";
  }
  return { id: value.id, title: value.title, status: value.status, phase, depends: dependencies, detail, updatedAt: now };
}

interface TaskResponse { task: TaskReadback }

export function createBoard(options: {
  context: BuildContext; run: Run; tmpDir: string; changed: () => void; now?: () => number; pollMs?: number;
}): BoardControl & { propose(plan: BoardPlan): void; close(): void } {
  const root = options.context.session.workspaceRoot;
  const now = options.now ?? Date.now;
  const listeners = new Set<() => void>();
  const known = new Map<string, TaskReadback>();
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
  function command(argv: readonly string[]): Promise<TaskResponse> {
    const cwd = available();
    return json<TaskResponse>(options.run, ["--root", cwd, "--json", ...argv], cwd);
  }
  function changed() { options.changed(); for (const listener of listeners) listener(); }
  function schedule() {
    clearTimeout(timer);
    if (!closed && statuses.some(task => task.phase !== "landed")) timer = setTimeout(() => { void refresh(); }, options.pollMs ?? 5000);
  }
  async function doRefresh() {
    clearTimeout(timer);
    const errors = new Map<string, string>();
    for (const task of statuses) {
      try { known.set(task.id, (await command(["show", task.id])).task); }
      catch (error) { errors.set(task.id, error instanceof Error ? error.message : String(error)); }
      if (closed) return;
    }
    const dependencies = new Set(statuses.flatMap(task => known.get(task.id)?.depends_on ?? []));
    const filed = new Set(statuses.map(task => task.id));
    for (const id of dependencies) if (!filed.has(id)) {
      try { known.set(id, (await command(["show", id])).task); }
      catch { known.delete(id); }
      if (closed) return;
    }
    statuses = statuses.map(task => {
      const error = errors.get(task.id);
      return error ? { ...task, phase: "failed", detail: error, updatedAt: now() } : boardTask(known.get(task.id)!, known, now());
    });
    changed(); schedule();
  }
  function refresh(): Promise<void> {
    if (!root || closed) return Promise.resolve();
    if (refreshing) return refreshing;
    refreshing = doRefresh().finally(() => { refreshing = undefined; });
    return refreshing;
  }
  async function body(task: TaskReadback, text: string, profile?: string): Promise<TaskReadback> {
    if (!text && !profile) return task;
    const path = join(options.tmpDir, `${task.id}-${crypto.randomUUID()}.md`);
    if (text) {
      await mkdir(options.tmpDir, { recursive: true, mode: 0o700 });
      await writeFile(path, text, { mode: 0o600, flag: "wx" });
    }
    try {
      return (await command(["edit", task.id, ...(text ? ["--body-file", path] : []), ...(profile ? [`--profile=${profile}`] : [])])).task;
    } finally { if (text) await rm(path, { force: true }); }
  }
  async function file(plan: BoardPlan, start: boolean): Promise<readonly BoardTaskStatus[]> {
    available();
    const ordered = orderedTasks(plan);
    const project = plan.project ?? options.context.session.project;
    if (!project) throw new Error("Board plans require a configured project.");
    const args = [`--project=${project}`];
    const parent = (await command(["new", ...args, "--", plan.title])).task;
    proposed = undefined;
    const ids = new Map<string, string>();
    const filed: TaskReadback[] = [];
    const update = (task: TaskReadback) => {
      known.set(task.id, task);
      const status = boardTask(task, known, now());
      const prior = statuses.findIndex(row => row.id === task.id);
      statuses = prior < 0 ? [...statuses, status] : statuses.map(row => row.id === task.id ? status : row);
      changed();
    };
    try {
      await body(parent, plan.why);
      for (const item of ordered) {
        const depends = item.depends.map(key => ids.get(key) ?? key);
        const task = (await command(["new", ...args, "--parent", parent.id,
          ...(item.kind ? [`--kind=${item.kind}`] : []), ...(item.size ? [`--size=${item.size}`] : []),
          ...(depends.length ? ["--depends-on", depends.join(",")] : []), "--", item.title])).task;
        ids.set(item.key, task.id);
        update(task);
        filed.push(await body(task, item.body, item.profile));
      }
      if (start) {
        for (const task of filed) update((await command(["move", task.id, "ready"])).task);
      }
      return statuses;
    } catch (error) {
      throw new Error(`Could not finish filing ${parent.id}: ${error instanceof Error ? error.message : String(error)}`);
    } finally { schedule(); }
  }
  return {
    tasks() { return statuses; },
    proposed() { return proposed; },
    propose(plan) { if (!root) return; validatePlan(plan); proposed = structuredClone(plan); changed(); },
    async preview(plan): Promise<BoardPreview> {
      available(); validatePlan(plan);
      // The engine assigns IDs only when the owner confirms; preview shows the plan's actual keys.
      return { tracker: plan.title, tasks: plan.tasks.map(task => ({ key: task.key, id: task.key, title: task.title, depends: task.depends })) };
    },
    file(plan, start) {
      if (filing) return filing;
      filing = file(plan, start).finally(() => { filing = undefined; });
      return filing;
    },
    dismiss() { if (!root) return; proposed = undefined; changed(); },
    refresh,
    close() { closed = true; clearTimeout(timer); },
    subscribe(listener) { listeners.add(listener); return () => listeners.delete(listener); },
  };
}
