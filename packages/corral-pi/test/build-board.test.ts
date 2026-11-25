import { expect, test } from "bun:test";
import { mkdtempSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { boardLabel, boardTask, createBoard, validatePlan } from "../src/build/board.ts";
import type { Run, TaskReadback } from "../src/build/corral-cli.ts";
import type { BoardPlan, BuildContext } from "../src/build/types.ts";
import { previewLines } from "../src/ui/build/build-dialogs.ts";

function context(workspaceRoot?: string): BuildContext {
  return { corral: "corral-next", dir: "/sessions/B-one", settings: { exitPolicy: "review", maxParallel: 3, profiles: [] }, session: {
    version: 1, id: "B-one", name: "parser", created: "2026-01-01T00:00:00Z", updated: "2026-01-01T00:00:00Z",
    repoRoot: "/repo", worktree: "/repo/.worktrees/build-parser", workspaceRoot, project: "app",
    here: false, branch: "build/parser", base: "base",
  } };
}

const plan: BoardPlan = { title: "Parser follow-ons", why: "Separate remaining work.", tasks: [
  { key: "test", title: "Add tests", depends: ["parser", "T-100"], body: "Test multiline input\nDo not change parser", profile: "pi-test" },
  { key: "parser", title: "Extract parser", depends: [], body: "Extract syntax", kind: "feature", size: "s" },
] };

test("an unacknowledged filing retries the same ID, bytes and owner choice after restart", async () => {
  const root = mkdtempSync(join(tmpdir(), "build-board-"));
  const calls: { args: readonly string[]; input?: string }[] = [];
  let attempts = 0;
  const run: Run = async (args, { input }) => {
    calls.push({ args, input });
    if (args[4] === "check") return { code: 0, stdout: "null", stderr: "" };
    expect(args[4]).toBe("file");
    if (++attempts === 1) return { code: 1, stdout: "", stderr: "lost reply" };
    const tasks = [
      { key: "test", project: "app", id: "T-2", status: "ready", depends_on: ["T-3", "T-100"] },
      { key: "parser", project: "app", id: "T-3", status: "ready", depends_on: [] },
    ];
    return { code: 0, stdout: JSON.stringify({ tasks }), stderr: "" };
  };
  const open = () => createBoard({ context: context(root), run, tmpDir: join(root, "board"), changed() {} });
  let board = open();
  try {
    const first = board.propose(plan);
    await expect(board.propose({ ...plan, title: "Replacement" })).rejects.toThrow("already pending");
    await first;
    const proposal = board.proposed()!;
    await expect(board.file(proposal, true)).rejects.toThrow("lost reply");
    expect(board.pendingChoice?.()).toBe(true);
    board.close();
    board = open();
    expect(board.proposed()).toEqual(proposal);
    const restored = board.proposed()!;
    await expect(board.file(restored, false)).rejects.toThrow("original filing choice");
    expect(() => board.dismiss()).toThrow("original filing choice");
    const [tasks, double] = await Promise.all([board.file(restored, true), board.file(restored, true)]);
    expect(double).toEqual(tasks);
    expect(tasks.map(task => [task.key, task.project, task.id, task.phase])).toEqual([
      ["test", "app", "T-2", "waiting"], ["parser", "app", "T-3", "queued"],
    ]);
    expect(await board.file(restored, true)).toEqual(tasks);
    expect(calls).toHaveLength(3);
    expect(calls[1]).toEqual(calls[2]);
    expect(board.proposed()).toBeUndefined();
    board.close();
    board = open();
    expect(board.tasks()).toEqual(tasks);
  } finally { board.close(); rmSync(root, { recursive: true, force: true }); }
});

test("outside-workspace lifecycle is safe while filing and preview refuse", async () => {
  const board = createBoard({ context: context(), tmpDir: "/unused", run: async () => { throw new Error("must not run"); }, changed() {} });
  await expect(board.propose(plan)).rejects.toThrow("workspace");
  expect(board.proposed()).toBeUndefined();
  await board.refresh();
  expect(board.tasks()).toEqual([]);
  await expect(board.preview(plan)).rejects.toThrow("workspace");
  await expect(board.file(plan, true)).rejects.toThrow("workspace");
  board.close();
});

test("board phases read simple engine task states", () => {
  const task: TaskReadback = { id: "T-1", title: "Work", status: "backlog" };
  const variants = [["backlog", "backlog"], ["ready", "queued"], ["review", "reviewing"],
    ["running", "running"], ["blocked", "blocked"], ["done", "landed"], ["dropped", "failed"]];
  for (const [status, phase] of variants) expect(boardTask({ ...task, status: status! }, new Map(), 100).phase).toBe(phase!);
  expect(boardTask({ ...task, status: "blocked", blocked: { reason: "Question" } }, new Map(), 100).detail).toBe("Question");
});

test("plan validation accepts existing tasks and rejects duplicate keys, unknown dependencies, cycles and oversized plans", () => {
  expect(() => validatePlan(plan)).not.toThrow();
  const [first, second] = plan.tasks;
  expect(() => validatePlan({ ...plan, tasks: [first!, first!] })).toThrow("unique");
  expect(() => validatePlan({ ...plan, tasks: [{ ...second!, depends: ["unknown"] }] })).toThrow("Unknown dependency");
  expect(() => validatePlan({ ...plan, tasks: [first!, { ...second!, depends: ["test"] }] })).toThrow("cycle");
  expect(() => validatePlan({ ...plan, tasks: Array.from({ length: 21 }, (_, i) => ({ ...second!, key: `key-${i}` })) })).toThrow("at most 20");
});

test("board keys remain distinct on surfaces that collapse whitespace or interpret bidi", () => {
  const keys = ["api spec", "api  spec", "api\u202espec"];
  const labels = keys.map(key => boardLabel(key));
  expect(labels.map(label => JSON.parse(label))).toEqual(keys);
  expect(new Set(labels.map(label => label.replace(/\s+/g, " "))).size).toBe(keys.length);
  expect(labels.join("")).not.toContain("\u202e");
  const proposal = { ...plan, project: "app", tasks: keys.map(key => ({ key, title: "Same title", depends: [], body: "" })) };
  const preview = { tracker: proposal.title, tasks: proposal.tasks.map(task => ({ ...task, id: task.key })) };
  const shown = previewLines(proposal, preview, 120, 22).join("\n");
  for (const label of labels) expect(shown).toContain(label);
});

test("plan tag validation rejects malformed labels and plans whose tags cannot fit the preview", () => {
  const task = plan.tasks[0]!;
  for (const tag of ["", " padded", "padded\u00a0", "line\nbreak", "control\u0085inside"]) {
    expect(() => validatePlan({ ...plan, tasks: [{ ...task, depends: [], tags: [tag] }] })).toThrow("Invalid tag");
  }
  expect(() => validatePlan({ ...plan, tasks: [{ ...task, depends: [], tags: ['"'.repeat(4096)] }] })).toThrow("preview budget");
});
