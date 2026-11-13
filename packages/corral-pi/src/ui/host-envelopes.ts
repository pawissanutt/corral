// Inbox envelopes and corral's own prompts as the quiet rows host-notices.ts draws. Each parser matches only the exact
// shape corral writes (crates/corral-core/src/inbox/envelope.rs, the member bodies of corral-ops, the role kickoffs in
// src/roles/*/index.ts, src/host/request.ts and src/host/assistant-relink.ts); any other text returns undefined and
// stays the owner's bubble. Rows carry no framing line, hash, digest or id longer than 8 characters.
import { homedir } from "node:os";
import type { NoticeRow } from "./host-notices.ts";

// `[corral envelope <key>@env<n>]`, the framing line of escaped-v1 payloads (absent before it), a blank line, then the
// first member's own header.
const ENVELOPE = /^\[corral envelope (\S+)@env\d+\]\n(?:\[corral framing escaped-v1\]\n)?\n/;
// A member header: `[corral <key>#<seq> <kind>]`, or the line lifecycle.ts records for a context-only member.
const MEMBER = /^\[corral (\S+)#\d+ ([a-z0-9_]+)(: given to this host as context only; Pi's session keeps this line instead)?\](?:\n|$)/;

/** An attempt or commit id cut to 8 characters: `ag-50202f08f3828b63` reads `50202f08`. */
function short(id: string): string { return (id.replace(/^ag-/, "").split(/[-:]/)[0] || id).slice(0, 8); }
/** A path as the owner knows it: from `.worktrees/` on inside one, else with `~` for the home directory. */
function where(path: string): string {
  const at = path.indexOf("/.worktrees/");
  if (at >= 0) return path.slice(at + 1);
  const home = homedir();
  return home && (path === home || path.startsWith(`${home}/`)) ? `~${path.slice(home.length)}` : path;
}
/** `T-950_R2` reads `T-950 R2`, a reviewer's `T-815_R1_rv-R1-review1` reads `T-815 R1-review1`; other keys none. */
function recipient(key: string): string | undefined {
  return /^T-\d+_/.test(key) ? key.split("_").map(part => part.replace(/^rv-/, "")).join(" ") : undefined;
}
/** `pm` reads `the PM`, `owner` `the owner`, `assistant:A-48` `A-48`. */
function who(name: string): string {
  const lower = name.toLowerCase();
  return lower === "pm" ? "the PM" : lower === "owner" ? "the owner" : name.replace(/^assistant:/, "");
}
/** The lines of a body that carry content, unescaped from the envelope's framing. */
function lines(body: string): string[] {
  return body.split("\n").map(line => line.replace(/^\\\[corral /, "[corral ")).filter(line => line.trim() !== "");
}

const GATE = /^Gate attempt (\S+) finished: ([a-z_]+)\.$/;
const GATE_TIER = /^gate_ref: \{.*"tier":"([^"]+)"/m;
const NOT_STARTED = /^Queued gate attempt (\S+) did not start: (.*?)\. Resolve the cause and call corral_gate again\.$/;
const UNCONFIRMED = /^corral could not confirm that gate attempt (\S+) started\./;
const WAIT = /^Wait (\S+) (?:\((.*)\) )?reached ([a-z_]+)(?:: (.*)|\.)$/;
const WAIT_WORDS: Readonly<Record<string, string>> = {
  ready: "is ready", failed: "failed", expired: "expired", delivery_unknown: "reached an unknown delivery",
};

function waitWake(body: string, key: string): NoticeRow | undefined {
  const first = lines(body)[0] ?? "";
  const gate = GATE.exec(first);
  if (gate) {
    const tier = GATE_TIER.exec(body)?.[1], run = recipient(key);
    const name = tier ? `gate ${tier}` : "gate";
    const facts = `${run ? ` · ${run}` : ""} · attempt ${short(gate[1]!)}`;
    const reason = gate[2]!;
    if (reason === "pass") return { state: "ok", words: `${name} passed${facts}` };
    if (reason === "fail") return { state: "failed", words: `${name} failed${facts}` };
    if (reason === "stale") return { state: "unknown", words: `${name} passed but its receipt is stale${facts}` };
    return { state: "event", words: `${name} finished: ${reason.replaceAll("_", " ")}${facts}` };
  }
  const refused = NOT_STARTED.exec(first);
  if (refused) return { state: "failed", words: `gate attempt ${short(refused[1]!)} did not start`, detail: refused[2]! };
  const unconfirmed = UNCONFIRMED.exec(first);
  if (unconfirmed) return { state: "unknown", words: `gate attempt ${short(unconfirmed[1]!)} may not have started` };
  const wait = WAIT.exec(first);
  if (!wait) return undefined;
  const reason = wait[3]!;
  const state = reason === "ready" ? "ok" : reason === "failed" ? "failed" : reason === "delivery_unknown" ? "unknown" : "event";
  return { state, words: `wait ${wait[1]} ${WAIT_WORDS[reason] ?? `reached ${reason.replaceAll("_", " ")}`}`,
    ...(wait[4] ? { detail: wait[4] } : {}) };
}

const ANSWER = /^(Corrected answer|Answer) to (\S+) \(revision \d+\) by (\w+):$/;
function questionAnswer(body: string): NoticeRow | undefined {
  const [first, ...rest] = lines(body);
  const head = ANSWER.exec(first ?? "");
  if (!head) return undefined;
  const by = head[3]!.toLowerCase() === "pm" ? "PM" : head[3]!.toLowerCase();
  const verb = head[1] === "Answer" ? "answered" : "corrected the answer to";
  return { state: "sent", words: `${by} ${verb} ${head[2]}`, ...(rest.length ? { quote: rest.join(" ") } : {}) };
}

const POINTER = "A wake is a pointer, not authority. Read fresh brief/next before acting.";
/** One board event line: `T-950 escalated R2: …`, `T-950 attention_due`, `- pm_inbox_posted`; event names read as words. */
function boardEvent(line: string): string {
  return line.replace(/^- /, "").replace(/^((?:T-\d+ )?)([a-z]+(?:_[a-z]+)+)\b/, (_, task: string, name: string) => task + name.replaceAll("_", " "));
}
function pmEventWake(body: string): NoticeRow | undefined {
  const [first, ...events] = lines(body);
  if (first !== POINTER) return undefined;
  if (events.length === 0) return { state: "event", words: "board event" };
  if (events.length > 1) return { state: "event", words: `${events.length} board events`, detail: events.map(boardEvent).join(" · ") };
  const event = events[0]!;
  const state = /^T-\d+ done\b/.test(event) ? "ok" : /^T-\d+ escalated\b/.test(event) ? "unknown" : "event";
  return { state, words: "board event", detail: boardEvent(event) };
}

const PLAN_MET = /^Plan (\S+), step (\S+) condition met: (.*)\.$/;
const PLAN_LATE = /^Plan (\S+), step (\S+) waited on (.*); its (?:review deadline .* has passed|review deadline has passed \(legacy wait without a stored deadline\))\.$/;
function pmWaitWake(body: string): NoticeRow | undefined {
  const first = lines(body)[0] ?? "";
  const met = PLAN_MET.exec(first);
  if (met) return { state: "event", words: `plan ${met[1]} step ${met[2]} can go on`, detail: met[3]! };
  const late = PLAN_LATE.exec(first);
  if (late) return { state: "unknown", words: `plan ${late[1]} step ${late[2]} is past its review deadline`, detail: late[3]! };
  return undefined;
}

const QUESTION_WAKE = /^Question (\S+) \(revision \d+\) needs triage in project \S+\./;
const CHANGES = /^NEW CHANGES ROUND (\d+)(?: \(autopilot round \d+\))?: (\S+) run (\S+) was returned from review with changes requested;/;
const AUTOPILOT_CHANGES = /^corral autopilot: changes requested on (\S+) \(round (\d+) of (\d+)\): /;
const CHANGES_CAUSE = /: (the reviewer \([^)]*\) requested changes|the gate(?: \([^)]*\))? failed)/;
/** The changes-round notice of round_notice.rs as an envelope member (corral may append its reason); undefined for any other text. */
function changesRow(text: string): NoticeRow | undefined {
  const first = lines(text)[0] ?? "";
  const cause = CHANGES_CAUSE.exec(first)?.[1];
  const round = CHANGES.exec(first);
  if (round) {
    return { state: "failed", words: `changes requested on ${round[2]} ${round[3]} · round ${round[1]}`, ...(cause ? { detail: cause } : {}) };
  }
  const auto = AUTOPILOT_CHANGES.exec(first);
  if (auto) return { state: "failed", words: `changes requested on ${auto[1]} · round ${auto[2]} of ${auto[3]}`, ...(cause ? { detail: cause } : {}) };
  return undefined;
}

const INBOX_ITEM = /^PM inbox item (\S+) \(revision \d+\) from (\S+) \[([^\]]+)\]: (.*)$/;
const INBOX_OUTCOME = /^PM inbox outcome \S+: your item (\S+) \(".*"\) was resolved (\S+) by (\S+)\.$/;
const INBOX_CHANGED = /^PM inbox item (\S+) changed\. /;
function pmInboxOutcome(body: string): NoticeRow | undefined {
  const [first, ...rest] = lines(body);
  const outcome = INBOX_OUTCOME.exec(first ?? "");
  if (!outcome) {
    const changed = INBOX_CHANGED.exec(first ?? "");
    return changed ? { state: "event", words: `PM inbox ${changed[1]} changed` } : undefined;
  }
  const resolution = outcome[2]!;
  const state = resolution === "done" ? "ok" : resolution === "declined" ? "failed" : resolution === "escalated_to_owner" ? "owner" : "event";
  // The outcome's own text, without its `Refs:` line and the closing pointer to `corral pm inbox show`.
  const text = rest.filter(line => !/^Refs: /.test(line) && !/^Read it with `corral pm inbox show /.test(line));
  const resolved = resolution === "escalated_to_owner" ? "escalated to the owner" : resolution.replaceAll("_", " ");
  return { state, words: `${outcome[1]} ${resolved} by ${who(outcome[3]!)}`,
    ...(text.length ? { quote: text.join(" ") } : {}) };
}

const MOVEMENT = /^\[corral\] Main movement for (\S+?):(\S+): (.+?)\. Overlapping paths: (.+?)\. Source headroom/;
/** A main-movement notice (corral-ops movement/delivery.rs), in an envelope or delivered on its own. */
function movementRow(text: string): NoticeRow | undefined {
  const move = MOVEMENT.exec(lines(text)[0] ?? "");
  if (!move) return undefined;
  const tasks = move[3]!.split(", ").map(task => task.split("@")[0]!);
  const named = tasks.length > 3 ? `${tasks.slice(0, 2).join(", ")} and ${tasks.length - 2} more` : tasks.join(", ");
  const paths = move[4]!.split(", ").length;
  return { state: "event", words: `main moved: ${named} · ${paths} ${paths === 1 ? "path overlaps" : "paths overlap"} ${move[1]} ${move[2]}` };
}

const DIAGNOSIS = /^Gate diagnosis needed for (\S+) \(attempt (\S+)\)\. /;
const OWNER_NOTE = /^Owner note for (\S+) \(author: (\w+), at: [^)]*\)\. Read the durable comment /;
const RECOVERY = /^corral resumed this host as generation (\d+) after generation \d+ was lost\./;

function memberRow(kind: string, body: string, held: boolean, key: string): NoticeRow {
  const words = kind.replaceAll("_", " ");
  if (held) return { state: "unknown", words: `${words} · given to the agent as context only` };
  const content = lines(body), first = content[0] ?? "";
  const known = ((): NoticeRow | undefined => {
    switch (kind) {
      case "wait_wake": return waitWake(body, key);
      case "question_answer": case "question_correction": return questionAnswer(body);
      case "pm_event_wake": return pmEventWake(body);
      case "pm_wait_wake": return pmWaitWake(body);
      case "question_wake": {
        const question = QUESTION_WAKE.exec(first);
        return question ? { state: "event", words: `question ${question[1]} needs triage` } : undefined;
      }
      case "changes_requested": return changesRow(body);
      case "recovery_notice": case "recovery_update": {
        const recovery = RECOVERY.exec(first);
        const calls = content.filter(line => line.startsWith("- ")).length;
        return recovery ? { state: "unknown", words: `corral resumed this host as generation ${recovery[1]}`,
          ...(calls ? { detail: `${calls} ${calls === 1 ? "call" : "calls"} left without a result` } : {}) } : undefined;
      }
      case "pm_inbox_wake": {
        const item = INBOX_ITEM.exec(first);
        return item ? { state: "event", words: `PM inbox ${item[1]} from ${who(item[2]!)} · ${item[3]}`, detail: item[4]! } : undefined;
      }
      case "owner_reply": return content.length ? { state: "sent", words: "owner reply", quote: content.join(" ") } : undefined;
      case "main_movement": return movementRow(body);
      case "gate_diagnosis_needed": {
        const diagnosis = DIAGNOSIS.exec(first);
        return diagnosis ? { state: "failed", words: `gate on ${diagnosis[1]} needs a diagnosis · attempt ${short(diagnosis[2]!)}` } : undefined;
      }
      case "pm_inbox_outcome": return pmInboxOutcome(body);
      case "owner_note": {
        const note = OWNER_NOTE.exec(first);
        return note ? { state: "sent", words: `${note[2] === "pm" ? "PM" : note[2]} note on ${note[1]}`,
          ...(content.length > 1 ? { quote: content.slice(1).join(" ") } : {}) } : undefined;
      }
      case "turn_nudge": return first ? { state: "event", words: "reminder", detail: first } : undefined;
      default: return undefined;
    }
  })();
  // A kind without a template, or a body in a shape this file does not know: the kind and the first content line.
  const detail = first.replace(/^\[corral\] /, "");
  return known ?? { state: "event", words, ...(detail ? { detail } : {}) };
}

// A commit, digest or numeric id quoted in free text: 9 or more hex digits with at least one digit, as a whole word.
const HASH = /(?<![0-9A-Za-z])(?=[a-f]*[0-9])[0-9a-f]{9,}(?![0-9A-Za-z])/g;
/** The row with every long hash in its free text cut to its first 8 characters; the expanded text keeps them whole. */
function shortHashes(row: NoticeRow): NoticeRow {
  const cut = (text: string | undefined) => text?.replace(HASH, hash => hash.slice(0, 8));
  const detail = cut(row.detail), quote = cut(row.quote);
  return { ...row, ...(detail !== undefined ? { detail } : {}), ...(quote !== undefined ? { quote } : {}) };
}

/** One row per member of an inbox envelope, or undefined when `text` is not one. */
export function envelopeRows(text: string): NoticeRow[] | undefined {
  const head = ENVELOPE.exec(text);
  if (!head) return undefined;
  const key = head[1]!, open = `[corral ${key}#`, rest = text.slice(head[0].length);
  if (!rest.startsWith(open)) return undefined;
  // The member boundaries holdContextOnly (src/host/lifecycle.ts) uses; escaped-v1 escapes them inside bodies.
  const members = `\n\n${rest}`.split(`\n\n${open}`).slice(1).map(member => open + member);
  const rows: NoticeRow[] = [];
  for (const member of members) {
    const header = MEMBER.exec(member);
    if (!header || header[1] !== key) return undefined;
    rows.push(shortHashes(memberRow(header[2]!, member.slice(header[0].length), header[3] !== undefined, key)));
  }
  return rows;
}

/**
 * The regular expression source of a message template: its text verbatim, each `{}` one slot, filled in order from
 * `slots` and then with a token (`\S*`). Corral's prompts are matched as whole messages built from these.
 */
function template(text: string, slots: readonly string[] = []): string {
  return text.split("{}").map(part => part.replace(/[.*+?^${}()|[\]\\]/g, "\\$&"))
    .reduce((source, part, index) => `${source}${slots[index - 1] ?? "(\\S*)"}${part}`);
}
const LINE = "([^\\n]+?)";

// The kickoffs of src/roles/{worker,reviewer,planner,plan-reviewer}/index.ts, each the whole message; the reviewer's,
// planner's and plan reviewer's may be followed by their request marker line (`[corral review request [...]]`).
const marker = (kind: string) => `(?:\\n\\n\\[corral ${kind} request \\[[^\\n]*\\]\\])?`;
const WORKER = new RegExp(`^${template("Read {} in full and follow it: that file is your complete prompt for corral task {}, run {}. "
  + "Wherever it names a corral command that changes state (done, gate, run wait, run progress, run checkpoint, evidence record, "
  + "artifact add, a question), call the matching corral_* tool instead. Its Gate contract section is your gate: call corral_gate "
  + "where it names the gate command. When it answers started or uncertain, end your turn and wait for the result message; when "
  + "it answers rejected, no gate runs and no wake comes, so fix the reason it gives and call it again.")}$`);
const REVIEWER = new RegExp(`^${template("Read {} in full and follow it: that file is your complete review prompt for corral task {}, "
  + "run {} (review {}). Do not edit files. Where it tells you to finish with `corral verdict`, call corral_verdict with the same "
  + "verdict and your notes instead; corral already knows the attempt. Attach evidence with corral_artifact or corral_evidence and "
  + "ask with corral_ask rather than in the pane. Its Gate contract (review, authoritative) section is the tier you judge the "
  + "candidate against.", ["(\\S*)", "(\\S*)", "(\\S*)", "([^\\s)]*)"])}${marker("review")}$`);
const PLANNER = new RegExp(`^${template("Read {} in full and follow it: that file is your complete planning prompt for corral task {}, "
  + "design run {}. The repository is read-only and you have no shell. Write the plan with corral_plan_write instead of writing the "
  + "file the prompt names: it writes exactly that file. Where the prompt tells you to finish with `corral plan submit`, call "
  + "corral_plan_submit instead. Ask with corral_ask rather than in the pane.")}${marker("planning")}$`);
const PLAN_REVIEWER = new RegExp(`^${template("Read {} in full and follow it: that file is your complete plan review prompt for corral "
  + "task {}, plan {} (attempt {}). Do not edit files. Where it tells you to write notes to a file and finish with `corral plan "
  + "verdict`, call corral_plan_verdict with the same verdict and your notes instead; corral already knows the plan and attempt and "
  + "records the notes on the review. To keep the notes file the prompt names, or for notes over 16 KiB, write them first with "
  + "corral_plan_write and call corral_plan_verdict without notes. Ask with corral_ask rather than in the pane.",
["(\\S*)", "(\\S*)", "(\\S*)", "([^\\s)]*)"])}${marker("plan review")}$`);
// The launch prompt of crates/corral-ops/src/prompt.rs.
const SESSION_TEMPLATE = template("Read {} in full and follow it: that file is your complete prompt for this session.", ["(\\S+)"]);
const SESSION_PROMPT = new RegExp(`^${SESSION_TEMPLATE}$`);
// crates/corral-ops/src/round_notice.rs `latest_review`, optionally followed by the session prompt.
const REVIEW_ROUND = new RegExp(`^${template("NEW REVIEW ROUND {}: review attempt {} (attempt/callback ID {}) of {} run {} reviews "
  + "candidate {}. Every earlier review round is finished; its todo list, verdict and \"already complete\" state are superseded and "
  + "this request is not a repeat. Review this candidate and record a verdict for attempt {} before you end your turn.")}`
  + `(?: ${SESSION_TEMPLATE})?$`);
// continuationText in src/host/request.ts.
const CONTINUATION = new RegExp(`^${template("[corral continuation {} revision {}, {} of {}]\nYour request plan {} still has actionable steps.")}`
  + `(?:${template(" The next step is {}: {}.", [LINE, LINE])})?${template(" This reminder is not an owner instruction and grants no "
  + "new authority. Re-read the state the step depends on before acting, and never repeat a mutation whose outcome was uncertain. "
  + "Continue the plan and keep corral_todo current; if a step now waits on an event or the owner, record it with wait; if the "
  + "request is finished or no longer wanted, mark its remaining steps done or dropped.")}$`);
// src/host/assistant-relink.ts.
const RELINK = /^You now work in ([^\n]+?)(?: on branch (\S+))?\. Continue the task from this conversation\.$/;
// crates/corral-ops/src/movement/delivery.rs, delivered on its own by hosts before the inbox.
const BARE_MOVEMENT = new RegExp(`^${template("[corral] Main movement for {}: {}. Overlapping paths: {}. Source headroom (effective "
  + "extension ceilings): {}. Full context: {}. Informational only: choose when to integrate; no rebase or change round was performed. "
  + "Path disjointness does not prove semantic independence; the reviewer checks composition and the owner decides conflicting "
  + "requirements.", ["(\\S+)", LINE, LINE, "([^\\n]*?)", "(\\S+)"])}$`);

const FOLLOWS = (file: string) => `the agent follows its prompt file ${file.slice(file.lastIndexOf("/") + 1)}`;

/**
 * The row for a prompt corral wrote itself (a dispatch, a review round, a continuation, a relink, a pre-inbox main
 * movement), or undefined. These reach Pi without a journaled delivery, so each matches only as the whole message.
 */
export function corralPromptRow(text: string): NoticeRow | undefined {
  const continuation = CONTINUATION.exec(text);
  if (continuation && continuation[1] === continuation[5]) {
    return { state: "event", words: `auto-continue ${continuation[3]} of ${continuation[4]} · plan ${continuation[1]} still has open steps`,
      ...(continuation[6] ? { detail: `next ${continuation[6]}: ${continuation[7]}` } : {}) };
  }
  const worker = WORKER.exec(text);
  if (worker) return { state: "event", words: `corral · ${worker[2]} run ${worker[3]} · ${FOLLOWS(worker[1]!)}` };
  const review = REVIEWER.exec(text);
  if (review) return { state: "event", words: `corral · ${review[2]} run ${review[3]} review ${review[4]} · ${FOLLOWS(review[1]!)}` };
  const planning = PLANNER.exec(text);
  if (planning) return { state: "event", words: `corral · ${planning[2]} design run ${planning[3]} · ${FOLLOWS(planning[1]!)}` };
  const planReview = PLAN_REVIEWER.exec(text);
  if (planReview) {
    return { state: "event", words: `corral · ${planReview[2]} plan ${planReview[3]} review ${planReview[4]} · ${FOLLOWS(planReview[1]!)}` };
  }
  const session = SESSION_PROMPT.exec(text);
  if (session) return { state: "event", words: `corral · ${FOLLOWS(session[1]!)}` };
  const round = REVIEW_ROUND.exec(text);
  if (round) {
    const file = round[8];
    return { state: "event", words: `corral · review round ${round[1]} of ${round[4]} ${round[5]} · candidate ${short(round[6]!)}${file ? ` · ${FOLLOWS(file)}` : ""}` };
  }
  const relink = RELINK.exec(text);
  if (relink) return { state: "sent", words: `moved to ${where(relink[1]!)}${relink[2] ? ` on ${relink[2]}` : ""} · the agent continues` };
  return BARE_MOVEMENT.test(text) ? movementRow(text) : undefined;
}
