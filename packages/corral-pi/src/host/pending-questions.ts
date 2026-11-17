// Local interactive choices, not board questions. The existing Pi journal is the durable store.
// Only the owner UI calls answer(); tools can ask/withdraw, never fabricate an answer.
// A host with a mirror (the pane PM) also opens each question as a canonical owner ask, so the
// owner can answer it in the app: corral's record then decides which answer the model receives.
import { z } from "zod";
import { canonicalJson, toolRequestId } from "../tools/corral.ts";
import { OWNER_INPUT_TEXT_MAX } from "../roles/assistant/owner-action.ts";
import { askOpen, type CanonicalAnswer, type CanonicalAsk, type QuestionMirror } from "./owner-ask-mirror.ts";

export const QUESTION_ENTRY = "corral_owner_question";
export const PENDING_MAX = 8;
export const QUESTION_MAX = 128;
export const QUESTION_TTL = 24 * 60 * 60 * 1000;
/** Canonical records are polled: quickly after a change here, then backing off while the owner is away. */
export const MIRROR_POLL_MIN = 2_000;
export const MIRROR_POLL_MAX = 15_000;
const ANSWER_MAX_BYTES = OWNER_INPUT_TEXT_MAX - 160;
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
const Record = z.object({ version: z.literal(1), id: z.string(), session: z.string(), generation: z.number(),
  key: z.string(), supersedes: z.string().optional(), questions: z.array(Choice).min(1).max(4), expires: z.number(),
  status: z.enum(["pending", "answered", "cancelled", "expired", "withdrawn", "ended"]),
  answers: z.array(Answer).optional(), message: z.string().optional(), recordedGeneration: z.number().optional(),
  // Canonical owner-ask keys, one per question, once corral opened them; `settled` once those
  // records reached a final state and nothing is left to reconcile.
  canonical: z.array(z.string()).optional(), settled: z.boolean().optional(),
});
export type PendingQuestion = z.infer<typeof Record>;
export type QuestionEntry = { type: string; customType?: string; data?: unknown; message?: unknown };
export interface QuestionHost {
  session(): string;
  generation: number;
  append(value: PendingQuestion): void;
  entries(): readonly QuestionEntry[];
  /** Records actual owner input via the host-only authenticated event/receipt path. */
  record(text: string, id: string): Promise<string>;
  /** Existing delivery module + normal follow-up; its user message is the consumption receipt. */
  deliver(text: string, key: string): Promise<boolean>;
  changed(question?: PendingQuestion): void;
  now?: () => number;
  /** Opens each question as a canonical owner ask the app can answer. */
  mirror?: QuestionMirror;
  /** A mirror pass failed; the next poll retries it. */
  mirrorFailed?(error: unknown): void;
  /** Runs `run` after `ms` and returns its cancel. Defaults to a timer that never keeps the host alive. */
  defer?(run: () => void, ms: number): () => void;
}

export const answerMarker = (id: string) => `[ask_user answer ${id}]`;
export function answerText(q: PendingQuestion): string {
  return q.questions.map((question, i) => {
    const a = q.answers?.[i];
    if (a?.option === undefined) return `${question.question}: ${a?.text ?? ""}`;
    return `${question.question}: ${a.option}${a.text ? `\n${a.text}` : ""}`;
  }).join("\n");
}
/** Corral requires answer text, so a bare option is recorded as its own label. */
function canonicalAnswer(a: ChoiceAnswer): CanonicalAnswer {
  return { text: a.text?.trim() ? a.text : a.option ?? "", ...(a.option === undefined ? {} : { option: a.option }) };
}
function choiceAnswer(a: CanonicalAnswer): ChoiceAnswer {
  return { ...(a.option === undefined ? {} : { option: a.option }), ...(a.text !== a.option ? { text: a.text } : {}) };
}
const answeredAll = (asks: readonly CanonicalAsk[]) => asks.every(ask => ask.state === "answered" && ask.answer !== undefined);
function deferred(run: () => void, ms: number): () => void {
  const timer = setTimeout(run, ms);
  timer.unref();
  return () => clearTimeout(timer);
}

export class PendingQuestions {
  private readonly questions = new Map<string, PendingQuestion>();
  private readonly consumed = new Set<string>();
  private readonly sending = new Map<string, Promise<void>>();
  private readonly queued = new Set<string>();
  private closed = false;
  private epoch = 0;
  private readonly now: () => number;
  private pass: Promise<boolean> | undefined;
  private cancelPoll: (() => void) | undefined;
  private pollDelay = MIRROR_POLL_MIN;
  constructor(private readonly host: QuestionHost) { this.now = host.now ?? Date.now; }

  restore(): void {
    this.closed = false; this.epoch++;
    this.questions.clear(); this.consumed.clear(); this.queued.clear(); this.sending.clear();
    const userTexts: string[] = [];
    for (const entry of this.host.entries()) {
      if (entry.type === "custom" && entry.customType === QUESTION_ENTRY) {
        const parsed = Record.safeParse(entry.data);
        if (!parsed.success || parsed.data.session !== this.host.session()) continue;
        this.questions.set(parsed.data.id, parsed.data);
      }
      const m = entry.message as { role?: string; content?: string | { type: string; text?: string }[] } | undefined;
      if (entry.type === "message" && m?.role === "user") {
        userTexts.push(typeof m.content === "string" ? m.content : (m.content ?? []).map(p => p.type === "text" ? p.text : "").join(""));
      }
    }
    // Check the actual persisted user message, not a delivery-intent entry or an in-memory queue.
    for (const q of this.questions.values()) {
      if (q.message && userTexts.some(text => text === q.message)) this.consumed.add(q.id);
    }
    for (const q of this.questions.values()) {
      if (q.supersedes && this.questions.get(q.supersedes)?.status === "pending") this.withdraw(q.supersedes);
    }
    this.expire(); this.host.changed();
  }

  /** `announce` is false for mirror bookkeeping, which changes nothing the owner or model sees. */
  private save(q: PendingQuestion, announce = true): void {
    this.host.append(q); // Append first: a disk failure cannot create a live undurable choice.
    this.questions.set(q.id, q); this.host.changed(announce ? q : undefined);
  }
  list(): readonly PendingQuestion[] { return [...this.questions.values()]; }
  observeUser(text: string): void {
    for (const q of this.questions.values()) if (q.message === text) this.consumed.add(q.id);
    this.host.changed();
  }
  pending(): readonly PendingQuestion[] { this.expire(); return this.list().filter(q => q.status === "pending"); }
  get(id: string): PendingQuestion | undefined { this.expire(); return this.questions.get(id); }
  status(id: string): string { return this.consumed.has(id) ? "delivered" : this.questions.get(id)?.status ?? "missing"; }

  ask(call: string, questions: ChoiceQuestion[], key = call, supersedes?: string): PendingQuestion {
    if (this.closed) throw new Error("question_session_closed");
    this.expire();
    const id = toolRequestId(this.host.session(), `ask_user:${key}`);
    const prior = this.questions.get(id);
    if (prior) {
      if (canonicalJson(prior.questions) !== canonicalJson(questions)) throw new Error("question_key_conflict");
      return prior;
    }
    if (this.questions.size >= QUESTION_MAX) throw new Error("question_session_limit: resume does not reset the limit");
    if (supersedes) {
      const old = this.questions.get(supersedes);
      if (!old || old.status !== "pending") throw new Error("superseded_question_not_pending");
    }
    if (this.pending().length - (supersedes ? 1 : 0) >= PENDING_MAX) throw new Error("pending_question_limit");
    const q: PendingQuestion = { version: 1, id, session: this.host.session(), generation: this.host.generation,
      key, ...(supersedes ? { supersedes } : {}), questions, expires: this.now() + QUESTION_TTL, status: "pending" };
    // Create first so interruption cannot lose both the old and replacement question.
    this.save(q);
    if (supersedes) this.withdraw(supersedes);
    this.mirrorSoon();
    return q;
  }

  withdraw(id: string): string { return this.finish(id, "withdrawn"); }
  cancel(id: string): string { return this.finish(id, "cancelled"); }
  private finish(id: string, status: "withdrawn" | "cancelled"): string {
    if (this.closed) throw new Error("question_session_closed");
    const q = this.get(id);
    if (!q) throw new Error("question_not_found");
    if (q.status === "pending") { this.save({ ...q, status }); this.mirrorSoon(); return status; }
    return q.status;
  }
  expire(): void {
    if (this.closed) return;
    for (const q of this.questions.values()) {
      if (q.status === "pending" && q.expires <= this.now()) { this.save({ ...q, status: "expired" }); this.mirrorSoon(); }
    }
  }
  async answer(id: string, answers: ChoiceAnswer[]): Promise<string> {
    if (this.closed) return "session_closed";
    const q = this.get(id);
    if (!q || q.status !== "pending") return q?.status ?? "missing";
    if (answers.length !== q.questions.length || answers.some((a, i) => !validAnswer(q.questions[i]!, a))) {
      throw new Error("invalid_question_answer");
    }
    if (Buffer.byteLength(answerText({ ...q, answers })) > ANSWER_MAX_BYTES) throw new Error("question_answer_too_large");
    const mirror = this.host.mirror;
    if (!mirror || !q.canonical) return this.settle(id, answers, false);
    // The app may have answered first: each record's canonical winner replaces the pane answer.
    const epoch = this.epoch, asks: CanonicalAsk[] = [];
    for (const [i, key] of q.canonical.entries()) asks.push(await mirror.answer(key, canonicalAnswer(answers[i]!)));
    if (this.closed || this.epoch !== epoch) return "session_closed";
    if (!answeredAll(asks)) return this.cancelMirrored(mirror, id, asks);
    return this.settle(id, asks.map((ask, i) => ask.mine ? answers[i]! : choiceAnswer(ask.answer!)), true);
  }
  /** The one transition out of pending for an answer, whichever surface the owner used. */
  private async settle(id: string, answers: ChoiceAnswer[], settled: boolean): Promise<string> {
    const q = this.questions.get(id);
    if (!q || q.status !== "pending") return q ? this.status(id) : "missing";
    let answered = { ...q, status: "answered" as const, answers, ...(settled ? { settled } : {}) };
    if (Buffer.byteLength(answerText(answered)) > ANSWER_MAX_BYTES) {
      // Only an app answer can be this large; the pane refuses one before recording it.
      answered = { ...answered, answers: (q.canonical ?? []).map(key => ({ text: `The owner's answer is on the board: corral question show ${key} --json` })) };
    }
    this.save(answered); // The actual owner answer survives a crash before journal/delivery.
    await this.send(id);
    return this.status(id);
  }
  /** The owner cancelled a record in the app, or corral closed it: no answer, and no consent. */
  private async cancelMirrored(mirror: QuestionMirror, id: string, asks: readonly CanonicalAsk[]): Promise<string> {
    for (const ask of asks) if (askOpen(ask)) await mirror.withdraw(ask.key);
    const q = this.questions.get(id);
    if (!q || this.closed) return q ? this.status(id) : "missing";
    if (q.status === "pending") this.save({ ...q, status: "cancelled", settled: true });
    return this.status(id);
  }

  private mirrorSoon(): void { this.pollDelay = MIRROR_POLL_MIN; this.poll(0); }
  private poll(ms: number): void {
    if (!this.host.mirror || this.closed) return;
    this.cancelPoll?.();
    const cancel = (this.host.defer ?? deferred)(() => {
      if (this.cancelPoll === cancel) this.cancelPoll = undefined;
      void this.reconcile().then(open => {
        if (!open || this.cancelPoll) return; // settled, or a newer change already scheduled its own pass
        this.poll(this.pollDelay);
        this.pollDelay = Math.min(this.pollDelay * 2, MIRROR_POLL_MAX);
      }, error => { this.host.mirrorFailed?.(error); });
    }, ms);
    this.cancelPoll = cancel;
  }
  /** One mirror pass, shared by concurrent callers. True while a canonical record can still change. */
  reconcile(): Promise<boolean> {
    const mirror = this.host.mirror;
    if (!mirror || this.closed) return Promise.resolve(false);
    if (this.pass) return this.pass;
    const pass = this.mirrorPass(mirror).finally(() => { if (this.pass === pass) this.pass = undefined; });
    this.pass = pass;
    return pass;
  }
  private async mirrorPass(mirror: QuestionMirror): Promise<boolean> {
    const epoch = this.epoch, live = () => !this.closed && this.epoch === epoch;
    let open = false;
    this.expire();
    for (const id of [...this.questions.keys()]) {
      try {
        if (await this.mirrorOne(mirror, id, live)) open = true;
      } catch (error) {
        open = true;
        this.host.mirrorFailed?.(error);
      }
      if (!live()) return false;
    }
    return open;
  }
  private async mirrorOne(mirror: QuestionMirror, id: string, live: () => boolean): Promise<boolean> {
    let q = this.questions.get(id);
    if (!q || q.settled) return false;
    if (!q.canonical) {
      if (q.status !== "pending") return false; // never mirrored: there is no record to close
      const keys: string[] = [];
      for (const [i, question] of q.questions.entries()) keys.push((await mirror.open(`${q.id}:${i}`, question.question, question.options)).key);
      if (!live()) return false;
      q = { ...this.questions.get(id)!, canonical: keys };
      this.save(q, false);
    }
    const keys = q.canonical!;
    if (q.status !== "pending") {
      // The pane question ended first (withdrawn, cancelled, expired or answered before its
      // records opened): the board must not keep asking.
      for (const key of keys) await mirror.withdraw(key);
      if (live()) this.save({ ...this.questions.get(id)!, settled: true }, false);
      return false;
    }
    const asks = await Promise.all(keys.map(key => mirror.get(key)));
    if (!live()) return false;
    if (answeredAll(asks)) { await this.settle(id, asks.map(ask => choiceAnswer(ask.answer!)), true); return false; }
    if (asks.every(ask => askOpen(ask) || ask.state === "answered")) return true; // still waiting on at least one
    await this.cancelMirrored(mirror, id, asks);
    return false;
  }

  /** Runs once the host is bound, and whenever the owner opens /questions: corral is reachable from here. */
  async recover(): Promise<void> {
    this.expire(); this.mirrorSoon();
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
    const epoch = this.epoch;
    let q = this.questions.get(id);
    if (this.closed || !q || q.status !== "answered" || this.consumed.has(id) || this.queued.has(id)) return;
    if (!q.message || q.recordedGeneration !== this.host.generation) {
      // Only selected UI answers reach here, never cancellation/status notifications.
      const text = await this.host.record(`${answerMarker(id)}\n${answerText(q)}`, id);
      if (this.closed || this.epoch !== epoch) return;
      // Re-read: a mirror pass may have saved this record's canonical keys during the await.
      q = { ...this.questions.get(id)!, message: text, recordedGeneration: this.host.generation };
      this.save(q);
    }
    if (this.closed || this.epoch !== epoch) return;
    this.queued.add(id);
    try {
      if (!await this.host.deliver(q.message!, `ask-user:${id}`)) this.queued.delete(id);
    } catch (error) { this.queued.delete(id); throw error; }
  }
  /** A temporary host exit suspends; a true session replacement closes all unresolved choices. */
  close(ended = false): void {
    if (ended) for (const q of this.questions.values()) {
      if (q.status === "pending" || (q.status === "answered" && !this.consumed.has(q.id))) this.save({ ...q, status: "ended" });
    }
    this.cancelPoll?.(); this.cancelPoll = undefined;
    this.closed = true; this.epoch++;
  }
}
