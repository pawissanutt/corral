import { BridgeRefusal, SESSION_PART_MAX } from "../../bridge/protocol.ts";
import type { BuildRecord, RecordUsage } from "../../build/record-types.ts";
import type { BoardControl, BoardPlan } from "../../build/types.ts";
import type { RemoteRegistry } from "./registry.ts";
import { clipText, fitRows, isoTime, jsonBytes, optionalText } from "./state.ts";

const KiB = 1024;

function boardPlan(plan: BoardPlan) {
  // Reserve room for all twenty task identities so a clipped body cannot hide a dependency.
  const tasks = plan.tasks.slice(0, 20).map(({ key, title, kind, size, profile, depends, body }) => ({
    key: clipText(key, 160), title: clipText(title, 240), kind: optionalText(kind, 64), size: optionalText(size, 64),
    profile: optionalText(profile, 160), depends: fitRows(depends, 0, 32, 512), body: "", body_bytes: Buffer.byteLength(body),
  }));
  const result = { title: clipText(plan.title, 512), project: optionalText(plan.project, 256), why: clipText(plan.why, 8 * KiB), tasks };
  let room = Math.max(0, 37 * KiB - jsonBytes(result));
  for (const [i, row] of tasks.entries()) {
    row.body = clipText(plan.tasks[i]!.body, Math.min(2 * KiB, Math.floor(room / (tasks.length - i))));
    room -= jsonBytes(row.body) - 2;
  }
  return result;
}

export function boardPart(board: Pick<BoardControl, "tasks" | "proposed"> | undefined, unavailable?: string) {
  const plan = board?.proposed();
  const proposed = plan === undefined ? undefined : boardPlan(plan);
  const tasks = [...board?.tasks() ?? []].slice(-100).reverse().map(({ id, title, status, phase, depends, detail, updatedAt }) => ({
    id, title: clipText(title, 512), status, phase, depends: fitRows(depends, 0, 64, KiB),
    detail: optionalText(detail, 512), updated_at: isoTime(updatedAt),
  }));
  return { proposed, unavailable, tasks: fitRows(tasks, 0, 100, SESSION_PART_MAX - jsonBytes({ proposed, unavailable, tasks: [] })) };
}

function recordUsage({ input, output, cost }: RecordUsage) { return { input, output, cost_usd: cost }; }

export function recordPart(record: BuildRecord) {
  const { outcome, diff, title } = record;
  const usage = { parent: recordUsage(record.usage.parent), children: recordUsage(record.usage.children),
    reviewers: recordUsage(record.usage.reviewers), total: recordUsage(record.usage.total) };
  const commits = record.commits.slice(0, 50).map(({ sha, subject, slice }) => ({ sha, subject: clipText(subject, 384), slice }));
  return { title: clipText(title, 512), outcome: { ...outcome, override_reason: optionalText(outcome.override_reason, 512),
      base_branch: optionalText(outcome.base_branch, 256) }, diff, usage, commits_total: Math.max(record.commits.length, diff?.commits ?? 0),
    commits: commits.length ? fitRows(commits, 0, 50, 8 * KiB).reverse() : undefined,
    verification: fitRows(record.verification.slice(-100).reverse().map(({ attempt, head, result, failed_command, started, ended }) => ({
      attempt, tier: "full", head, result, failed_command: optionalText(failed_command, 512), started, ended,
    })), 0, 100, 5 * KiB).reverse(),
    reviews: fitRows(record.reviews.slice(-50).reverse().map(({ round, reviewer, model, head, verdict, at, summary, findings }) => ({
      round, reviewer, model: clipText(model, 512), head, verdict, at, summary: optionalText(summary, 768),
      findings: fitRows(findings.slice(0, 50).map(({ id, severity, path, line, text, resolution }) => ({ id, severity,
        path: optionalText(path, 512), line, text: clipText(text, 768), resolution: resolution ? { ...resolution,
          reason: optionalText(resolution.reason, 512) } : undefined })), 0, 50, 7 * KiB),
    })), 0, 50, 15 * KiB).reverse(),
    decisions: fitRows(record.decisions.slice(-100).reverse().map(({ at, kind, text, answer }) => ({
      at, kind, text: clipText(text, 768), answer: optionalText(answer, 512),
    })), 0, 100, 8 * KiB).reverse(),
    timeline: fitRows(record.timeline.slice(-200).reverse().map(({ at, phase, detail }) => ({
      at, phase, detail: optionalText(detail, 512),
    })), 0, 200, 8 * KiB).reverse(),
  };
}

export function registerBoard(registry: RemoteRegistry, source: {
  board?: BoardControl; unavailable?: string; record(): Promise<BuildRecord>; ready(): boolean;
}): void {
  const { board, unavailable } = source;
  registry.register({ name: "record", parse: () => undefined, run: async () => recordPart(await source.record()) });
  registry.register({ name: "board", parse: () => undefined, run: () => boardPart(board, unavailable) });
  if (board && !unavailable) registry.register({ name: "board_refresh", parse: () => undefined, run: async () => {
    if (!source.ready()) throw new BridgeRefusal("host_not_ready", "Session is not accepting board actions");
    await board.refresh();
    return boardPart(board);
  } });
}
