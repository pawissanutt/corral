// /handoff: the assistant side of the one handoff experience. Reads the shared responsibility
// projection, shows the goal-aware sheet, and only after an actual keyboard/pointer selection
// journals a native Choice bound to the exact action payload, then asks corral to consume it once
// through the same C3 services the board and owner shell use. No model call can reach this path.
import type { ExtensionUIContext } from "@earendil-works/pi-coding-agent";
import type { EventLog } from "../bridge/events.ts";
import { type CorralClient, eventRequestId, sha256Hex, toolRequestId } from "../tools/corral.ts";
import { type ContextMode, HandoffSheet, PreviewView, implementationActions, planActions, type SheetAction, type SheetResult } from "../ui/handoff-sheet.ts";

export type HandoffState = {
  driver: "assistant" | "board" | "reconciling"; label: string; waiting: string | null; plan_only: boolean;
  phase: string | null; task?: string | null; contract_revision?: string; transfer?: string | null; revision?: number | null;
  shown?: string; implement?: ImplementOffer | { unavailable: string } | null;
  planning?: Planning | null; local?: LocalOffer | { unavailable: string } | null;
};
/** Corral's offer for assistant-led work: the project and, without a workflow, the entry request. */
/** A ready local plan revision and its declared scope; only then is Implement here offered. */
type ImplementOffer = { project: string; plan_revision: number; paths: string[]; actions: string[] };
type LocalOffer = { project: string; entry: string | null; goal_text: string; title: string };
type Review = { review?: string; revision?: number; current_revision?: number | null; freshness?: string; lifecycle?: string };
type Planning = { plan_revision: number | null; plan_stage: string | null; reviews: Review[] | { unavailable: string } };

/** Exact local-review freshness: a review of r3 after r4 is published stays stale, never approves r4. */
export function planningLine(planning: Planning | null | undefined): string | undefined {
  if (!planning) return undefined;
  const plan = planning.plan_revision ? `Plan r${planning.plan_revision} · ${planning.plan_stage ?? "draft"}` : "No plan revision yet";
  if (!Array.isArray(planning.reviews)) return `${plan} · Local review unavailable: ${planning.reviews.unavailable}`;
  const last = planning.reviews.at(-1);
  if (!last) return `${plan} · No local review`;
  const state = last.freshness === "current" ? "current" : `stale (current r${last.current_revision ?? "?"})`;
  return `${plan} · Local review r${last.revision ?? "?"} ${last.lifecycle ?? "unknown"} · ${state} · advisory only`;
}
type Native = { action: "save" | "take_back"; task: string; contract_revision: string }
  | { action: "start"; choice: { task: string; goal: string } & Record<string, unknown> }
  | { action: "implement"; implement: { project: string; plan_revision: number } }
  | { action: "local"; local: { project: string; goal: string; context: ContextMode; save: boolean; title: string;
      entry: string | null; history?: History } };

/** Visible conversation through a named checkpoint: one contiguous, bounded range. */
export type History = { branch: string; checkpoint: string; messages: { ordinal: number; role: "user" | "assistant"; text: string }[];
  excluded: string[] };
/** Bound selections travel inside 64 KiB envelopes, so the archived range is kept small. */
export const HISTORY_BUDGET = 16 * 1024;

/** Most recent contiguous visible messages that fit the budget; everything else is named as excluded. */
export function selectHistory(branch: string, checkpoint: string, visible: { role: "user" | "assistant"; text: string }[]): History | undefined {
  const messages: History["messages"] = [];
  let bytes = 0;
  for (let i = visible.length - 1; i >= 0; i--) {
    const m = visible[i]!;
    const size = Buffer.byteLength(m.text) + 64;
    if (bytes + size > HISTORY_BUDGET) break;
    bytes += size;
    messages.unshift({ ordinal: i + 1, role: m.role, text: m.text });
  }
  if (!messages.length) return undefined;
  const earlier = messages[0]!.ordinal - 1;
  return { branch, checkpoint, messages, excluded: [
    ...(earlier ? [`${earlier} earlier visible messages (over the ${HISTORY_BUDGET / 1024} KiB selection budget)`] : []),
    "tool calls and results, reasoning, images and attachments",
  ] };
}

export const LOCAL_UNAVAILABLE = "No project or request to hand off yet; nothing was prepared. "
  + "Bind a project or describe the work in this pane first.";
export const CONVERSATION_UNAVAILABLE = "No visible conversation on this branch yet; choose Task or Minimal context.";

function implementOffer(state: HandoffState): ImplementOffer | undefined {
  const ready = state.implement;
  return state.driver === "assistant" && ready && "plan_revision" in ready ? ready : undefined;
}

function offer(state: HandoffState): LocalOffer | undefined {
  const local = state.local;
  return local && "project" in local ? local : undefined;
}

export interface HandoffDeps {
  client: CorralClient; events: EventLog; generation: number;
  sessionId: () => string; turn: () => number | undefined;
  ui: Pick<ExtensionUIContext, "custom">; hasUI: boolean;
  /** Re-read authority after a consumed choice (planning ends, scratch opens, scope leaves). */
  after?: () => Promise<void>;
  /** The current branch's visible conversation selection, if any. */
  history?: () => History | undefined;
}

let reads = 0;
async function call(deps: HandoffDeps, op: "assistant_handoff" | "event", payload: Record<string, unknown>, requestId: string) {
  const receipt = await deps.client.call(op, { requestId, payload, turn: deps.turn() });
  if (receipt.state !== "committed") throw new Error(`${String((receipt.result as { reason?: unknown })?.reason ?? JSON.stringify(receipt.result)).slice(0, 400)}`);
  return receipt.result as Record<string, unknown>;
}

export function readState(deps: HandoffDeps): Promise<HandoffState> {
  return call(deps, "assistant_handoff", { action: "read" },
    toolRequestId(deps.sessionId(), `handoff-read:${deps.generation}:${++reads}`)) as Promise<HandoffState>;
}

/** Board continuation goals: the option id IS the goal, so the binding names it exactly. */
export const START_GOALS = new Set(["plan_continue", "plan_review_only", "implementation_ship"]);

/** The sheet's actions for this state. Board-owned work: Save, the goal-aware continuation
 * for its kind (plan goals never offer implementation), and Work on this here. */
export function sheetFor(state: HandoffState, history = false): { actions: SheetAction[]; plan: boolean; withContext: boolean;
  offContexts?: Partial<Record<ContextMode, string>> } {
  if (state.driver === "board" && state.task && state.contract_revision) {
    const saved = /Held/.test(state.label) ? "Already saved and held" : undefined;
    const goals: SheetAction[] = state.plan_only ? [
      { id: "plan_continue", label: "Continue planning", stop: "Stops at a reviewed plan; never code" },
      { id: "plan_review_only", label: "Review this plan", stop: "One independent review; findings only" },
    ] : [{ id: "implementation_ship", label: "Take over and finish", stop: "Board continues under its policy; ships when checks pass" }];
    return { plan: state.plan_only, withContext: false, actions: [
      { id: "save_for_later", label: state.plan_only ? "Save plan for later" : "Save for later", stop: "Held; no agent starts", disabled: saved },
      ...goals,
      { id: "work_on_this_here", label: "Work on this here", stop: state.plan_only ? "Returns planning-only; no coding scratch" : "Returns prepare-only; Check/Ship needs a new instruction" },
    ] };
  }
  const reconciling = state.driver === "reconciling" ? `Reconciling: ${state.waiting ?? state.label}` : undefined;
  const plan = state.phase === "planning";
  const local = state.driver === "assistant" ? offer(state) : undefined;
  const missing = state.local && "unavailable" in state.local ? `Unavailable: ${state.local.unavailable}` : LOCAL_UNAVAILABLE;
  const reason = reconciling ?? (local ? undefined : missing);
  const off = reason ? Object.fromEntries(["save_for_later", "continue_selected_goal", "plan_continue", "plan_review_only"].map(k => [k, reason])) : {};
  const ready = implementOffer(state);
  const implement: SheetAction = ready
    ? { id: "implement_here", label: "Implement here", stop: `Opens a scratch for plan r${ready.plan_revision} scope `
      + `(${ready.project}: ${ready.paths.slice(0, 3).join(", ")}${ready.paths.length > 3 ? ", …" : ""}); not shipping consent` }
    : { id: "implement_here", label: "Implement here", stop: "", disabled: reconciling
      ?? "Needs a ready plan revision with a declared scope (publish it as ready first)" };
  // Read-only: the exact context, range, exclusions and scope; never prepares or commits.
  const preview: SheetAction[] = local ? [{ id: "preview_context", label: "Preview context",
    stop: "Shows exactly what would be packaged; changes nothing" }] : [];
  return { plan, withContext: !reconciling, offContexts: history ? {} : { conversation: CONVERSATION_UNAVAILABLE },
    actions: [...preview, ...(plan ? [...planActions(off), implement] : implementationActions(off))] };
}

const GOALS: Record<string, string> = { plan_continue: "plan_continue", plan_review_only: "plan_review_only",
  continue_selected_goal: "implementation_ship" };

export function nativeFor(state: HandoffState, choice: NonNullable<SheetResult>, history?: History): Native | undefined {
  const ready = implementOffer(state);
  if (choice.action === "implement_here") {
    return ready ? { action: "implement", implement: { project: ready.project, plan_revision: ready.plan_revision } } : undefined;
  }
  const local = state.driver === "assistant" ? offer(state) : undefined;
  if (local) {
    const save = choice.action === "save_for_later";
    const goal = save ? (state.phase === "planning" ? "plan_continue" : "implementation_prepare") : GOALS[choice.action];
    if (!goal || (choice.context === "conversation" && !history)) return undefined;
    return { action: "local", local: { project: local.project, goal, context: choice.context, save, title: local.title, entry: local.entry,
      ...(choice.context === "conversation" ? { history } : {}) } };
  }
  if (!state.task || !state.contract_revision) return undefined;
  if (choice.action === "save_for_later") return { action: "save", task: state.task, contract_revision: state.contract_revision };
  if (choice.action === "work_on_this_here") return { action: "take_back", task: state.task, contract_revision: state.contract_revision };
  return undefined; // goal continuations need corral's exact current-revision preview first
}

/** Board continuation: corral previews the exact current-revision Choice for the goal. */
async function startFor(deps: HandoffDeps, state: HandoffState, goal: string): Promise<Native | undefined> {
  if (state.driver !== "board" || !START_GOALS.has(goal)) return undefined;
  const preview = await call(deps, "assistant_handoff", { action: "continue_preview", goal },
    toolRequestId(deps.sessionId(), `handoff-continue:${deps.generation}:${++reads}`));
  const choice = preview.choice as { task: string; goal: string } & Record<string, unknown> | undefined;
  return choice && choice.task === state.task && choice.goal === goal ? { action: "start", choice } : undefined;
}

/** Truthful package preview: archived vs loaded, the exact range/checkpoint and what is excluded. */
export function previewLines(p: Record<string, unknown>): string[] {
  if (typeof p.unavailable === "string") return [`Preview unavailable: ${p.unavailable}`, "Nothing changed."];
  const range = Array.isArray(p.range) ? `messages #${p.range[0]}–#${p.range[1]}` : "no conversation history";
  return [
    `Project ${String(p.project)} · scope: ${(p.scope as string[] | undefined)?.join(", ") || "none (choice would refuse)"}`,
    `Source: ${p.source ? JSON.stringify(p.source) : "no scratch"}`,
    `Context: ${String(p.mode)} · ${range}${p.checkpoint ? ` through checkpoint ${String(p.checkpoint)}` : ""}`,
    `Archived ${String(p.archived_messages)} messages (${String(p.archived_bytes)} bytes); loaded into the first prompt `
      + `${String(p.loaded_messages)} (${String(p.loaded_bytes)} bytes)`,
    `Native fork: ${p.native_fork ? "yes" : "no"} · ${String(p.semantics)}`,
    ...((p.excluded as string[] | undefined)?.length ? ["Excluded:", ...(p.excluded as string[]).map(e => `  - ${e}`)] : []),
    "", `Initial prompt (${String(p.prompt_bytes)} bytes):`, ...String(p.prompt).split("\n"),
  ];
}

/** Returns a plain owner-facing outcome line. Cancel and Escape change nothing. */
export async function runHandoff(deps: HandoffDeps): Promise<{ text: string; level: "info" | "warning" | "error" }> {
  const state = await readState(deps);
  if (!deps.hasUI) return { text: `${state.label}. Handoff choices need the interactive pane; nothing changed.`, level: "warning" };
  const history = deps.history?.();
  const shape = sheetFor(state, history !== undefined);
  const local = state.driver === "assistant" ? offer(state) : undefined;
  const summary = [state.label, state.waiting, planningLine(state.planning),
    local ? `${local.project} · ${local.goal_text.split("\n")[0]}` : undefined].filter(Boolean).join(" · ");
  const overlay = { overlay: true, overlayOptions: { width: "80%" as const, maxHeight: "85%" as const, anchor: "center" as const } };
  let context: ContextMode = "task";
  let choice: SheetResult;
  for (;;) {
    choice = await deps.ui.custom<SheetResult>((tui, _theme, _keys, done) => {
      const sheet = new HandoffSheet(state.task ?? local?.title ?? "current work", summary, shape.plan, shape.actions, done,
        () => tui.requestRender(), shape.withContext, shape.offContexts,
        () => Math.floor(tui.terminal.rows * 0.85));
      sheet.context = context;
      return sheet;
    }, overlay);
    if (choice?.action !== "preview_context" || !local) break;
    context = choice.context;
    const probe = nativeFor(state, { action: "save_for_later", context }, history);
    const lines = probe?.action === "local"
      ? previewLines(await call(deps, "assistant_handoff", { action: "local_preview", local: probe.local },
        toolRequestId(deps.sessionId(), `handoff-preview:${deps.generation}:${++reads}`)).catch((e: unknown) =>
        ({ unavailable: e instanceof Error ? e.message : String(e) })))
      : ["Preview unavailable for this context; nothing changed."];
    await deps.ui.custom<void>((tui, _theme, _keys, done) =>
      new PreviewView(`Preview · ${context} context`, lines, done, () => tui.requestRender(),
        () => Math.floor(tui.terminal.rows * 0.85)), overlay);
  }
  if (!choice) return { text: "Handoff cancelled; nothing changed.", level: "info" };
  const native = nativeFor(state, choice, history) ?? await startFor(deps, state, choice.action);
  if (!native) return { text: "That choice is unavailable here; nothing changed.", level: "warning" };
  if (!state.shown) return { text: "Handoff state is unavailable; nothing changed.", level: "warning" };
  // Bind to exactly what the sheet showed; corral refuses if any of it changed since.
  const bound = await call(deps, "assistant_handoff", { action: "binding", native, shown: state.shown },
    toolRequestId(deps.sessionId(), `handoff-binding:${deps.generation}:${++reads}`));
  if (bound.option !== choice.action || typeof bound.digest !== "string") return { text: "Handoff binding changed; refresh and choose again.", level: "warning" };
  const text = choice.action;
  const sha256 = sha256Hex(text);
  const journaled = deps.events.emit("owner_input", { source: "choice", sha256, bytes: Buffer.byteLength(text) });
  const input = await call(deps, "event", { kind: "owner_input", source: "choice", text, sha256, binding: bound.digest },
    eventRequestId(deps.generation, journaled.seq));
  if (typeof input.input !== "string") throw new Error("native choice was not journaled");
  const subject = native.action === "local" ? native.local.project
    : native.action === "implement" ? native.implement.project
    : native.action === "start" ? native.choice.task : native.task;
  const consumed = deps.events.emit("handoff_choice", { input: input.input, action: native.action, task: subject });
  const result = await call(deps, "event", { kind: "handoff_choice", input: input.input, native, shown: state.shown },
    eventRequestId(deps.generation, consumed.seq));
  await deps.after?.().catch(() => undefined);
  const after = await readState(deps).catch(() => undefined);
  return { text: `${after?.label ?? `${native.action} ${subject}`}${after?.waiting ? ` · ${after.waiting}` : ""}`
    + ` (${String(result.action ?? native.action)}${typeof result.task === "string" ? ` ${result.task}` : ""})`, level: "info" };
}
