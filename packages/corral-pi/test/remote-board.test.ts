import { expect, test } from "bun:test";
import { SESSION_PART_MAX } from "../src/bridge/protocol.ts";
import type { BuildRecord } from "../src/build/record-types.ts";
import type { BoardPlan } from "../src/build/types.ts";
import { boardPart, recordPart } from "../src/host/remote/board.ts";

const at = "2026-01-01T00:00:00Z";

test("a clipped waiting board plan keeps every task key and its original body length", () => {
  const body = "大\0\n".repeat(7_000);
  const plan: BoardPlan = { title: "A plan", project: "project", why: body, tasks: Array.from({ length: 20 }, (_, i) => ({
    key: `task-${i}`, title: `Task ${i}`, depends: i === 0 ? [] : [`task-${i - 1}`], body,
  })) };
  const part = boardPart({ proposed: () => plan, tasks: () => [{ id: "T-123", title: "Filed", status: "blocked", phase: "blocked",
    depends: ["T-122"], detail: "blocked by T-122", updatedAt: Date.parse(at) }] });
  expect(part.proposed?.tasks.map(task => task.key)).toEqual(plan.tasks.map(task => task.key));
  expect(part.proposed?.tasks.every(task => task.body_bytes === Buffer.byteLength(body) && Buffer.byteLength(task.body) <= 2048)).toBeTrue();
  expect(part.tasks[0]).toMatchObject({ id: "T-123", phase: "blocked", depends: ["T-122"], updated_at: new Date(at).toISOString() });
  expect(Buffer.byteLength(JSON.stringify(part))).toBeLessThanOrEqual(SESSION_PART_MAX);
});

test("a build record keeps failed verification, resolved findings, decisions, and timeline in the wire projection", () => {
  const head = "a".repeat(40), parent = { input: 3, output: 6, cost: 0.25 }, empty = { input: 0, output: 0 };
  const record: BuildRecord = { version: 1, id: "B-x", kind: "build", title: "App parity", created: at, updated: at, intent: [], repo_root: "/repo",
    worktree: "/repo/build", branch: "build/app", base: null, outcome: { state: "open" }, commits: [], slices: [], messages: [],
    verification: [{ attempt: "G1", head, result: "fail", failed_command: "gate", started: at, ended: at }],
    reviews: [{ round: 1, reviewer: "r", model: "provider/model", head, verdict: "changes", at,
      findings: [{ id: "F1", severity: "major", text: "Race", resolution: { state: "fixed", commit: head } }] }],
    decisions: [{ at, kind: "question", text: "Question", answer: "Yes" }], timeline: [{ at, phase: "verify" }],
    usage: { parent, children: empty, reviewers: empty, total: parent }, transcripts: { children: {} } };
  const part = JSON.parse(JSON.stringify(recordPart(record)));
  expect(part.verification).toEqual([{ attempt: "G1", tier: "full", head, result: "fail", failed_command: "gate", started: at, ended: at }]);
  expect(part.reviews[0].findings[0].resolution).toEqual({ state: "fixed", commit: head });
  expect(part.decisions).toEqual(record.decisions);
  expect(part.timeline).toEqual(record.timeline);
  expect(part.usage.total).toEqual({ input: 3, output: 6, cost_usd: 0.25 });
});
