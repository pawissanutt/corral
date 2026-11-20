import { BridgeRefusal, SESSION_PART_MAX } from "../../bridge/protocol.ts";
import type { Commands } from "../../extensions/jobs/commands.ts";
import type { OutputOptions } from "../../extensions/jobs/output.ts";
import type { JobInspection } from "../../extensions/jobs/runtime.ts";
import type { JobAction } from "../job-controls.ts";
import type { RemoteRegistry } from "./registry.ts";
import { clipText, cursorOffset, fingerprintPart, fitRows, jsonBytes, optionalText, optionalTime, parseCursor, partCursor } from "./state.ts";

export type ReadJobOutput = (job: string, options: OutputOptions) => Record<string, unknown>;

interface JobRow {
  id: string; command: string; dir?: string; state: string; started_at?: string; ended_at?: string;
  exit_code?: number; output_bytes: number; tail: string[]; monitors: string[]; actions: readonly string[];
}
export interface SessionJobs { jobs: JobRow[]; unavailable?: string }
const OUTPUT_BYTES = 40 * 1024;
const textOf = (result: Record<string, unknown>, key: string) => typeof result[key] === "string" ? result[key] : undefined;
const numberOf = (result: Record<string, unknown>, key: string) => typeof result[key] === "number" ? result[key] : undefined;
const tail = (text: string) => text.trimEnd().split("\n").slice(-2).filter(Boolean).map(line => clipText(line, 320));

export function jobsPart(commands: Pick<Commands, "inspection" | "output" | "recovered"> | undefined,
  allowed: (action: "stop" | "background") => boolean, live = commands?.inspection() ?? [], recovered = commands?.recovered() ?? []): SessionJobs {
  if (!commands) return { jobs: [], unavailable: "Managed commands are not enabled for this session" };
  const rows = [...live].reverse().map(job => liveJob(job, allowed));
  for (const job of [...recovered].reverse()) {
    const output = commands.output(job.job, { tail_lines: 2, max_bytes: 1024 });
    rows.push({ id: job.job, command: clipText(textOf(output, "command") ?? "", 512), dir: undefined, state: job.state,
      started_at: undefined, ended_at: undefined, exit_code: numberOf(output, "exit_code"),
      output_bytes: numberOf(output, "bytes_total") ?? numberOf(output, "next_offset") ?? 0,
      tail: tail(textOf(output, "text") ?? ""), monitors: [], actions: [] });
  }
  return { jobs: fitRows(rows, 0, 100, SESSION_PART_MAX - 64) };
}

function liveJob(job: JobInspection, allowed: (action: "stop" | "background") => boolean): JobRow {
  return { id: job.job, command: clipText(job.command, 512), dir: optionalText(job.cwd, 512), state: job.state,
    started_at: optionalTime(job.startedAt), ended_at: optionalTime(job.endedAt), exit_code: job.exitCode ?? undefined,
    output_bytes: job.bytes, tail: tail(job.tail), monitors: job.watches.filter(watch => watch.armed).slice(-8)
      .map(watch => clipText(watch.pattern || watch.wait, 128)),
    actions: job.state === "running" ? (["stop", "background"] as const).filter(allowed) : [] };
}

function outputChunk(result: Record<string, unknown>) {
  return { text: textOf(result, "text") ?? "", offset: numberOf(result, "offset") ?? 0 };
}

/** The cursor is an exclusive upper byte offset; the running job may append without shifting it. */
export function jobOutputPage(read: ReadJobOutput, job: string, after?: string) {
  const key = fingerprintPart(["job_output", job]);
  let end = after === undefined ? undefined : cursorOffset(after, key);
  const latest = read(job, { max_bytes: end === undefined ? OUTPUT_BYTES : 4, tail_lines: OUTPUT_BYTES });
  const bytes = numberOf(latest, "bytes_total") ?? numberOf(latest, "next_offset") ?? 0;
  const stored = latest.retained_snapshot ? 0 : numberOf(latest, "bytes_stored") ?? 0;
  const live = latest.state === "running";
  if (end !== undefined && (end === 0 || end > bytes)) throw new BridgeRefusal("bad_input", "job cursor is outside the retained output");
  let { offset, text } = outputChunk(latest);
  if (end !== undefined) {
    // Tail reads and stored-prefix reads use different retention windows; never concatenate over a gap.
    if (end > stored) {
      const newest = outputChunk(read(job, { max_bytes: 64 * 1024, tail_lines: 64 * 1024 }));
      if (end > newest.offset && newest.text) {
        const bytes = Buffer.from(newest.text);
        const to = Math.min(end - newest.offset, bytes.length);
        let from = Math.max(0, to - OUTPUT_BYTES);
        while (from < to && (bytes[from]! & 0xc0) === 0x80) from++;
        text = bytes.subarray(from, to).toString("utf8");
        offset = newest.offset + from;
      } else text = "";
    } else text = "";
    if (!text) {
      end = Math.min(end, stored);
      const start = Math.max(0, end - OUTPUT_BYTES);
      ({ offset, text } = end > start ? outputChunk(read(job, { offset: start, max_bytes: end - start })) : { offset: end, text: "" });
      if (offset >= end) throw new BridgeRefusal("stale_page", "job output expired; read the newest page");
    }
  }
  const budget = SESSION_PART_MAX - 128;
  if (jsonBytes(text) > budget) {
    let lo = 0, hi = text.length;
    while (lo < hi) {
      const mid = Math.floor((lo + hi) / 2);
      if (jsonBytes(text.slice(mid)) <= budget) hi = mid; else lo = mid + 1;
    }
    if (/[\uDC00-\uDFFF]/.test(text[lo] ?? "")) lo++;
    offset += Buffer.byteLength(text.slice(0, lo));
    text = text.slice(lo);
  }
  const floor = stored || offset === 0 ? 0 : outputChunk(read(job, { offset: 0, max_bytes: 4 })).offset;
  return { text, offset, bytes, live, next: offset > floor ? partCursor(key, offset) : undefined };
}

export function registerJobs(registry: RemoteRegistry, source: {
  list(): SessionJobs; read?: ReadJobOutput;
  act(job: string, action: JobAction): Promise<Record<string, unknown>>;
}): void {
  registry.register({ name: "jobs", parse: () => undefined, run: () => source.list() });
  if (!source.read) return;
  const read = source.read;
  const id = (job: unknown) => {
    if (typeof job !== "string" || !/^[A-Za-z0-9._:-]{1,128}$/.test(job)) throw new BridgeRefusal("bad_input", "invalid job id");
    return job;
  };
  registry.register({ name: "job_output", parse: ({ job, after }) => ({ job: id(job), after: parseCursor(after) }),
    run: ({ job, after }) => jobOutputPage(read, job, after) });
  registry.register({ name: "job_act", parse: ({ job, action }) => {
    if (action !== "stop" && action !== "background") throw new BridgeRefusal("bad_input", "invalid job action");
    return { job: id(job), action } as const;
  }, run: async ({ job, action }) => {
    const result = await source.act(job, action);
    return { state: String(result.state), reason: optionalText(textOf(result, "reason"), 512) };
  } });
}
