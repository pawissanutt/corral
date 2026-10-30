// Bridge v1 wire format (corral to host): one JSON request per LF-terminated line, at most
// 64 KiB, each carrying ctl_cap and the generation; one reply line per request.
import type { BusyPolicy } from "../delivery/types.ts";

export const BRIDGE_VERSION = 1;
export const FRAME_MAX_BYTES = 64 * 1024;
export const INPUT_MAX_BYTES = 8 * 1024;
export const MAX_IN_FLIGHT = 4;
/** Distinct input keys one generation admits; it refuses new keys beyond, never forgets old ones. */
export const SUBMISSIONS_MAX = 4096;
/** How long a submit_input reply waits for upstream's preflight before answering `pending`. */
export const INPUT_REPLY_MS = 30_000;
/** Room left in a frame for the reply envelope around a page of events. */
export const EVENT_PAGE_BYTES = FRAME_MAX_BYTES - 4 * 1024;

export type Command = "hello" | "get_state" | "submit_input" | "cancel" | "events_since" | "shutdown";
export const COMMANDS: readonly Command[] = ["hello", "get_state", "submit_input", "cancel", "events_since", "shutdown"];

/** `pending`: admitted, upstream has not answered yet; the outcome follows as `input_settled`. */
export type Disposition = "started" | "queued" | "handled" | "rejected" | "pending";

export class BridgeRefusal extends Error {
  constructor(readonly code: string, readonly reason: string) {
    super(`${code}: ${reason}`);
  }
}

export interface RequestFrame {
  readonly id: string;
  readonly ctlCap: string;
  readonly generation: number;
  readonly command: Command;
  readonly args: Record<string, unknown>;
}

export interface SubmitInput {
  readonly text: string;
  readonly busyPolicy: BusyPolicy;
  readonly idempotencyKey: string;
  readonly origin: string;
}

export interface SubmitResult {
  readonly disposition: Disposition;
  readonly turn?: number;
  readonly reason?: string;
}

const ID = /^[A-Za-z0-9._:-]{1,80}$/;
const KEY = /^[A-Za-z0-9._:-]{1,128}$/;
const ORIGIN = /^[A-Za-z0-9._:-]{1,64}$/;

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
  if (typeof value.ctl_cap !== "string") throw new BridgeRefusal("auth_failed", "ctl_cap is missing");
  if (typeof value.generation !== "number" || !Number.isSafeInteger(value.generation)) {
    throw new BridgeRefusal("bad_frame", "generation must be an integer");
  }
  if (typeof value.command !== "string" || !COMMANDS.includes(value.command as Command)) {
    throw new BridgeRefusal("unknown_command", "command is not a bridge v1 command");
  }
  const args = value.args ?? {};
  if (!record(args)) throw new BridgeRefusal("bad_frame", "args must be an object");
  return { id, ctlCap: value.ctl_cap, generation: value.generation, command: value.command as Command, args };
}

export function parseSubmitInput(args: Record<string, unknown>): SubmitInput {
  const { text, busy_policy: busyPolicy, idempotency_key: key, origin } = args;
  if (typeof text !== "string" || text.length === 0 || Buffer.byteLength(text) > INPUT_MAX_BYTES) {
    throw new BridgeRefusal("bad_input", `text must be 1-${INPUT_MAX_BYTES} bytes`);
  }
  if (busyPolicy !== "steer" && busyPolicy !== "follow_up" && busyPolicy !== "prompt") {
    throw new BridgeRefusal("bad_input", "busy_policy must be steer, follow_up or prompt");
  }
  if (typeof key !== "string" || !KEY.test(key)) {
    throw new BridgeRefusal("bad_input", "idempotency_key must be 1-128 characters of [A-Za-z0-9._:-]");
  }
  if (typeof origin !== "string" || !ORIGIN.test(origin)) {
    throw new BridgeRefusal("bad_input", "origin must be 1-64 characters of [A-Za-z0-9._:-]");
  }
  return { text, busyPolicy, idempotencyKey: key, origin };
}

export function parseTurn(args: Record<string, unknown>): number | undefined {
  if (args.turn === undefined || args.turn === null) return undefined;
  if (typeof args.turn !== "number" || !Number.isSafeInteger(args.turn) || args.turn < 1) {
    throw new BridgeRefusal("bad_input", "turn must be a positive integer");
  }
  return args.turn;
}

export function parseSeq(args: Record<string, unknown>): number {
  if (typeof args.seq !== "number" || !Number.isSafeInteger(args.seq) || args.seq < 0) {
    throw new BridgeRefusal("bad_input", "seq must be a non-negative integer");
  }
  return args.seq;
}
