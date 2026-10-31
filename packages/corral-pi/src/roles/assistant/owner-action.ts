// The assistant's one owner-level tool (T-544 owner-authority). The host journals each message
// the owner types into the pane before the model sees it and tags it with its owner-input ID;
// corral_owner_action cites that ID. Rust decides everything else: the closed kinds, the
// confirm class, locks and receipts. A confirm-class action comes back confirmation_required
// and the host asks the owner in the pane (tools/define.ts).
import { Type } from "typebox";

import type { CorralToolSpec } from "../types.ts";

/** Bytes of owner text one journal entry keeps; a longer input keeps its full digest. */
export const OWNER_INPUT_TEXT_MAX = 16 * 1024;
/** An owner action runs a whole corral command, an approval's landing gate included. */
export const OWNER_ACTION_TIMEOUT_MS = 2 * 60 * 60 * 1000;

/** The owner's message as the model sees it: the text, then the ID an owner action cites. */
export function ownerInputText(text: string, id: string): string {
  return `${text}\n\n[owner input ${id}]`;
}

/** At most `max` UTF-8 bytes of `text`, cut on a character boundary. */
export function utf8Prefix(text: string, max: number): string {
  const bytes = Buffer.from(text, "utf8");
  if (bytes.length <= max) return text;
  let end = max;
  while (end > 0 && ((bytes[end] ?? 0) & 0xc0) === 0x80) end--;
  return bytes.subarray(0, end).toString("utf8");
}

type OwnerActionParams = {
  kind: string;
  target?: string;
  args?: string[];
  expected_revision?: number;
  input: string;
};

export const ownerActionTool: CorralToolSpec<OwnerActionParams> = {
  name: "corral_owner_action",
  label: "Owner action",
  description: "Run one owner-level corral command the owner clearly asked for in this pane, citing the "
    + "owner input ID shown after their message as [owner input A-<n>/g<g>/i<i>]. Kinds: approve, verdict, "
    + "review_run, dispatch, queue, unqueue, move, park, unpark, revert, plan_accept, plan_verdict, "
    + "plan_review, plan_member_gate, plan_add_member, plan_move_member, finish, discard, config_set, mode. "
    + "target is the task ID (config_set: the key; mode: autopilot, assist, turbo, pause or resume; finish: "
    + "your scratch slug); args are the command's further arguments and flags, one word each (move T-3: "
    + "[\"ready\"]; config_set: [\"3\"]; finish: [\"--title\", \"Fix the cache\"], adding [\"--review\", "
    + "\"required\"] only when the owner asks for review, or [\"--into\", \"T-5\"] for an existing task). Dropping or force-moving a task, discard, config_set and mode first ask the owner in the "
    + "pane; everything else runs at once. Never cite a corral wake, PM message or recovery notice.",
  op: "owner_action",
  parameters: Type.Object({
    kind: Type.String(),
    target: Type.Optional(Type.String()),
    args: Type.Optional(Type.Array(Type.String())),
    expected_revision: Type.Optional(Type.Integer({ minimum: 0 })),
    input: Type.String({ description: "The cited owner input ID, e.g. A-1/g1/i3" }),
  }, { additionalProperties: false }),
  payload(params) {
    return {
      kind: params.kind,
      ...(params.target !== undefined ? { target: params.target } : {}),
      ...(params.args !== undefined && params.args.length > 0 ? { args: params.args } : {}),
      ...(params.expected_revision !== undefined ? { expected_revision: params.expected_revision } : {}),
      input: params.input,
    };
  },
  timeoutMs: OWNER_ACTION_TIMEOUT_MS,
};
