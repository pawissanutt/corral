// Build children as agents-view rows, their actions over FleetControl, and their edits as Changes authors.
// Every value comes from the fleet snapshot; unknown cost and context stay absent, never zero.
import type { AgentActions, AgentExtras, AgentNote, AgentRow, AgentSpan, AgentEnd, AgentState } from "../agents-model.ts";
import { agentStatus, duration } from "../agents-model.ts";
import type { ChangeAuthor } from "../changes-view.ts";
import { helperText } from "../helper-chat.ts";
import { plainLabel } from "../panels-view.ts";
import { LIVE_CHILD_STATES } from "../../build/types.ts";
import type { BuildPanelState, ChildSnapshot, ChildState, FleetControl, FleetSnapshot } from "../../build/types.ts";
import { childHue, childIdOf, childKey } from "./contract.ts";

const BUCKET = 5000, HISTORY_CAP = 240;

function clean(text: string): string { return plainLabel(helperText(text)); }
function firstLine(text: string | undefined): string {
  return clean((text ?? "").split("\n").find(line => line.trim()) ?? "");
}
function shortModel(model: string): string { return clean(model.slice(model.lastIndexOf("/") + 1)); }
function live(state: ChildState): boolean { return LIVE_CHILD_STATES.includes(state); }
function editedPaths(child: ChildSnapshot): { readonly files: readonly string[]; readonly outside: readonly string[] } {
  const files = [...new Set(child.edits.map(edit => edit.path))];
  const outside = [...new Set(child.edits.filter(edit => edit.outside).map(edit => edit.path))];
  return { files, outside };
}
/** The first dependency that has not finished, or undefined when only a free slot is missing. */
function waitingFor(child: ChildSnapshot, fleet: FleetSnapshot): string | undefined {
  return child.after.find(id => fleet.children.find(other => other.id === id)?.state !== "done");
}

/** One to one: queued, running, needs, done, failed, cancelled. */
export function childState(state: ChildState): AgentState { return state; }

/** Steerable now: running or needs (the runtime refuses a child that has no session yet). */
export function steerable(child: ChildSnapshot): boolean { return child.state === "running" || child.state === "needs"; }

/** Live children whose `after` reaches `id`, transitively: the ones a cancel also cancels. */
export function dependents(fleet: FleetSnapshot, id: string): readonly string[] {
  const reached = new Set([id]);
  for (let grew = true; grew;) {
    grew = false;
    for (const child of fleet.children) {
      if (reached.has(child.id) || !live(child.state) || !child.after.some(after => reached.has(after))) continue;
      reached.add(child.id); grew = true;
    }
  }
  return fleet.children.filter(child => child.id !== id && reached.has(child.id)).map(child => child.id);
}

/** The activity column text (section 1.1). */
export function childActivity(child: ChildSnapshot, fleet: FleetSnapshot): string {
  switch (child.state) {
    case "running": return clean(child.activity ?? "") || "working";
    case "needs": return "waiting for your answer";
    case "queued": {
      const blocker = waitingFor(child, fleet);
      return blocker === undefined ? "waits for a free slot" : `waits for ${clean(blocker)}`;
    }
    case "done": {
      const { files, outside } = editedPaths(child), report = firstLine(child.report);
      return [`${files.length} file${files.length === 1 ? "" : "s"}`, ...(outside.length ? [`${outside.length} outside its files`] : []), ...(report ? [report] : [])].join(" · ");
    }
    case "failed": return firstLine(child.error) || "failed";
    case "cancelled": return "cancelled · its edits stay in the worktree";
  }
}

/**
 * `history` (one char per 5 s bucket since `fleet.startedAt`: q r n d f c, space before creation) as spans.
 * Bucket i starts at `origin + (first + i) × 5000`, where `first` is 0 while the history is shorter than 240
 * characters and `floor(((endedAt ?? now) − origin) / 5000) − length + 1` once it is truncated.
 */
export function childSpans(child: ChildSnapshot, origin: number, now: number): { readonly spans: AgentSpan[]; readonly end?: AgentEnd } {
  const history = child.history, length = history.length;
  const first = length < HISTORY_CAP ? 0 : Math.floor(((child.endedAt ?? now) - origin) / BUCKET) - length + 1;
  const at = (index: number) => origin + (first + index) * BUCKET;
  const spans: AgentSpan[] = [];
  let open: { kind: AgentSpan["kind"]; from: number } | undefined;
  const close = (to?: number) => {
    if (open) spans.push(to === undefined ? { kind: open.kind, from: open.from } : { kind: open.kind, from: open.from, to });
    open = undefined;
  };
  for (let index = 0; index < length; index++) {
    const mark = history[index];
    if (mark === "d" || mark === "f" || mark === "c") {
      const endAt = child.endedAt ?? at(index);
      close(endAt);
      return { spans, end: { at: endAt, outcome: mark === "d" ? "done" : mark === "f" ? "failed" : "cancelled" } };
    }
    const kind = mark === "q" ? "wait" : mark === "r" ? "run" : mark === "n" ? "ask" : undefined;
    if (kind === open?.kind) continue;
    close(at(index));
    if (kind) open = { kind, from: at(index) };
  }
  // A live child's last stretch stays open and is drawn up to now.
  if (live(child.state)) close();
  else close(child.endedAt ?? at(length));
  return { spans };
}

export function childRow(child: ChildSnapshot, fleet: FleetSnapshot, now: number): AgentRow {
  const id = clean(child.id), state = childState(child.state), { files, outside } = editedPaths(child);
  const status = agentStatus(state, child.startedAt === undefined ? undefined : duration((child.endedAt ?? now) - child.startedAt));
  const pending = child.pendingModel === undefined ? undefined : shortModel(child.pendingModel);
  const meta = [`model ${clean(child.model)}`, ...(child.effort ? [`effort ${clean(child.effort)}`] : []), `call ${clean(child.call)}`,
    ...(child.sessionFile ? [`session file ${clean(child.sessionFile)}`] : [])];
  const blocker = child.state === "queued" ? waitingFor(child, fleet) : undefined;
  // Detail order: the live wait (muted), a pending model, edits outside its files, the error.
  const notes: AgentNote[] = [
    ...(child.state === "queued" ? [{ text: blocker === undefined ? `Waiting for a free slot · ${fleet.maxParallel} at once` : `Waiting for ${clean(blocker)}`, tone: "muted" as const }] : []),
    ...(child.pendingModel ? [{ text: `Model change pending: ${clean(child.pendingModel)}, from its next request`, tone: "accent" as const }] : []),
    ...(outside.length ? [{ text: "Edited outside its files: " + outside.map(clean).join(", "), tone: "warning" as const }] : []),
    ...(child.error ? [{ text: "Error: " + clean(child.error), tone: "error" as const }] : []),
  ];
  const finished = child.state === "done" || child.state === "failed" || child.state === "cancelled";
  const { spans, end } = childSpans(child, fleet.startedAt ?? child.createdAt, now);
  return {
    key: childKey(child.id), source: "child", id: child.id, name: id, title: firstLine(child.title), hue: childHue(fleet, child.id),
    profile: clean(child.profile), model: shortModel(child.model), ...(pending === undefined ? {} : { pendingModel: pending }),
    state, status, createdAt: child.createdAt,
    ...(child.startedAt === undefined ? {} : { startedAt: child.startedAt }),
    ...(child.endedAt === undefined ? {} : { endedAt: child.endedAt }),
    activity: childActivity(child, fleet),
    usage: {
      ...(child.usage.cost === undefined ? {} : { cost: child.usage.cost }),
      ...(child.usage.contextPercent === undefined ? {} : { contextPercent: child.usage.contextPercent }),
      tokens: child.usage.input + child.usage.output,
    },
    files: files.length, outside: outside.length, pulse: child.pulse,
    spans, ...(end ? { end } : {}),
    owns: child.owns.map(clean), after: child.after.map(clean), steers: child.steers.map(clean),
    ...(child.question ? { question: clean(child.question.question) } : {}),
    meta, notes,
    capabilities: { open: true, transcript: false, steer: true, cancel: true, model: true, answer: child.state === "needs" },
    ...(finished ? { disabled: { steer: "not running", cancel: "not running", model: "not running" } }
      : child.state === "queued" ? { disabled: { steer: "not started yet" } } : {}),
    conversation: { request: child.id, prompt: child.brief, phase: child.state, messages: child.messages,
      ...(child.report === undefined ? {} : { result: child.report }) },
  };
}

export function childRows(fleet: FleetSnapshot, now: number): AgentRow[] {
  return fleet.children.map(child => childRow(child, fleet, now));
}

/** `{ agents: childRows(...), maxParallel: fleet.maxParallel, origin: fleet.startedAt }`; undefined outside build. */
export function fleetExtras(state: BuildPanelState | undefined, now: number): AgentExtras | undefined {
  if (!state) return undefined;
  const fleet = state.fleet;
  return { agents: childRows(fleet, now), maxParallel: fleet.maxParallel, ...(fleet.startedAt === undefined ? {} : { origin: fleet.startedAt }) };
}

export interface ChildActionHost {
  readonly fleet: FleetControl;
  /** `ui.setTarget(id)`, then logical focus to the composer. */
  point(id: string): void;
  /** Docks the child's question dialog. */
  question(id: string): void;
  /** Pi's confirm dialog. */
  confirm(title: string, message: string): Promise<boolean>;
  /** Pi's select dialog; resolves the chosen label. */
  select(title: string, options: readonly string[]): Promise<string | undefined>;
  notice(text: string, tone: "text" | "success" | "warning" | "error"): void;
}

/** Model choices for `m`: profiles first (`deep · anthropic/claude-opus-5.5`, value = profile name), then approved models no profile covers (value = `<provider>/<id>`); the current one ends with ` · current`, a pending one with ` · pending`. */
export function modelChoices(fleet: FleetControl, child: ChildSnapshot): readonly { readonly label: string; readonly value: string }[] {
  const profiles = fleet.profiles();
  const entries = [
    ...profiles.map(profile => ({ text: `${clean(profile.name)} · ${clean(profile.model)}`, value: profile.name, model: profile.model })),
    ...fleet.models().filter(model => !profiles.some(profile => profile.model === model)).map(model => ({ text: clean(model), value: model, model })),
  ];
  // The runtime names a child's profile after its model when it runs on a bare model.
  const byProfile = entries.findIndex(entry => entry.value === child.profile);
  const current = byProfile >= 0 ? byProfile : entries.findIndex(entry => entry.model === child.model);
  const pending = child.pendingModel === undefined ? -1 : entries.findIndex(entry => entry.model === child.pendingModel);
  return entries.map((entry, index) => ({
    label: entry.text + (index === current ? " · current" : index === pending ? " · pending" : ""), value: entry.value,
  }));
}

/** Ignores non-child keys, so it merges with the workbench's open/transcript: `{ ...workbenchActions, ...childActions(host) }`. */
export function childActions(host: ChildActionHost): Required<Pick<AgentActions, "steer" | "cancel" | "model" | "answer">> {
  const find = (key: string): { readonly child: ChildSnapshot; readonly fleet: FleetSnapshot } | undefined => {
    const id = childIdOf(key);
    if (id === undefined) return undefined;
    const fleet = host.fleet.snapshot(), child = fleet.children.find(item => item.id === id);
    return child ? { child, fleet } : undefined;
  };
  const failed = (error: unknown) => host.notice(error instanceof Error ? error.message : String(error), "error");
  return {
    steer(key) {
      const found = find(key);
      if (!found) return;
      const { child } = found, id = clean(child.id);
      if (steerable(child)) host.point(child.id);
      else host.notice(child.state === "queued" ? `${id} has not started yet` : `${id} is not running`, "warning");
    },
    cancel(key) {
      const found = find(key);
      if (!found) return;
      const { child, fleet } = found, id = clean(child.id);
      if (!live(child.state)) { host.notice(`${id} is no longer running`, "warning"); return; }
      const also = dependents(fleet, child.id).map(clean);
      const message = "Its edits so far stay in the worktree."
        + (also.length ? ` ${also.join(", ")} ${also.length === 1 ? "waits" : "wait"} for it and ${also.length === 1 ? "is" : "are"} cancelled too.` : "");
      void host.confirm(`Cancel ${id}?`, message).then(confirmed => {
        if (!confirmed) return;
        if (host.fleet.cancel(child.id)) host.notice(`Cancelled ${id} · its edits stay in the worktree`, "success");
        else host.notice(`${id} is no longer running`, "warning");
      }, failed);
    },
    model(key) {
      const found = find(key);
      if (!found) return;
      const { child } = found, id = clean(child.id);
      if (!live(child.state)) { host.notice(`${id} is not running`, "warning"); return; }
      const choices = modelChoices(host.fleet, child);
      void host.select(`Model for ${id}, from its next request`, choices.map(choice => choice.label)).then(label => {
        const choice = choices.find(item => item.label === label);
        if (!choice || label?.endsWith(" · current")) return;
        try {
          host.fleet.setModel(child.id, choice.value);
          host.notice(`${id} uses ${clean(choice.value)} from its next request`, "success");
        } catch (error) { failed(error); }
      }, failed);
    },
    answer(key) {
      const found = find(key);
      if (!found) return;
      const { child } = found;
      if (child.state === "needs") host.question(child.id);
      else host.notice(`${clean(child.id)} has no open question`, "warning");
    },
  };
}

/** Children with at least one edit; `paths` maps each edited path to true when any of its edits there was outside its owns. */
export function childAuthors(fleet: FleetSnapshot): ChangeAuthor[] {
  return fleet.children.filter(child => child.edits.length > 0).map(child => {
    const paths = new Map<string, boolean>();
    for (const edit of child.edits) paths.set(edit.path, (paths.get(edit.path) ?? false) || edit.outside);
    return { id: child.id, label: clean(child.id), hue: childHue(fleet, child.id), paths };
  });
}
