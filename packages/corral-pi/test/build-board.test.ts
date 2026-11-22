import { expect, test } from "bun:test";
import { mkdtempSync, readFileSync, rmSync, statSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { boardTask, createBoard, validatePlan } from "../src/build/board.ts";
import type { Run, TaskReadback } from "../src/build/corral-cli.ts";
import type { BoardPlan, BuildContext } from "../src/build/types.ts";

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

test("confirmation files parent and dependency-ordered tasks with new; Ready waits for complete bodies", async () => {
  const root = mkdtempSync(join(tmpdir(), "build-board-"));
  const known = new Map<string, TaskReadback>([["T-100", { id: "T-100", title: "Existing", status: "done" }]]);
  const calls: string[][] = [];
  const files: string[] = [];
  const bodies = new Map<string, string>();
  let counter = 0;
  const run: Run = async (argv, { cwd }) => {
    expect(cwd).toBe(root);
    expect(argv.slice(0, 3)).toEqual(["--root", root, "--json"]);
    const args = argv.slice(3);
    calls.push(args);
    const [command, id] = args;
    let task = known.get(id!);
    if (command === "new") {
      const depends = args.indexOf("--depends-on");
      task = { id: `T-${++counter}`, title: args.at(-1)!, status: "backlog",
        depends_on: depends >= 0 ? args[depends + 1]!.split(",") : [] };
    } else if (command === "edit") {
      const path = args[args.indexOf("--body-file") + 1]!;
      expect(statSync(path).mode & 0o777).toBe(0o600);
      files.push(path);
      bodies.set(id!, readFileSync(path, "utf8"));
    } else if (command === "move") {
      expect(bodies.has(id!)).toBeTrue();
      task = { ...task!, status: "ready" };
    } else expect(command).toBe("show");
    if (!task) throw new Error(`Unexpected argv: ${args.join(" ")}`);
    known.set(task.id, task);
    return { code: 0, stdout: JSON.stringify({ task }), stderr: "" };
  };
  const board = createBoard({ context: context(root), run, tmpDir: join(root, "board"), changed() {} });
  try {
    board.propose(plan);
    expect(await board.preview(plan)).toMatchObject({ tracker: plan.title, tasks: [{ id: "test" }, { id: "parser" }] });
    expect(calls).toEqual([]);
    const tasks = await board.file(plan, true);
    expect(calls.filter(argv => argv[0] === "new")).toEqual([
      ["new", "--project=app", "--", "Parser follow-ons"],
      ["new", "--project=app", "--parent", "T-1", "--kind=feature", "--size=s", "--", "Extract parser"],
      ["new", "--project=app", "--parent", "T-1", "--depends-on", "T-2,T-100", "--", "Add tests"],
    ]);
    expect(calls.find(argv => argv[0] === "edit" && argv[1] === "T-3")?.at(-1)).toBe("--profile=pi-test");
    expect([...bodies.values()]).toEqual([plan.why, plan.tasks[1]!.body, plan.tasks[0]!.body]);
    for (const path of files) expect(() => statSync(path)).toThrow();
    expect(tasks.map(task => [task.id, task.phase])).toEqual([["T-2", "queued"], ["T-3", "waiting"]]);
    known.set("T-2", { ...known.get("T-2")!, status: "done" });
    await board.refresh();
    expect(board.tasks().map(task => [task.id, task.phase])).toEqual([["T-2", "landed"], ["T-3", "queued"]]);
  } finally { board.close(); rmSync(root, { recursive: true, force: true }); }
});

test("outside-workspace lifecycle is safe while filing and preview refuse", async () => {
  const board = createBoard({ context: context(), tmpDir: "/unused", run: async () => { throw new Error("must not run"); }, changed() {} });
  board.propose(plan);
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

test("plan tag validation rejects malformed labels and plans whose tags cannot fit the preview", () => {
  const task = plan.tasks[0]!;
  for (const tag of ["", " padded", "padded\u00a0", "line\nbreak", "control\u0085inside"]) {
    expect(() => validatePlan({ ...plan, tasks: [{ ...task, depends: [], tags: [tag] }] })).toThrow("Invalid tag");
  }
  expect(() => validatePlan({ ...plan, tasks: [{ ...task, depends: [], tags: ['"'.repeat(4096)] }] })).toThrow("preview budget");
});
