import { BridgeRefusal, type BusyPolicy, type SessionInput } from "../../bridge/protocol.ts";
import { utf8Prefix } from "../../roles/assistant/owner-action.ts";

export interface InputTag {
  readonly inputId?: string;
  readonly to?: string;
  readonly typedAt?: string;
  readonly device?: string;
}

const INPUT_ID = /^[A-Za-z0-9._:-]{1,80}$/;
const CHILD_ID = /^[A-Za-z0-9._:-]{1,128}$/;

export function isInputId(value: unknown): value is string {
  return typeof value === "string" && INPUT_ID.test(value);
}

export function parseChildId(value: unknown): string {
  if (typeof value !== "string" || !CHILD_ID.test(value) || value === "." || value === "..") {
    throw new BridgeRefusal("bad_input", "invalid child id");
  }
  return value;
}

export function parseInputTag(args: Record<string, unknown>): InputTag {
  const { input_id: inputId, to, typed_at: typedAt, device } = args;
  if (inputId !== undefined && !isInputId(inputId)) {
    throw new BridgeRefusal("bad_input", "invalid input_id");
  }
  const bounded = (value: unknown, field: string): string | undefined => {
    if (value === undefined) return undefined;
    if (typeof value !== "string" || Buffer.byteLength(value) > 64) {
      throw new BridgeRefusal("bad_input", `${field} must be a string of at most 64 bytes`);
    }
    return value;
  };
  return { ...(inputId === undefined ? {} : { inputId }), ...(to === undefined ? {} : { to: parseChildId(to) }),
    ...(typedAt === undefined ? {} : { typedAt: bounded(typedAt, "typed_at") }),
    ...(device === undefined ? {} : { device: bounded(device, "device") }) };
}

export type InputOrigin = "owner" | "auto" | "pane";
export interface TranscriptInput {
  readonly origin: InputOrigin;
  readonly typed_at?: string;
  readonly device?: string;
  readonly to?: string;
}

export function transcriptInput(input: SessionInput, origin: InputOrigin): TranscriptInput {
  return { origin, ...(input.typedAt === undefined ? {} : { typed_at: input.typedAt }),
    ...(input.device === undefined ? {} : { device: input.device }), ...(input.to === undefined ? {} : { to: input.to }) };
}

export type LiveInput = { readonly input: string } & (
  { readonly state: "accepted" | "withdrawn" } |
  { readonly state: "queued"; readonly policy: BusyPolicy; readonly position: number } |
  { readonly state: "started"; readonly turn: number } |
  { readonly state: "rejected"; readonly reason: string }
);

export function rejectInput(input: string, reason: string | undefined): LiveInput {
  return { input, state: "rejected", reason: utf8Prefix(reason ?? "input_rejected", 512) };
}
