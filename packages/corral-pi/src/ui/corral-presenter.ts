// Rows for corral tool calls in the kit's grammar. One entry per corral operation speaks as the agent ("Finished the
// run", "Attached report: Session record") and takes the outcome from the call's structured result: on success the
// engine's or host's result itself, on failure its `{reason, next?}`, never the text the model reads. Direct calls
// and Code Mode children (through `presenterFor`, which may carry only the result text) share it. Presentation only:
// tool results are read, never changed.
import { getMarkdownTheme } from "@earendil-works/pi-coding-agent";
import { Container, Markdown, Spacer, Text, type Component } from "@earendil-works/pi-tui";

import { LIVE_CHILD_STATES } from "../build/types.ts";
import { helperConversationId } from "../extensions/subagent/conversation.ts";
import type { CorralToolSpec } from "../roles/types.ts";
import type { PanelView } from "./panels.ts";
import { resultText, type BodyLine, type CallInput, type Presenter, type RowModel, type RowState, type Tone } from "./tool-row.ts";
import type { PresenterModule, ToolRenderDeps } from "./tool-rendering.ts";

type Fields = Readonly<Record<string, unknown>>;
type Op = CorralToolSpec["op"];

/** What the presenter reads from the host. Every field is absent in plain contexts (a Code Mode child's row). */
export type CorralDeps = Partial<Pick<ToolRenderDeps, "openView" | "ledger" | "corralSpecs">>;

/** Collapsed prose rows: lines kept once settled, and the newest lines kept while the arguments stream. */
const PROSE_ROWS = 2;
const STREAM_ROWS = 3;
/** A failure's reason rows. */
const REASON_ROWS = 3;

/** Views a corral row points to, and their labels. */
const VIEW_LABEL: Readonly<Partial<Record<PanelView, string>>> = {
  plan: "Plan", artifacts: "Artifacts", proof: "Proof", task: "Task",
};

function record(value: unknown): value is Fields {
  return value !== null && typeof value === "object" && !Array.isArray(value);
}
function fields(value: unknown): Fields {
  return record(value) ? value : {};
}
function str(value: unknown): string | undefined {
  return typeof value === "string" && value.trim() !== "" ? value : undefined;
}
function int(value: unknown): number | undefined {
  return typeof value === "number" && Number.isFinite(value) ? value : undefined;
}
function list(value: unknown): readonly unknown[] {
  return Array.isArray(value) ? value : [];
}
function flat(value: string): string {
  return value.replace(/\s+/g, " ").trim();
}
function plural(count: number, one: string, many = `${one}s`): string {
  return `${count.toLocaleString("en-US")} ${count === 1 ? one : many}`;
}
function words(code: string): string {
  return code.replaceAll("_", " ");
}
function basename(path: string): string {
  return path.split(/[/:]/).filter(Boolean).at(-1) ?? path;
}

// ---------------------------------------------------------------------------------------------------------------
// The settled outcome of a call, from structured data.

type Settled =
  | { readonly kind: "result"; readonly result: Fields }
  | { readonly kind: "failure"; readonly reason: string; readonly next: readonly string[] }
  | { readonly kind: "schema"; readonly field: string }
  | { readonly kind: "text"; readonly text: string };

/** The first field error of Pi's schema refusal (`Validation failed for tool "x":\n  - op: expected …`). */
function schemaField(text: string): string {
  const line = text.split("\n").map(row => row.trim()).find(row => row.startsWith("- "));
  return line ? line.slice(2) : "the arguments do not match the tool";
}

function parsed(text: string): Fields | undefined {
  const trimmed = text.trim();
  if (!trimmed.startsWith("{")) return undefined;
  try {
    const value: unknown = JSON.parse(trimmed);
    return record(value) ? value : undefined;
  } catch { return undefined; }
}

/** A corrective step as one line: the engine's command text, or its structured step as compact JSON. */
function nextLines(value: unknown): readonly string[] {
  return list(value).flatMap(item => typeof item === "string" ? str(item) ? [flat(item)] : [] : [JSON.stringify(item) ?? ""])
    .filter(Boolean);
}

/** The engine's own refusal text (`refused: <reason>` then `next: <command>` lines), as a Code Mode child carries it. */
function engineRefusal(text: string): Settled | undefined {
  const [first = "", ...rest] = text.trim().split("\n");
  if (!first.startsWith("refused: ")) return undefined;
  const at = rest.findIndex(line => line.startsWith("next: "));
  const reason = [first.slice(9), ...rest.slice(0, at < 0 ? undefined : at)].join("\n");
  const next = at < 0 ? [] : rest.slice(at).filter(line => line.startsWith("next: ")).map(line => line.slice(6));
  return { kind: "failure", reason, next: nextLines(next) };
}

function settle(call: CallInput): Settled | undefined {
  if (!call.result || call.partial) return undefined;
  const details = fields(call.result.details), text = resultText(call.result);
  if (text.startsWith("Validation failed for tool")) return { kind: "schema", field: schemaField(text) };
  if (call.isError) {
    const failure = str(details.reason) ? details : parsed(text);
    if (failure && str(failure.reason)) return { kind: "failure", reason: String(failure.reason), next: nextLines(failure.next) };
    return engineRefusal(text) ?? { kind: "text", text };
  }
  return { kind: "result", result: Object.keys(details).length ? details : parsed(text) ?? {} };
}

// ---------------------------------------------------------------------------------------------------------------
// Reasons in plain words.

/** Codes the host-local todo ledger refuses with. */
const CODE_TEXT: Readonly<Record<string, string>> = {
  multiple_current_steps: "another step is already in progress",
  invalid_todo_wait: "that wait is not valid for this step",
  terminal_todo_step: "that step is already finished",
};

/** A reason with known codes said in words, and a bare code with its underscores as spaces. */
function plainReason(raw: string): string {
  const text = flat(raw).replace(/[;.\s]+$/, "");
  const coded = /^([a-z][a-z0-9]*(?:_[a-z0-9]+)+)(?::\s*(.*))?$/.exec(text), code = coded?.[1];
  if (!code) return text;
  const rest = coded?.[2] ?? "";
  return CODE_TEXT[code] ?? (rest ? `${words(code)}: ${rest}` : words(code));
}

interface Failure { readonly word: string; readonly reason: string; readonly next: readonly string[] }

/** Not sent (Pi or the host refused the arguments), refused (a reason came back) or failed, and why. */
function failureOf(settled: Settled): Failure | undefined {
  switch (settled.kind) {
    case "schema":
      return { word: "not sent", reason: settled.field, next: [] };
    case "text":
      return { word: "failed", reason: settled.text.split("\n").find(line => line.trim()) ?? "", next: [] };
    case "failure":
      return settled.reason.startsWith("Invalid arguments:")
        ? { word: "not sent", reason: settled.reason, next: settled.next }
        : { word: "refused", reason: plainReason(settled.reason), next: settled.next };
    case "result":
      return undefined;
  }
}

// ---------------------------------------------------------------------------------------------------------------
// Expanded content.

interface ExpandedParts {
  readonly reference?: string;
  readonly reason?: string;
  readonly prose?: string;
  readonly answer?: string;
  readonly next?: readonly string[];
}

function expandedView({ reference, reason, prose, answer, next }: ExpandedParts): () => Component {
  return () => {
    const parts = new Container();
    const add = (component: Component) => {
      if (parts.children.length) parts.addChild(new Spacer(1));
      parts.addChild(component);
    };
    if (reference) add(new Text(reference, 0, 0));
    if (reason) add(new Text(reason, 0, 0));
    if (prose) add(new Markdown(prose, 0, 0, getMarkdownTheme()));
    if (answer) add(new Markdown(answer, 0, 0, getMarkdownTheme()));
    if (next?.length) add(new Text(next.map(line => `next: ${line}`).join("\n"), 0, 0));
    return parts;
  };
}

// ---------------------------------------------------------------------------------------------------------------
// One entry per operation.

interface Ctx {
  readonly args: Fields;
  /** The successful call's result; undefined before it settles and for failures. */
  readonly result: Fields | undefined;
  readonly call: CallInput;
  readonly deps: CorralDeps;
}
type Done = Ctx & { readonly result: Fields };
type Phrase = string | ((c: Ctx) => string);
interface Digest {
  readonly state?: RowState;
  readonly outcome?: string;
  readonly tone?: Tone;
}
interface Entry {
  /** What the agent did, as it would say it ("Attached"). */
  readonly did: Phrase;
  /** While the call runs ("Attaching"). */
  readonly doing: Phrase;
  /** What a failure is about ("Attach"): `✗ Attach · refused`. */
  readonly noun: Phrase;
  readonly target?: (c: Ctx) => string | undefined;
  readonly digest?: (c: Done) => Digest | undefined;
  /** The text the agent wrote, shown under the row and in full when expanded. */
  readonly prose?: (c: Ctx) => string | undefined;
  readonly view?: PanelView;
}
function phrase(value: Phrase, c: Ctx): string {
  return typeof value === "string" ? value : value(c);
}

// Todo: the plan's position and the step's label come from the ledger's result, or the host ledger before it.
interface PlanStep { readonly id: string; readonly label: string; readonly state: string }
function planSteps(progress: unknown): readonly PlanStep[] | undefined {
  if (!record(progress) || !Array.isArray(progress.phases)) return undefined;
  return progress.phases.flatMap(phase => list(fields(phase).steps).map(fields).flatMap(step =>
    str(step.id) ? [{ id: String(step.id), label: str(step.label) ?? String(step.id), state: str(fields(step.status).state) ?? "pending" }] : []));
}
const TODO_DID: Readonly<Record<string, string>> = {
  init: "Opened a plan", append: "Added steps to", start: "Started step", done: "Finished step", drop: "Dropped step", wait: "Step waits",
};
const TODO_DOING: Readonly<Record<string, string>> = {
  init: "Opening a plan", append: "Adding steps to", start: "Starting step", done: "Finishing step", drop: "Dropping step", wait: "Parking step",
};
function todoTarget(c: Ctx): string | undefined {
  const op = str(c.args.op), progress = c.result?.progress ?? c.deps.ledger?.()?.current;
  if (op === "init") return str(c.result?.plan);
  if (op === "append") {
    const phase = str(c.args.phase) ?? "";
    const known = list(fields(progress).phases).map(fields).find(item => item.id === phase);
    return str(known?.label) ?? str(c.args.label) ?? phase;
  }
  const step = str(c.args.step);
  if (!step) return undefined;
  return planSteps(progress)?.find(item => item.id === step)?.label ?? step;
}
function todoDigest(c: Done): Digest | undefined {
  const op = str(c.args.op), steps = planSteps(c.result.progress);
  const counted = steps?.filter(step => step.state !== "dropped");
  const position = counted ? `plan ${counted.filter(step => step.state === "done").length}/${counted.length}` : undefined;
  if (op === "init") {
    const phases = list(fields(c.result.progress).phases).length;
    return steps ? { outcome: `${plural(phases, "phase")}, ${plural(steps.length, "step")}` } : undefined;
  }
  if (op === "append") return { outcome: [`+${plural(list(c.args.steps).length, "step")}`, position].filter(Boolean).join(" · ") };
  if (op === "wait") {
    const on = fields(c.args.on), disposition = fields(c.result.disposition);
    if (on.kind === "owner" || disposition.state === "awaiting_owner") return { state: "needs-you", outcome: "waiting for your answer", tone: "warning" };
    return { state: "waiting", outcome: str(on.event) ? `on ${flat(String(on.event))}` : "waiting" };
  }
  return position ? { outcome: position } : undefined;
}

const ENTRIES: Readonly<Record<Op, Entry>> = {
  done: {
    did: "Finished the run", doing: "Finishing the run", noun: "Finish", view: "task",
    digest: () => ({ outcome: "sent to review" }),
    prose: c => str(c.args.summary),
  },
  verdict: {
    did: c => c.args.verdict === "approve" ? "Approved the candidate" : c.args.verdict === "changes" ? "Requested changes" : "Recorded the verdict",
    doing: "Recording the verdict", noun: "Verdict", view: "proof",
    prose: c => str(c.args.notes),
  },
  artifact: {
    did: "Attached", doing: "Attaching", noun: "Attach", view: "artifacts",
    target: c => {
      const title = str(c.args.title), path = str(c.args.path);
      return title ? `${str(c.args.kind) ? `${String(c.args.kind)}: ` : ""}${flat(title)}` : path ? basename(path) : undefined;
    },
  },
  todo: {
    did: c => TODO_DID[str(c.args.op) ?? ""] ?? "Updated the plan",
    doing: c => TODO_DOING[str(c.args.op) ?? ""] ?? "Updating the plan",
    noun: "Plan update", view: "plan",
    target: todoTarget, digest: todoDigest,
    prose: c => c.args.op === "drop" ? str(c.args.reason) : str(fields(c.args.on).question),
  },
  checkpoint: {
    did: "Saved checkpoint", doing: "Saving a checkpoint", noun: "Checkpoint",
    prose: c => str(c.args.note),
  },
};

/** The corral operation behind each model-facing tool name. */
export const CORRAL_TOOL_OPS: Readonly<Record<string, Op>> = {
  corral_done: "done", corral_verdict: "verdict", corral_artifact: "artifact", corral_todo: "todo", corral_checkpoint: "checkpoint",
};

/** Prose under the row: the first rows once settled, the newest while the arguments stream; the kit marks a cut with `…`. */
function proseBody(prose: string, streaming: boolean): readonly BodyLine[] {
  return [{ text: prose, tone: "muted", wrap: streaming ? { max: STREAM_ROWS, from: "end" } : { max: PROSE_ROWS } }];
}
function reasonBody(reason: string): readonly BodyLine[] {
  return reason ? [{ text: reason, tone: "muted", wrap: { max: REASON_ROWS } }] : [];
}

/** The row of one corral operation's call. */
function presentOperation(op: Op, call: CallInput, deps: CorralDeps): RowModel {
  const entry = ENTRIES[op], settled = settle(call);
  const result = settled?.kind === "result" ? settled.result : undefined;
  const ctx: Ctx = { args: fields(call.args), result, call, deps };
  // A refused schema says nothing about what the arguments meant.
  const known = settled?.kind !== "schema";
  const target = known ? entry.target?.(ctx) : undefined;
  const prose = known ? entry.prose?.(ctx)?.trim() || undefined : undefined;
  const at = (model: RowModel): RowModel => target && model.target === undefined ? { ...model, target } : model;

  if (!settled) {
    const state: RowState = call.argsComplete ? "running" : "writing";
    return at({
      state, verb: phrase(entry.doing, ctx),
      ...(prose && state === "writing" ? { outcome: plural(prose.length, "char") } : {}),
      ...(prose ? { body: proseBody(prose, state === "writing") } : {}),
    });
  }

  const failure = failureOf(settled);
  if (failure) {
    return at({
      state: "failed", verb: phrase(entry.noun, ctx), outcome: failure.word, outcomeTone: "error",
      body: reasonBody(failure.reason),
      expanded: expandedView({ ...(failure.reason ? { reason: failure.reason } : {}), ...(prose ? { prose } : {}), next: failure.next }),
    });
  }

  const digest = result ? entry.digest?.({ ...ctx, result }) : undefined;
  const state: RowState = digest?.state ?? "ok";
  const open = deps.openView, label = entry.view ? VIEW_LABEL[entry.view] : undefined, view = entry.view;
  return at({
    state, verb: phrase(entry.did, ctx),
    ...(digest?.outcome ? { outcome: digest.outcome } : {}),
    ...(digest?.tone ? { outcomeTone: digest.tone } : {}),
    ...(prose ? { body: proseBody(prose, false), expanded: expandedView({ prose }) } : {}),
    ...(open && label && view ? { view: { label, open: () => open(view) } } : {}),
  });
}

// ---------------------------------------------------------------------------------------------------------------
// Tools that are not corral operations.

function runningState(call: CallInput): RowState | undefined {
  return call.result && !call.partial ? undefined : call.argsComplete ? "running" : "writing";
}

const expertPresenter: Presenter = call => {
  const question = str(call.args.question) ?? "", target = question ? flat(question) : undefined;
  const live = runningState(call);
  if (live) {
    const progress = fields(fields(call.result?.details).progress), latest = resultText(call.result).split("\n").find(line => line.trim());
    const counts = [int(progress.turns) !== undefined ? plural(int(progress.turns) ?? 0, "turn") : undefined,
      int(progress.tools) !== undefined ? plural(int(progress.tools) ?? 0, "tool call") : undefined].filter(Boolean).join(" · ");
    return {
      state: live, verb: "Consulting the expert", ...(target ? { target } : {}), ...(counts ? { outcome: counts } : {}),
      ...(live === "writing" && question ? { body: proseBody(question, true) }
        : latest ? { body: [{ text: latest, tone: "muted", wrap: { max: 1 } }] } : {}),
    };
  }
  const answer = resultText(call.result).trim();
  const id = helperConversationId(call.name, fields(call.result?.details).conversationId);
  const reference = id ? `Conversation ID: ${id}` : undefined;
  if (call.isError) {
    return { state: "failed", verb: "Expert consultation", ...(target ? { target } : {}), outcome: "failed", outcomeTone: "error", body: reasonBody(answer),
      ...(reference ? { expanded: expandedView({ reference, reason: answer }) } : {}) };
  }
  return {
    state: "ok", verb: "Expert answered", ...(target ? { target } : {}),
    ...(answer ? { body: proseBody(answer, false) } : {}),
    ...(answer || reference ? { expanded: expandedView({ reference, ...(question ? { prose: `**Question**\n\n${question}` } : {}), answer }) } : {}),
  };
};

/** A request is not proof of cancellation: only settled runner teardown reports success. */
const cancelAgentPresenter: Presenter = call => {
  const id = str(call.args.id);
  const target = id ? { target: id } : {};
  const live = runningState(call);
  if (live) return { ...target, verb: "Stopping", state: live };
  const reply = resultText(call.result).trim(), details = fields(call.result?.details);
  if (details.stopping === true || /^Still stopping /i.test(reply)) {
    return { ...target, verb: "Stop", state: "waiting", outcome: "still stopping" };
  }
  if (call.isError || details.cancelled === false || /nothing cancelled/i.test(reply)) {
    return { ...target, verb: "Stop", state: "failed", outcome: "not confirmed stopped", outcomeTone: "error", body: reasonBody(reply) };
  }
  return { ...target, verb: "Cancelled", state: "ok" };
};

/** An agent tool's outcome: its details, or the JSON of its text (a Code Mode child carries only text). */
function agentOutcome(call: CallInput): Fields {
  const details = fields(call.result?.details);
  if (record(details.message)) return details;
  try { return fields(JSON.parse(resultText(call.result))); } catch { return details; }
}

const UNANSWERED: Readonly<Record<string, string>> = {
  timeout: "no answer · timed out", would_deadlock: "no answer · would deadlock", recipient_finished: "no answer · it finished",
};

/** `agent_message` (src/extensions/fleet/tools.ts): who it went to, and what became of it. */
const agentMessagePresenter: Presenter = call => {
  const to = str(call.args.to), text = str(call.args.text) ?? "", replyTo = int(call.args.reply_to);
  const target = to ? (to === "all" ? "all agents" : to) + (replyTo !== undefined ? ` #${replyTo}` : "") : undefined;
  const live = runningState(call);
  const base = { ...(target ? { target } : {}) };
  if (live) {
    return {
      ...base, state: live, verb: replyTo !== undefined ? "Replying to" : "Messaging",
      ...(call.args.wait_reply === true && live === "running" ? { outcome: "waiting for a reply" } : {}),
      ...(text ? { body: proseBody(text, live === "writing") } : {}),
    };
  }
  const verb = replyTo !== undefined ? "Replied to" : "Messaged";
  const outcome = agentOutcome(call), message = fields(outcome.message);
  const delivery = str(message.delivery), reply = str(outcome.reply), report = str(outcome.report), unanswered = str(outcome.unanswered);
  if (call.isError || delivery === "undeliverable") {
    return { ...base, state: "failed", verb, outcome: "not delivered", outcomeTone: "error", body: reasonBody(resultText(call.result).trim()) };
  }
  const [state, said]: [RowState, string] = delivery === "queued" ? ["ok", "queued · read when it starts"]
    : delivery === "answered_from_report" ? ["ok", "it had finished · its report came back"]
    : reply !== undefined ? ["ok", "answered"]
    : unanswered !== undefined ? ["unknown", UNANSWERED[unanswered] ?? `no answer · ${words(unanswered)}`]
    : ["ok", "delivered"];
  const shown = reply ?? report ?? text;
  const prose = "**Sent**\n\n" + text + (reply ? "\n\n**Reply**\n\n" + reply : report ? "\n\n**Report**\n\n" + report : "");
  return {
    ...base, state, verb, outcome: said,
    ...(shown ? { body: proseBody(shown, false) } : {}),
    ...(text ? { expanded: expandedView({ prose }) } : {}),
  };
};

/** Rows of an agent_wait listing children, before `… n more`. */
const WAIT_ROWS = 6;

/** `agent_wait`: the children it collected, one row each. */
const agentWaitPresenter: Presenter = call => {
  const ids = list(call.args.ids).map(str).filter((id): id is string => id !== undefined);
  const target = ids.length ? ids.join(", ") : "every live agent";
  const live = runningState(call);
  if (live) return { state: live, verb: "Waiting for", target };
  const details = fields(call.result?.details), text = resultText(call.result);
  const children = list(details.children).map(fields);
  const stateOf = (child: Fields) => str(child.state) ?? "unknown";
  const isLive = (child: Fields) => (LIVE_CHILD_STATES as readonly string[]).includes(stateOf(child));
  const done = children.filter(child => stateOf(child) === "done").length;
  const failed = children.filter(child => ["failed", "cancelled"].includes(stateOf(child))).length;
  const running = children.filter(isLive).length, settled = children.length - running;
  const parts = children.length === 0 ? ["none to collect"] : [
    settled ? `${settled} settled` : "", done ? `✓${done}` : "", failed ? `✗${failed}` : "", running ? `${running} still running` : "",
  ].filter(Boolean);
  if (details.interrupted === true) parts.push("new input arrived");
  const rows: BodyLine[] = children.slice(0, WAIT_ROWS).map(child => {
    const state = stateOf(child), bad = state === "failed" || state === "cancelled";
    const first = (str(child.report) ?? "").split("\n").find(line => line.trim());
    return { text: `${state === "done" ? "✓" : bad ? "✗" : "○"} ${str(child.id) ?? "?"} · ${state}${first ? ` · ${first.trim()}` : ""}`,
      tone: bad ? "error" : "muted", wrap: { max: 1 } };
  });
  if (children.length > WAIT_ROWS) rows.push({ text: `… ${children.length - WAIT_ROWS} more`, tone: "muted" });
  return {
    state: call.isError || (children.length > 0 && failed === children.length) ? "failed" : "ok",
    verb: "Collected", target, outcome: parts.join(" · "),
    ...(rows.length ? { body: rows } : {}), ...(text.trim() ? { expanded: expandedView({ prose: text }) } : {}),
  };
};

/** `agent_roster`: how many agents there are and how many are live. */
const agentRosterPresenter: Presenter = call => {
  const live = runningState(call);
  if (live) return { state: live, verb: "Listing agents" };
  const text = resultText(call.result);
  const rows = text.split("\n").map(line => line.split(" | ")).filter(fields => fields.length >= 3);
  const running = rows.filter(fields => fields[2] === "live" || (LIVE_CHILD_STATES as readonly string[]).includes(fields[2]!)).length;
  return {
    state: call.isError ? "failed" : "ok", verb: "Listed agents", outcome: `${plural(rows.length, "agent")} · ${running} live`,
    ...(text.trim() ? { expanded: expandedView({ prose: "```\n" + text + "\n```" }) } : {}),
  };
};

const OTHER: Readonly<Record<string, Presenter>> = {
  corral_expert: expertPresenter,
  agent_message: agentMessagePresenter,
  agent_wait: agentWaitPresenter,
  agent_roster: agentRosterPresenter,
  agent_cancel: cancelAgentPresenter,
};

/** The row presenter of a corral tool, or undefined for any other tool. */
export function corralPresenter(name: string, deps: CorralDeps = {}): Presenter | undefined {
  const op = CORRAL_TOOL_OPS[name] ?? deps.corralSpecs?.find(spec => spec.name === name)?.op;
  if (op) return call => presentOperation(op, call, deps);
  return OTHER[name];
}

/** Every `corral_*` tool and the agent tools (`agent_message`, `agent_wait`, `agent_roster`, `agent_cancel`). Build cards keep their own renderers. */
export const corralPresenters: PresenterModule = {
  presenter: (name, deps) => corralPresenter(name, deps),
};
