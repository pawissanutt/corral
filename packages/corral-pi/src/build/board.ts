import { closeSync, fsyncSync, mkdirSync, openSync, readFileSync, realpathSync, renameSync, rmSync, writeFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { json, type Run, type TaskReadback } from "./corral-cli.ts";
import { TASK_DIFFICULTIES, TASK_SIZES, type BoardControl, type BoardPhase, type BoardPlan, type BoardPreview,
  type BoardTaskPlan, type BoardTaskStatus, type BuildContext } from "./types.ts";

// Leave room for all task keys and tags in the bounded remote preview. Brief text can be clipped; labels cannot.
const PLAN_TAGS_BYTES = 8 * 1024;

function checkedTags(task: BoardTaskPlan): string[] | undefined {
  if (task.tags === undefined) return;
  if (!Array.isArray(task.tags) || task.tags.some(tag => typeof tag !== "string" || !tag || /^\p{White_Space}|\p{White_Space}$|\p{Cc}/u.test(tag))) {
    throw new Error(`Invalid tag for ${task.key}: use nonempty labels without surrounding whitespace or control characters.`);
  }
  return [...new Set(task.tags)];
}

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
    if (/^T-\d+$/.test(task.key)) throw new Error(`Board key ${task.key} looks like an existing task ID.`);
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

/** Validate before offering confirmation, keeping exactly the tags the engine will write. */
export function validatePlan(plan: BoardPlan): BoardPlan {
  orderedTasks(plan);
  const tasks = plan.tasks.map(task => {
    if (task.size !== undefined && !TASK_SIZES.some(size => size === task.size)) {
      throw new Error(`Invalid size for ${task.key}: use ${TASK_SIZES.join(", ")}.`);
    }
    if (task.difficulty !== undefined && !TASK_DIFFICULTIES.some(difficulty => difficulty === task.difficulty)) {
      throw new Error(`Invalid difficulty for ${task.key}: use ${TASK_DIFFICULTIES.join(", ")}.`);
    }
    const tags = checkedTags(task);
    return tags === undefined ? task : { ...task, tags };
  });
  if (Buffer.byteLength(JSON.stringify(tasks.map(task => task.tags ?? []))) > PLAN_TAGS_BYTES) {
    throw new Error("Board plan tags exceed the 8 KiB preview budget. Use fewer or shorter tags.");
  }
  return { ...plan, tasks };
}

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
  return { id: value.id, title: value.title, status: value.status, phase, depends: dependencies, detail, updatedAt: now,
    project: value.project ?? undefined, size: value.size ?? undefined, difficulty: value.difficulty ?? undefined, tags: value.tags };
}

interface TaskResponse { task: TaskReadback }
interface FilingReceipt {
  parents: { project: string; id: string }[];
  tasks: (Pick<TaskReadback, "id" | "status"> & { key: string; project: string; depends_on: string[] })[];
}
interface Pending {
  id: string;
  // Keep the actual bytes the CLI hashes, even across host upgrades and restarts.
  request: string;
  start?: boolean;
}
interface SavedBoard {
  pending?: Pending;
  filed?: readonly BoardTaskStatus[];
}

function loadBoard(path: string): SavedBoard {
  try { return JSON.parse(readFileSync(path, "utf8")) as SavedBoard; }
  catch (error) {
    if ((error as NodeJS.ErrnoException).code === "ENOENT") return {};
    throw error;
  }
}

function syncDirectory(dir: string): void {
  const fd = openSync(dir, "r");
  try { fsyncSync(fd); } finally { closeSync(fd); }
}

/** The host never substitutes a new proposal ID after sending an authorized filing. */
function saveBoard(dir: string, path: string, saved: SavedBoard): void {
  mkdirSync(dir, { recursive: true, mode: 0o700 });
  const staging = join(dir, `.filing-${crypto.randomUUID()}.tmp`);
  try {
    writeFileSync(staging, JSON.stringify(saved), { mode: 0o600, flag: "wx", flush: true });
    renameSync(staging, path);
    if (saved.pending?.start !== undefined) {
      // A new build can have freshly created ancestors. Flush the whole path before the engine can commit,
      // even if a previously interrupted mkdir left some of those directories already present.
      for (let here = realpathSync(dir); ; here = dirname(here)) {
        syncDirectory(here);
        if (here === dirname(here)) break;
      }
    } else syncDirectory(dir);
  } finally { rmSync(staging, { force: true }); }
}

class InvalidPlan extends Error {}

/** Keep exact identifiers distinct even on surfaces that collapse spaces or interpret bidi text. */
export function boardLabel(value: string, quoted = false): string {
  return !quoted && /^[\w./:@-]+$/.test(value) ? value : JSON.stringify(value)
    .replace(/[ \u007f-\uffff]/g, unit => "\\u" + unit.charCodeAt(0).toString(16).padStart(4, "0"));
}

/** The complete decision-critical text must fit the remote card before optional titles or briefs. */
export function planIdentityLines(plan: BoardPlan): string[] {
  return plan.tasks.map(task => `${boardLabel(task.key, true)} → ${boardLabel(task.project ?? plan.project ?? "", true)}`
    + (task.depends.length ? ` · after ${task.depends.map(dep => boardLabel(dep, true)).join(", ")}` : " · no dependencies"));
}

function effectivePlan(plan: BoardPlan, fallback?: string): BoardPlan {
  const project = plan.project ?? fallback;
  if (!project) throw new Error("Board plans require a configured default project.");
  return validatePlan({ ...plan, project, tasks: plan.tasks.map(task => ({ ...task, project: task.project ?? project })) });
}

export function createBoard(options: {
  context: BuildContext; run: Run; tmpDir: string; changed: () => void; now?: () => number; pollMs?: number;
}): BoardControl & { propose(plan: BoardPlan): Promise<void>; close(): void } {
  const root = options.context.session.workspaceRoot;
  const path = join(options.tmpDir, "filing.json");
  const saved = root ? loadBoard(path) : {};
  const now = options.now ?? Date.now;
  const listeners = new Set<() => void>();
  const known = new Map<string, TaskReadback>();
  let statuses: readonly BoardTaskStatus[] = saved.filed ?? [];
  let pending = saved.pending;
  let proposed: BoardPlan | undefined = pending && JSON.parse(pending.request) as BoardPlan;
  let proposing = false;
  const receipts = new WeakMap<BoardPlan, { start: boolean; tasks: readonly BoardTaskStatus[] }>();
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
  async function submit<T>(args: readonly string[], input: string): Promise<T> {
    const cwd = available();
    const result = await options.run(["--root", cwd, "--json", "board-plan", ...args], { cwd, input });
    let value;
    try { value = JSON.parse(result.stdout); } catch { /* A missing/unreadable reply is not a refusal. */ }
    if (result.code === 0 && value !== undefined) return value as T;
    if (result.code === 2 && value?.code === "invalid_plan" && typeof value.refusal === "string") throw new InvalidPlan(value.refusal);
    throw new Error(result.stderr.trim() || value?.refusal || "No valid reply from corral-next board-plan.");
  }
  function persist(next: Pending | undefined, filed = statuses) {
    saveBoard(options.tmpDir, path, { pending: next, filed: filed.slice(-100) });
    pending = next;
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
      const value = known.get(task.id);
      return error ? { ...task, phase: "failed", detail: error, updatedAt: now() }
        : value ? { ...boardTask(value, known, now()), key: task.key, project: value.project ?? task.project } : task;
    });
    changed(); schedule();
  }
  function refresh(): Promise<void> {
    if (!root || closed) return Promise.resolve();
    if (refreshing) return refreshing;
    refreshing = doRefresh().finally(() => { refreshing = undefined; });
    return refreshing;
  }
  async function file(plan: BoardPlan, start: boolean, saved: Pending): Promise<readonly BoardTaskStatus[]> {
    // Persist the owner-authorized mode before sending; an uncertain reply is never permission to switch modes.
    persist({ ...saved, start });
    try {
      const receipt = await submit<FilingReceipt>(["file", "--id", saved.id, ...(start ? ["--start"] : [])], saved.request);
      if (!Array.isArray(receipt?.tasks) || receipt.tasks.length !== plan.tasks.length || receipt.tasks.some((row, index) => {
        const item = plan.tasks[index]!;
        return row?.key !== item.key || row.project !== item.project || !/^T-\d+$/.test(row.id)
          || row.status !== (start ? "ready" : "backlog") || !Array.isArray(row.depends_on);
      })) throw new Error("Corral returned an incomplete or mismatched filing receipt.");
      const values = receipt.tasks.map((row, index) => {
        const { title, size, difficulty, tags } = plan.tasks[index]!;
        return { ...row, title, size, difficulty, tags };
      });
      for (const value of values) known.set(value.id, value);
      const tasks = values.map(value => ({ ...boardTask(value, known, now()), key: value.key }));
      const next = [...statuses, ...tasks];
      persist(undefined, next);
      statuses = next;
      proposed = undefined;
      receipts.set(plan, { start, tasks });
      changed();
      schedule();
      return tasks;
    } catch (error) {
      // The engine only issues invalid_plan after checking that this ID did not already commit.
      if (error instanceof InvalidPlan) {
        persist({ id: saved.id, request: saved.request });
        changed();
        throw error;
      }
      throw new Error(`Filing was not acknowledged. Retry the same choice to recover it: ${error instanceof Error ? error.message : String(error)}`);
    }
  }
  schedule();
  return {
    tasks() { return statuses; },
    proposed() { return proposed; },
    pendingChoice() { return pending?.start; },
    async propose(plan) {
      available();
      if (pending || proposing) throw new Error("Another board plan is already pending. Resolve it before proposing a new plan.");
      const effective = effectivePlan(plan, options.context.session.project);
      if (Buffer.byteLength(JSON.stringify(planIdentityLines(effective).join("\n"))) > 7 * 1024) {
        throw new Error("Board plan keys, projects and dependencies exceed the confirmation preview budget.");
      }
      const request = JSON.stringify(effective);
      if (Buffer.byteLength(request) > 128 * 1024) throw new Error("Board plans may contain at most 128 KiB of text and metadata.");
      proposing = true;
      try {
        await submit<null>(["check"], request);
        if (closed) throw new Error("The board session closed during validation.");
        persist({ id: crypto.randomUUID(), request });
        proposed = JSON.parse(request) as BoardPlan;
        changed();
      } finally { proposing = false; }
    },
    async preview(plan): Promise<BoardPreview> {
      available();
      // The engine assigns IDs only when the owner confirms; preview shows the plan's actual keys.
      const tasks = effectivePlan(plan, options.context.session.project).tasks.map(({ key, title, project, depends, size, difficulty, tags }) =>
        ({ key, id: key, title, project, depends, size, difficulty, tags }));
      return { tracker: plan.title, tasks };
    },
    async file(plan, start) {
      available();
      const prior = receipts.get(plan);
      if (prior?.start === start) return prior.tasks;
      if (!pending || proposed !== plan) throw new Error("That board plan is no longer pending.");
      if (pending.start !== undefined && pending.start !== start) throw new Error("Retry the original filing choice; its result is not yet known.");
      return filing ??= file(plan, start, pending).finally(() => { filing = undefined; });
    },
    dismiss() {
      if (!pending) return;
      if (filing || pending.start !== undefined) throw new Error("Retry the original filing choice before replacing or dismissing it.");
      persist(undefined);
      proposed = undefined;
      changed();
    },
    refresh,
    close() { closed = true; clearTimeout(timer); },
    subscribe(listener) { listeners.add(listener); return () => listeners.delete(listener); },
  };
}
