// Plan mode (docs/plans/pi-plan-mode.md). The agent plans read-only with the owner, submits one plan
// document, and the owner's review choice starts execution. The phase lives in the transcript, so a
// restart restores it and a rewind undoes it. Nothing here changes the system prompt or the tool list.
import { createHash } from "node:crypto";
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

const Approved = z.object({ title: z.string(), revision: z.number(), digest: z.string(), path: z.string(),
  context: z.enum(["keep", "compact", "clean"]), model: z.string().optional() });
const Saved = z.object({ version: z.literal(1), phase: z.enum(["off", "planning", "review"]),
  revision: z.number().int().nonnegative(), at: z.number(), title: z.string().optional(), digest: z.string().optional(),
  // The owner left without approving, or answered a review with an ordinary message.
  left: z.boolean().optional(), replied: z.boolean().optional(), approved: Approved.optional() });
export type PlanRecord = z.infer<typeof Saved>;
export type PlanPhase = PlanRecord["phase"];
export type PlanChoice = "execute" | "execute_clean" | "execute_compact" | "revise";
type NoteKind = "contract" | "left" | "replied" | "pointer";

export interface PlanEntry { readonly type: string; readonly customType?: string; readonly data?: unknown; readonly details?: unknown }

export interface PlanHost {
  /** The session's state directory: the plan document and its approved copies live here. */
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
  stopTurn(): Promise<void>;
  compact(instructions: string): Promise<void>;
  /** Replace the model's context with `summary`, keeping no earlier entry. */
  reset(summary: string): Promise<void>;
  /** Start or steer a turn; false when the session refused the input. */
  submit(text: string, origin: "owner" | "auto"): Promise<boolean>;
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
const RESET_SUMMARY = "The earlier conversation was a planning discussion. It was cleared at the owner's request when they "
  + "approved the plan. The approved plan follows in the next message and is the whole brief.";

const sha = (text: string): string => createHash("sha256").update(text).digest("hex");
const heading = (text: string): string | undefined => /^[ \t]*#[ \t]+(.+?)[ \t]*$/m.exec(text)?.[1]?.trim() || undefined;
const slug = (title: string): string => title.toLowerCase().replace(/[^a-z0-9]+/g, "-").replace(/^-+|-+$/g, "").slice(0, 48) || "plan";
const cardId = (record: PlanRecord): string => `plan-${record.revision}-${record.digest?.slice(0, 12) ?? ""}`;

interface Scan { record: PlanRecord; due?: NoteKind }

/** The phase and the one message the model has not yet been given about it, read from the branch alone. */
function scan(entries: readonly PlanEntry[]): Scan {
  let record = OFF, announced = false, told = false;
  let due: Exclude<NoteKind, "contract"> | undefined;
  for (const entry of entries) {
    if (entry.type === "custom" && entry.customType === PLAN_ENTRY) {
      const parsed = Saved.safeParse(entry.data);
      if (!parsed.success) continue;
      const next = parsed.data;
      if (next.phase === "off") {
        due = next.left && told ? "left" : undefined;
        announced = told = false;
      } else if (next.replied) due = "replied";
      record = next;
    } else if (entry.type === "compaction") {
      // A summary is not the contract, and it can lose an approved plan's text.
      announced = false;
      if (record.phase === "off" && record.approved) due = "pointer";
    } else if (entry.type === "custom_message" && entry.customType === PLAN_NOTE) {
      const kind = (entry.details as { kind?: unknown } | undefined)?.kind;
      if (kind === "contract") announced = told = true;
      else if (kind === due) due = undefined;
    }
  }
  if (record.phase === "planning" && !announced) return { record, due: "contract" };
  return record.phase === "review" || due === undefined ? { record } : { record, due };
}

export class PlanMode {
  /** The one plan document of the session. */
  readonly path: string;
  private cache: { leaf: string | null; scan: Scan } | undefined;
  private submitted: { digest: string; body: string } | undefined;
  private starting = false;
  private readonly listeners = new Set<() => void>();

  constructor(private readonly host: PlanHost) { this.path = join(host.dir, PLAN_FILE); }

  private read(): Scan {
    const leaf = this.host.leaf();
    if (!this.cache || this.cache.leaf !== leaf) this.cache = { leaf, scan: scan(this.host.entries()) };
    return this.cache.scan;
  }

  get record(): PlanRecord { return this.read().record; }
  get phase(): PlanPhase { return this.record.phase; }
  /** The id of the open review card, which names one submission. */
  get review(): string | undefined {
    const record = this.record;
    return record.phase === "review" ? cardId(record) : undefined;
  }

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
    this.host.changed();
    for (const listener of [...this.listeners]) { try { listener(); } catch { /* A view does not own the mode. */ } }
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

  /** The one message the starting turn needs to know the mode; the transcript records that it was given. */
  turnMessage(): { customType: string; content: string; display: boolean; details: { kind: NoteKind } } | undefined {
    const { record, due } = this.read();
    if (!due) return undefined;
    const content = due === "contract"
      ? this.host.prompts.contract({ path: this.path, existing: this.exists(), askUser: this.host.askUser })
      : due === "left" ? "[corral plan mode]\nPlan mode is off: the owner left it without approving a plan. Normal tool access is "
        + `restored. The draft at \`${this.path}\` is not approved; do not implement it unless the owner asks.`
      : due === "replied" ? "[corral plan review]\nThe owner replied instead of approving the submitted plan. Treat their "
        + "message as feedback: revise the plan, then call `plan_submit` again."
      : `[corral plan reference]\nThe approved plan for this work, "${record.approved?.title ?? "Plan"}", is at `
        + `\`${record.approved?.path ?? this.path}\`. If you are still executing it and its details are no longer in context, `
        + "read it. Ignore this if that work is finished or unrelated.";
    return { customType: PLAN_NOTE, content, display: false, details: { kind: due } };
  }

  private exists(): boolean { return lstatSync(this.path, { throwIfNoEntry: false })?.isFile() === true; }

  private idle(): void {
    const busy = this.host.busy();
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

  /** The agent's `plan_submit`: record the submission and open the review. */
  submit(title?: string): string {
    const { phase, revision } = this.record;
    if (phase === "review") throw new Error("A plan is already awaiting the owner's review; nothing was submitted.");
    if (phase !== "planning") throw new Error("Plan mode is not active; nothing was submitted.");
    const body = this.readPlan();
    const digest = sha(body);
    this.submitted = { digest, body };
    this.write({ phase: "review", revision: revision + 1, digest,
      title: clipText((title?.trim() || heading(body) || "Plan").replace(/\s+/g, " "), TITLE_BYTES) });
    return "Submitted for the owner's review. End your turn now and do not implement; the owner's choice arrives as a later message.";
  }

  /** The submitted text, or undefined when the file no longer matches what was submitted. */
  reviewText(): string | undefined {
    const digest = this.record.digest;
    if (digest === undefined) return undefined;
    if (this.submitted?.digest !== digest) {
      try {
        const body = this.readPlan();
        this.submitted = sha(body) === digest ? { digest, body } : undefined;
      } catch { this.submitted = undefined; }
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

  private answer({ options, text }: CardAnswer): AnswerStatus {
    const choice = options[0];
    // Feedback typed without picking an option can only mean the plan is not approved yet.
    if (choice === undefined && text?.trim()) return this.choose("revise", text);
    if (choice !== "revise" && !Object.hasOwn(EXECUTE, choice ?? "")) invalidAnswer("choose one of the listed plan actions");
    return this.choose(choice as PlanChoice, text);
  }

  /** The owner's review choice, from the terminal or a card answer. */
  choose(choice: PlanChoice, text?: string): AnswerStatus {
    if (this.phase !== "review") return "missing";
    if (this.starting) return "started";
    if (choice === "revise") {
      void this.revise(text);
      return "delivered";
    }
    void this.execute(EXECUTE[choice]!, text);
    return "started";
  }

  /** An ordinary owner message during review is feedback: the review is withdrawn and planning resumes. */
  ownerSpoke(): void {
    const { phase, revision, title } = this.record;
    if (phase === "review" && !this.starting) this.write({ phase: "planning", revision, title, replied: true });
  }

  private async revise(feedback?: string): Promise<void> {
    const { revision, title } = this.record;
    this.write({ phase: "planning", revision, title });
    const text = feedback?.trim();
    if (!text) {
      this.host.note("Back to planning. Tell the agent what to change.");
      return;
    }
    const sent = await this.host.submit("[corral plan review]\nThe owner reviewed the plan and wants changes before approving:\n"
      + `${text}\n\nRevise \`${this.path}\`, then call plan_submit again.`, "owner");
    if (!sent) this.host.note("The plan feedback was not delivered; send it again.", "error");
  }

  private async execute(context: PlanContext, note?: string): Promise<void> {
    this.starting = true;
    try {
      const record = this.record;
      if (record.phase !== "review" || !record.digest) return;
      const plan = this.readPlan();
      if (sha(plan) !== record.digest) {
        throw new Error("the plan file changed after it was submitted; choose Keep planning and have it submitted again");
      }
      await this.host.stopTurn();
      const title = record.title ?? "Plan", { revision, digest } = record;
      const copy = join(this.host.dir, "plans", `${revision}-${slug(title)}.md`);
      mkdirSync(join(this.host.dir, "plans"), { recursive: true });
      writeFileSync(copy, plan, { mode: 0o600 });
      if (context === "compact") await this.host.compact(COMPACT_INSTRUCTIONS);
      else if (context === "clean") await this.host.reset(RESET_SUMMARY);
      const model = this.host.model();
      this.write({ phase: "off", revision, approved: { title, revision, digest, path: copy, context, ...(model ? { model } : {}) } });
      this.host.approved?.(`Approved plan: ${title} (revision ${revision}, ${CONTEXT_LABEL[context]})`);
      const kickoff = this.host.prompts.approved({ path: copy, title, revision, plan, context, note, todo: this.host.todo });
      if (!await this.host.submit(kickoff, "auto")) {
        this.host.note(`The approved plan was not delivered to the agent. Ask it to execute ${copy}.`, "error");
      }
    } catch (error) {
      this.host.note(`The plan was not started: ${error instanceof Error ? error.message : String(error)}. The review is still open.`, "error");
      this.announce();
    } finally {
      this.starting = false;
    }
  }

  /** One byte-range page of the plan document, in any phase. */
  doc(after?: string) {
    let text: string;
    try { text = this.readPlan(); }
    catch {
      if (after !== undefined) throw new BridgeRefusal("stale_page", "the plan document changed; read it again from the first page");
      throw new BridgeRefusal("plan_mode_doc_missing", "this session has no plan document");
    }
    const record = this.record;
    const token = fingerprintPart(["plan_mode_doc", sha(text)]);
    const full = Buffer.from(text, "utf8"), bytes = full.length;
    const offset = after === undefined ? 0 : cursorOffset(after, token);
    if (offset > bytes || ((full[offset] ?? 0) & 0xc0) === 0x80) {
      throw new BridgeRefusal("bad_input", "plan document cursor is not on a UTF-8 character boundary");
    }
    const head = { title: record.title ?? heading(text) ?? "Plan", revision: record.revision, bytes, offset };
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
