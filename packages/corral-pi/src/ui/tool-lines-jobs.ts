// Rows for commands: `bash` (managed and Pi's own), `job_start`, `job_wait`, `job_output`, `job_kill` and
// `corral_job_monitor`. A command reads as its first line with the session's own `cd` dropped; a success is one row
// with its exit code; a failure adds its last 3 output lines. A command that outlived its call (backgrounded) keeps
// its row live from the job's state until the job exits. Job tools name the command, not only the id.
import {
  formatElapsed, baseState, firstLine, lineCount, numberArg, outputLines, recordOf, resultText, shortPath, stringArg, tailLines, textLines,
} from "./tool-row.ts";
import type { BodyLine, CallInput, Presenter, RowModel, RowView } from "./tool-row.ts";
import type { JobFacts, PresenterModule, ToolRenderDeps } from "./tool-rendering.ts";

/** The command as one line: a leading `cd <session cwd> &&` dropped (other `cd` paths shortened), a heredoc summarised. */
export function commandLine(command: string, cwd: string): string {
  let text = command.replace(/^\s+/, "");
  const cd = /^cd\s+(["']?)([^"'\s;&]+)\1\s*(?:&&|;)\s*/.exec(text);
  if (cd) {
    const where = shortPath(cd[2]!, cwd), rest = text.slice(cd[0].length);
    text = where === "." ? rest : `cd ${where} && ${rest}`;
  }
  const lines = text.split("\n"), first = (lines[0] ?? "").trimEnd();
  const heredoc = /<<-?\s*(['"]?)([A-Za-z_]\w*)\1/.exec(first);
  if (heredoc) {
    const end = lines.findIndex((line, index) => index > 0 && line.trim() === heredoc[2]);
    const body = end > 0 ? end - 1 : lines.length - 1;
    const after = end > 0 ? lines.slice(end + 1).filter(line => line.trim() !== "").length : 0;
    return `${first} … ${heredoc[2]} (${body}-line heredoc${after ? `, +${after} more lines` : ""})`;
  }
  const more = lines.slice(1).filter(line => line.trim() !== "").length;
  return more ? `${first} (+${more} lines)` : first;
}

/** How a command ended, in words; `ok` only for exit 0 without a signal. */
interface Exit { readonly ok: boolean; readonly words: string }
function exitOf(exitCode: unknown, signal: unknown, termination?: unknown): Exit {
  if (termination === "deadline") return { ok: false, words: "timed out" };
  if (termination === "cancelled") return { ok: false, words: typeof signal === "string" ? `stopped (${signal})` : "stopped" };
  if (termination === "host_stopped") return { ok: false, words: "stopped with the host" };
  if (typeof signal === "string") return { ok: false, words: `killed by ${signal}` };
  if (typeof exitCode === "number") return { ok: exitCode === 0, words: `exit ${exitCode}` };
  return { ok: true, words: "exited" };
}

/** Pi's own bash appends its status to the output; split it off. */
function sdkOutput(text: string): { readonly output: string; readonly exit?: Exit } {
  const lines = text.replace(/\s+$/u, "").split("\n");
  let exit: Exit | undefined;
  while (lines.length) {
    const last = lines[lines.length - 1]!.trim();
    const code = /^Command exited with code (\d+)$/.exec(last), timeout = /^Command timed out after (\d+) seconds?$/.exec(last);
    if (code) exit = { ok: code[1] === "0", words: `exit ${code[1]}` };
    else if (timeout) exit = { ok: false, words: `timed out after ${formatElapsed(Number(timeout[1]) * 1000)}` };
    else if (/^Command aborted$/.test(last)) exit = { ok: false, words: "stopped" };
    else if (!/^\[(Full output|Truncated|Showing)[^\]]*\]$/.test(last) && last !== "") break;
    lines.pop();
  }
  return { output: lines.join("\n").trim() === "(no output)" ? "" : lines.join("\n"), ...(exit ? { exit } : {}) };
}

/**
 * The last line worth reading after a success: at least three letters or digits, never a `set -x` trace (`+ cmd`)
 * nor a line of JSON (`{"actor":...`).
 */
function lastWords(output: string): string | undefined {
  const line = output.split("\n").reverse()
    .find(item => !/^\s*\++ /.test(item) && !/^\s*[[{]/.test(item) && (item.match(/[\p{L}\p{N}]/gu)?.length ?? 0) >= 3);
  return line === undefined ? undefined : firstLine(line);
}

/**
 * A finished command: `✓ exit 0` (its last output line only when the row has room) or `✗ exit 1` with its last 3
 * lines. `total` is the real line count when `output` is a clipped preview; undefined there means unknown.
 */
function ended(base: Omit<RowModel, "state">, exit: Exit, output: string, failed: boolean, total: number | undefined = lineCount(output)): RowModel {
  const shown = lineCount(output), lines = total ?? shown;
  if (exit.ok && !failed) {
    const last = lastWords(output);
    return { ...base, state: "ok", outcome: exit.words + (lines === 0 ? " · no output" : ""), ...(last ? { detail: last } : {}),
      ...(total ? { hiddenHint: `${total} ${total === 1 ? "line" : "lines"}` } : {}) };
  }
  return { ...base, state: "failed", outcome: exit.ok ? "failed" : exit.words, ...(shown ? { body: tailLines(output, 3) } : {}),
    ...(total !== undefined && total > 3 ? { hiddenHint: `${total - 3} more lines` } : {}) };
}

/** `j1-4 · cargo test -p host`, or the id alone when the job is not known here. */
function jobTarget(id: string, deps: ToolRenderDeps): string {
  const facts = deps.job(id);
  return facts ? `${id} · ${commandLine(facts.command, deps.cwd)}` : id;
}
function agentsView(id: string, deps: ToolRenderDeps): { readonly view?: RowView } {
  const open = deps.openView;
  return open ? { view: { label: "Agents", open: () => open("agents", `job:${id}`) } } : {};
}
/** The output inside a job result: its `text` field, else the part of the model text after the metadata. */
function jobOutput(details: Readonly<Record<string, unknown>> | undefined, text: string): string {
  if (typeof details?.["text"] === "string") return details["text"];
  const marker = text.indexOf("Output (untrusted data):\n");
  return marker >= 0 ? text.slice(marker + "Output (untrusted data):\n".length) : text;
}
function failedWith(base: Omit<RowModel, "state">, text: string, details: Readonly<Record<string, unknown>> | undefined): RowModel {
  const reason = typeof details?.["error"] === "string" ? details["error"] : firstLine(text);
  const rest = typeof details?.["error"] === "string" ? [] : text.split("\n").filter(line => line.trim() !== "").slice(1);
  return { ...base, state: "failed", outcome: reason || "failed", ...(rest.length ? { body: tailLines(rest.join("\n"), 3) } : {}) };
}

/** A command still running after its call returned: live from the job until it exits. */
function background(base: Omit<RowModel, "state">, id: string, call: CallInput, deps: ToolRenderDeps, command: string): RowModel {
  const facts: JobFacts | undefined = deps.job(id), view = agentsView(id, deps);
  if (!facts) return { ...base, state: "waiting", outcome: "continued in the background", right: id };
  const since = facts.startedAt ?? call.startedAt;
  if (facts.state === "running") {
    return { ...base, ...view, state: "running", outcome: "running in the background", right: id,
      ...(since === undefined ? {} : { duration: { since } }), live: `tool-row:${call.toolCallId}` };
  }
  const duration = since !== undefined && facts.endedAt !== undefined ? { duration: facts.endedAt - since } : {};
  const expanded = (): readonly BodyLine[] => [...textLines(command, "text"), ...(facts.tail.trim() ? [{ text: "" }, ...textLines(facts.tail)] : [])];
  return ended({ ...base, ...view, expanded, ...duration }, exitOf(facts.exitCode, facts.signal), facts.tail, false, facts.lines);
}

/** `bash` (managed or Pi's own) and `job_start`. */
function bashPresenter(deps: ToolRenderDeps): Presenter {
  return call => {
    const command = stringArg(call.args, "command") ?? "", target = commandLine(command, call.cwd);
    const text = resultText(call.result), details = recordOf(call.result?.details);
    const output = details && typeof details["job"] === "string" ? jobOutput(details, text) : sdkOutput(text).output;
    const timeout = numberArg(call.args, "timeout");
    const base: Omit<RowModel, "state"> = {
      verb: "bash", ...(target ? { target } : {}),
      expanded: (): readonly BodyLine[] => [...textLines(command, "text"), ...(output.trim() ? [{ text: "" }, ...textLines(output)] : [])],
    };
    const state = baseState(call);
    if (state === "writing") return { ...base, state };
    if (state === "running") {
      const tail = tailLines(text, 2);
      return { ...base, state, ...(timeout === undefined ? {} : { right: `timeout ${formatElapsed(timeout * 1000)}` }), ...(tail.length ? { body: tail } : {}) };
    }
    const job = details && typeof details["job"] === "string" ? details["job"] : undefined;
    if (job && details?.["state"] === "running") return background(base, job, call, deps, command);
    if (job && details?.["state"] === "exited") {
      return ended(base, exitOf(details["exit_code"], details["signal"], details["termination"]), output, call.isError, outputLines(call, output));
    }
    if (details && typeof details["error"] === "string") return failedWith(base, text, details);
    const sdk = sdkOutput(text);
    if (sdk.exit) return ended(base, sdk.exit, sdk.output, call.isError, outputLines(call, sdk.output));
    // A blocked or refused call: its text is the reason.
    if (call.isError) return failedWith(base, text, details);
    return ended(base, { ok: true, words: "exit 0" }, sdk.output, false, outputLines(call, sdk.output));
  };
}

/** `job_wait`: the command it waited for and how that ended, or why it stopped waiting. */
function waitPresenter(deps: ToolRenderDeps): Presenter {
  return call => {
    const asked = stringArg(call.args, "job"), waitMs = numberArg(call.args, "wait_ms");
    const text = resultText(call.result), details = recordOf(call.result?.details);
    const id = asked ?? (typeof details?.["job"] === "string" ? details["job"] : undefined);
    const output = jobOutput(details, text);
    const base: Omit<RowModel, "state"> = {
      verb: "wait", target: id ? jobTarget(id, deps) : "next job to finish", ...(id ? agentsView(id, deps) : {}),
      ...(output.trim() ? { expanded: (): readonly BodyLine[] => textLines(output) } : {}),
    };
    const state = baseState(call);
    if (state === "writing") return { ...base, state };
    if (state === "running") return { ...base, state, ...(waitMs === undefined ? {} : { right: `up to ${formatElapsed(waitMs)}` }) };
    // The time slot already shows how long the call waited.
    switch (details?.["state"]) {
      case "exited": return ended(base, exitOf(details["exit_code"], details["signal"], details["termination"]), output, false, outputLines(call, output));
      case "running": return { ...base, state: "waiting", outcome: "still running · stopped waiting" };
      case "waiting": return { ...base, state: "waiting", outcome: "nothing finished yet" };
      case "none": return { ...base, state: "waiting", outcome: "no jobs to wait for" };
      case "wait_cancelled": return { ...base, state: "waiting", outcome: "wait cancelled · the job keeps running" };
      default: return call.isError ? failedWith(base, text, details) : { ...base, state, outcome: firstLine(text) };
    }
  };
}

/** `job_output`: a peek at a job's log, its state and size; the lines open on expand. */
function outputPresenter(deps: ToolRenderDeps): Presenter {
  return call => {
    const id = stringArg(call.args, "job") ?? "";
    const text = resultText(call.result), details = recordOf(call.result?.details), output = jobOutput(details, text);
    const lines = outputLines(call, output);
    const base: Omit<RowModel, "state"> = {
      verb: "output", ...(id ? { target: jobTarget(id, deps), ...agentsView(id, deps) } : {}),
      ...(output.trim() ? { expanded: (): readonly BodyLine[] => textLines(output) } : {}),
      ...(lines ? { hiddenHint: `${lines} ${lines === 1 ? "line" : "lines"}` } : {}),
    };
    const state = baseState(call);
    if (state === "writing" || state === "running") return { ...base, state };
    if (call.isError || typeof details?.["error"] === "string") return failedWith(base, text, details);
    const exit = details?.["state"] === "exited" ? exitOf(details["exit_code"], details["signal"], details["termination"]) : undefined;
    const last = lastWords(output);
    return { ...base, state: "ok", outcome: [exit ? exit.words : "running", ...(lines === 0 ? ["no output"] : []),
      ...(details?.["retained_snapshot"] === true ? ["retained snapshot"] : [])].join(" · "), ...(last ? { detail: last } : {}),
    ...(exit && !exit.ok ? { outcomeTone: "error" as const } : {}) };
  };
}

/** `job_kill`: which command was stopped and how. */
function killPresenter(deps: ToolRenderDeps): Presenter {
  return call => {
    const id = stringArg(call.args, "job") ?? "", text = resultText(call.result), details = recordOf(call.result?.details);
    const base: Omit<RowModel, "state"> = { verb: "stop", ...(id ? { target: jobTarget(id, deps) } : {}) };
    const state = baseState(call);
    if (state === "writing" || state === "running") return { ...base, state };
    if (call.isError || typeof details?.["error"] === "string") return failedWith(base, text, details);
    if (details?.["state"] === "lost") return { ...base, state: "unknown", outcome: "job no longer tracked" };
    const signal = typeof details?.["signal"] === "string" ? ` (${details["signal"]})` : "";
    const before = details?.["termination"] === "exit" ? "had already exited" : `stopped${signal}`;
    return { ...base, state: "ok", outcome: `${before} · output kept` };
  };
}

/** `corral_job_monitor`: an armed watch on a job is waiting, not done, until the job exits or the line matches. */
function monitorPresenter(deps: ToolRenderDeps): Presenter {
  return call => {
    const id = stringArg(call.args, "job") ?? "", on = stringArg(call.args, "on"), pattern = stringArg(call.args, "pattern");
    const wake = on === "line" ? `until /${pattern ?? ""}/ matches` : "until it exits";
    const text = resultText(call.result), details = recordOf(call.result?.details), result = recordOf(details?.["result"]);
    const base: Omit<RowModel, "state"> = { verb: "watching", ...(id ? { target: jobTarget(id, deps) } : {}) };
    const state = baseState(call);
    if (state === "writing" || state === "running") return { ...base, state };
    if (call.isError || typeof details?.["error"] === "string") return failedWith(base, text, details);
    if (details?.["state"] === "rejected") {
      const reason = [result?.["code"], result?.["reason"] ?? result?.["message"]].filter(part => typeof part === "string").join(": ");
      return { ...base, state: "failed", outcome: reason || "refused" };
    }
    if (details?.["state"] !== "committed") return { ...base, state: "unknown", outcome: `${wake} · ${String(details?.["state"] ?? "no receipt")}` };
    if (result?.["settled"] === true) return { ...base, verb: "watched", state: "ok", outcome: "already settled" };
    const deadline = typeof result?.["deadline"] === "string" ? new Date(result["deadline"]) : undefined;
    const until = deadline && !Number.isNaN(deadline.getTime()) ? ` · until ${deadline.toTimeString().slice(0, 5)}` : "";
    return { ...base, state: "waiting", outcome: `${wake}${until}` };
  };
}

/** Presenters for commands and jobs. */
export const jobPresenters: PresenterModule = {
  presenter(name, deps) {
    switch (name) {
      case "bash": case "job_start": return bashPresenter(deps);
      case "job_wait": return waitPresenter(deps);
      case "job_output": return outputPresenter(deps);
      case "job_kill": return killPresenter(deps);
      case "corral_job_monitor": return monitorPresenter(deps);
      default: return undefined;
    }
  },
};
