import { describe, expect, test } from "bun:test";
import { mkdtemp, readFile, rm, stat } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { boardTask, createBoard, planFile, validatePlan } from "../src/build/board.ts";
import type { Run, TaskReadback } from "../src/build/corral-cli.ts";
import type { BoardPlan, BuildContext } from "../src/build/types.ts";

// Read the emitted YAML subset: indented mappings/lists whose values are JSON-quoted scalars.
function quotedFrontmatter(text: string): { title: string; project: string; tasks: Record<string, unknown>[] } {
  const result = { title: "", project: "", tasks: [] as Record<string, unknown>[] };
  let task: Record<string, unknown> | undefined;
  for (const line of text.split("\n")) {
    if (line === "tasks:") continue;
    const match = /^( *)(- )?([a-z]+): (.*)$/.exec(line);
    if (!match) throw new Error(`Unexpected plan frontmatter: ${line}`);
    if (match[1] === "") {
      if (match[3] !== "title" && match[3] !== "project") throw new Error("Unexpected document key");
      result[match[3]] = JSON.parse(match[4]!) as string;
    } else {
      if (match[2]) { task = {}; result.tasks.push(task); }
      if (!task) throw new Error("Task field outside a task");
      task[match[3]!] = JSON.parse(match[4]!);
    }
  }
  return result;
}

function context(workspaceRoot: string | undefined = "/workspace"): BuildContext {
  return { corral: "corral", dir: "/sessions/B-one", settings: { exitPolicy: "review", maxParallel: 3, profiles: [] }, session: {
    version: 1, id: "B-one", name: "Parser", created: "2026-01-01T00:00:00Z", updated: "2026-01-01T00:00:00Z",
    repoRoot: "/repo", worktree: "/repo/.worktrees/build-parser", workspaceRoot, project: "app", here: false,
    branch: "build/parser", base: "a".repeat(40),
  } };
}
const plan: BoardPlan = {
  title: "Parser follow-ons", why: "Separate remaining work.",
  tasks: [
    { key: "consumer", title: "Consumer", body: "Consume the parser", depends: ["parser"] },
    { key: "independent", title: "Independent", body: "Independent work", depends: [] },
    { key: "parser", title: "Parser", body: "Implement parser", depends: [] },
  ],
};
const imported = {
  tracker: { id: "T-10", title: plan.title, status: "backlog" }, dry_run: false,
  tasks: [
    { id: "T-11", title: "Consumer", status: "backlog", depends_on: ["T-13"] },
    { id: "T-12", title: "Independent", status: "backlog", depends_on: [] },
    { id: "T-13", title: "Parser", status: "backlog", depends_on: [] },
  ],
};

describe("board import", () => {
  test("plan frontmatter round-trips quoted titles and arbitrary multiline task bodies", () => {
    const titles = ["a: b", 'a "quoted" title', "- leading dash", "---\nnext line", "it's a title", "a\u0085b\u2028c\u2029d"];
    for (const title of titles) {
      const body = '- first: "quoted"\n---\ncolon: value\n\nlast line\n';
      const input: BoardPlan = { title, project: 'app: "one"', why: "- why: \"yes\"\n---\nremaining body\n", tasks: [{ key: "one", title, body, depends: ["T-2"] }] };
      const output = planFile(input);
      const boundary = output.indexOf("\n---\n", 4);
      const frontmatter = quotedFrontmatter(output.slice(4, boundary));
      expect(frontmatter.title).toBe(title);
      expect(frontmatter.project).toBe(input.project);
      expect(frontmatter.tasks).toEqual(input.tasks);
      expect(output.slice(boundary + 5)).toBe(input.why);
    }
  });

  test("preview uses private files, dry-run and the tracker's object id", async () => {
    const dir = await mkdtemp(join(tmpdir(), "corral-build-board-"));
    const calls: string[][] = [];
    const run: Run = async (argv, options) => {
      calls.push([...argv]);
      expect(options.cwd).toBe("/workspace");
      expect(argv[0]).toBe("import");
      expect(argv.slice(2)).toEqual(["--dry-run", "--json"]);
      expect((await stat(argv[1]!)).mode & 0o777).toBe(0o600);
      expect(await readFile(argv[1]!, "utf8")).toContain('project: "app"');
      return { code: 0, stdout: JSON.stringify({ ...imported, dry_run: true }), stderr: "" };
    };
    const board = createBoard({ context: context(), run, tmpDir: dir, changed() {} });
    try {
      board.propose(plan);
      expect(board.proposed()).toEqual(plan);
      const preview = await board.preview(plan);
      expect(preview.tracker).toBe("T-10");
      expect(preview.tasks.map(task => [task.key, task.id, task.depends])).toEqual([
        ["consumer", "T-11", ["T-13"]], ["independent", "T-12", []], ["parser", "T-13", []],
      ]);
      expect(board.proposed()).toEqual(plan);
      board.dismiss(); expect(board.proposed()).toBeUndefined();
      expect(calls).toHaveLength(1);
    } finally { board.close(); await rm(dir, { recursive: true, force: true }); }
  });

  test("filing queues dependencies first and keeps going after a task refusal", async () => {
    const dir = await mkdtemp(join(tmpdir(), "corral-build-board-"));
    const queued: string[] = [];
    const run: Run = async (argv, options) => {
      expect(options.cwd).toBe("/workspace");
      if (argv[0] === "import") {
        expect(argv.slice(2)).toEqual(["--origin", "owner", "--json"]);
        return { code: 0, stdout: JSON.stringify(imported), stderr: "" };
      }
      expect(argv.slice(0, 2)).toEqual(["queue", "add"]);
      const id = argv[2]!; queued.push(id);
      return { code: id === "T-13" ? 1 : 0, stdout: "", stderr: id === "T-13" ? "capacity_refused: no available slot" : "" };
    };
    const board = createBoard({ context: context(), run, tmpDir: dir, changed() {}, pollMs: 60_000 });
    try {
      board.propose(plan);
      const statuses = await board.file(plan, true);
      expect(queued).toEqual(["T-13", "T-11", "T-12"]);
      expect(statuses.find(task => task.id === "T-13")?.detail).toContain("capacity_refused");
      expect(statuses.find(task => task.id === "T-13")?.phase).toBe("backlog");
      expect(statuses.find(task => task.id === "T-11")?.phase).toBe("waiting");
      expect(statuses.find(task => task.id === "T-12")?.phase).toBe("queued");
      expect(board.proposed()).toBeUndefined();
    } finally { board.close(); await rm(dir, { recursive: true, force: true }); }
  });

  test("refresh follows filed tasks and checks dependencies rather than assuming Ready is runnable", async () => {
    const dir = await mkdtemp(join(tmpdir(), "corral-build-board-"));
    const shown: string[] = [];
    let landed = false;
    const run: Run = async argv => {
      if (argv[0] === "import") return { code: 0, stdout: JSON.stringify(imported), stderr: "" };
      expect(argv[0]).toBe("show");
      shown.push(argv[1]!);
      const value = imported.tasks.find(task => task.id === argv[1])!;
      return { code: 0, stdout: JSON.stringify({ ...value, status: "ready", ...(landed ? { landed: { sha: "a".repeat(40) } } : {}) }), stderr: "" };
    };
    const board = createBoard({ context: context(), run, tmpDir: dir, changed() {}, pollMs: 60_000 });
    try {
      await board.file(plan, false);
      await board.refresh();
      expect(board.tasks().find(task => task.id === "T-11")?.phase).toBe("waiting");
      landed = true; await board.refresh();
      expect(board.tasks().every(task => task.phase === "landed")).toBe(true);
      expect(shown).toEqual(["T-11", "T-12", "T-13", "T-11", "T-12", "T-13"]);
    } finally { board.close(); await rm(dir, { recursive: true, force: true }); }
  });

  test("every outside-workspace entry rejects before running corral", async () => {
    const outside = createBoard({ context: { ...context(), session: { ...context().session, workspaceRoot: undefined } },
      run: async () => { throw new Error("Unexpected corral command"); }, tmpDir: "/unused", changed() {} });
    for (const action of [() => outside.tasks(), () => outside.proposed(), () => outside.propose(plan), () => outside.dismiss(), () => outside.subscribe(() => {}), () => outside.refresh(), () => outside.file(plan, true), () => outside.close()]) {
      expect(action).toThrow("Board plans require a corral workspace.");
    }
    await expect(outside.preview(plan)).rejects.toThrow("Board plans require a corral workspace.");
  });
});

test("board phase projection honors current runs, blockers and landing receipts", () => {
  const task: TaskReadback = { id: "T-1", title: "Work", status: "backlog" };
  const cases: [Partial<TaskReadback>, string][] = [
    [{ status: "backlog" }, "backlog"],
    [{ status: "ready", depends_on: ["T-2"] }, "waiting"],
    [{ status: "ready" }, "queued"],
    [{ status: "running" }, "running"],
    [{ status: "review", runs: [{ autopilot: { stage: "gating" } }] }, "gating"],
    [{ status: "review", runs: [{ autopilot: { stage: "reviewing" } }] }, "reviewing"],
    [{ status: "done", landed: { sha: "abc" } }, "landed"],
    [{ status: "ready", automation_blocker: "Paused by owner" }, "blocked"],
    [{ status: "review", runs: [{ gate: { result: "fail", failed_cmd: "test" } }] }, "failed"],
    [{ status: "review", landed: { sha: "abc" }, runs: [{ gate: { result: "fail" } }] }, "landed"],
    [{ status: "running", runs: [{ gate: { result: "fail" } }, {}] }, "running"],
  ];
  for (const [fields, phase] of cases) expect(boardTask({ ...task, ...fields }, new Map(), 100).phase).toBe(phase);
  expect(boardTask({ ...task, status: "ready", depends_on: ["T-2"] }, new Map([["T-2", { id: "T-2", title: "Done", status: "done" }]]), 100).phase).toBe("queued");
});

test("plan validation accepts existing tasks and rejects duplicate keys, unknown dependencies, cycles and oversized plans", () => {
  expect(() => validatePlan({ ...plan, tasks: [{ ...plan.tasks[0]!, depends: ["T-9"] }] })).not.toThrow();
  expect(() => validatePlan({ ...plan, tasks: [plan.tasks[0]!, plan.tasks[0]!] })).toThrow("unique");
  expect(() => validatePlan({ ...plan, tasks: [{ ...plan.tasks[0]!, depends: ["missing"] }] })).toThrow("Unknown dependency");
  expect(() => validatePlan({ ...plan, tasks: [{ ...plan.tasks[0]!, depends: ["consumer"] }] })).toThrow("cycle");
  expect(() => validatePlan({ ...plan, tasks: Array.from({ length: 21 }, (_, index) => ({ ...plan.tasks[0]!, key: String(index), depends: [] })) })).toThrow("20");
});
