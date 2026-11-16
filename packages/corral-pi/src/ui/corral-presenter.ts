// Rows for corral tool calls in the kit's grammar. One entry per corral operation speaks as the agent ("Asked the PM",
// "Read T-950 overview") and takes the outcome from the structured receipt or host failure, never from the text the
// model reads. Direct calls, Code Mode children (through `presenterFor`) and the host receipts panel share it.
// Presentation only: tool results are read, never changed.
import { type AgentToolResult, getMarkdownTheme } from "@earendil-works/pi-coding-agent";
import { Container, Markdown, Spacer, Text, type Component } from "@earendil-works/pi-tui";

import type { CorralOp } from "../roles/types.ts";
import type { PanelView } from "./panels.ts";
import { PANEL_MUTED, RESET } from "./panels-view.ts";
import { outputLines, resultText, type BodyLine, type CallInput, type Presenter, type RowModel, type RowState, type Tone } from "./tool-row.ts";
import type { PresenterModule, ToolRenderDeps } from "./tool-rendering.ts";

type Fields = Readonly<Record<string, unknown>>;

/** What the presenter reads from the host. Every field is absent in plain contexts (the host receipts panel). */
export type CorralDeps = Partial<Pick<ToolRenderDeps, "openView" | "ledger" | "job" | "corralSpecs">>;

/** Collapsed prose rows: lines kept once settled, and the newest lines kept while the arguments stream. */
const PROSE_ROWS = 2;
const STREAM_ROWS = 3;
/** A failure's reason rows. */
const REASON_ROWS = 3;
/** Expanded listings stop here. */
const LISTING_MAX = 400;

const GLYPH: Readonly<Record<RowState, string>> = {
  writing: "…", running: "…", ok: "✓", failed: "✗", waiting: "○", "needs-you": "▲", unknown: "!",
};
/** Views a corral row points to, and their labels. */
const VIEW_LABEL: Readonly<Partial<Record<PanelView, string>>> = {
  plan: "Plan", inbox: "Inbox", artifacts: "Artifacts", proof: "Proof", task: "Task",
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
function clock(value: unknown): string | undefined {
  const text = str(value);
  if (!text) return undefined;
  const time = new Date(text);
  return Number.isNaN(time.getTime()) ? text : time.toLocaleTimeString("en-GB", { hour: "2-digit", minute: "2-digit" });
}
function basename(path: string): string {
  return path.split(/[/:]/).filter(Boolean).at(-1) ?? path;
}

// ---------------------------------------------------------------------------------------------------------------
// The settled outcome of a call, from structured data.

type ReceiptState = "committed" | "rejected" | "reserved" | "queued" | "uncertain" | "abandoned";
const RECEIPT_STATES: Readonly<Record<ReceiptState, true>> = {
  committed: true, rejected: true, reserved: true, queued: true, uncertain: true, abandoned: true,
};
function receiptState(value: unknown): value is ReceiptState {
  return typeof value === "string" && Object.hasOwn(RECEIPT_STATES, value);
}
/** Receipt envelope keys; everything else on a reduced (Code Mode) receipt is a result field. */
const ENVELOPE: Readonly<Record<string, true>> = {
  version: true, op: true, state: true, request_id: true, replayed: true, identity: true, payload_sha256: true, at: true,
  committed_at: true, stored: true, effect: true, evidence: true, result: true,
};

type Settled =
  | { readonly kind: "receipt"; readonly state: ReceiptState; readonly result: Fields; readonly receipt: Fields }
  | { readonly kind: "failure"; readonly error: Fields; readonly request?: string }
  | { readonly kind: "schema"; readonly field: string }
  | { readonly kind: "text"; readonly text: string; readonly isError: boolean };

function receiptOf(details: Fields): Settled | undefined {
  const state = details.state;
  if (!receiptState(state)) return undefined;
  const result = record(details.result) ? details.result
    : Object.fromEntries(Object.entries(details).filter(([key]) => !Object.hasOwn(ENVELOPE, key)));
  return { kind: "receipt", state, result, receipt: details };
}

/** `key: value` lines as the host's deterministic receipt and failure text writes them (tools/presentation.ts). */
function renderedFields(lines: readonly string[]): Record<string, unknown> {
  const out: Record<string, unknown> = {};
  for (const line of lines) {
    const match = /^([A-Za-z_]\w*): (.*)$/.exec(line), key = match?.[1];
    if (!key) continue;
    const raw = match?.[2] ?? "";
    try { out[key] = JSON.parse(raw); } catch { out[key] = raw; }
  }
  return out;
}

/**
 * A Code Mode child may carry no structured value, only the text the host produced for it. That text has two fixed
 * shapes (`op: state` or `op: host error` plus `key: value` lines, or the receipt as JSON); read only those.
 */
function fromHostText(text: string): Settled | undefined {
  const trimmed = text.trim();
  if (trimmed.startsWith("{")) {
    try {
      const parsed: unknown = JSON.parse(trimmed);
      if (record(parsed)) return record(parsed.error) ? { kind: "failure", error: parsed.error } : receiptOf(parsed);
    } catch { return undefined; }
  }
  const [head = "", ...rest] = trimmed.split("\n");
  const match = /^[a-z_]+: (host error|committed|rejected|reserved|queued|uncertain|abandoned)( \(replayed\))?$/.exec(head);
  if (!match) return undefined;
  const values = renderedFields(rest);
  if (match[1] === "host error") return { kind: "failure", error: values };
  const result = Object.fromEntries(Object.entries(values).filter(([key]) => !Object.hasOwn(ENVELOPE, key)));
  const state = match[1];
  return receiptState(state) ? { kind: "receipt", state, result, receipt: { ...values, state, replayed: match[2] !== undefined } } : undefined;
}

/** The first field error of Pi's schema refusal (`Validation failed for tool "x":\n  - op: expected …`). */
function schemaField(text: string): string {
  const line = text.split("\n").map(row => row.trim()).find(row => row.startsWith("- "));
  return line ? line.slice(2) : "the arguments do not match the tool";
}

function settle(call: CallInput): Settled | undefined {
  if (!call.result || call.partial) return undefined;
  const details = fields(call.result.details), text = resultText(call.result);
  if (record(details.error)) return { kind: "failure", error: details.error, ...(str(details.request_id) ? { request: str(details.request_id) } : {}) };
  // Job tools report their own refusals as `{ error: "<message>" }`; nothing ran.
  if (typeof details.error === "string") return { kind: "failure", error: { reason: details.error, effect: "not_started" } };
  const receipt = receiptOf(details);
  if (receipt) return receipt;
  if (text.startsWith("Validation failed for tool")) return { kind: "schema", field: schemaField(text) };
  return fromHostText(text) ?? { kind: "text", text, isError: call.isError };
}

// ---------------------------------------------------------------------------------------------------------------
// Reasons in plain words.

const CODE_TEXT: Readonly<Record<string, (rest: string) => string>> = {
  checkpoint_required: () => "a checkpoint is required first",
  checkpoint_stale: rest => {
    const changed = (rest.split(";")[0] ?? "").split(",").map(item => words(item.trim().replace(/_changed$/, ""))).filter(Boolean);
    return `the checkpoint is out of date${changed.length ? ` (${changed.join(", ")} changed)` : ""}; save a new one`;
  },
  unknown_evidence_reference: rest => `evidence not found${rest ? `: ${rest}` : ""}`,
  multiple_current_steps: () => "another step is already in progress",
  invalid_todo_wait: () => "that wait is not valid for this step",
  terminal_todo_step: () => "that step is already finished",
  pending_obligation_removed: () => "a pending obligation would be dropped; keep it or record a resolution",
  gate_ref_stale: () => "the gate reference is not the run's current gate",
  pm_scope_unapproved: () => "no approved PM scope",
};

/** A corral reason without its process prefix, with known codes said in words. */
export function plainReason(raw: string): string {
  const text = flat(raw)
    .replace(/^call_\w+: corral agent call --op [\w-]+: exit=[^;]*;\s*/, "")
    .replace(/^error:\s*/, "")
    .replace(/[;.\s]+$/, "");
  const coded = /^([a-z][a-z0-9]*(?:_[a-z0-9]+)+)(?::\s*(.*))?$/.exec(text), code = coded?.[1];
  if (!code) return text;
  const rest = coded?.[2] ?? "";
  const say = CODE_TEXT[code];
  return say ? say(rest) : rest ? `${words(code)}: ${rest}` : words(code);
}

interface Failure { readonly state: RowState; readonly word: string; readonly reason: string }

/** Refused, not sent, failed or unknown, and why. Undefined for a committed or pending receipt. */
function failureOf(settled: Settled, read: boolean): Failure | undefined {
  switch (settled.kind) {
    case "schema":
      return { state: "failed", word: "not sent", reason: settled.field };
    case "text":
      return settled.isError ? { state: "failed", word: "failed", reason: settled.text.split("\n").find(line => line.trim()) ?? "" } : undefined;
    case "receipt": {
      const { result, state } = settled;
      const reason = str(result.reason) ? plainReason(String(result.reason)) : str(result.code) ? plainReason(String(result.code)) : "";
      if (state === "rejected") return { state: "failed", word: result.outcome === "not_started" ? "not started" : "refused", reason };
      if (state === "uncertain" || state === "abandoned") return { state: "unknown", word: "outcome unknown", reason };
      return undefined;
    }
    case "failure": {
      const error = settled.error, code = str(error.code) ?? "", effect = str(error.effect) ?? "unknown";
      const diagnostic = str(error.diagnostic) ?? "";
      const timedOut = code === "call_timeout" || error.timed_out === true;
      const reason = timedOut ? "corral did not answer in time"
        : diagnostic ? plainReason(diagnostic)
          : str(error.signal) ? "the corral call was interrupted" : plainReason(str(error.reason) ?? code);
      if (code === "bad_request" || code === "bad_payload" || error.category === "invalid_arguments") {
        return { state: "failed", word: "not sent", reason };
      }
      // corral answered with its own error and exit status: it refused the request.
      const refused = typeof error.exit_code === "number" && error.exit_code !== 0 && /^error:/.test(diagnostic.trim());
      if (refused) return { state: "failed", word: "refused", reason };
      if (effect === "not_started" || read) return { state: "failed", word: timedOut ? "timed out" : "failed", reason };
      return { state: "unknown", word: "outcome unknown", reason };
    }
  }
}

/** One muted line of receipt facts for the expanded view. */
function factsLine(settled: Settled): string {
  if (settled.kind === "receipt") {
    const receipt = settled.receipt, request = str(receipt.request_id);
    return [clock(receipt.committed_at ?? receipt.at) ? `recorded ${clock(receipt.committed_at ?? receipt.at)}` : undefined,
      settled.state, receipt.replayed === true ? "replayed" : undefined, receipt.stored === false ? "nothing stored" : undefined,
      request ? `request ${request}` : undefined].filter(Boolean).join(" · ");
  }
  if (settled.kind === "failure") {
    const error = settled.error;
    return [str(error.code), str(error.effect) ? `effect ${str(error.effect)}` : undefined,
      settled.request ? `request ${settled.request}` : undefined].filter(Boolean).join(" · ");
  }
  return "";
}

// ---------------------------------------------------------------------------------------------------------------
// Expanded content.

type Expanded = { readonly kind: "markdown"; readonly text: string } | { readonly kind: "lines"; readonly lines: readonly string[] };

function scalarText(value: unknown): string {
  return typeof value === "string" ? value : JSON.stringify(value) ?? String(value);
}
/** A JSON value as indented `key: value` lines, so a person reads fields instead of escaped JSON. */
function fieldLines(value: unknown, indent = ""): string[] {
  if (value === null || value === undefined) return [];
  if (typeof value !== "object") return scalarText(value).split("\n").map(line => indent + line);
  if (Array.isArray(value)) {
    return value.flatMap(item => item !== null && typeof item === "object"
      ? [`${indent}-`, ...fieldLines(item, `${indent}  `)] : [`${indent}- ${scalarText(item)}`]);
  }
  return Object.entries(value).flatMap(([key, item]) => {
    if (item === null || item === undefined || (Array.isArray(item) && item.length === 0)) return [];
    if (typeof item !== "object") {
      const [first = "", ...more] = scalarText(item).split("\n");
      return [`${indent}${key}: ${first}`, ...more.map(line => `${indent}  ${line}`)];
    }
    return [`${indent}${key}:`, ...fieldLines(item, `${indent}  `)];
  });
}
/** A `content_json` page: its fields when the page is a whole document, else the chunk with JSON escapes decoded. */
function jsonPage(value: unknown): Expanded | undefined {
  const text = str(value);
  if (!text) return undefined;
  try { return { kind: "lines", lines: fieldLines(JSON.parse(text)) }; } catch { /* a partial page */ }
  return { kind: "lines", lines: text.replace(/\\n/g, "\n").replace(/\\"/g, "\"").replace(/\\\\/g, "\\").split("\n") };
}
function fieldsContent(value: unknown): Expanded | undefined {
  const lines = fieldLines(value);
  return lines.length ? { kind: "lines", lines } : undefined;
}

function expandedView(prose: string | undefined, content: Expanded | undefined, facts: string, reason?: string): () => Component {
  return () => {
    const parts = new Container();
    const add = (component: Component) => {
      if (parts.children.length) parts.addChild(new Spacer(1));
      parts.addChild(component);
    };
    if (reason) add(new Text(reason, 0, 0));
    if (prose) add(new Markdown(prose, 0, 0, getMarkdownTheme()));
    if (content?.kind === "markdown") add(new Markdown(content.text, 0, 0, getMarkdownTheme()));
    if (content?.kind === "lines") {
      const shown = content.lines.slice(0, LISTING_MAX);
      const more = content.lines.length - shown.length;
      add(new Text(shown.join("\n") + (more > 0 ? `\n${PANEL_MUTED}… ${plural(more, "more line")}${RESET}` : ""), 0, 0));
    }
    if (facts) add(new Text(`${PANEL_MUTED}${facts}${RESET}`, 0, 0));
    return parts;
  };
}

// ---------------------------------------------------------------------------------------------------------------
// One entry per operation.

interface Ctx {
  readonly args: Fields;
  /** The committed (or queued) receipt's result fields; undefined before the receipt and for failures. */
  readonly result: Fields | undefined;
  readonly call: CallInput;
  readonly deps: CorralDeps;
}
type Done = Ctx & { readonly result: Fields };
type Phrase = string | ((c: Ctx) => string);
interface Digest {
  readonly state?: RowState;
  readonly verb?: string;
  readonly target?: string;
  readonly outcome?: string;
  readonly tone?: Tone;
}
interface Entry {
  /** What the agent did, as it would say it ("Commented on"). */
  readonly did: Phrase;
  /** While the call runs ("Commenting on"). */
  readonly doing: Phrase;
  /** What a failure is about ("Comment"): `✗ Comment · refused`. */
  readonly noun: Phrase;
  readonly target?: (c: Ctx) => string | undefined;
  readonly digest?: (c: Done) => Digest | undefined;
  /** The text the agent wrote, shown under the row and in full when expanded. */
  readonly prose?: (c: Ctx) => string | undefined;
  /** What a read returned, for the expanded view. */
  readonly content?: (c: Done) => Expanded | undefined;
  readonly view?: PanelView;
  /** Reads have no side effect: their failures are definite. */
  readonly read?: true;
  /** Clip the target in its middle, keeping a file name's end and extension. */
  readonly clip?: "middle";
}
function phrase(value: Phrase, c: Ctx): string {
  return typeof value === "string" ? value : value(c);
}

/**
 * Which page a paged read asked for, from the call's own offset and limit: `page n of m` once the document's size is
 * known, `page n` before; undefined for a whole document read from its start. Offsets snap to UTF-8 boundaries, so a
 * page starts a little before a multiple of the limit; rounding still names it.
 */
function pageOf(args: Fields, fallbackLimit: number, total: unknown): string | undefined {
  const size = int(args.limit) ?? fallbackLimit, from = int(args.offset) ?? 0, whole = int(total);
  if (size <= 0) return undefined;
  const at = Math.round(from / size) + 1;
  if (whole === undefined) return from > 0 ? `page ${at}` : undefined;
  const pages = Math.max(1, Math.ceil(whole / size));
  return pages === 1 && from === 0 ? undefined : `page ${Math.min(at, pages)} of ${pages}`;
}
function withPage(target: string, page: string | undefined): string {
  return page ? `${target} · ${page}` : target;
}
/** `more to read` while a paged read has a next page whose count the target cannot state (`page n of m` already does). */
function moreToRead(result: Fields, total: unknown): Digest | undefined {
  return int(total) !== undefined || result.next_offset === null || result.next_offset === undefined ? undefined : { outcome: "more to read" };
}

const OWNER_KINDS: Readonly<Record<string, true>> = { owner_only: true, permission: true };
function askOwner(args: Fields): boolean {
  return [args.kind, ...list(args.questions).map(item => fields(item).kind)].some(kind => Object.hasOwn(OWNER_KINDS, str(kind) ?? ""));
}
function askText(args: Fields): string | undefined {
  const one = (item: Fields) => [str(item.text),
    list(item.options).length ? `Options:\n${list(item.options).map(option => `- ${scalarText(option)}`).join("\n")}` : undefined,
    str(item.recommendation) ? `Recommended: ${str(item.recommendation)}` : undefined].filter(Boolean).join("\n\n");
  const batch = list(args.questions).map(fields);
  return batch.length ? batch.map((item, index) => `${index + 1}. ${one(item)}`).join("\n\n") : one(args) || undefined;
}
function verdictWords(verdict: unknown, plan: boolean): string {
  return verdict === "approve" ? plan ? "Approved the plan" : "Approved the candidate"
    : verdict === "changes" ? plan ? "Requested plan changes" : "Requested changes"
      : verdict === "escalate" ? plan ? "Escalated the plan to the owner" : "Escalated the review to the owner" : "Recorded the verdict";
}

// Todo: the plan's position and the step's label come from the receipt's progress, or the host ledger before it.
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

const ASSISTANT_PLAN: Readonly<Record<string, string>> = {
  read: "Read the local design", read_revision: "Read a design revision", stage: "Staged design text", publish: "Published the local design",
  enter: "Entered the design", checkpoint: "Checkpointed the design", begin_execution: "Began executing the design",
  pause: "Paused the design", candidate: "Marked the design candidate", close: "Closed the design",
};
const PLAN_REVIEW: Readonly<Record<string, string>> = {
  start: "Requested a plan review", read: "Read the plan review", cancel: "Cancelled the plan review",
};
const NOTES_DID: Readonly<Record<string, string>> = {
  list: "Read PM notes", add: "Noted", supersede: "Replaced note", remove: "Removed note",
};
const BOARD_DID: Readonly<Record<string, string>> = { act: "Ran board action", park: "Parked", attend: "Handled" };
const OWNER_ASK_DID: Readonly<Record<string, string>> = {
  open: "Asked the owner", get: "Checked the owner's answer", withdraw: "Withdrew the owner question",
};
/** Whether an owner prompt asks for permission or a confirmation; a status read carries it on its result only. */
function promptKind(c: Ctx): string | undefined {
  return str(c.args.kind) ?? str(c.result?.kind);
}
/** Where a canonical owner question or prompt stands: open ones wait for the owner, settled ones say how they ended. */
function ownerDigest(result: Fields, kind: string | undefined): Digest | undefined {
  const state = str(result.state);
  if (!state) return undefined;
  if (state === "open" || state === "escalated") return { state: "needs-you", tone: "warning", outcome: "waiting for your answer" };
  if (state === "answered") {
    if (kind === "permission") return result.authorized === true ? { outcome: "allowed" } : { outcome: "denied", tone: "warning" };
    if (kind === "confirmation") return result.authorized === true ? { outcome: "confirmed" } : { outcome: "declined", tone: "warning" };
    return { outcome: "answered" };
  }
  return { outcome: words(state), tone: state === "expired" ? "warning" : "muted" };
}

const ENTRIES: Readonly<Record<CorralOp, Entry>> = {
  hello: { did: "Connected to corral", doing: "Connecting to corral", noun: "Connection" },
  event: { did: "Reported an event", doing: "Reporting an event", noun: "Event" },
  job: { did: "Registered a job", doing: "Registering a job", noun: "Job" },
  peer_send: { did: "Sent an agent message", doing: "Sending an agent message", noun: "Agent message" },
  peer_roster: { did: "Listed peer sessions", doing: "Listing peer sessions", noun: "Peer roster", read: true },
  receipt_lookup: {
    did: "Looked up a receipt", doing: "Looking up a receipt", noun: "Receipt lookup", read: true,
    target: c => str(c.args.task),
    digest: c => {
      if (c.result.found === false) return { state: "unknown", verb: "No receipt found", outcome: "the request may still commit", tone: "warning" };
      const found = fields(c.result.receipt);
      return { outcome: [str(found.op) ? words(String(found.op)) : undefined, str(found.state)].filter(Boolean).join(" ") || "found" };
    },
  },
  ask: {
    did: c => {
      const count = list(c.args.questions).length;
      return `Asked the ${askOwner(c.args) ? "owner" : "PM"}${count > 1 ? ` ${count} questions` : ""}`;
    },
    doing: c => `Asking the ${askOwner(c.args) ? "owner" : "PM"}`,
    noun: "Question",
    target: c => str(c.result?.key) ?? str(c.result?.group),
    digest: c => ({
      ...(askOwner(c.args) ? { state: "needs-you" as const, tone: "warning" as const } : {}),
      outcome: [c.result.blocking === true ? "blocking" : undefined, askOwner(c.args) ? "waiting for your answer" : str(c.result.state)]
        .filter(Boolean).join(", "),
    }),
    prose: c => askText(c.args),
  },
  withdraw: {
    did: "Withdrew question", doing: "Withdrawing question", noun: "Withdrawal",
    target: c => str(c.args.key), prose: c => str(c.args.reason),
  },
  wait: {
    did: "Waiting for", doing: "Declaring a wait", noun: "Wait",
    target: c => str(c.args.task_id) ? `${String(c.args.task_id)} ${str(c.args.task_status) ?? "done"}` : str(c.args.id),
    digest: c => ({ state: "waiting", ...(clock(c.args.deadline) ? { outcome: `until ${clock(c.args.deadline)}` } : {}) }),
    prose: c => str(c.args.condition),
  },
  progress: {
    did: "Updated run progress", doing: "Updating run progress", noun: "Progress update", view: "plan",
    digest: c => {
      const steps = planSteps(c.args.update);
      return steps ? { outcome: `${steps.filter(step => step.state === "done").length} of ${plural(steps.length, "step")} done` } : undefined;
    },
  },
  checkpoint: {
    did: "Saved checkpoint", doing: "Saving a checkpoint", noun: "Checkpoint", view: "task",
    target: c => int(c.result?.revision) !== undefined ? String(c.result?.revision) : undefined,
    digest: c => {
      const checkpoint = fields(c.args.checkpoint), head = str(checkpoint.head), evidence = list(checkpoint.evidence_refs).length;
      return { outcome: [head ? `head ${head.slice(0, 7)}` : undefined, evidence ? plural(evidence, "evidence file") : undefined].filter(Boolean).join(" · ") };
    },
    prose: c => str(fields(c.args.checkpoint).next_action),
  },
  evidence: {
    did: "Recorded evidence", doing: "Recording evidence", noun: "Evidence", view: "proof",
    target: c => {
      const observation = fields(c.args.observation);
      return [str(observation.criterion), str(observation.status) ? words(String(observation.status)) : undefined].filter(Boolean).join(": ") || undefined;
    },
    prose: c => str(fields(c.args.observation).observed),
  },
  artifact: {
    did: "Attached", doing: "Attaching", noun: "Attach", view: "artifacts",
    target: c => {
      const title = str(c.args.title), path = str(c.args.path);
      return title ? `${str(c.args.kind) ? `${String(c.args.kind)}: ` : ""}${flat(title)}` : path ? basename(path) : undefined;
    },
  },
  gate: {
    did: "Gate started", doing: "Starting the gate", noun: "Gate", view: "proof",
    digest: c => {
      const outcome = str(c.result.outcome);
      if (outcome === "started") return { state: "waiting", outcome: "the result arrives as a message" };
      if (outcome === "adopted" || c.result.adopted === true) return { state: "waiting", verb: "Gate already running", outcome: "the result arrives as a message" };
      if (outcome === "queued") return { state: "waiting", verb: "Gate queued", outcome: "it starts when capacity frees" };
      return outcome ? { state: "waiting", verb: `Gate ${words(outcome)}` } : { state: "waiting" };
    },
  },
  done: {
    did: "Finished the run", doing: "Finishing the run", noun: "Finish", view: "task",
    digest: () => ({ outcome: "sent to review" }),
    prose: c => [str(c.args.summary), str(c.args.result)].filter(Boolean).join("\n\n") || undefined,
  },
  verdict: {
    did: c => verdictWords(c.args.verdict, false), doing: "Recording the verdict", noun: "Verdict", view: "proof",
    digest: c => c.args.verdict === "escalate" ? { state: "needs-you", tone: "warning" } : undefined,
    prose: c => str(c.args.notes),
  },
  todo: {
    did: c => TODO_DID[str(c.args.op) ?? ""] ?? "Updated the plan",
    doing: c => TODO_DOING[str(c.args.op) ?? ""] ?? "Updating the plan",
    noun: "Plan update", view: "plan",
    target: todoTarget, digest: todoDigest,
    prose: c => c.args.op === "drop" ? str(c.args.reason) : str(fields(c.args.on).question),
  },
  question_answer: {
    did: "Answered", doing: "Answering", noun: "Answer",
    target: c => str(c.args.key),
    digest: c => {
      const delivery = str(c.result.delivery);
      return delivery && delivery !== "none" ? { outcome: `delivery ${words(delivery)}` } : undefined;
    },
    prose: c => {
      const answers = list(c.args.answers).map(fields);
      if (answers.length) return answers.map(item => `- ${str(item.key) ?? ""}: ${str(item.text) ?? str(item.option) ?? ""}`).join("\n")
        + (str(c.args.text) ? `\n\n${String(c.args.text)}` : "");
      return [str(c.args.option) ? `Option: ${String(c.args.option)}` : undefined, str(c.args.text)].filter(Boolean).join("\n\n") || undefined;
    },
  },
  question_escalate: {
    did: "Escalated to the owner", doing: "Escalating to the owner", noun: "Escalation",
    target: c => str(c.args.key), digest: () => ({ state: "needs-you", outcome: "waiting for your answer", tone: "warning" }),
    prose: c => str(c.args.note),
  },
  question_cancel: {
    did: "Cancelled question", doing: "Cancelling question", noun: "Cancellation",
    target: c => str(c.args.key), prose: c => str(c.args.reason),
  },
  reply: {
    did: "Replied to", doing: "Replying to", noun: "Reply",
    target: c => str(c.args.task) ?? str(c.result?.task), prose: c => str(c.args.text),
  },
  comment: {
    did: "Commented on", doing: "Commenting on", noun: "Comment",
    target: c => str(c.args.task), prose: c => str(c.args.text),
  },
  capture: {
    did: "Filed", doing: "Filing", noun: "Capture",
    target: c => [str(c.result?.task), str(c.args.title) ? flat(String(c.args.title)) : undefined].filter(Boolean).join(" ") || undefined,
    digest: c => ({ outcome: `as ${str(c.args.status) ?? "idea"}` }),
    prose: c => str(c.args.body),
  },
  read_brief: {
    did: "Read the PM brief", doing: "Reading the PM brief", noun: "Brief read", read: true,
    digest: c => {
      const counts = fields(c.result.counts);
      const parts = [["ready", "ready"], ["needs_me", "need you"], ["review", "in review"], ["running", "running"]]
        .flatMap(([key = "", label]) => int(counts[key]) ? [`${int(counts[key])} ${label}`] : []);
      return parts.length ? { outcome: parts.join(" · ") } : undefined;
    },
    content: c => fieldsContent(c.result),
  },
  read_next: {
    did: "Read next actions", doing: "Reading next actions", noun: "Next actions read", read: true,
    digest: c => {
      const task = str(c.result.task);
      if (!task) return { outcome: "nothing to do" };
      const recommended = list(c.result.options).map(fields).find(option => option.id === c.result.recommended);
      return { outcome: `next: ${task}${str(recommended?.label) ? ` ${String(recommended?.label)}` : ""}` };
    },
    content: c => fieldsContent(c.result),
  },
  read_show: {
    did: "Read", doing: "Reading", noun: "Task read", read: true,
    target: c => withPage(str(c.args.key) ? `question ${String(c.args.key)}` : `${str(c.args.task) ?? "task"} ${str(c.args.section) ?? "overview"}`,
      pageOf(c.args, 1024, c.result?.total_chars)),
    digest: c => moreToRead(c.result, c.result.total_chars),
    content: c => jsonPage(c.result.content_json),
  },
  read_list: {
    did: "Listed tasks", doing: "Listing tasks", noun: "Task list", read: true,
    target: c => str(c.args.status) ? words(String(c.args.status)) : undefined,
    digest: c => int(c.result.total) !== undefined ? { outcome: plural(int(c.result.total) ?? 0, "task") } : undefined,
    content: c => ({ kind: "lines", lines: list(c.result.items).map(fields).map(item =>
      [str(item.id), str(item.status) ? words(String(item.status)) : undefined, str(item.title)].filter(Boolean).join("  ")) }),
  },
  read_search: {
    did: "Searched tasks for", doing: "Searching tasks for", noun: "Search", read: true,
    target: c => {
      const query = str(c.args.query) ? flat(String(c.args.query)) : undefined;
      return query && !query.includes("\"") ? `"${query}"` : query;
    },
    digest: c => int(c.result.total) !== undefined ? { outcome: plural(int(c.result.total) ?? 0, "match", "matches") } : undefined,
    content: c => ({ kind: "lines", lines: list(c.result.items).map(fields).map(item =>
      [str(item.reference) ?? str(item.id), str(item.title) ?? (str(item.snippet) ? flat(String(item.snippet)) : undefined)].filter(Boolean).join("  ")) }),
  },
  read_content: {
    did: "Read", doing: "Reading", noun: "Content read", read: true, clip: "middle",
    target: c => {
      const reference = str(c.args.reference);
      if (!reference) return undefined;
      const parts = reference.replace(/^artifacts\//, "").split(/[/:]/).filter(Boolean);
      return withPage(parts.length > 2 ? `${parts.slice(0, -1).join("/")} ${basename(reference)}` : reference, pageOf(c.args, 4096, c.result?.bytes));
    },
    digest: c => moreToRead(c.result, c.result.bytes),
    content: c => {
      const text = str(c.result.text);
      if (!text) return undefined;
      return /\.(md|markdown)$/i.test(str(c.args.reference) ?? "") ? { kind: "markdown", text } : { kind: "lines", lines: text.split("\n") };
    },
  },
  read_route: {
    did: "Read routing for", doing: "Reading routing for", noun: "Routing read", read: true,
    target: c => str(c.args.task),
    digest: c => ({ outcome: [str(c.result.profile), str(c.result.model)].filter(Boolean).join(" · ") }),
    content: c => fieldsContent(c.result),
  },
  read_artifacts: {
    did: "Listed artifacts of", doing: "Listing artifacts of", noun: "Artifact list", read: true,
    target: c => str(c.args.task),
    digest: c => int(c.result.total) !== undefined ? { outcome: plural(int(c.result.total) ?? 0, "artifact") } : undefined,
    content: c => ({ kind: "lines", lines: list(c.result.items).map(fields).map(item =>
      [str(item.kind), str(item.title), str(item.path)].filter(Boolean).join("  ")) }),
  },
  read_lessons: {
    did: "Read lessons", doing: "Reading lessons", noun: "Lessons read", read: true,
    content: c => fieldsContent(c.result),
  },
  read_annotations: {
    did: "Read annotations of", doing: "Reading annotations of", noun: "Annotations read", read: true,
    target: c => str(c.args.task), content: c => fieldsContent(c.result),
  },
  inbox_read: {
    did: c => str(c.args.id) ? "Read inbox item" : "Read the inbox",
    doing: c => str(c.args.id) ? "Reading inbox item" : "Reading the inbox",
    noun: "Inbox read", read: true, view: "inbox",
    target: c => str(c.args.id) ? withPage(String(c.args.id), pageOf(c.args, 1024, c.result?.total_chars)) : undefined,
    digest: c => {
      if (!str(c.args.id)) {
        const total = int(c.result.total) ?? (Array.isArray(c.result.items) ? c.result.items.length : undefined);
        return total === undefined ? undefined : { outcome: plural(total, `${str(c.args.state) ?? "open"} item`) };
      }
      const item = fields(c.result.item);
      const about = [str(item.kind), str(item.from) ? `from ${String(item.from)}` : undefined].filter(Boolean).join(" ");
      return { outcome: [about || undefined, moreToRead(c.result, c.result.total_chars)?.outcome].filter(Boolean).join(" · ") };
    },
    content: c => str(c.args.id) ? jsonPage(c.result.content_json)
      : { kind: "lines", lines: list(c.result.items).map(fields).map(item =>
        [str(item.id), str(item.kind), str(item.from) ? `from ${String(item.from)}` : undefined, str(item.title) ?? str(item.summary)].filter(Boolean).join("  ")) },
  },
  inbox_resolve: {
    did: c => c.args.action === "claim" ? "Claimed inbox item" : "Resolved inbox item",
    doing: c => c.args.action === "claim" ? "Claiming inbox item" : "Resolving inbox item",
    noun: "Inbox update", view: "inbox",
    target: c => str(c.args.id),
    digest: c => c.args.action === "resolve" && str(c.args.outcome) ? { outcome: `as ${words(String(c.args.outcome))}` } : undefined,
    prose: c => str(c.args.text),
  },
  memory_list: {
    did: "Listed memory", doing: "Listing memory", noun: "Memory list", read: true,
    target: c => {
      const scope = fields(c.args.scope);
      return str(scope.key) ? `${String(scope.scope)} ${String(scope.key)}` : str(scope.scope);
    },
    // A Code Mode child's reduced receipt keeps scalars only: no list, no count.
    digest: c => Array.isArray(c.result.entries) ? { outcome: plural(c.result.entries.length, "entry", "entries") } : undefined,
    content: c => fieldsContent(c.result.entries),
  },
  memory_read: {
    did: "Read memory", doing: "Reading memory", noun: "Memory read", read: true,
    target: c => str(fields(c.args.selector).id) ? `"${String(fields(c.args.selector).id)}"` : undefined,
    digest: c => str(c.result.body) ? undefined : { outcome: "not found" },
    content: c => str(c.result.body) ? { kind: "markdown", text: String(c.result.body) } : undefined,
  },
  memory_put: {
    did: "Saved memory", doing: "Saving memory", noun: "Memory update",
    target: c => {
      const metadata = fields(fields(c.args.entry).metadata);
      return [str(metadata.kind), str(metadata.title)].filter(Boolean).join(": ") || str(fields(c.args.selector).id);
    },
    prose: c => str(fields(c.args.entry).body),
  },
  memory_remove: {
    did: "Removed memory", doing: "Removing memory", noun: "Memory removal",
    target: c => str(fields(c.args.selector).id),
  },
  job_monitor: {
    did: "Watching job", doing: "Arming a job watch", noun: "Job watch",
    target: c => {
      const job = str(c.args.job);
      return job ? flat(c.deps.job?.(job)?.command ?? job) : undefined;
    },
    digest: c => ({
      state: "waiting",
      outcome: [c.args.on === "line" && str(c.args.pattern) ? `for /${String(c.args.pattern)}/` : "until it exits",
        clock(c.result.deadline) ? `deadline ${clock(c.result.deadline)}` : undefined].filter(Boolean).join(" · "),
    }),
  },
  plan_write: {
    did: "Wrote plan text", doing: "Writing plan text", noun: "Plan write",
    target: c => (int(c.args.offset) ?? 0) === 0 ? "from the start" : "continued",
    digest: c => {
      const bytes = int(c.result.bytes);
      return bytes === undefined ? undefined : { outcome: `${bytes < 1024 * 1024 ? `${(bytes / 1024).toFixed(1)} KB` : `${(bytes / 1024 / 1024).toFixed(1)} MB`} in total` };
    },
    prose: c => str(c.args.content),
  },
  plan_submit: { did: "Submitted the plan", doing: "Submitting the plan", noun: "Plan submission", view: "task" },
  plan_verdict: {
    did: c => verdictWords(c.args.verdict, true), doing: "Recording the plan verdict", noun: "Plan verdict", view: "task",
    digest: c => c.args.verdict === "escalate" ? { state: "needs-you", tone: "warning" } : undefined,
    prose: c => str(c.args.notes),
  },
  owner_action: {
    did: "Ran owner action", doing: "Running owner action", noun: "Owner action",
    target: c => [str(c.args.kind) ? words(String(c.args.kind)) : undefined, str(c.args.target)].filter(Boolean).join(" ") || undefined,
    digest: c => {
      const output = str(c.result.output)?.split("\n").find(line => line.trim()), outcome = str(c.result.outcome);
      // Some owner commands print JSON; its fields are on expand, not in the row.
      return { outcome: outcome && outcome !== "executed" ? words(outcome) : output && !/^[[{]/.test(output.trim()) ? flat(output) : "done" };
    },
    content: c => {
      const output = str(c.result.output);
      if (!output) return undefined;
      try { return fieldsContent(JSON.parse(output)); } catch { return { kind: "lines", lines: output.split("\n") }; }
    },
  },
  write_permission: { did: "Recorded a write permission", doing: "Recording a write permission", noun: "Write permission" },
  assistant_plan: {
    did: c => ASSISTANT_PLAN[str(c.args.action) ?? ""] ?? "Updated the local design",
    doing: "Updating the local design", noun: "Local design",
    prose: c => str(c.args.chunk),
  },
  assistant_plan_review: {
    did: c => PLAN_REVIEW[str(c.args.action) ?? ""] ?? "Updated the plan review",
    doing: "Updating the plan review", noun: "Plan review",
  },
  assistant_handoff: {
    did: c => `Handoff to the PM: ${words(str(c.args.action) ?? "update")}`, doing: "Updating the handoff", noun: "Handoff",
  },
  notes: {
    did: c => NOTES_DID[str(c.args.op) ?? ""] ?? "Updated PM notes",
    doing: "Updating PM notes", noun: "Note",
    target: c => c.args.op === "add" ? str(c.args.kind) : str(c.args.id),
    digest: c => c.args.op === "list" && Array.isArray(c.result.entries) ? { outcome: plural(c.result.entries.length, "note") } : undefined,
    prose: c => str(c.args.text),
    content: c => c.args.op === "list" ? fieldsContent(c.result.entries) : undefined,
  },
  pm_action: {
    did: c => BOARD_DID[str(c.args.action) ?? ""] ?? "Took board action",
    doing: "Taking board action", noun: "Board action",
    target: c => str(c.args.id) ?? str(c.args.task) ?? str(c.args.key),
    digest: c => {
      const until = list(c.args.resume_when).map(scalarText);
      return c.args.action === "park" && until.length ? { outcome: `until ${until.join(", ")}` } : undefined;
    },
    prose: c => str(c.args.note) ?? str(c.args.reason),
  },
  gate_triage: {
    did: c => c.result?.outcome === "recorded" ? "Recorded gate diagnosis for"
      : c.result?.outcome === "declined" ? "Gate diagnosis declined for" : "Proposed gate diagnosis for",
    doing: "Proposing a gate diagnosis", noun: "Gate diagnosis",
    target: c => str(c.args.task),
    digest: c => {
      const kind = str(fields(c.args.diagnosis).class), outcome = str(c.result.outcome);
      if (outcome === "proposed") return { state: "needs-you", outcome: "waiting for the owner's approval", tone: "warning" };
      if (outcome === "declined") return { outcome: "the owner declined it" };
      return kind ? { outcome: words(kind) } : undefined;
    },
    prose: c => str(fields(c.args.diagnosis).explanation),
  },
  pm_shadow: { did: "Recorded a shadow decision", doing: "Recording a shadow decision", noun: "Shadow decision" },
  owner_ask: {
    did: c => OWNER_ASK_DID[str(c.args.action) ?? ""] ?? "Asked the owner",
    doing: c => c.args.action === "withdraw" ? "Withdrawing the owner question"
      : c.args.action === "get" ? "Waiting for the owner's answer" : "Asking the owner",
    noun: "Owner question",
    target: c => str(c.result?.key) ?? str(c.args.key),
    digest: c => ownerDigest(c.result, "question"),
    prose: c => c.args.action === "open" ? str(c.args.text) : undefined,
  },
  owner_prompt: {
    did: c => promptKind(c) === "confirmation" ? "Asked you to confirm" : "Asked your permission",
    doing: c => c.args.action === "get" ? "Waiting for your decision"
      : promptKind(c) === "confirmation" ? "Asking you to confirm" : "Asking your permission",
    noun: "Owner prompt",
    target: c => str(c.args.command) ?? str(c.args.tool) ?? str(c.result?.key),
    digest: c => ownerDigest(c.result, promptKind(c)),
    prose: c => str(c.args.preview),
  },
  idea_file: {
    did: "Filed the owner's idea", doing: "Filing the owner's idea", noun: "Filed idea",
    target: c => str(c.result?.task) ?? str(c.args.title),
    digest: c => ({
      outcome: [str(c.result.task) ? str(c.args.title) : undefined, "owner's words kept verbatim"]
        .filter(Boolean).map(part => flat(String(part))).join(" · "),
    }),
    prose: c => str(c.args.body),
  },
};

/** The corral operation behind each model-facing tool name. */
export const CORRAL_TOOL_OPS: Readonly<Record<string, CorralOp>> = {
  corral_receipt_lookup: "receipt_lookup", corral_ask: "ask", corral_withdraw: "withdraw", corral_wait: "wait",
  corral_progress: "progress", corral_checkpoint: "checkpoint", corral_evidence: "evidence", corral_artifact: "artifact",
  corral_gate: "gate", corral_done: "done", corral_verdict: "verdict", corral_todo: "todo",
  corral_question_answer: "question_answer", corral_question_escalate: "question_escalate", corral_question_cancel: "question_cancel",
  corral_reply: "reply", corral_comment: "comment", corral_capture: "capture",
  corral_brief: "read_brief", corral_next: "read_next", corral_show: "read_show", corral_list: "read_list",
  corral_search: "read_search", corral_content_read: "read_content", corral_route: "read_route",
  corral_artifacts: "read_artifacts", corral_lessons: "read_lessons", corral_annotations: "read_annotations",
  corral_inbox_read: "inbox_read", corral_inbox_resolve: "inbox_resolve",
  corral_memory_list: "memory_list", corral_memory_read: "memory_read", corral_memory_put: "memory_put", corral_memory_remove: "memory_remove",
  corral_job_monitor: "job_monitor",
  corral_plan_write: "plan_write", corral_plan_submit: "plan_submit", corral_plan_verdict: "plan_verdict",
  corral_owner_action: "owner_action", corral_assistant_plan: "assistant_plan",
  corral_assistant_plan_review: "assistant_plan_review", corral_assistant_handoff: "assistant_handoff",
  corral_notes: "notes", corral_action: "pm_action", corral_gate_triage: "gate_triage",
};

/** Prose under the row: the first rows once settled, the newest while the arguments stream; the kit marks a cut with `…`. */
function proseBody(prose: string, streaming: boolean): readonly BodyLine[] {
  return [{ text: prose, tone: "muted", wrap: streaming ? { max: STREAM_ROWS, from: "end" } : { max: PROSE_ROWS } }];
}
function reasonBody(reason: string): readonly BodyLine[] {
  return reason ? [{ text: reason, tone: "muted", wrap: { max: REASON_ROWS } }] : [];
}

/** The row of one corral operation's call. */
export function presentOperation(op: CorralOp, call: CallInput, deps: CorralDeps = {}): RowModel {
  const entry = ENTRIES[op], settled = settle(call);
  const result = settled?.kind === "receipt" ? settled.result : undefined;
  const ctx: Ctx = { args: fields(call.args), result, call, deps };
  // A refused schema says nothing about what the arguments meant.
  const known = settled?.kind !== "schema";
  const target = known ? entry.target?.(ctx) : undefined;
  const prose = known ? entry.prose?.(ctx)?.trim() || undefined : undefined;
  const at = (model: RowModel): RowModel => target && model.target === undefined
    ? { ...model, target, ...(entry.clip ? { targetClip: entry.clip } : {}) } : model;

  if (!settled) {
    const state: RowState = call.argsComplete ? "running" : "writing";
    return at({
      state, verb: phrase(entry.doing, ctx),
      ...(prose && state === "writing" ? { outcome: plural(prose.length, "char") } : {}),
      ...(prose ? { body: proseBody(prose, state === "writing") } : {}),
    });
  }

  const failure = failureOf(settled, entry.read === true);
  if (failure) {
    return at({
      state: failure.state, verb: phrase(entry.noun, ctx), outcome: failure.word,
      outcomeTone: failure.state === "unknown" ? "warning" : "error",
      body: reasonBody(failure.reason),
      expanded: expandedView(prose, undefined, factsLine(settled), failure.reason || undefined),
    });
  }

  const done: Done | undefined = result ? { ...ctx, result } : undefined;
  const digest = done ? entry.digest?.(done) : undefined;
  const queued = settled.kind === "receipt" && (settled.state === "queued" || settled.state === "reserved");
  const replayed = settled.kind === "receipt" && settled.receipt.replayed === true;
  const state: RowState = queued ? "waiting" : digest?.state ?? "ok";
  const outcome = [queued ? "queued" : undefined, digest?.outcome || undefined, replayed ? "already recorded (replayed)" : undefined]
    .filter(Boolean).join(" · ");
  const content = done ? entry.content?.(done) : undefined;
  const open = deps.openView, label = entry.view ? VIEW_LABEL[entry.view] : undefined, view = entry.view;
  return at({
    state, verb: digest?.verb ?? phrase(entry.did, ctx),
    ...(digest?.target ? { target: digest.target } : {}),
    ...(outcome ? { outcome } : {}),
    ...(digest?.tone ? { outcomeTone: digest.tone } : {}),
    ...(prose ? { body: proseBody(prose, false) } : {}),
    ...(prose || content ? { expanded: expandedView(prose, content, factsLine(settled)) } : {}),
    ...(open && label && view && state !== "failed" ? { view: { label, open: () => open(view) } } : {}),
  });
}

// ---------------------------------------------------------------------------------------------------------------
// Tools without a corral receipt of their own.

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
  if (call.isError) {
    return { state: "failed", verb: "Expert consultation", ...(target ? { target } : {}), outcome: "failed", outcomeTone: "error", body: reasonBody(answer) };
  }
  return {
    state: "ok", verb: "Expert answered", ...(target ? { target } : {}),
    ...(answer ? { body: proseBody(answer, false), expanded: expandedView(question ? `**Question**\n\n${question}` : undefined, { kind: "markdown", text: answer }, "") } : {}),
  };
};

const INSPECT_VERB: Readonly<Record<string, string>> = {
  head: "Read Git HEAD", status: "Read Git status", diff: "Read the unstaged diff", staged: "Read the staged diff",
};
const inspectPresenter: Presenter = call => {
  const fact = str(call.args.fact) ?? "", verb = INSPECT_VERB[fact] ?? `Read Git ${words(fact)}`;
  const live = runningState(call);
  if (live) return { state: live, verb };
  const details = fields(call.result?.details), full = str(details.output);
  const output = full ?? resultText(call.result);
  if (call.isError) return { state: "failed", verb, outcome: "failed", outcomeTone: "error", body: reasonBody(output.trim()) };
  const lines = output.split("\n").filter(line => line.trim());
  // The details carry the whole output; a Code Mode child's text is only a preview, counted by its totals.
  const count = full !== undefined ? lines.length : outputLines(call, output);
  const outcome = [fact === "head" ? (lines[0] ?? "").slice(0, 7)
    : count === undefined ? undefined : fact === "status" ? count ? plural(count, "changed path") : "clean"
      : count ? plural(count, "line") : "no changes",
  details.truncated === true ? "truncated" : undefined].filter(Boolean).join(" · ");
  return {
    state: "ok", verb, ...(outcome ? { outcome } : {}),
    ...(lines.length > 1 ? { expanded: () => lines.map(line => ({ text: line, tone: "text" as const })) } : {}),
  };
};

/** A build control reports a no-op in its text and a boolean in its details; either means nothing happened. */
function buildControl(verb: string, flag: string, refused: RegExp, missed: string): Presenter {
  return call => {
    const id = str(call.args.id), text = str(call.args.text);
    const live = runningState(call);
    const base = { verb, ...(id ? { target: id } : {}) };
    if (live) return { ...base, state: live, ...(text ? { body: proseBody(text, live === "writing") } : {}) };
    const reply = resultText(call.result).trim(), details = fields(call.result?.details);
    if (call.isError || details[flag] === false || refused.test(reply)) {
      return { ...base, state: "failed", outcome: missed, outcomeTone: "error", body: reasonBody(reply) };
    }
    return { ...base, state: "ok", ...(text ? { body: proseBody(text, false), expanded: expandedView(text, undefined, "") } : {}) };
  };
}

const OTHER: Readonly<Record<string, Presenter>> = {
  corral_expert: expertPresenter,
  corral_assistant_inspect: inspectPresenter,
  agent_message: buildControl("Steered", "delivered", /no message delivered/i, "not delivered"),
  agent_cancel: buildControl("Cancelled", "cancelled", /nothing cancelled/i, "not cancelled"),
};

/** The row presenter of a corral tool, or undefined for any other tool. */
export function corralPresenter(name: string, deps: CorralDeps = {}): Presenter | undefined {
  const op = CORRAL_TOOL_OPS[name] ?? deps.corralSpecs?.find(spec => spec.name === name)?.op;
  if (op) return call => presentOperation(op, call, deps);
  return OTHER[name];
}

/** Every `corral_*` tool, `agent_message` and `agent_cancel`. Build cards keep their own renderers. */
export const corralPresenters: PresenterModule = {
  presenter: (name, deps) => corralPresenter(name, deps),
};

/** One plain line for a settled call, as the row reads (`✗ Finish · refused: a checkpoint is required first`), at most 240 characters. */
export function corralSummary(name: string, args: Record<string, unknown>, result: AgentToolResult<unknown>, isError: boolean): string | undefined {
  const present = corralPresenter(name);
  if (!present) return undefined;
  const model = present({
    name, args: { ...fields(args) }, argsComplete: true, executionStarted: true, result, partial: false, isError,
    expanded: false, cwd: "", toolCallId: "",
  });
  const reason = model.state === "failed" || model.state === "unknown" ? model.body?.[0]?.text : undefined;
  const line = `${GLYPH[model.state]} ${model.verb}${model.target ? ` ${flat(model.target)}` : ""}`
    + `${model.outcome ? ` · ${model.outcome}` : ""}${reason ? `: ${flat(reason)}` : ""}`;
  return line.length > 240 ? `${line.slice(0, 239)}…` : line;
}
