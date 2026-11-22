// Plan-mode prompts shared by the roles that plan with the owner (docs/plans/pi-plan-mode.md).
import type { PlanContext, PlanPrompts } from "../types.ts";
import { readAsset } from "./assets.ts";

export const PLAN_MODE_ASSET = "assets/plan-mode.md";
export const PLAN_APPROVED_ASSET = "assets/plan-approved.md";

const CONTEXT: Readonly<Record<PlanContext, string>> = {
  keep: "The planning conversation above is still in context.",
  compact: "The planning conversation was summarised.",
  clean: "The planning conversation was cleared at the owner's request; this message and the plan are your whole brief.",
};

/** Replace each `{{key}}` once, without reinterpreting text a value brings in. */
function fill(template: string, values: Readonly<Record<string, string>>): string {
  return template.replace(/\{\{([a-z]+)\}\}/g, (match, key: string) => values[key] ?? match);
}

export function planPrompts(root: string): PlanPrompts {
  return {
    contract: ({ path, existing, askUser }) => fill(readAsset(root, PLAN_MODE_ASSET), {
      path,
      existing: existing ? "It already exists: read it first. Revise it if this is the same work; replace it if the request is different."
        : "Create it with `write` and revise it with `edit`.",
      questions: askUser
        ? "ask the owner with `ask_user`, giving two to four distinct options with your recommendation first. An unanswered question is not approval: use your recommended default and record it under Assumptions."
        : "there is no question dialog in this session; choose a recommended default, record it under Assumptions and continue.",
    }).trimEnd(),
    approved: ({ path, title, revision, plan, context, note, todo }) => fill(readAsset(root, PLAN_APPROVED_ASSET), {
      path, title, revision: String(revision), plan: plan.trimEnd(), context: CONTEXT[context],
      note: note?.trim() ? `\nOwner's note: ${note.trim()}` : "",
      todo: todo ? "Start by opening a `corral_todo` plan with one step per plan step, and keep it current." : "",
    }).trimEnd(),
  };
}
