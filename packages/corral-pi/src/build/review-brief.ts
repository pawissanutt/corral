import type { Run } from "./corral-cli.ts";
import type { RecordReview } from "./record-types.ts";

/** The most a child's brief may hold (`FleetRuntime.start` refuses a longer one). */
export const BRIEF_LIMIT = 32768;

export interface ReviewBriefParts {
  /** What the reviewer always needs, in order before the variable sections. */
  head: string[];
  /** The preceding review and committed delta, before the original build context. */
  previous?: { overview: string; findings: string[]; delta: string };
  /** The owner's prompts, verbatim while they fit. */
  prompts: string[];
  /** One entry per slice: its id, title, owned paths and report. */
  slices: string[];
  /** What follows the variable sections: the gate, the focus, how to finish. */
  tail: string[];
  /** Where the full variable sections stay, named if any are shortened. */
  record: string;
}

const JOIN = "\n\n";
const DELTA_LIMIT = 8192;

export const quoteGitArg = (value: string) => `'${value.replaceAll("'", "'\\''")}'`;

export async function previousReviewBrief(git: Run, cwd: string, prior: RecordReview, head: string, base: string | null):
  Promise<NonNullable<ReviewBriefParts["previous"]>> {
  const overview = `Previous review round ${prior.round}: ${prior.verdict} (${prior.base ?? "unknown base"} → ${prior.head || "no Git head"}).`
    + "\nCheck the findings and dispositions independently against what changed; inspect the full candidate where needed.";
  const findings = prior.findings.map(({ id, severity, path, line, text, resolution }) =>
    `${id} [${severity}] ${path ? `${path}${line ? `:${line}` : ""}: ` : ""}${text}\nDisposition: ${resolution?.state ?? "open"}`
    + `${resolution?.commit ? ` at ${resolution.commit}` : ""}${resolution?.reason ? ` — ${resolution.reason}` : ""}`);
  if (prior.summary) findings.unshift(`Previous reviewer summary: ${prior.summary}`);
  return { overview, findings, delta: await committedDelta(git, cwd, prior, head, base) };
}

async function committedDelta(git: Run, cwd: string, prior: RecordReview, head: string, base: string | null) {
  if (!prior.head || !head) return "Incremental Git delta unavailable: no committed comparison point. Inspect the current files.";
  const gitIn = `git -C ${quoteGitArg(cwd)}`;
  const range = `${prior.head}..${head}`;
  const flags = ["--no-ext-diff", "--no-textconv", "--no-color"];
  const command = `${gitIn} diff ${flags.join(" ")} --end-of-options ${quoteGitArg(range)} --`;
  let context = `Incremental Git delta (previous reviewed head → current head):\n${command}`;
  if (prior.base !== base) {
    context += "\nThe base changed or was not recorded; the head-to-head patch can include upstream changes.";
    if (prior.base && base) {
      context += `\nCompare the two build commit ranges: ${gitIn} range-diff ${quoteGitArg(`${prior.base}..${prior.head}`)} ${quoteGitArg(`${base}..${head}`)}`;
    }
  }
  if (prior.head === head) return `${context}\nNo committed tree changes.`;
  const chunks: Buffer[] = [];
  let bytes = 0;
  let clipped = false;
  try {
    const result = await git(["diff", ...flags, "--end-of-options", range, "--"], { cwd, stdoutChunk(chunk) {
      const remaining = DELTA_LIMIT - bytes;
      if (remaining > 0) {
        chunks.push(chunk.subarray(0, remaining));
        bytes += Math.min(remaining, chunk.length);
      }
      if (chunk.length > remaining) clipped = true;
    } });
    if (result.code !== 0) return `${context}\nCould not read the prior Git delta; inspect the full current candidate.`;
  } catch {
    return `${context}\nCould not read the prior Git delta; inspect the full current candidate.`;
  }
  const patch = Buffer.concat(chunks, bytes).toString("utf8").trimEnd();
  return `${context}\n${patch || "No committed tree changes."}${clipped ? "\n[Patch preview truncated; use the command above for the full delta.]" : ""}`;
}

/**
 * The reviewer's brief within `BRIEF_LIMIT`. A long build's slice reports and prompts together
 * outgrew the limit, and the fleet refused the reviewer, so no build of that size could be
 * reviewed. The brief is whole when it fits. Otherwise the fixed instructions and delta preview
 * stay whole and the findings, prompts and slices share what is left. Each shortens only as
 * far as the others leave no room for it (the shortest stay whole); when even individual
 * omission notes do not fit, one line stands for them all.
 */
export function reviewBrief(parts: ReviewBriefParts): string {
  const findings = parts.previous?.findings ?? [];
  const whole = assemble(parts, findings, parts.prompts, parts.slices, false);
  if (whole.length <= BRIEF_LIMIT) return whole;
  const fixed = assemble(parts, [], [], [], true).length;
  const items = [...findings, ...parts.prompts, ...parts.slices];
  const fitted = fit(items, BRIEF_LIMIT - fixed);
  if (!fitted) throw new Error(`The review brief needs ${fixed} characters before any variable sections; shorten the focus.`);
  if (fitted.length !== items.length) {
    return parts.previous ? assemble(parts, fitted, [], [], true) : assemble(parts, [], fitted, [], true);
  }
  const promptEnd = findings.length + parts.prompts.length;
  return assemble(parts, fitted.slice(0, findings.length), fitted.slice(findings.length, promptEnd), fitted.slice(promptEnd), true);
}

function assemble(parts: ReviewBriefParts, findings: string[], prompts: string[], slices: string[], shortened: boolean): string {
  const notice = `Some brief sections are shortened; the full prior findings, prompts and reports are in ${parts.record}.`;
  const previous = parts.previous ? [parts.previous.overview, ...findings, parts.previous.delta] : [];
  return [...parts.head, ...(shortened ? [notice] : []), ...previous, "Owner prompts (verbatim):", ...prompts, "Slices:", ...slices, ...parts.tail]
    .join(JOIN);
}

/**
 * `items` within `room`, counting the separator each one adds: every item cut to one common cap,
 * the largest that fits with every cut's note; else one line for them all; else none.
 */
function fit(items: string[], room: number): string[] | undefined {
  const cut = (item: string, cap: number) => {
    if (item.length <= cap) return item;
    const note = `\n[${item.length - cap} characters omitted]`;
    return item.slice(0, Math.max(0, cap - note.length)) + note;
  };
  const size = (cap: number) => items.reduce((sum, item) => sum + cut(item, cap).length + JOIN.length, 0);
  if (size(0) > room) {
    const all = [`[all ${items.length} variable entries omitted]`];
    return all[0]!.length + JOIN.length <= room ? all : undefined;
  }
  let low = 0;
  let high = Math.max(0, ...items.map(item => item.length));
  while (low < high) {
    const mid = Math.ceil((low + high) / 2);
    if (size(mid) <= room) low = mid; else high = mid - 1;
  }
  return items.map(item => cut(item, low));
}
