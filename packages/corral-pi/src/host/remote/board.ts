import { BridgeRefusal, SESSION_PART_MAX } from "../../bridge/protocol.ts";
import type { BuildRecord, RecordUsage } from "../../build/record-types.ts";
import type { BoardControl, BoardPlan } from "../../build/types.ts";
import type { RemoteRegistry } from "./registry.ts";
import { clipText, cursorOffset, fingerprintPart, fitRows, isoTime, jsonBytes, optionalText, parseCursor, partCursor } from "./state.ts";

const KiB = 1024;
const BOARD_KEY_BYTES = 160;
const BOARD_PROJECT_BYTES = 256;
/** What the proposal as a whole may take once the briefs are in; the filed tasks keep the rest of the part. */
const BOARD_PLAN_BYTES = 37 * KiB;
/** What the board part keeps for its own fields (`unavailable`, the filed tasks' list) whatever the proposal holds. */
const BOARD_ENVELOPE_BYTES = 2 * KiB;
const BRIEF_PAGE_BYTES = 40 * KiB;
const proposalFingerprints = new WeakMap<BoardPlan, string>();

/** A displayed key must identify exactly one task, even when its original key cannot fit the board. */
function boardKeys(plan: BoardPlan): Map<string, string> {
  const keys = new Map(plan.tasks.map(({ key }) => [key, key]));
  for (const [index, { key }] of plan.tasks.entries()) {
    if (clipText(key, BOARD_KEY_BYTES) === key) continue;
    let alias = `@${index + 1}`;
    while (keys.has(alias)) alias = `@${alias}`;
    keys.set(key, alias);
  }
  return keys;
}

function boardPlan(plan: BoardPlan) {
  const keys = boardKeys(plan);
  // Identities (key, project, every accepted dependency) are never clipped; only the reasons and the briefs give way.
  const tasks = plan.tasks.slice(0, 20).map(({ key, project, title, kind, size, difficulty, tags, profile, depends, body }) => ({
    key: keys.get(key)!, project: optionalText(project ?? plan.project, BOARD_PROJECT_BYTES), title: clipText(title, 240),
    kind: optionalText(kind, 64), size: optionalText(size, 64), difficulty, tags, profile: optionalText(profile, 160),
    depends: depends.map(key => keys.get(key) ?? key), body: "", body_bytes: Buffer.byteLength(body),
  }));
  const head = { title: clipText(plan.title, 512), project: optionalText(plan.project, BOARD_PROJECT_BYTES) };
  // Leave the filed tasks' list and the part's envelope their share, then give the reasons what the identities leave.
  const why = clipText(plan.why, Math.min(8 * KiB, Math.max(0, SESSION_PART_MAX - BOARD_ENVELOPE_BYTES - jsonBytes({ ...head, why: "", tasks }))));
  const result = { ...head, why, tasks };
  let room = Math.max(0, BOARD_PLAN_BYTES - jsonBytes(result));
  for (const [i, row] of tasks.entries()) {
    row.body = clipText(plan.tasks[i]!.body, Math.min(2 * KiB, Math.floor(room / (tasks.length - i))));
    room -= jsonBytes(row.body) - 2;
  }
  return result;
}

function boardPlanTask(plan: BoardPlan | undefined, key: string, after?: string) {
  const keys = plan ? boardKeys(plan) : undefined;
  const task = plan?.tasks.slice(0, 20).find(task => keys?.get(task.key) === key);
  if (!task || !plan) {
    if (after !== undefined) throw new BridgeRefusal("stale_page", "the proposed board task changed or was dismissed");
    throw new BridgeRefusal("board_plan_task_missing", "no proposed board task has this key");
  }

  // The board replaces the proposal object rather than mutating it.
  let fingerprint = proposalFingerprints.get(plan);
  if (fingerprint === undefined) {
    fingerprint = fingerprintPart(plan);
    proposalFingerprints.set(plan, fingerprint);
  }
  const token = fingerprintPart(["board_plan_task", fingerprint, key]);
  const full = Buffer.from(task.body, "utf8"), bytes = full.length;
  const offset = after === undefined ? 0 : cursorOffset(after, token);
  if (offset > bytes || ((full[offset] ?? 0) & 0xc0) === 0x80) {
    throw new BridgeRefusal("bad_input", "board brief cursor is not on a UTF-8 character boundary");
  }
  const head = { key, bytes, offset };
  let end = Math.min(bytes, offset + BRIEF_PAGE_BYTES);
  while (((full[end] ?? 0) & 0xc0) === 0x80) end--;
  const budget = SESSION_PART_MAX - jsonBytes({ ...head, body: "", next: partCursor(token, bytes) });
  const body = clipText(full.toString("utf8", offset, end), budget);
  end = offset + Buffer.byteLength(body);
  return { ...head, body, next: end < bytes ? partCursor(token, end) : undefined };
}

export function boardPart(board: Pick<BoardControl, "tasks" | "proposed"> | undefined, unavailable?: string) {
  const plan = board?.proposed();
  const proposed = plan === undefined ? undefined : boardPlan(plan);
  const tasks = [...board?.tasks() ?? []].slice(-100).reverse().map(({ id, key, project, title, status, phase, size, difficulty, tags, depends, detail, updatedAt }) => ({
    id, key: optionalText(key, BOARD_KEY_BYTES), project: optionalText(project, BOARD_PROJECT_BYTES), title: clipText(title, 512), status, phase, size: optionalText(size, 64), difficulty, tags, depends: fitRows(depends, 0, 64, KiB),
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
  board?: Pick<BoardControl, "tasks" | "proposed" | "refresh">; unavailable?: string; record(): Promise<BuildRecord>; ready(): boolean;
}): void {
  const { board, unavailable } = source;
  registry.register({ name: "record", parse: () => undefined, run: async () => recordPart(await source.record()) });
  registry.register({ name: "board", parse: () => undefined, run: () => boardPart(board, unavailable) });
  registry.register({ name: "board_plan_task", parse: ({ key, after }) => {
    if (typeof key !== "string" || !key || Buffer.byteLength(key) > BOARD_KEY_BYTES) {
      throw new BridgeRefusal("bad_input", "invalid proposed board task key");
    }
    return { key, after: parseCursor(after) };
  }, run: ({ key, after }) => boardPlanTask(board?.proposed(), key, after) });
  if (board && !unavailable) registry.register({ name: "board_refresh", parse: () => undefined, run: async () => {
    if (!source.ready()) throw new BridgeRefusal("host_not_ready", "Session is not accepting board actions");
    await board.refresh();
    return boardPart(board);
  } });
}
