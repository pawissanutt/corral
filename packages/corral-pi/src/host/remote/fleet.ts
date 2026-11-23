import { lstatSync, realpathSync } from "node:fs";
import { isAbsolute, join, relative, resolve, sep } from "node:path";
import { BridgeRefusal } from "../../bridge/protocol.ts";
import { LIVE_CHILD_STATES, type ChildSnapshot, type FleetSnapshot } from "../../build/types.ts";
import type { HelperInspection } from "../../extensions/subagent/inspection.ts";
import { utf8Prefix } from "../../roles/assistant/owner-action.ts";
import { parseChildId } from "./input.ts";
import type { RemoteRegistry } from "./registry.ts";
import { jsonBytes, type Counts } from "./state.ts";

export const LIVE_FLEET_BYTES = 32 * 1024;

export interface SessionChild {
  id: string; kind: string; title: string; state: string; model: string;
  started_at?: number; ended_at?: number; activity?: string; tokens: number; cost_usd?: number;
  verdict?: "approve" | "changes"; question?: boolean; profile?: string; effort?: string; pending_model?: string;
  turns?: number; context_percent?: number; files?: number; outside?: number; owns?: number;
  after?: readonly string[]; steerable: boolean; error?: string;
}

export function helperState(helper: HelperInspection): string {
  return helper.phase === "preparing" ? "queued" : helper.phase === "completed" ? "done"
    : helper.phase === "uncertain" || helper.phase === "not_started" ? "failed" : helper.phase;
}

export interface SessionFleet {
  children: SessionChild[];
  messages: { seq: number; at: number; from: string; to: string; text: string; delivery: string }[];
  messages_total: number;
  max_parallel: number;
}

export function childRow(child: ChildSnapshot): SessionChild {
  return {
    id: child.id, kind: child.kind, title: utf8Prefix(child.title, 256), state: child.state, model: utf8Prefix(child.model, 512),
    started_at: child.startedAt, ended_at: child.endedAt,
    // Activity in the runtime includes arguments (often paths); publish only the tool name.
    activity: child.activity ? utf8Prefix(child.activity.split(/\s/, 1)[0]!, 64) : undefined,
    tokens: child.usage.input + child.usage.output, cost_usd: child.usage.cost,
    verdict: child.verdict?.verdict, question: child.question ? true : undefined,
    profile: utf8Prefix(child.profile, 128), effort: child.effort, pending_model: child.pendingModel,
    turns: child.usage.turns, context_percent: child.usage.contextPercent,
    files: child.edits.length, outside: child.edits.reduce((total, edit) => total + Number(edit.outside), 0),
    owns: child.owns.length, after: child.after, steerable: child.state === "running" || child.state === "needs",
    error: child.error === undefined ? undefined : utf8Prefix(child.error, 256),
  };
}

export function helperRow(helper: HelperInspection): SessionChild {
  return { id: helper.request, kind: `helper:${helper.kind}`, title: helper.kind, state: helperState(helper),
    model: utf8Prefix(helper.model, 512), started_at: helper.startedAt, ended_at: helper.endedAt, tokens: 0, steerable: false,
    error: helper.error === undefined ? undefined : utf8Prefix(helper.error, 256) };
}

export function projectFleet(snapshot: FleetSnapshot | undefined, helpers: readonly HelperInspection[] = []): {
  fleet: SessionFleet; counts: Counts;
} {
  const children: SessionChild[] = [];
  let live = 0, waiting = 0;
  for (const child of snapshot?.children ?? []) {
    children.push(childRow(child));
    if (LIVE_CHILD_STATES.includes(child.state)) live++;
    if (child.state === "needs" && child.question) waiting++;
  }
  for (const helper of helpers) {
    const state = helperState(helper);
    if (state === "queued" || state === "running") live++;
    children.push(helperRow(helper));
  }
  if (helpers.length && snapshot?.children.length) {
    const created = new Map(snapshot.children.map(child => [child.id, child.createdAt]));
    for (const helper of helpers) created.set(helper.request, helper.createdAt);
    children.sort((a, b) => created.get(a.id)! - created.get(b.id)!);
  }
  const messages = (snapshot?.mail ?? []).slice(-100).map(message => ({
    seq: message.seq, at: message.at, from: message.from.slice(0, 128), to: message.to.slice(0, 128),
    text: Array.from(message.text).slice(0, 2048).join(""), delivery: message.delivery,
  }));
  const fleet = { children, messages, messages_total: snapshot?.mail.at(-1)?.seq ?? 0, max_parallel: snapshot?.maxParallel ?? 0 };
  return { fleet, counts: { live, waiting, total: children.length } };
}

export function boundFleet(fleet: SessionFleet, limit = LIVE_FLEET_BYTES): SessionFleet {
  let bytes = jsonBytes({ ...fleet, children: [], messages: [] });
  // Newest rows win; all older messages are dropped before dropping any children.
  let firstChild = fleet.children.length, firstMessage = fleet.messages.length;
  while (firstChild > 0) {
    const cost = jsonBytes(fleet.children[firstChild - 1]) + Number(firstChild < fleet.children.length);
    if (bytes + cost > limit) break;
    bytes += cost;
    firstChild--;
  }
  if (firstChild === 0) while (firstMessage > 0) {
    const cost = jsonBytes(fleet.messages[firstMessage - 1]) + Number(firstMessage < fleet.messages.length);
    if (bytes + cost > limit) break;
    bytes += cost;
    firstMessage--;
  }
  if (firstChild === 0 && firstMessage === 0) return fleet;
  return { ...fleet, children: fleet.children.slice(firstChild), messages: fleet.messages.slice(firstMessage) };
}

/** Parent-local confinement: never follow a symlink in a child path. */
export function confinedRegularFile(root: string, candidate: string): string | undefined {
  if (!isAbsolute(candidate) || resolve(candidate) !== candidate) return undefined;
  const path = relative(root, candidate);
  if (!path || path === ".." || path.startsWith(`..${sep}`) || isAbsolute(path)) return undefined;
  try {
    let file = realpathSync(root);
    const parts = path.split(sep);
    for (const [index, part] of parts.entries()) {
      file = join(file, part);
      const stat = lstatSync(file);
      if (!(index === parts.length - 1 ? stat.isFile() : stat.isDirectory())) return undefined;
    }
    return file;
  } catch { return undefined; }
}

export function registerFleet(registry: RemoteRegistry, root: string, transcript: (child: string) => string | undefined): void {
  registry.register({ name: "child_file", parse: args => parseChildId(args.child), run: child => {
    const candidate = transcript(child);
    if (candidate !== undefined) try {
      const file = confinedRegularFile(realpathSync(root), candidate);
      if (file?.endsWith(".jsonl")) return { session_file: file };
    } catch { /* A removed session root is unavailable. */ }
    throw new BridgeRefusal("child_file_unavailable", "the child has no transcript in this session");
  } });
}
