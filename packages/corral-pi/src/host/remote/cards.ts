import { BridgeRefusal, SESSION_PART_MAX } from "../../bridge/protocol.ts";
import { utf8Prefix } from "../../roles/assistant/owner-action.ts";
import { ANSWER_MAX_BYTES, QUESTION_TTL, validAnswer, type PendingQuestions } from "../pending-questions.ts";
import type { RemoteRegistry } from "./registry.ts";
import type { CardBrief } from "./state.ts";

const CARD_MAX_BYTES = 12 * 1024;
const BODY_MAX_BYTES = 8 * 1024;
export const CARD_OPTION_MAX = 64;

export type CardKind = "question" | "select" | "confirm" | "input" | "exit" | "child_question" | "board_plan";
export interface CardOption {
  id: string; label: string; detail?: string; unavailable?: string; needs_text?: string; destructive: boolean;
}
export interface CardQuestion {
  header?: string; text: string; options: CardOption[]; initial?: string; multi: boolean; free_text: boolean;
}
export interface SessionCard extends Omit<CardBrief, "kind"> {
  kind: CardKind; meta?: string; body?: string; body_more: boolean; questions: CardQuestion[]; tool?: string; opened_at: string;
}
export interface CardAnswer { options: string[]; text?: string }
export type AnswerStatus = "delivered" | "answered" | "started" | "dismissed" | "expired" | "missing";
export interface CardEntry {
  card: Omit<SessionCard, "body_more">;
  answer(answers: readonly CardAnswer[], dismiss: boolean, beforeQuestion: () => Promise<unknown>): AnswerStatus | Promise<AnswerStatus>;
}

const size = (value: unknown) => Buffer.byteLength(JSON.stringify(value));
const stamp = (at: number) => new Date(at).toISOString();
const key = (card: CardEntry["card"]) => `${card.opened_at}/${card.id}`;

// Labels remain IDs when short; indexed IDs preserve the exact choice even when its wire label is clipped.
export function optionId(label: string, index: number): string {
  return label && label.length <= 82 && !label.startsWith("@") && size(label) <= 84 ? label : `@${index + 1}`;
}
export function cardOptions(options: readonly { label: string; detail?: string }[]): CardOption[] {
  return options.slice(0, CARD_OPTION_MAX).map(({ label, detail }, index) => ({ id: optionId(label, index), label, detail, destructive: false }));
}
export function cardQuestion(text: string, options: CardOption[], free_text = false, initial?: string): CardQuestion {
  return { text, options, multi: false, free_text, initial };
}
export function invalidAnswer(reason: string): never { throw new BridgeRefusal("invalid_card_answer", reason); }
export function singleAnswer(answers: readonly CardAnswer[]): CardAnswer {
  const answer = answers[0];
  if (!answer || answers.length !== 1 || answer.options.length > 1) invalidAnswer("the card takes one answer");
  return answer;
}

export function questionEntries(pending: PendingQuestions | undefined): CardEntry[] {
  if (!pending) return [];
  return pending.pending().map<CardEntry>(record => ({
    card: { id: record.id, kind: "question", title: record.questions[0]?.question ?? "Owner question", tool: record.tool,
      opened_at: stamp(record.expires - QUESTION_TTL), expires_at: stamp(record.expires),
      questions: record.questions.map(q => cardQuestion(q.question, cardOptions(q.options.map(label => ({ label }))), q.freeText !== false)) },
    async answer(answers, dismiss, beforeQuestion) {
      if (dismiss) return "dismissed";
      if (answers.length !== record.questions.length) invalidAnswer("answer each question");
      const choices = answers.map(({ options, text }, i) => {
        const question = record.questions[i]!;
        const option = options.length === 1 ? question.options.find((label, index) => optionId(label, index) === options[0]) : undefined;
        const answer = { option, text };
        if (options.length > 1 || options.length === 1 && option === undefined || !validAnswer(question, answer)) {
          invalidAnswer("choose a listed option or use free text if allowed");
        }
        return answer;
      });
      return questionStatus(await pending.answer(record.id, choices, beforeQuestion));
    },
  }));
}
export function questionStatus(status: string): AnswerStatus {
  return status === "delivered" || status === "answered" || status === "expired" ? status : "missing";
}

/** Count escaped JSON as well as UTF-8, without breaking a code point. */
function clip(value: string | undefined, bytes: number, jsonBytes = bytes + 2): string | undefined {
  if (value === undefined) return;
  const prefix = utf8Prefix(value, bytes);
  if (size(prefix) <= jsonBytes) return prefix;
  let lo = 0, hi = bytes;
  while (lo < hi) {
    const mid = Math.ceil((lo + hi) / 2);
    if (size(utf8Prefix(prefix, mid)) <= jsonBytes) lo = mid;
    else hi = mid - 1;
  }
  return utf8Prefix(prefix, lo);
}

export function cardDetail(source: CardEntry["card"]): SessionCard {
  const { body, questions, ...base } = source;
  const card: SessionCard = { ...base, title: clip(base.title, 160)!, meta: clip(base.meta, 480), body_more: false,
    questions: questions.map(q => ({ ...q, header: clip(q.header, 160), text: clip(q.text, 1000)!, initial: clip(q.initial, 512),
      options: q.options.map(o => ({ ...o, label: clip(o.label, 300)!, detail: clip(o.detail, 300),
        unavailable: clip(o.unavailable, 512), needs_text: clip(o.needs_text, 512) })) })) };
  if (size(card) > CARD_MAX_BYTES) {
    // Large selectors fit by sharing space across labels. Never remove an available choice to fit a page.
    const count = card.questions.reduce((sum, q) => sum + q.options.length, 0);
    for (const q of card.questions) {
      q.text = clip(q.text, 384)!;
      q.initial = clip(q.initial, 160);
      for (const o of q.options) { o.label = ""; delete o.detail; }
    }
    const labelBytes = Math.min(300, Math.floor((CARD_MAX_BYTES - size(card)) / Math.max(1, count)));
    for (const [i, q] of card.questions.entries()) {
      for (const [j, o] of q.options.entries()) o.label = clip(source.questions[i]!.options[j]!.label, labelBytes)!;
    }
  }
  if (body !== undefined) {
    // Reserve the worst-case size of the body field plus the two JSON string delimiters.
    const remaining = CARD_MAX_BYTES - size(card) - 8;
    if (remaining >= 2) card.body = clip(body, BODY_MAX_BYTES, remaining);
    card.body_more = card.body !== body;
  }
  return card;
}

export function sortedCards(entries: CardEntry[]): CardEntry[] {
  return entries.sort(({ card: a }, { card: b }) => {
    if (a.opened_at !== b.opened_at) return a.opened_at < b.opened_at ? -1 : 1;
    return a.id < b.id ? -1 : a.id > b.id ? 1 : 0;
  });
}
export function cardBrief({ id, kind, title, child, expires_at }: CardEntry["card"]): CardBrief {
  return { id, kind, title: utf8Prefix(title, 160), child, expires_at };
}

function parseAnswer(args: Record<string, unknown>): { card: string; answers: CardAnswer[]; dismiss: boolean } {
  const { card, answers = [], dismiss = false } = args;
  if (typeof card !== "string" || !/^[A-Za-z0-9._:-]{1,160}$/.test(card) || typeof dismiss !== "boolean" ||
      !Array.isArray(answers) || answers.length > 4) throw new BridgeRefusal("bad_input", "invalid card answer");
  const parsed: CardAnswer[] = [];
  let textBytes = 0;
  for (const value of answers) {
    if (typeof value !== "object" || value === null || Array.isArray(value)) throw new BridgeRefusal("bad_input", "invalid card answer");
    const { options = [], text } = value as Record<string, unknown>;
    if (!Array.isArray(options) || options.length > CARD_OPTION_MAX || options.some(o => typeof o !== "string" || size(o) > 84) ||
        text !== undefined && (typeof text !== "string" || (textBytes += Buffer.byteLength(text)) > ANSWER_MAX_BYTES)) {
      throw new BridgeRefusal("bad_input", "invalid card answer");
    }
    parsed.push({ options, text });
  }
  return { card, answers: parsed, dismiss };
}

export function registerCards(registry: RemoteRegistry, source: {
  entries(): CardEntry[]; ready(): boolean; absent(id: string): AnswerStatus; beforeQuestion(): Promise<unknown>;
}): void {
  registry.register({ name: "cards", parse: ({ after }) => {
    if (after !== undefined && (typeof after !== "string" || Buffer.byteLength(after) > 128)) {
      throw new BridgeRefusal("bad_input", "invalid card page cursor");
    }
    return after;
  }, run: after => {
    const cards: SessionCard[] = [];
    let bytes = 256, next: string | undefined;
    for (const { card: raw } of sortedCards(source.entries())) {
      if (after !== undefined && key(raw) <= after) continue;
      const card = cardDetail(raw);
      bytes += size(card) + 1;
      if (bytes > SESSION_PART_MAX) return { cards, next };
      cards.push(card);
      next = key(raw);
    }
    return { cards };
  } });
  registry.register({ name: "card_answer", parse: parseAnswer, run: async ({ card, answers, dismiss }) => {
    if (!source.ready()) throw new BridgeRefusal("host_not_ready", "session is not accepting card answers");
    const entry = source.entries().find(entry => entry.card.id === card);
    const status = entry ? await entry.answer(answers, dismiss, source.beforeQuestion) : source.absent(card);
    return { status };
  } });
}
