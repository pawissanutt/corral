// Plan mode (docs/plans/pi-plan-mode.md). The agent plans read-only with the owner, submits one plan
// document, and the owner's review choice starts execution. The phase is read from the transcript, so a
// restart restores it and a rewind undoes it. Nothing here changes the system prompt or the tool list.
import { createHash, randomBytes } from "node:crypto";
import { lstatSync, mkdirSync, readFileSync, writeFileSync } from "node:fs";
import { join } from "node:path";
import type { ToolDefinition } from "@earendil-works/pi-coding-agent";
import { Type } from "typebox";
import { z } from "zod";
import { BridgeRefusal, SESSION_PART_MAX } from "../bridge/protocol.ts";
import { real, resolveToolPath } from "../roles/assistant/paths.ts";
import type { PlanContext, PlanPrompts } from "../roles/types.ts";
import { cardQuestion, invalidAnswer, singleAnswer, type AnswerStatus, type CardAnswer, type CardEntry } from "./remote/cards.ts";
import { clipText, cursorOffset, fingerprintPart, jsonBytes, partCursor } from "./remote/state.ts";

export const PLAN_ENTRY = "corral_plan_mode";
export const PLAN_NOTE = "corral_plan_note";
export const PLAN_SUBMIT_TOOL = "plan_submit";
const PLAN_FILE = "plan.md";
const PLAN_MAX_BYTES = 256 * 1024;
const DOC_PAGE_BYTES = 40 * 1024;
const TITLE_BYTES = 160;

const Saved = z.object({ version: z.literal(1), phase: z.enum(["off", "planning", "review"]),
  revision: z.number().int().nonnegative(), at: z.number(), title: z.string().optional(),
  // A submission: its own id, the digest of its text and the write-once copy that review and execution read.
  id: z.string().optional(), digest: z.string().optional(), path: z.string().optional(),
  // The owner left without approving.
  left: z.boolean().optional() });
export type PlanRecord = z.infer<typeof Saved>;
export type PlanPhase = PlanRecord["phase"];
export type PlanChoice = "execute" | "execute_clean" | "execute_compact" | "revise";
/** A plan whose kickoff reached the transcript. */
export interface ApprovedPlan { readonly title: string; readonly revision: number; readonly path: string }
type NoteKind = "contract" | "left" | "pointer";

export interface PlanEntry {
  readonly type: string; readonly customType?: string; readonly data?: unknown; readonly details?: unknown; readonly message?: unknown;
}

export interface PlanHost {
  /** The session's state directory: the plan document and the copies of its submissions live here. */
  readonly dir: string;
  /** The directory the file tools resolve relative paths in. */
  readonly cwd: string;
  readonly prompts: PlanPrompts;
  readonly askUser: boolean;
  readonly todo: boolean;
  /** The current branch, root to leaf. */
  entries(): readonly PlanEntry[];
  leaf(): string | null;
  append(record: PlanRecord): void;
  /** Why the mode cannot change now; undefined when the session is bound and idle. */
  busy(): string | undefined;
  /** Why the session could not start a turn for a review choice now: a compaction is running, or it is paused. */
  blocked(): string | undefined;
  stopTurn(): Promise<void>;
  compact(instructions: string): Promise<void>;
  /** Replace the model's context with `summary`, keeping no earlier entry. */
  reset(summary: string): Promise<void>;
  /**
   * Start or steer a turn; false when the session refused the input. `kickoff` is the card id of the approval
   * this input starts: the host tags the exact message it admits with it, and only that tag ends the review.
   */
  submit(text: string, origin: "owner" | "auto", kickoff?: string): Promise<boolean>;
  model(): string | undefined;
  changed(): void;
  note(text: string, tone?: "text" | "warning" | "error"): void;
  /** A build session's record of what the owner approved. */
  approved?(summary: string): void;
  now?: () => number;
}

const OFF: PlanRecord = { version: 1, phase: "off", revision: 0, at: 0 };
const EXECUTE: Readonly<Record<string, PlanContext>> = { execute: "keep", execute_clean: "clean", execute_compact: "compact" };
const CONTEXT_LABEL: Readonly<Record<PlanContext, string>> = { keep: "context kept", clean: "clean context", compact: "compacted context" };
export const PLAN_OPTIONS: readonly { id: PlanChoice; label: string; detail: string }[] = [
  { id: "execute", label: "Execute", detail: "Keep the planning conversation in context" },
  { id: "execute_clean", label: "Execute with clean context", detail: "Clear the conversation; the plan is the whole brief" },
  { id: "execute_compact", label: "Execute with compacted context", detail: "Summarise the planning discussion first" },
  { id: "revise", label: "Keep planning", detail: "Send feedback and return to planning" },
];
const COMPACT_INSTRUCTIONS = "The owner approved a plan and execution starts next. Distil the planning discussion for the executor. "
  + "Keep the reasons behind the plan's choices, alternatives the owner rejected, constraints and preferences the owner stated, "
  + "and the files, symbols and code paths that were discovered. Drop tool output already reflected in the plan, superseded "
  + "drafts and anything the plan itself states. Do not restate the plan; it is delivered in full after this summary.";
const CHANGED = "the submitted plan's copy is missing or was changed; choose Keep planning and have it submitted again";
const STARTING = "The approved plan is starting; wait for it";

const sha = (text: string): string => createHash("sha256").update(text).digest("hex");
const heading = (text: string): string | undefined => /^[ \t]*#[ \t]+(.+?)[ \t]*$/m.exec(text)?.[1]?.trim() || undefined;
const slug = (title: string): string => title.toLowerCase().replace(/[^a-z0-9]+/g, "-").replace(/^-+|-+$/g, "").slice(0, 48) || "plan";
// A rewind rolls the revision back, so the submission's own id is what makes a card and its copy unique.
const cardId = (record: PlanRecord): string => `plan-${record.revision}-${record.id ?? ""}`;
/** The first line of the message that starts an approved plan, which tells the model that this is the approval. */
const kickoffLine = (card: string): string => `[corral plan approved ${card}]`;
/** The field the host sets on the one user message it admitted as an approval's kickoff; its value is the card id. */
export const PLAN_KICKOFF = "corralPlanKickoff";

/** The card a user message was admitted as the kickoff for. Its text proves nothing: anyone can send the words. */
function kickoffOf(entry: PlanEntry): string | undefined {
  const message = entry.type === "message" ? entry.message as { role?: unknown; [PLAN_KICKOFF]?: unknown } | undefined : undefined;
  const card = message?.role === "user" ? message[PLAN_KICKOFF] : undefined;
  return typeof card === "string" ? card : undefined;
}

interface Scan { record: PlanRecord; approved?: ApprovedPlan; due: readonly NoteKind[] }

/**
 * The phase and what the model has not yet been told about it, read from the branch alone. A review ends as
 * approved exactly when the message the host admitted as its kickoff is in the transcript: no separate record
 * could be written without it, lost with it, or outlive a rewind of it.
 */
function scan(entries: readonly PlanEntry[]): Scan {
  let record = OFF, announced = false, told = false;
  let approved: ApprovedPlan | undefined;
  let due: Exclude<NoteKind, "contract"> | undefined;
  for (const entry of entries) {
    if (entry.type === "custom" && entry.customType === PLAN_ENTRY) {
      const parsed = Saved.safeParse(entry.data);
      if (!parsed.success) continue;
      const next = parsed.data;
      due = next.phase === "off" && next.left && told ? "left" : undefined;
      if (next.phase === "off") announced = told = false;
      record = next;
      approved = undefined;
    } else if (entry.type === "compaction") {
      // A summary is not the contract, and a later one can lose an approved plan's text.
      announced = false;
      if (approved) due = "pointer";
    } else if (entry.type === "custom_message" && entry.customType === PLAN_NOTE) {
      const kinds = (entry.details as { kinds?: unknown } | undefined)?.kinds;
      for (const kind of Array.isArray(kinds) ? kinds : []) {
        if (kind === "contract") announced = told = true;
        else if (kind === due) due = undefined;
      }
    } else if (record.phase === "review" && record.path && kickoffOf(entry) === cardId(record)) {
      approved = { title: record.title ?? "Plan", revision: record.revision, path: record.path };
      record = { version: 1, phase: "off", revision: record.revision, at: record.at };
      announced = told = false;
      due = undefined;
    }
  }
  // The read-only rules hold in review too, so a turn there needs the contract as much as a planning one.
  if (record.phase !== "off") return { record, due: announced ? [] : ["contract"] };
  return { record, ...(approved ? { approved } : {}), due: due ? [due] : [] };
}

export class PlanMode {
  /** The one plan document of the session. */
  readonly path: string;
  private cache: { leaf: string | null; scan: Scan } | undefined;
  private submitted: { card: string; body: string } | undefined;
  /** An approval is being carried out, up to the admission of its kickoff. */
  private starting = false;
  /** The approval's own compaction or reset is running. */
  private preparing = false;
  /** The kickoff on its way into the transcript: its card, its text, its plan, and what the Build record will say of it. */
  private kickoff: { readonly card: string; readonly text: string; readonly path: string; readonly journal: string; admitted: boolean } | undefined;
  /** The phase the views were last told; unknown until the first change, as after a restart. */
  private seen: PlanPhase | undefined;
  private readonly listeners = new Set<() => void>();

  constructor(private readonly host: PlanHost) { this.path = join(host.dir, PLAN_FILE); }

  private read(): Scan {
    const leaf = this.host.leaf();
    if (!this.cache || this.cache.leaf !== leaf) this.cache = { leaf, scan: scan(this.host.entries()) };
    return this.cache.scan;
  }

  get record(): PlanRecord { return this.read().record; }
  get phase(): PlanPhase { return this.record.phase; }
  /** The plan whose kickoff started the work now under way, until plan mode is entered again. */
  get approved(): ApprovedPlan | undefined { return this.read().approved; }
  /** The id of the open review card, which names one submission. */
  get review(): string | undefined {
    const record = this.record;
    return record.phase === "review" ? cardId(record) : undefined;
  }
  /** An approval is on its way into the transcript: nothing else may decide or change the mode meanwhile. */
  private get pending(): boolean { return this.starting || (this.kickoff !== undefined && this.kickoff.card === this.review); }

  onChange(listener: () => void): () => void {
    this.listeners.add(listener);
    return () => { this.listeners.delete(listener); };
  }

  private write(record: Omit<PlanRecord, "version" | "at">): void {
    this.host.append({ ...record, version: 1, at: (this.host.now ?? Date.now)() });
    this.cache = undefined;
    this.announce();
  }

  private announce(): void {
    this.seen = this.phase;
    this.host.changed();
    for (const listener of [...this.listeners]) { try { listener(); } catch { /* A view does not own the mode. */ } }
  }

  /** The transcript grew: an approval's kickoff may have arrived in it, which ends the review and is the approval. */
  refresh(): void {
    // The kickoff is settled by what the transcript holds, whatever the views were last told.
    const kickoff = this.kickoff;
    if (kickoff && this.review !== kickoff.card) {
      this.kickoff = undefined;
      if (this.approved?.path === kickoff.path) this.host.approved?.(kickoff.journal);
    }
    if (this.phase !== this.seen) this.announce();
  }

  /**
   * A turn ended. If it is the kickoff's own, a kickoff that never reached the transcript no longer holds the
   * review. A turn that settles before the kickoff was admitted is an earlier one and changes nothing.
   */
  turnEnded(): void {
    if (this.kickoff?.admitted) this.kickoff = undefined;
  }

  brief(): { phase: "planning" | "review"; title?: string; revision?: number } | undefined {
    const { phase, title, revision } = this.record;
    return phase === "off" ? undefined : { phase, ...(title ? { title } : {}), ...(revision ? { revision } : {}) };
  }

  private targetsPlan(path: unknown): boolean {
    if (typeof path !== "string") return false;
    // A link at the plan's own name would let a write land somewhere else.
    if (lstatSync(this.path, { throwIfNoEntry: false })?.isSymbolicLink()) return false;
    const target = real(resolveToolPath(path, this.host.cwd));
    return target !== undefined && target === real(this.path);
  }

  /** True when an edit or write lands on the plan document, which needs no write-scope decision. */
  writesPlan(tool: string, input: Readonly<Record<string, unknown>>): boolean {
    return this.phase === "planning" && (tool === "edit" || tool === "write") && this.targetsPlan(input.path);
  }

  /** Why a tool call is refused while planning or in review; undefined leaves it to the other controls. */
  refusal(tool: string, input: Readonly<Record<string, unknown>>): string | undefined {
    const phase = this.phase;
    if (phase === "off") return undefined;
    if (tool === "edit" || tool === "write") {
      if (!this.targetsPlan(input.path)) {
        return `Plan mode: the working tree is read-only. Write the plan to ${this.path}; nothing was written.`;
      }
      return phase === "review" ? "A plan is awaiting the owner's review; nothing was written." : undefined;
    }
    if (tool === "task") {
      const agents = Array.isArray(input.agents) ? input.agents as readonly ({ kind?: unknown } | null)[] : [];
      if (agents.some(agent => agent?.kind !== "explore" && agent?.kind !== "review")) {
        return "Plan mode: only children with kind explore or review can start; nothing was started.";
      }
    }
    return undefined;
  }

  /**
   * What the model needs to be told about the mode now, as one message; the transcript records what it covered.
   * `prompt` is the text that starts the run, when a run is starting.
   */
  turnMessage(prompt?: string): { customType: string; content: string; display: boolean; details: { kinds: readonly NoteKind[] } } | undefined {
    const { approved, due } = this.read();
    // Only the approval's own compaction and its own kickoff go without the planning contract: any other turn
    // that gets in before the kickoff is still a turn under review.
    if (!due.length || this.preparing || (prompt !== undefined && prompt === this.kickoff?.text)) return undefined;
    const content = due.map(kind => kind === "contract"
      ? this.host.prompts.contract({ path: this.path, existing: this.exists(), askUser: this.host.askUser })
      : kind === "left" ? "[corral plan mode]\nPlan mode is off: the owner left it without approving a plan. Normal tool access is "
        + `restored. The draft at \`${this.path}\` is not approved; do not implement it unless the owner asks.`
      : `[corral plan reference]\nThe approved plan for this work, "${approved?.title ?? "Plan"}", is at `
        + `\`${approved?.path ?? this.path}\`. If you are still executing it and its details are no longer in context, `
        + "read it. Ignore this if that work is finished or unrelated.").join("\n\n");
    return { customType: PLAN_NOTE, content, display: false, details: { kinds: due } };
  }

  private exists(): boolean { return lstatSync(this.path, { throwIfNoEntry: false })?.isFile() === true; }

  private idle(): void {
    const busy = this.pending ? STARTING : this.host.busy();
    if (busy) throw new BridgeRefusal("plan_mode_unavailable", busy);
  }

  async enter(request?: string): Promise<string> {
    this.idle();
    if (this.phase !== "off") return "Plan mode is already on.";
    mkdirSync(this.host.dir, { recursive: true });
    this.write({ phase: "planning", revision: this.record.revision });
    const text = `Plan mode on. The agent explores and asks, then submits a plan for your review. Plan file: ${this.path}`;
    const first = request?.trim();
    if (first && !await this.host.submit(first, "owner")) return `${text}\nThe request was not delivered; send it again.`;
    return text;
  }

  leave(): string {
    this.idle();
    const { phase, revision } = this.record;
    if (phase === "off") return "Plan mode is already off.";
    this.write({ phase: "off", revision, left: true });
    return phase === "review" ? "Plan mode off. The submitted plan was not approved." : "Plan mode off. The draft plan stays on disk.";
  }

  /** `/plan [off | request]`. In review it reports the pending review, which a terminal opens instead. */
  async command(args: string): Promise<{ text: string; review?: true }> {
    const text = args.trim();
    if (text === "off") return { text: this.leave() };
    const phase = this.phase;
    if (phase === "off") return { text: await this.enter(text) };
    if (text) {
      throw new BridgeRefusal("bad_input", "Plan mode is already on; send the request as an ordinary message, or /plan off to leave");
    }
    if (phase === "planning") return { text: this.leave() };
    return { text: `"${this.record.title ?? "Plan"}" is awaiting your review.`, review: true };
  }

  private readPlan(): string {
    const stat = lstatSync(this.path, { throwIfNoEntry: false });
    if (!stat?.isFile()) throw new Error(`No plan at ${this.path}; write it before plan_submit.`);
    if (stat.size > PLAN_MAX_BYTES) throw new Error(`The plan at ${this.path} is over 256 KiB; shorten it.`);
    const body = readFileSync(this.path, "utf8");
    if (!body.trim()) throw new Error(`The plan at ${this.path} is empty; write it before plan_submit.`);
    return body;
  }

  /**
   * The agent's `plan_submit`: copy the plan as this submission and open its review. The review and the
   * execution read that copy, never the working file, so the owner approves exactly the text they were shown.
   */
  submit(title?: string): string {
    const { phase, revision } = this.record;
    if (phase === "review") throw new Error("A plan is already awaiting the owner's review; nothing was submitted.");
    if (phase !== "planning") throw new Error("Plan mode is not active; nothing was submitted.");
    const body = this.readPlan();
    const named = clipText((title?.trim() || heading(body) || "Plan").replace(/\s+/g, " "), TITLE_BYTES);
    const id = randomBytes(4).toString("hex"), next = revision + 1;
    const copy = join(this.host.dir, "plans", `${next}-${slug(named)}-${id}.md`);
    mkdirSync(join(this.host.dir, "plans"), { recursive: true });
    writeFileSync(copy, body, { flag: "wx", mode: 0o600 });
    this.write({ phase: "review", revision: next, title: named, id, digest: sha(body), path: copy });
    return "Submitted for the owner's review. End your turn now and do not implement. Approval arrives only as a host "
      + "message whose first line is [corral plan approved <id>]; any other message from the owner is feedback, so revise "
      + "the plan and call plan_submit again.";
  }

  /** A submission's copy as it is on disk now, or undefined when it is missing or no longer matches its digest. */
  private copy(record: PlanRecord): string | undefined {
    if (record.phase !== "review" || !record.path || !record.digest) return undefined;
    try {
      const body = readFileSync(record.path, "utf8");
      return sha(body) === record.digest ? body : undefined;
    } catch {
      return undefined;
    }
  }

  /** The text of the submission under review, for showing; execution reads the copy again. */
  reviewText(): string | undefined {
    const record = this.record, card = cardId(record);
    if (this.submitted?.card !== card) {
      const body = this.copy(record);
      this.submitted = body === undefined ? undefined : { card, body };
    }
    return this.submitted?.body;
  }

  /** The review card while a submission awaits the owner. Its id names the submission, so an older answer is missing. */
  card(): CardEntry | undefined {
    const record = this.record;
    if (record.phase !== "review") return undefined;
    const body = this.reviewText();
    return {
      card: { id: cardId(record), kind: "plan_review", title: record.title ?? "Plan", tool: PLAN_SUBMIT_TOOL,
        meta: [`revision ${record.revision}`, this.host.model()].filter(Boolean).join(" · "),
        ...(body === undefined ? {} : { body }), opened_at: new Date(record.at).toISOString(),
        questions: [cardQuestion("How should this plan proceed?", PLAN_OPTIONS.map(option => ({ ...option, destructive: false })), true)] },
      answer: (answers, dismiss) => dismiss ? "dismissed" : this.answer(singleAnswer(answers)),
    };
  }

  private answer({ options, text }: CardAnswer): AnswerStatus | Promise<AnswerStatus> {
    const choice = options[0];
    // Feedback typed without picking an option can only mean the plan is not approved yet.
    if (choice === undefined && text?.trim()) return this.choose("revise", text);
    if (choice !== "revise" && !Object.hasOwn(EXECUTE, choice ?? "")) invalidAnswer("choose one of the listed plan actions");
    return this.choose(choice as PlanChoice, text);
  }

  /** The owner's review choice, from the terminal or a card answer. Refused while it could not be carried out. */
  choose(choice: PlanChoice, text?: string): AnswerStatus | Promise<AnswerStatus> {
    if (this.phase !== "review") return "missing";
    if (this.pending) return "started";
    // Neither the plan nor the feedback could start a turn now, and the review would be gone.
    const blocked = this.host.blocked();
    if (blocked) throw new BridgeRefusal("plan_mode_unavailable", blocked);
    if (choice === "revise") return this.revise(text);
    void this.execute(EXECUTE[choice]!, text);
    return "started";
  }

  /**
   * An ordinary owner message that reached the model during review is feedback: the review is withdrawn and
   * planning resumes. The agent knows what it is without being told again: only a kickoff is an approval.
   */
  ownerSpoke(): void {
    const { phase, revision, title } = this.record;
    if (phase === "review" && !this.pending) this.write({ phase: "planning", revision, title });
  }

  /**
   * Keep planning. Feedback is the owner's message: like any owner message it withdraws the review when it
   * reaches the model, so feedback the session does not take leaves the review, and the owner's text, as they were.
   */
  private revise(feedback?: string): AnswerStatus | Promise<AnswerStatus> {
    const text = feedback?.trim();
    if (!text) {
      const { revision, title } = this.record;
      this.write({ phase: "planning", revision, title });
      this.host.note("Back to planning. Tell the agent what to change.");
      return "delivered";
    }
    return this.host.submit("[corral plan review]\nThe owner reviewed the plan and wants changes before approving:\n"
      + `${text}\n\nRevise \`${this.path}\`, then call plan_submit again.`, "owner").then(taken => {
      if (!taken) throw new BridgeRefusal("plan_mode_unavailable", "The session did not take the feedback; the review is still open");
      return "delivered";
    });
  }

  /**
   * Carry out an approval. Nothing is recorded for it: the review ends when the kickoff itself is in the
   * transcript, so a refusal, a failure or a restart at any point before that leaves the same review open.
   */
  private async execute(context: PlanContext, note?: string): Promise<void> {
    this.starting = true;
    try {
      const record = this.record;
      if (record.phase !== "review") return;
      await this.host.stopTurn();
      // Read after the planning turn can no longer touch it: what runs must be what the digest names, now.
      const plan = this.copy(record);
      if (plan === undefined || !record.path) {
        this.submitted = undefined;
        throw new Error(CHANGED);
      }
      const title = record.title ?? "Plan", { revision, path } = record, card = cardId(record);
      this.preparing = true;
      try {
        if (context === "compact") await this.host.compact(COMPACT_INSTRUCTIONS);
        // The reset summary carries the plan itself, so the cleared context holds it whatever happens next.
        else if (context === "clean") await this.host.reset(this.host.prompts.reset({ path, title, revision, plan }));
      } finally {
        this.preparing = false;
      }
      // The owner may have paused while the context was prepared; the admission order then decides for good.
      const blocked = this.host.blocked();
      if (blocked) throw new Error(blocked);
      const text = `${kickoffLine(card)}\n${this.host.prompts.approved({ path, title, revision, plan, context, note, todo: this.host.todo })}`;
      this.kickoff = { card, text, path, admitted: false,
        journal: `Approved plan: ${title} (revision ${revision}, ${CONTEXT_LABEL[context]})` };
      const kickoff = this.kickoff;
      if (!await this.host.submit(text, "auto", card)) throw new Error("the session did not take it");
      kickoff.admitted = true;
    } catch (error) {
      this.kickoff = undefined;
      this.host.note(`The plan was not started: ${error instanceof Error ? error.message : String(error)}. The review is still open.`, "error");
      this.announce();
    } finally {
      this.starting = false;
    }
  }

  /** One byte-range page of the submission `card` reviews: the text an execute answer to that card runs. */
  doc(card: string, after?: string) {
    const record = this.record, text = card === this.review ? this.reviewText() : undefined;
    if (text === undefined) {
      if (after !== undefined) throw new BridgeRefusal("stale_page", "the plan under review changed; read it again from the first page");
      throw new BridgeRefusal("plan_mode_doc_missing", "no plan is under review with this card");
    }
    const token = fingerprintPart(["plan_mode_doc", card, record.digest]);
    const full = Buffer.from(text, "utf8"), bytes = full.length;
    const offset = after === undefined ? 0 : cursorOffset(after, token);
    if (offset > bytes || ((full[offset] ?? 0) & 0xc0) === 0x80) {
      throw new BridgeRefusal("bad_input", "plan document cursor is not on a UTF-8 character boundary");
    }
    const head = { title: record.title ?? "Plan", revision: record.revision, bytes, offset };
    let end = Math.min(bytes, offset + DOC_PAGE_BYTES);
    while (((full[end] ?? 0) & 0xc0) === 0x80) end--;
    const budget = SESSION_PART_MAX - jsonBytes({ ...head, body: "", next: partCursor(token, bytes) });
    const body = clipText(full.toString("utf8", offset, end), budget);
    end = offset + Buffer.byteLength(body);
    return { ...head, body, next: end < bytes ? partCursor(token, end) : undefined };
  }
}

export function planSubmitTool(plan: () => PlanMode): ToolDefinition {
  return {
    name: PLAN_SUBMIT_TOOL, label: "Submit the plan", exposure: "model-only", executionMode: "sequential",
    description: "Plan mode only: submit the plan document for the owner's review. Write the finished plan to the plan file "
      + "first. The owner then chooses to execute it or to keep planning; end your turn after calling this. Outside plan mode "
      + "it fails and submits nothing.",
    promptSnippet: "Submit the finished plan for the owner's review (plan mode only)",
    parameters: Type.Object({
      title: Type.Optional(Type.String({ minLength: 1, maxLength: 160, description: "A short title; defaults to the plan's first heading" })),
    }, { additionalProperties: false }),
    async execute(_id, params: { title?: string }) {
      const text = plan().submit(params.title);
      const { title, revision } = plan().record;
      return { content: [{ type: "text", text }], details: { status: "review", title, revision } };
    },
  };
}
