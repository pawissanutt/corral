// Session socket v2: one JSON request and reply per line, with the Unix socket as the boundary.
import { parseOwnerAttachments, type OwnerAttachment } from "../host/owner-attachments.ts";
export type BusyPolicy = "prompt" | "steer" | "follow_up";

export const BRIDGE_VERSION = 2;
export const FRAME_MAX_BYTES = 64 * 1024;
export const INPUT_MAX_BYTES = FRAME_MAX_BYTES;
export const MAX_IN_FLIGHT = 4;
/** How long a submit_input reply waits for upstream's preflight before answering `pending`. */
export const INPUT_REPLY_MS = 30_000;
/** Room left in a frame for the reply envelope around a page of events. */
export const EVENT_PAGE_BYTES = FRAME_MAX_BYTES - 4 * 1024;

export const COMMANDS = [
  "hello", "get_state", "submit_input", "cancel", "events_since", "live_since", "shutdown", "queue_state", "queue_withdraw",
  "set_model", "answer", "pause", "resume", "rewind",
] as const;
export type Command = typeof COMMANDS[number];

/** `pending`: admitted, upstream has not answered yet; the outcome follows as `input_settled`. */
export type Disposition = "started" | "queued" | "handled" | "rejected" | "pending";

export class BridgeRefusal extends Error {
  constructor(readonly code: string, readonly reason: string) {
    super(`${code}: ${reason}`);
  }
}

export interface RequestFrame {
  readonly id: string;
  readonly command: Command;
  readonly args: Record<string, unknown>;
}

/** Session socket input, also used for automated input saved while paused. */
export interface SessionInput {
  readonly text: string;
  readonly busyPolicy: BusyPolicy;
  readonly origin: "owner" | "auto";
  readonly attachments?: OwnerAttachment[];
}

export interface SubmitResult {
  readonly disposition: Disposition;
  readonly turn?: number;
  readonly reason?: string;
}

const ID = /^[A-Za-z0-9._:-]{1,80}$/;
const KEY = /^[A-Za-z0-9._:-]{1,128}$/;

function record(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}

/** The request ID when one can be read, so even a refusal names its request. */
export function frameId(value: unknown): string | null {
  return record(value) && typeof value.id === "string" && ID.test(value.id) ? value.id : null;
}

export function parseFrame(value: unknown): RequestFrame {
  if (!record(value)) throw new BridgeRefusal("bad_frame", "a request is one JSON object");
  if (value.version !== BRIDGE_VERSION) throw new BridgeRefusal("bad_frame", `version must be ${BRIDGE_VERSION}`);
  const id = frameId(value);
  if (id === null) throw new BridgeRefusal("bad_frame", "id must be 1-80 characters of [A-Za-z0-9._:-]");
  if (typeof value.command !== "string" || !COMMANDS.includes(value.command as Command)) {
    throw new BridgeRefusal("unknown_command", "command is not a session socket command");
  }
  const args = value.args ?? {};
  if (!record(args)) throw new BridgeRefusal("bad_frame", "args must be an object");
  return { id, command: value.command as Command, args };
}

export function parseSubmitInput(args: Record<string, unknown>): SessionInput {
  const { text, busy_policy: busyPolicy = "steer", origin, attachments } = args;
  if (typeof text !== "string" || !text.trim() || Buffer.byteLength(text) > INPUT_MAX_BYTES) {
    throw new BridgeRefusal("bad_input", `text must be nonblank and fit in the ${FRAME_MAX_BYTES}-byte frame`);
  }
  if (busyPolicy !== "steer" && busyPolicy !== "follow_up" && busyPolicy !== "prompt") {
    throw new BridgeRefusal("bad_input", "busy_policy must be steer, follow_up or prompt");
  }
  if (origin !== "owner" && origin !== "auto") throw new BridgeRefusal("bad_input", "origin must be owner or auto");
  return { text, busyPolicy, origin, ...(attachments !== undefined ? { attachments: parseOwnerAttachments(attachments) } : {}) };
}

export function parseQueueLimit(args: Record<string, unknown>): number {
  const limit = args.limit === undefined ? 64 : args.limit;
  if (typeof limit !== "number" || !Number.isSafeInteger(limit) || limit < 1 || limit > 64) {
    throw new BridgeRefusal("bad_input", "limit must be an integer in 1..64");
  }
  return limit;
}

export function parseQueueWithdraw(args: Record<string, unknown>): string {
  if (typeof args.key !== "string" || !KEY.test(args.key)) {
    throw new BridgeRefusal("bad_input", "key must be 1-128 characters of [A-Za-z0-9._:-]");
  }
  return args.key;
}

export function parseTurn(args: Record<string, unknown>): number | undefined {
  if (args.turn === undefined || args.turn === null) return undefined;
  if (typeof args.turn !== "number" || !Number.isSafeInteger(args.turn) || args.turn < 1) {
    throw new BridgeRefusal("bad_input", "turn must be a positive integer");
  }
  return args.turn;
}

export type RewindInput = { readonly turn: number } | { readonly list: true };

export function parseRewind(args: Record<string, unknown>): RewindInput {
  if (args.list === true && args.turn === undefined) return { list: true };
  const turn = parseTurn(args);
  if (turn !== undefined && args.list === undefined) return { turn };
  throw new BridgeRefusal("bad_input", "specify a turn or list: true");
}

export interface AnswerInput {
  readonly question: string;
  readonly option?: string;
  readonly text?: string;
}

export function parseAnswer(args: Record<string, unknown>): AnswerInput {
  const { question, option, text } = args;
  if (typeof question !== "string" || !/^[A-Za-z0-9._:-]{1,160}$/.test(question)
      || (option !== undefined && (typeof option !== "string" || !option.trim()))
      || (text !== undefined && (typeof text !== "string" || !text.trim()))
      || (option === undefined && text === undefined)) {
    throw new BridgeRefusal("bad_input", "answer needs a question and an option or text");
  }
  return { question, option, text };
}

export function parseSeq(args: Record<string, unknown>): number {
  if (typeof args.seq !== "number" || !Number.isSafeInteger(args.seq) || args.seq < 0) {
    throw new BridgeRefusal("bad_input", "seq must be a non-negative integer");
  }
  return args.seq;
}
