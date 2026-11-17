// The pane PM's ask_user questions as canonical owner asks (`owner_ask`), so the owner can read
// and answer them in the app Inbox as well as in the pane. Corral's record decides: a pane
// answer is a CAS on the revision the app answers too, and only the canonical winner reaches
// the model. Reads are unjournaled polls; open, answer and withdraw leave receipts.
import { randomBytes } from "node:crypto";
import { canonicalJson, type CorralClient, type CorralReceipt, toolRequestId } from "../tools/corral.ts";
import type { DialogAnswer, DialogQuestion } from "./ask-user-dialog.ts";

export interface CanonicalAnswer { readonly text: string; readonly option?: string }
export interface CanonicalAsk {
  readonly key: string;
  readonly revision: number;
  readonly state: string;
  readonly answer?: CanonicalAnswer;
  /** This host's own answer committed, rather than one the app recorded first. */
  readonly mine?: boolean;
}
/** What a question store needs from corral to mirror its questions. */
export type QuestionMirror = Pick<OwnerAskMirror, "open" | "get" | "answer" | "withdraw">;

export const askOpen = (ask: CanonicalAsk): boolean => ask.state === "open" || ask.state === "escalated";
const DIALOG_POLL_MS = 1000;

const object = (value: unknown): value is Record<string, unknown> => typeof value === "object" && value !== null && !Array.isArray(value);

function parse(result: unknown): CanonicalAsk {
  if (!object(result) || typeof result.key !== "string" || typeof result.revision !== "number" || typeof result.state !== "string") {
    throw new Error(`owner_ask_invalid_result: ${canonicalJson(result)}`);
  }
  const answer = object(result.question) && object(result.question.answer) ? result.question.answer : undefined;
  return { key: result.key, revision: result.revision, state: result.state,
    ...(answer && typeof answer.text === "string"
      ? { answer: { text: answer.text, ...(typeof answer.option === "string" ? { option: answer.option } : {}) } } : {}) };
}

export class OwnerAskMirror {
  constructor(private readonly client: Pick<CorralClient, "call">, private readonly sessionId: () => string,
    private readonly turn: () => number | undefined) {}

  private send(payload: Record<string, unknown>, id: string): Promise<CorralReceipt> {
    return this.client.call("owner_ask", { requestId: toolRequestId(this.sessionId(), id), payload, turn: this.turn() });
  }
  private async committed(payload: Record<string, unknown>, id: string): Promise<CanonicalAsk> {
    const receipt = await this.send(payload, id);
    if (receipt.state !== "committed") throw new Error(`owner_ask_refused: ${canonicalJson(receipt.result)}`);
    return parse(receipt.result);
  }
  /** A fresh nonce per attempt: the revision CAS, not a replayed receipt, makes a retry safe. */
  private async settle(action: "answer" | "withdraw", current: CanonicalAsk, fields: Record<string, unknown>): Promise<CanonicalAsk | undefined> {
    const receipt = await this.send({ action, key: current.key, revision: current.revision, ...fields },
      `owner-ask:${action}:${current.key}:${randomBytes(8).toString("hex")}`);
    if (receipt.state === "committed") return parse(receipt.result);
    // The record settled between this host's read and its write: the canonical state now decides.
    const code = receipt.state === "rejected" && object(receipt.result) ? receipt.result.code : undefined;
    if (code === "stale_revision" || code === "question_state") return undefined;
    throw new Error(`owner_ask_refused: ${canonicalJson(receipt.result)}`);
  }

  /** `id` is stable per question, so reopening after a crash replays the same record. */
  open(id: string, text: string, options: readonly string[]): Promise<CanonicalAsk> {
    return this.committed({ action: "open", text, options, kind: "clarification", blocking: false }, `owner-ask:open:${id}`);
  }
  get(key: string): Promise<CanonicalAsk> {
    return this.committed({ action: "get", key }, `owner-ask:get:${key}:${randomBytes(8).toString("hex")}`);
  }
  /** Answers an open record; a record the app settled first is returned as it stands. */
  async answer(key: string, answer: CanonicalAnswer): Promise<CanonicalAsk> {
    const current = await this.get(key);
    if (!askOpen(current)) return current;
    const done = await this.settle("answer", current, { text: answer.text, ...(answer.option === undefined ? {} : { option: answer.option }) });
    return done ? { ...done, answer, mine: true } : this.get(key);
  }
  /** Withdraws an open record; a settled one is returned as it stands. */
  async withdraw(key: string): Promise<CanonicalAsk> {
    const current = await this.get(key);
    if (!askOpen(current)) return current;
    return await this.settle("withdraw", current, {}) ?? this.get(key);
  }
}

/** The question as the app shows it: the canonical record holds labels, so descriptions join the text. */
function askText(q: DialogQuestion): string {
  return [q.question, ...(q.multiSelect === true ? ["Choose all applicable options; record the selections in your answer."] : []),
    ...q.options.flatMap(option => option.description ? [`${option.label}: ${option.description}`] : [])].join("\n");
}

function paused(signal: AbortSignal, ms: number): Promise<void> {
  if (signal.aborted) return Promise.resolve();
  const { promise, resolve } = Promise.withResolvers<void>();
  const done = () => { clearTimeout(timer); signal.removeEventListener("abort", done); resolve(); };
  const timer = setTimeout(done, ms);
  signal.addEventListener("abort", done, { once: true });
  return promise;
}

/**
 * Runs a pane dialog while the same questions are open in the owner's Inbox. The first answer
 * wins: an app answer closes the dialog, a pane answer is recorded on the canonical record, and
 * a question nobody answered is withdrawn when the dialog closes. Without corral the dialog is
 * pane-only, as before.
 */
export async function mirroredDialog(mirror: QuestionMirror, id: string, questions: readonly DialogQuestion[],
  signal: AbortSignal | undefined, show: (signal?: AbortSignal) => Promise<(DialogAnswer | undefined)[]>,
  pollMs = DIALOG_POLL_MS): Promise<(DialogAnswer | undefined)[]> {
  const keys: string[] = [];
  try {
    for (const [i, q] of questions.entries()) keys.push((await mirror.open(`${id}:${i}`, askText(q), q.options.map(option => option.label))).key);
  } catch {
    await Promise.allSettled(keys.map(key => mirror.withdraw(key)));
    return show(signal);
  }
  const answered = new AbortController(), stop = new AbortController();
  const watch = (async () => {
    for (;;) {
      await paused(stop.signal, pollMs);
      if (stop.signal.aborted) return;
      try {
        if (!(await Promise.all(keys.map(key => mirror.get(key)))).some(askOpen)) { answered.abort(); return; }
      } catch { /* the next poll retries */ }
    }
  })();
  let raw: (DialogAnswer | undefined)[];
  try {
    raw = await show(signal ? AbortSignal.any([signal, answered.signal]) : answered.signal);
  } catch (error) {
    await Promise.allSettled(keys.map(key => mirror.withdraw(key))); // a failed dialog must not leave the board asking
    throw error;
  } finally {
    stop.abort();
    await watch;
  }
  const result: (DialogAnswer | undefined)[] = [];
  for (const [i, key] of keys.entries()) {
    const pane = raw[i] && (raw[i].selected.length > 0 || raw[i].text.trim()) ? raw[i] : undefined;
    const choices = pane?.selected.join(", ") ?? "", note = pane?.text.trim() ?? "";
    try {
      // One choice is the record's option; several are spelled out in its text, ahead of any note.
      const ask = pane
        ? await mirror.answer(key, pane.selected.length === 1 ? { text: note || choices, option: choices }
          : { text: [choices, note].filter(Boolean).join("\n") })
        : await mirror.withdraw(key);
      result.push(ask.state !== "answered" || !ask.answer ? undefined : ask.mine ? pane
        : { selected: ask.answer.option === undefined ? [] : [ask.answer.option], text: ask.answer.text === ask.answer.option ? "" : ask.answer.text });
    } catch {
      result.push(pane); // corral is unreachable: the owner's pane answer still stands
    }
  }
  return result;
}
