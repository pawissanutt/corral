// Session questions live in the Pi transcript. Tools can ask or withdraw; only the owner can answer.
// Pane and socket answers settle the same record before any follow-up enters the conversation.
import { z } from "zod";
import { canonicalJson, sha256Hex } from "../tools/corral.ts";

export const QUESTION_ENTRY = "corral_owner_question";
export const PENDING_MAX = 8;
export const QUESTION_MAX = 128;
export const QUESTION_TTL = 24 * 60 * 60 * 1000;
export const ANSWER_MAX_BYTES = 16 * 1024;
export const Choice = z.object({ question: z.string().min(1).max(1000),
  options: z.array(z.string().min(1).max(300)).min(2).max(4), freeText: z.boolean().optional() });
export type ChoiceQuestion = z.infer<typeof Choice>;
// An answer is a listed option (with an optional freeText note) or, unless freeText is false, the
// owner's own typed text in place of an option.
const Answer = z.object({ option: z.string().optional(), text: z.string().optional() });
export type ChoiceAnswer = z.infer<typeof Answer>;
export function validAnswer(q: ChoiceQuestion, a: ChoiceAnswer): boolean {
  if (a.option === undefined) return q.freeText !== false && !!a.text?.trim();
  return q.options.includes(a.option) && (!a.text || q.freeText === true);
}
const Record = z.object({ version: z.literal(1), id: z.string(), session: z.string(),
  questions: z.array(Choice).min(1).max(4), expires: z.number(),
  tool: z.enum(["ask_user", "bash", "file_write"]).optional(),
  status: z.enum(["pending", "answered", "cancelled", "expired", "withdrawn", "ended"]),
  answers: z.array(Answer).optional(), message: z.string().optional(),
});
export type PendingQuestion = z.infer<typeof Record>;
export type QuestionEntry = { type: string; customType?: string; data?: unknown; message?: unknown };
export interface QuestionHost {
  session(): string;
  append(value: PendingQuestion): void;
  entries(): readonly QuestionEntry[];
  /** Submit the answer as owner input; the persisted user message proves delivery. */
  deliver(text: string): Promise<boolean>;
  changed(question?: PendingQuestion): void;
  now?: () => number;
}

export const answerMarker = (id: string) => `[ask_user answer ${id}]`;
function answerText(questions: readonly ChoiceQuestion[], answers: readonly ChoiceAnswer[]): string {
  return questions.map((question, i) => {
    const a = answers[i]!;
    if (a.option === undefined) return `${question.question}: ${a.text ?? ""}`;
    return `${question.question}: ${a.option}${a.text ? `\n${a.text}` : ""}`;
  }).join("\n");
}

export class PendingQuestions {
  private readonly questions = new Map<string, PendingQuestion>();
  private readonly consumed = new Set<string>();
  private readonly sending = new Map<string, Promise<void>>();
  private readonly queued = new Map<string, symbol>();
  private closed = false;
  private expiry: ReturnType<typeof setTimeout> | undefined;
  private readonly now: () => number;
  constructor(private readonly host: QuestionHost) { this.now = host.now ?? Date.now; }

  restore(): void {
    this.closed = false;
    this.questions.clear(); this.consumed.clear(); this.queued.clear(); this.sending.clear();
    const session = this.host.session();
    const userTexts = new Set<string>();
    for (const entry of this.host.entries()) {
      if (entry.type === "custom" && entry.customType === QUESTION_ENTRY) {
        const parsed = Record.safeParse(entry.data);
        if (!parsed.success || parsed.data.session !== session) continue;
        this.questions.set(parsed.data.id, parsed.data);
      }
      const m = entry.message as { role?: string; content?: string | { type: string; text?: string }[] } | undefined;
      if (entry.type === "message" && m?.role === "user") {
        userTexts.add(typeof m.content === "string" ? m.content : (m.content ?? []).map(p => p.type === "text" ? p.text : "").join(""));
      }
    }
    // Check the actual persisted user message, not a delivery-intent entry or an in-memory queue.
    for (const q of this.questions.values()) {
      if (q.message && userTexts.has(q.message)) this.consumed.add(q.id);
    }
    // A process-local approval binding cannot be restored, including after rewind or reload.
    for (const q of this.questions.values()) {
      if ((q.tool === "bash" || q.tool === "file_write") &&
          (q.status === "pending" || (q.status === "answered" && !this.consumed.has(q.id)))) {
        this.save({ ...q, status: "cancelled" });
      }
    }
    this.expire();
    this.scheduleExpiry();
    for (const q of this.questions.values()) if (q.status === "pending") this.host.changed(q);
    this.host.changed();
  }

  private save(q: PendingQuestion): void {
    this.host.append(q); // Append first: a disk failure cannot create a live undurable choice.
    this.questions.set(q.id, q);
    this.scheduleExpiry();
    this.host.changed(q);
  }

  private scheduleExpiry(): void {
    clearTimeout(this.expiry);
    if (this.closed) return;
    let deadline = Infinity;
    for (const q of this.questions.values()) if (q.status === "pending") deadline = Math.min(deadline, q.expires);
    if (Number.isFinite(deadline)) {
      this.expiry = setTimeout(() => { this.expire(); this.scheduleExpiry(); },
        Math.min(QUESTION_TTL, Math.max(1, deadline - this.now())));
      this.expiry.unref();
    }
  }

  list(): readonly PendingQuestion[] { return [...this.questions.values()]; }
  observeUser(text: string): void {
    for (const q of this.questions.values()) if (q.message === text) this.consumed.add(q.id);
    this.host.changed();
  }
  pending(): readonly PendingQuestion[] { this.expire(); return this.list().filter(q => q.status === "pending"); }
  get(id: string): PendingQuestion | undefined { this.expire(); return this.questions.get(id); }
  allowed(id: string): boolean {
    const q = this.get(id);
    return q?.status === "answered" && q.expires > this.now() && q.answers?.[0]?.option === "allow";
  }
  status(id: string): string { return this.consumed.has(id) ? "delivered" : this.questions.get(id)?.status ?? "missing"; }

  ask(call: string, questions: ChoiceQuestion[], tool?: PendingQuestion["tool"]): PendingQuestion {
    if (this.closed) throw new Error("question_session_closed");
    const pendingCount = this.pending().length;
    const session = this.host.session();
    const id = `q-${sha256Hex(`${session}\0${call}`).slice(0, 32)}`;
    const prior = this.questions.get(id);
    if (prior) {
      if (canonicalJson(prior.questions) !== canonicalJson(questions)) throw new Error("question_call_conflict");
      return prior;
    }
    if (this.questions.size >= QUESTION_MAX) throw new Error("question_session_limit: resume does not reset the limit");
    if (pendingCount >= PENDING_MAX) throw new Error("pending_question_limit");
    const q: PendingQuestion = { version: 1, id, session,
      questions, ...(tool ? { tool } : {}), expires: this.now() + QUESTION_TTL, status: "pending" };
    this.save(q);
    return q;
  }

  withdraw(id: string): string { return this.finish(id, "withdrawn"); }
  cancel(id: string): string { return this.finish(id, "cancelled"); }
  private finish(id: string, status: "withdrawn" | "cancelled"): string {
    if (this.closed) throw new Error("question_session_closed");
    const q = this.get(id);
    if (!q) throw new Error("question_not_found");
    if (q.status === "pending") { this.save({ ...q, status }); return status; }
    return q.status;
  }
  expire(): void {
    if (this.closed) return;
    for (const q of this.questions.values()) {
      if (q.status === "pending" && q.expires <= this.now()) this.save({ ...q, status: "expired" });
    }
  }
  async answer(id: string, answers: ChoiceAnswer[], before?: () => Promise<unknown>): Promise<string> {
    if (this.closed) return "session_closed";
    const q = this.get(id);
    if (!q || q.status !== "pending") return q?.status ?? "missing";
    if (answers.length !== q.questions.length || answers.some((a, i) => !validAnswer(q.questions[i]!, a))) {
      throw new Error("invalid_question_answer");
    }
    const text = answerText(q.questions, answers);
    if (Buffer.byteLength(text) > ANSWER_MAX_BYTES) throw new Error("question_answer_too_large");
    if (before) {
      await before();
      if (this.closed) return "session_closed";
      if (this.get(id) !== q) return this.status(id);
    }
    // Only actual answers, never a cancelled or withdrawn status, become owner input.
    this.save({ ...q, status: "answered", answers, message: `${answerMarker(id)}\n${text}` });
    await this.send(id);
    return this.status(id);
  }
  /** Runs on host startup and whenever the owner reopens /questions. */
  async recover(): Promise<void> {
    this.expire();
    await Promise.all(this.list().filter(q => q.status === "answered").map(q => this.send(q.id)));
  }
  private send(id: string): Promise<void> {
    const running = this.sending.get(id);
    if (running) return running;
    const result = this.sendAnswer(id).finally(() => { if (this.sending.get(id) === result) this.sending.delete(id); });
    this.sending.set(id, result);
    return result;
  }
  private async sendAnswer(id: string): Promise<void> {
    const q = this.questions.get(id);
    if (this.closed || q?.status !== "answered" || !q.message || this.consumed.has(id) || this.queued.has(id)) return;
    const attempt = Symbol();
    this.queued.set(id, attempt);
    let accepted = false;
    try {
      accepted = await this.host.deliver(q.message);
    } finally {
      // A completion from before close or restore must not clear a newer queued delivery.
      if (!accepted && this.queued.get(id) === attempt) this.queued.delete(id);
    }
  }
  /** A temporary host exit suspends; a true session replacement closes all unresolved choices. */
  close(ended = false): void {
    if (ended) for (const q of this.questions.values()) {
      if (q.status === "pending" || (q.status === "answered" && !this.consumed.has(q.id))) this.save({ ...q, status: "ended" });
    }
    this.closed = true;
    clearTimeout(this.expiry);
    this.queued.clear();
  }
}
