// Plan-mode prompts shared by the roles that plan with the owner (docs/plans/pi-plan-mode.md).
import type { PlanContext, PlanPrompts } from "../types.ts";
import { readAsset } from "./assets.ts";

export const PLAN_MODE_ASSET = "assets/plan-mode.md";
export const PLAN_APPROVED_ASSET = "assets/plan-approved.md";

const CONTEXT: Readonly<Record<PlanContext, string>> = {
  keep: "The planning conversation above is still in context.",
  compact: "The planning conversation was summarised.",
  clean: "The planning conversation was cleared at the owner's request; the summary above holds the approved plan and is your whole brief.",
};

/** Replace each `{{key}}` once, without reinterpreting text a value brings in. */
function fill(template: string, values: Readonly<Record<string, string>>): string {
  return template.replace(/\{\{([a-z]+)\}\}/g, (match, key: string) => values[key] ?? match);
}

const block = (path: string, plan: string): string => `<plan path="${path}">\n${plan.trimEnd()}\n</plan>`;

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
    // Written before the kickoff is admitted, so it claims no approval: only the kickoff that follows does.
    reset: ({ path, title, revision, plan }) => "The earlier conversation was a planning discussion, cleared at the owner's "
      + `request. The plan below, "${title}" (revision ${revision}), is the one submitted for the owner's approval; a durable `
      + `copy is at \`${path}\`. Act on it only once a host message whose first line is \`[corral plan approved <id>]\` follows, `
      + `and then it is the whole brief.\n\n${block(path, plan)}`,
    // A clean context already holds the plan in its summary; sending it again would only double it.
    approved: ({ path, title, revision, plan, context, note, todo }) => fill(readAsset(root, PLAN_APPROVED_ASSET), {
      path, title, revision: String(revision), context: CONTEXT[context],
      where: context === "clean" ? "in the summary above" : "below",
      block: context === "clean" ? "" : `\n\n${block(path, plan)}`,
      note: note?.trim() ? `\nOwner's note: ${note.trim()}` : "",
      todo: todo ? "Start by opening a `corral_todo` plan with one step per plan step, and keep it current." : "",
    }).trimEnd(),
  };
}
