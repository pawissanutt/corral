// The bound task's spec and status, read from `corral show --json`. Presentation only: the
// task record is canonical; criteria shown here are the spec, never proof they passed.
import { z } from "zod";
import type { HostIdentity } from "../roles/types.ts";
import { plainLabel, type PanelLine } from "./panels-view.ts";

const Text = z.string().max(65_536);
const TaskSchema = z.object({
  id: Text, title: Text, status: Text,
  status_since: Text.optional(), kind: Text.nullable().optional(), size: Text.nullable().optional(),
  difficulty: Text.nullable().optional(), priority: Text.nullable().optional(), project: Text.optional(),
  body: Text.nullable().optional(),
  criteria_progress: z.array(z.object({
    criterion: z.object({ id: Text, required: z.boolean(), scenario: Text, expected: Text }),
    steps: z.array(z.unknown()).default([]),
    evidence: z.array(z.unknown()).default([]),
  })).max(100).optional(),
  runs: z.array(z.object({
    id: Text, agent: Text.nullable().optional(), outcome: Text.nullable().optional(),
    agent_state: Text.nullable().optional(), started: Text.nullable().optional(), ended: Text.nullable().optional(),
    summary: Text.nullable().optional(),
  })).max(200).default([]),
  comments: z.array(z.object({ at: Text, author: Text, text: Text })).max(500).default([]),
});
export type TaskRecord = z.infer<typeof TaskSchema>;

/** One cached, validated read. Rendering never does I/O. */
export class TaskStore {
  current: TaskRecord | undefined;
  error: string | undefined;
  stale = true; refreshing = false; checkedAt: number | undefined;
  private active: Promise<void> | undefined; private version = 0; private disposed = false;
  constructor(private readonly identity: HostIdentity, private readonly load: (() => Promise<unknown>) | undefined, private readonly changed: () => void) {}
  get bound(): boolean { return !!this.identity.task && !!this.load; }
  refresh(force = false): Promise<void> {
    if (this.disposed || !this.bound) return Promise.resolve();
    if (this.active) return this.active;
    if (!force && !this.stale && this.checkedAt && Date.now() - this.checkedAt < 5000) return Promise.resolve();
    const operation = this.read(); this.active = operation;
    void operation.finally(() => { if (this.active === operation) this.active = undefined; });
    return operation;
  }
  private async read(): Promise<void> {
    const version = this.version;
    this.refreshing = true; this.changed();
    try {
      if (!this.load) throw new Error("No task bound to this session");
      const data = TaskSchema.parse(await this.load());
      if (data.id !== this.identity.task) throw new Error("Task identity mismatch");
      if (this.disposed || version !== this.version) return;
      this.current = data; this.error = undefined; this.stale = false; this.checkedAt = Date.now();
    } catch (error) {
      if (this.disposed || version !== this.version) return;
      this.error = error instanceof Error ? error.message : String(error); this.stale = true;
    } finally { this.refreshing = false; if (!this.disposed) this.changed(); }
  }
  dispose(): void { this.disposed = true; this.version++; }
}

const STATUS_GLYPH: Readonly<Record<string, string>> = { running: "●", review: "◐", done: "✓", landed: "✓", blocked: "⚠", failed: "✗", backlog: "○", ready: "○", idea: "·" };
export function statusChip(status: string): string {
  return `${STATUS_GLYPH[status] ?? "◇"} ${status.replaceAll("_", " ")}`;
}
/** Header facts: task ID, title and the small chips. Undefined when no task is bound. */
export function taskHeadline(identity: HostIdentity, store: TaskStore): { readonly id: string; readonly title: string; readonly chips: readonly string[] } | undefined {
  if (!identity.task) return undefined;
  const data = store.current;
  if (!data) return { id: identity.task, title: store.error ? "Task unavailable" : "Reading task…", chips: [] };
  const chips = [statusChip(data.status), data.priority, data.size].filter((chip): chip is string => !!chip).map(plainLabel);
  return { id: data.id, title: plainLabel(data.title), chips: store.stale ? [...chips, "stale"] : chips };
}

const SECTION = /^#{1,3}\s+(.*)$/;
/** Body sections in order; text before the first heading is "Summary". */
function sections(body: string): { readonly heading: string; readonly lines: string[] }[] {
  const result: { heading: string; lines: string[] }[] = [];
  let current = { heading: "Summary", lines: [] as string[] };
  for (const raw of body.split("\n")) {
    const heading = SECTION.exec(raw);
    if (heading) { if (current.lines.some(line => line.trim())) result.push(current); current = { heading: heading[1] ?? "", lines: [] }; continue; }
    current.lines.push(raw);
  }
  if (current.lines.some(line => line.trim())) result.push(current);
  return result;
}

function age(at: string | null | undefined, now: number): string {
  const time = at ? Date.parse(at) : NaN;
  if (Number.isNaN(time)) return "";
  const minutes = Math.max(0, Math.round((now - time) / 60_000));
  return minutes < 60 ? `${minutes}m` : minutes < 48 * 60 ? `${Math.round(minutes / 60)}h` : `${Math.round(minutes / 1440)}d`;
}

export function taskLines(identity: HostIdentity, store: TaskStore, now = Date.now()): PanelLine[] {
  const rows: PanelLine[] = [];
  const add = (text: string, tone?: PanelLine["tone"], extra: Partial<PanelLine> = {}) => rows.push({ text: plainLabel(text), ...(tone ? { tone } : {}), ...extra });
  if (!identity.task) {
    add(identity.role === "assistant" ? "No task bound · this assistant session holds no task" : "No task bound to this session", "muted");
    return rows;
  }
  if (store.stale && store.error) add("STALE / unavailable: " + store.error, "warning");
  else if (store.refreshing && !store.current) add("Reading task…", "muted");
  const data = store.current;
  if (!data) return rows;
  const facts = [data.kind, data.size, data.difficulty, data.project].filter(Boolean).join(" · ");
  add(`${data.status.replaceAll("_", " ")}${data.status_since ? ` for ${age(data.status_since, now)}` : ""}${facts ? " · " + facts : ""}`, "muted");
  add("");
  for (const section of sections(data.body ?? "")) {
    add(section.heading, "accent", { heading: true });
    for (const line of section.lines.join("\n").trim().split("\n")) {
      const bullet = /^\s*[-*]\s+(.*)$/.exec(line);
      if (bullet) add("• " + (bullet[1] ?? ""), undefined, { indent: 1, continuationIndent: 3 });
      else add(line);
    }
    add("");
  }
  if (!data.body?.trim()) { add("No task description", "muted"); add(""); }
  const criteria = data.criteria_progress ?? [];
  if (criteria.length) {
    add(`Criteria · ${criteria.length} in spec (proof lives in Proof, F10)`, "accent", { heading: true });
    for (const c of criteria) {
      add(`${c.criterion.required ? "☐" : "◇"} ${c.criterion.id} · ${c.criterion.expected}`, undefined, { continuationIndent: 2 });
      add(`${c.steps.length} plan step${c.steps.length === 1 ? "" : "s"} · ${c.evidence.length} evidence record${c.evidence.length === 1 ? "" : "s"} · not a pass`, "muted", { indent: 2 });
    }
    add("");
  }
  const run = data.runs.find(run => run.id === identity.run) ?? data.runs.at(-1);
  if (run) {
    add(`Run ${run.id}${run.id === identity.run ? " (this session)" : ""}`, "accent", { heading: true });
    add([run.agent, run.agent_state ?? run.outcome, run.started ? `started ${age(run.started, now)} ago` : ""].filter(Boolean).join(" · "), "muted");
    if (run.summary) for (const line of run.summary.split("\n")) add(line);
    if (data.runs.length > 1) add(`${data.runs.length - 1} earlier run${data.runs.length === 2 ? "" : "s"}`, "muted");
    add("");
  }
  const comments = data.comments.slice(-3);
  if (comments.length) {
    add(`Comments · last ${comments.length} of ${data.comments.length}`, "accent", { heading: true });
    for (const comment of comments) {
      add(`${comment.author} · ${age(comment.at, now)} ago`, "muted");
      add(comment.text, undefined, { indent: 2, continuationIndent: 2 });
    }
  }
  return rows;
}
