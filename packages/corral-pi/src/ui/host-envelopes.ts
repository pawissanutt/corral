// The worker's and reviewer's kickoffs as the quiet rows host-notices.ts draws. Each matches only the whole message
// the role writes (`kickoff` in src/roles/worker/index.ts and src/roles/reviewer/index.ts); any other text returns
// undefined and stays the owner's bubble. The plan-mode kickoff (the host's `[corral plan approved <id>]` line, then
// `assets/plan-approved.md`) is matched by its opening alone: the whole plan follows it, and belongs behind the row
// rather than in a bubble.
import type { NoticeRow } from "./host-notices.ts";

/**
 * The regular expression source of a message template: its text verbatim, each `{}` one slot, filled in order from
 * `slots` and then with a token (`\S*`).
 */
function template(text: string, slots: readonly string[] = []): string {
  return text.split("{}").map(part => part.replace(/[.*+?^${}()|[\]\\]/g, "\\$&"))
    .reduce((source, part, index) => `${source}${slots[index - 1] ?? "(\\S*)"}${part}`);
}

const WORKER = new RegExp(`^${template("Read {} in full and follow it: that file is your complete prompt for corral task {}, run {}. "
  + "Use corral_done to report completion and corral_artifact to attach files, not their shell equivalents. Run any requested "
  + "engine gate with `\"$CORRAL\" gate {}` in bash. Use ask_user for owner questions.")}$`);
// The reviewer's kickoff may be followed by its request marker line (`[corral review request [...]]`).
const REVIEWER = new RegExp(`^${template("Read {} in full and follow it: that file is your complete review prompt for corral task {}, "
  + "run {}. Do not edit the worktree. Record one verdict for this request with corral_verdict (approve or changes, with "
  + "notes), instead of writing a verdict notes file and running the shell command. Use corral_artifact for evidence files and "
  + "ask_user for owner questions.")}(?:\\n\\n\\[corral review request \\[[^\\n]*\\]\\])?$`);

const PLAN_APPROVED = /^\[corral plan approved [^\]\n]*\]\nThe owner approved the plan "([^\n]*)" \(revision (\d+)\)\. Plan mode is off /;

const FOLLOWS = (file: string) => `the agent follows its prompt file ${file.slice(file.lastIndexOf("/") + 1)}`;

/** The row for a kickoff a role wrote itself, or undefined. */
export function corralPromptRow(text: string): NoticeRow | undefined {
  const worker = WORKER.exec(text);
  if (worker) return { state: "event", words: `corral · ${worker[2]} run ${worker[3]} · ${FOLLOWS(worker[1]!)}` };
  const review = REVIEWER.exec(text);
  if (review) return { state: "event", words: `corral · ${review[2]} run ${review[3]} review · ${FOLLOWS(review[1]!)}` };
  const plan = PLAN_APPROVED.exec(text);
  // The row names the message, not the session's state: only the host's own tagged kickoff approves a plan.
  if (plan) return { state: "event", words: `corral · plan kickoff · ${plan[1]} · revision ${plan[2]}` };
  return undefined;
}
