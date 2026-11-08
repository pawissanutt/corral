// Internal ctl_cap-only proof of C1's actual source drain. Not a model tool,
// owner decision, transfer consent, or another ownership/admission controller.
import { BridgeRefusal } from "./protocol.ts";
import type { HostIdentity } from "../roles/types.ts";
import type { PlanningState, WorkflowExpected } from "../host/planning-policy.ts";
import { sha256Hex } from "../tools/corral.ts";

interface SourceHost {
  readonly bound: boolean;
  sessionId(): string;
  drainPlanning(): Promise<PlanningState>;
}

function object(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}
const fail = (reason: string): never => { throw new BridgeRefusal("source_boundary_unavailable", reason); };
function expected(value: unknown): WorkflowExpected {
  if (!object(value) || Object.keys(value).some(k => !["workflow", "state_revision", "authority_epoch", "plan_revision"].includes(k))
      || typeof value.workflow !== "string" || !/^W[1-9][0-9]*$/.test(value.workflow)
      || !Number.isSafeInteger(value.state_revision) || Number(value.state_revision) < 1
      || !Number.isSafeInteger(value.authority_epoch) || Number(value.authority_epoch) < 1
      || !(value.plan_revision === null || Number.isSafeInteger(value.plan_revision) && Number(value.plan_revision) > 0)) {
    return fail("invalid current workflow/epoch binding");
  }
  return { workflow: value.workflow, state_revision: Number(value.state_revision), authority_epoch: Number(value.authority_epoch),
    plan_revision: value.plan_revision as number | null };
}
function same(a: WorkflowExpected | undefined, b: WorkflowExpected): boolean {
  return a?.workflow === b.workflow && a.state_revision === b.state_revision
    && a.authority_epoch === b.authority_epoch && a.plan_revision === b.plan_revision;
}

/** Core has already reserved/fenced the exact source. C1 refuses unverified shell
 * drain and refreshes canonical ownership; unknown/active ledgers cannot attest. */
export async function attestSourceDrain(host: SourceHost, identity: HostIdentity,
  process: { readonly pid: number; readonly start_time: string }, args: Record<string, unknown>): Promise<Record<string, unknown>> {
  if (Object.keys(args).some(k => !["version", "transfer", "expected"].includes(k)) || args.version !== 1
      || typeof args.transfer !== "string" || !/^[A-Za-z0-9._:/-]{1,512}$/.test(args.transfer)) return fail("unsupported source drain request");
  if (identity.role !== "assistant" || !identity.assistant || !host.bound) return fail("a bound source assistant is required");
  const pin = expected(args.expected), session = host.sessionId();
  if (!session || identity.generation < 1 || !Number.isSafeInteger(process.pid) || process.pid < 1
      || !process.start_time) return fail("source host identity is unavailable");
  // This existing C1 operation checks its generation-pinned verified drain support.
  // It never acknowledges an unknown process group from a cancelled Promise alone.
  const state = await host.drainPlanning();
  const boundary = state.boundary;
  if (!host.bound || host.sessionId() !== session || state.phase !== "handoff_preparing" || !same(state.expected, pin)
      || !boundary || boundary.active_count !== 0 || boundary.uncertain !== false || boundary.activities.length !== 0
      || boundary.identities_digest !== sha256Hex("")) return fail("source changed or has not actually drained");
  return { version: 1, transfer: args.transfer, assistant: identity.assistant, generation: identity.generation,
    session_id: session, pid: process.pid, start_time: process.start_time, expected: pin };
}
