// Worker-only corral tools: wait, progress, checkpoint, gate and done (OD3). The run they act on
// is the launcher-bound one; corral's Rust operation table stays the authority on what runs.
import { Type } from "typebox";

import type { CorralToolSpec } from "../types.ts";
import { pick } from "../common/tools.ts";

type Params = Record<string, unknown>;

export const waitTool: CorralToolSpec = {
  name: "corral_wait",
  label: "Wait for an external event",
  description: "Declare that this run waits on an external event (another task landing, CI, a person) with a "
    + "deadline. corral wakes you with a message when the wait is satisfied, fails or reaches its deadline; end "
    + "your turn after declaring it. Gate results need no wait: corral_gate arms its own wake.",
  op: "wait",
  parameters: Type.Object({
    id: Type.String({ description: "A wait ID unique within this run, [A-Za-z0-9._-]" }),
    condition: Type.String({ description: "What must become true" }),
    channel_id: Type.String({ description: "Where the event is observed, for example a task ID or CI run" }),
    deadline: Type.String({ description: "RFC 3339 time after which the wait expires and wakes you" }),
    owner: Type.Optional(Type.String({ description: "Who resolves the wait (default worker)" })),
  }),
  payload: (params: Params) => ({
    ...pick(params, ["id", "condition", "owner", "deadline"]),
    handle: { kind: "channel", ...pick(params, ["channel_id"]) },
  }),
};

export const progressTool: CorralToolSpec = {
  name: "corral_progress",
  label: "Write the run's progress ledger",
  description: "Write the full phase and step ledger as one ProgressUpdate v1 (the JSON `corral run progress "
    + "<task> --file` takes). Read the identity strings with `corral run progress <task> --json` first.",
  op: "progress",
  parameters: Type.Object({
    update: Type.Record(Type.String(), Type.Unknown(), { description: "The ProgressUpdate v1 object" }),
  }),
  payload: (params: Params) => params.update as Params,
};

export const checkpointTool: CorralToolSpec = {
  name: "corral_checkpoint",
  label: "Save a resume checkpoint",
  description: "Save one CheckpointUpdate v1 (the JSON `corral run checkpoint <task> --file` takes), built from "
    + "a fresh `corral run progress <task> --json` readback after you committed.",
  op: "checkpoint",
  parameters: Type.Object({
    checkpoint: Type.Record(Type.String(), Type.Unknown(), { description: "The CheckpointUpdate v1 object" }),
  }),
  payload: (params: Params) => params.checkpoint as Params,
};

export const gateTool: CorralToolSpec = {
  name: "corral_gate",
  label: "Start this run's gate",
  description: "Start the run's one receipted gate on the committed, rebased tree, exactly once per round. "
    + "started: a gate runs; end your turn and corral wakes you with its result and the gate_ref corral_done "
    + "carries. uncertain: corral could not confirm the start; end your turn and corral wakes you with the "
    + "result or gate_start_uncertain. queued: admission is pending; end your turn without sleeping or "
    + "retrying. corral starts the gate when capacity permits and wakes you with its result or not_started. "
    + "rejected: no gate started and no wake will come; read the code and "
    + "reason, fix the cause (for example commit or rebase) and call corral_gate again, or ask with corral_ask.",
  op: "gate",
  parameters: Type.Object({}),
  payload: () => ({}),
};

const GateRef = Type.Object({
  attempt: Type.String(),
  tier: Type.String(),
  receipt_sha256: Type.String(),
}, { description: "The gate_ref from the gate's wake message, exactly as corral show --json reports it" });

export const doneTool: CorralToolSpec = {
  name: "corral_done",
  label: "Finish the run",
  description: "Report the run complete and move the task to Review, exactly as `corral done` would. It refuses "
    + "while any of your questions is pending (wait for the answer or withdraw it) and when the gate_ref is "
    + "not the run's current gate receipt.",
  op: "done",
  parameters: Type.Object({
    summary: Type.String({ description: "This run's activity, then a `Verified:` line and a `Not verified:` line" }),
    result: Type.Optional(Type.String({ description: "The cumulative task outcome and remaining limitations" })),
    gate_ref: Type.Optional(GateRef),
    contract_sha256: Type.Optional(Type.String({ description: "The contract_sha256 of the gate section you followed" })),
  }),
  payload: (params: Params) => pick(params, ["summary", "result", "gate_ref", "contract_sha256"]),
};
