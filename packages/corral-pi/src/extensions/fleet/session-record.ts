import type { RecordMessage, RecordReview, RecordSlice, RecordUsage } from "../../build/record-types.ts";
import type { FleetSnapshot } from "../../build/types.ts";

export interface SessionRecordOptions {
  readonly task?: string;
  readonly run?: string;
}

/** A worker's advisory fleet report, not a build record or completion authority. */
export interface SessionRecord extends SessionRecordOptions {
  readonly version: 1;
  readonly slices: readonly RecordSlice[];
  // Unlike a build review, an advisory verdict may not name a commit.
  readonly reviews: readonly (Omit<RecordReview, "head"> & { readonly head?: string })[];
  readonly messages: readonly RecordMessage[];
  readonly usage: { readonly children: RecordUsage; readonly reviewers: RecordUsage; readonly total: RecordUsage };
}

const iso = (at: number) => new Date(at).toISOString();
function sum(values: readonly RecordUsage[]): RecordUsage {
  return { input: values.reduce((n, value) => n + value.input, 0), output: values.reduce((n, value) => n + value.output, 0),
    ...(values.every(value => value.cost !== undefined) ? { cost: values.reduce((n, value) => n + value.cost!, 0) } : {}) };
}

export function sessionRecord(fleet: FleetSnapshot, options: SessionRecordOptions = {}): SessionRecord {
  const children = sum(fleet.children.filter(child => child.kind !== "review").map(child => child.usage));
  const reviewers = sum(fleet.children.filter(child => child.kind === "review").map(child => child.usage));
  let round = 0;
  return { version: 1, ...options,
    slices: fleet.children.map(child => ({ id: child.id, kind: child.kind, title: child.title, owns: child.owns,
      model: child.model, effort: child.effort, state: child.state,
      started: child.startedAt === undefined ? undefined : iso(child.startedAt),
      ended: child.endedAt === undefined ? undefined : iso(child.endedAt),
      usage: { input: child.usage.input, output: child.usage.output, turns: child.usage.turns,
        ...(child.usage.cost === undefined ? {} : { cost: child.usage.cost }) },
      outside: [...new Set(child.edits.filter(edit => edit.outside).map(edit => edit.path))], report: child.report?.slice(0, 8192) })),
    reviews: fleet.children.flatMap(child => {
      const verdict = child.kind === "review" ? child.verdict : undefined;
      return verdict ? [{ round: ++round, reviewer: child.id, model: child.model, verdict: verdict.verdict,
        summary: verdict.summary, head: verdict.head, at: iso(verdict.at),
        findings: verdict.findings.map((finding, index) => ({ id: `F${index + 1}`, ...finding })) }] : [];
    }),
    messages: fleet.mail.slice(-500).map(message => ({ from: message.from, to: message.to, at: iso(message.at),
      text: message.text.slice(0, 2048), delivery: message.delivery })),
    usage: { children, reviewers, total: sum([children, reviewers]) } };
}
