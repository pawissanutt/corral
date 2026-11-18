import type { BudgetOptions, BudgetReply } from "../capabilities/budget.ts";
import { ownerRun, type Run } from "./corral-cli.ts";
import type { BuildLaunch } from "./types.ts";

/** No local allowance. Every physical POST commits a reservation in Rust before dispatch. */
export function createBuildBudgetRecord(launch: BuildLaunch, run: Run = ownerRun(launch.corral ?? "")): BudgetOptions["record"] | undefined {
  const paid = launch.paid;
  if (!paid) return undefined;
  if (!launch.workspaceRoot || !launch.corral || (!launch.binding && !paid.mixed)) throw new Error("paid_admission_required: Build financial authority is missing");
  const root = launch.workspaceRoot;
  return async payload => {
    const result = await run(["--root", root, "build", "budget"], { cwd: root, timeoutMs: 30_000,
      input: JSON.stringify({ ...payload, version: 1, session: paid.session, generation: paid.generation, cap: paid.cap }) });
    // Do not surface subprocess text: it must never echo the inherited financial capability.
    if (result.code !== 0) throw new Error("budget_exhausted: Build financial authority refused the request");
    try {
      const reply = JSON.parse(result.stdout) as BudgetReply;
      if (!reply || typeof reply !== "object" || typeof reply.admitted !== "boolean") throw new Error("invalid reply");
      return reply;
    } catch { throw new Error("budget_accounting_refused: Build financial authority returned invalid JSON"); }
  };
}
