import { describe, expect, test } from "bun:test";
import { ownerRun, json, type Run } from "../src/build/corral-cli.ts";
import { createExit } from "../src/build/exit.ts";
import { summarize } from "../src/build/git.ts";
import type { BuildContext, ExitControl, ExitPolicy, ExitRecord, FleetSnapshot } from "../src/build/types.ts";

const head = "a".repeat(40), base = "b".repeat(40);
const fleet: FleetSnapshot = { children: [], maxParallel: 3 };
function context(policy: ExitPolicy = "gate"): BuildContext {
  return { corral: "corral", dir: "/sessions/B-one", settings: { exitPolicy: policy, maxParallel: 3, profiles: [] }, session: {
    version: 1, id: "B-one", name: "Build parser", created: "2026-01-01T00:00:00Z", updated: "2026-01-01T00:00:00Z",
    repoRoot: "/repo", worktree: "/repo/.worktrees/build-parser", workspaceRoot: "/workspace", project: "app",
    here: false, branch: "build/parser", base,
  } };
}
function git(dirty = false, commits = 2, calls: string[][] = []): Run {
  return async argv => {
    calls.push([...argv]);
    const stdout = argv[0] === "rev-parse" ? (argv[1] === "HEAD" ? head : "build/parser")
      : argv[0] === "rev-list" ? String(commits) : argv[0] === "diff" ? "4\t2\tsrc/parser.ts\n-\t-\timage.png\n"
      : argv[0] === "status" ? (dirty ? " M src/parser.ts\n" : "") : "";
    return { code: 0, stdout, stderr: "" };
  };
}
function scripted(steps: readonly { argv: readonly string[]; value?: unknown; error?: string }[], events: string[]): Run {
  let index = 0;
  return async (argv, options) => {
    const step = steps[index++];
    expect(options.cwd).toBe("/workspace");
    expect(argv).toEqual(step?.argv);
    events.push(argv.join(" "));
    return { code: step?.error ? 1 : 0, stdout: step?.value === undefined ? "" : JSON.stringify(step.value), stderr: step?.error ?? "" };
  };
}
function submission() {
  return [
    { argv: ["assistant", "create", "--profile", "pi", "--json"], value: { id: "A-42" } },
    { argv: ["assistant", "scratch", "new", "--project", "app", "--slug", "build-b-one", "--base", base, "--session", "A-42", "--json"],
      value: { path: "/repo/.worktrees/A-42-build-b-one", branch: "corral/A-42/build-b-one", base } },
    { argv: ["assistant", "scratch", "finish", "build-b-one", "--title", "Build parser", "--intent", "ship", "--expected-head", head, "--session", "A-42", "--json"],
      value: { task: "T-7", run: "R1", gate: "queued" } },
  ];
}
async function settled(control: ExitControl) {
  if (["landed", "failed"].includes(control.state().phase)) return;
  const { promise, resolve } = Promise.withResolvers<void>();
  const unsubscribe = control.subscribe(() => {
    if (["landed", "failed"].includes(control.state().phase)) { unsubscribe(); resolve(); }
  });
  await promise;
}

describe("build exits", () => {
  test("ship threads assistant and scratch identities through the fast-forward and finish", async () => {
    const events: string[] = [], records: ExitRecord[] = [];
    const underlyingGit = git();
    const control = createExit({ context: context(), run: scripted(submission(), events),
      git: async (argv, options) => {
        if (argv[0] === "-C") {
          events.push(argv.join(" "));
          expect(options.cwd).toBe("/workspace");
          expect(argv).toEqual(["-C", "/repo/.worktrees/A-42-build-b-one", "merge", "--ff-only", head]);
        }
        return underlyingGit(argv, options);
      }, fleet: () => fleet, save: value => { records.push(value); }, changed() {} });
    try {
      const first = control.choose("ship");
      expect(control.choose("land")).toBe(first);
      expect((await first).phase).toBe("shipped");
      expect(events.map(event => event.split(" ").slice(0, 3).join(" "))).toEqual([
        "assistant create --profile", "assistant scratch new", "-C /repo/.worktrees/A-42-build-b-one merge", "assistant scratch finish",
      ]);
      expect(records.at(-1)).toMatchObject({ choice: "ship", task: "T-7", assistant: "A-42" });
      await control.choose("ship");
      expect(events).toHaveLength(4);
    } finally { control.close(); }
  });

  test("finish refusal retains the named assistant and scratch without deleting either", async () => {
    const steps = submission();
    const events: string[] = [];
    const run = scripted([...steps.slice(0, 2), { argv: steps[2]!.argv, error: "scratch_attached: already attached; nothing finished" }], events);
    const control = createExit({ context: context(), run, git: git(), fleet: () => fleet, save() {}, changed() {} });
    try {
      const state = await control.choose("ship");
      expect(state.phase).toBe("failed");
      expect(state.detail).toContain("scratch_attached");
      expect(state.detail).toContain("A-42");
      expect(state.detail).toContain("/repo/.worktrees/A-42-build-b-one");
      expect(events).toHaveLength(3);
    } finally { control.close(); }
  });

  test("land waives review, syncs and observes a later landed receipt", async () => {
    const events: string[] = [], records: ExitRecord[] = [];
    const run = scripted([...submission(),
      { argv: ["waive-review", "T-7", "--reason", "build session exit: project policy requires the gate only"] },
      { argv: ["sync"] },
      { argv: ["show", "T-7", "--json"], value: { id: "T-7", status: "review", runs: [{ gate: { result: "running" } }] } },
      { argv: ["show", "T-7", "--json"], value: { id: "T-7", status: "done", landed: { sha: head } } },
    ], events);
    const control = createExit({ context: context(), run, git: git(), fleet: () => fleet, save: record => { records.push(record); }, changed() {}, pollMs: 1 });
    try {
      await control.choose("land"); await settled(control);
      expect(control.state().phase).toBe("landed");
      expect(records.at(-1)).toMatchObject({ choice: "land", task: "T-7", assistant: "A-42", reviewWaived: true });
      expect(events).toHaveLength(7);
    } finally { control.close(); }
  });

  test("outside-workspace exit lifecycle and reads remain usable without corral", async () => {
    const initial = context("review");
    let commands = 0;
    const control = createExit({ context: { ...initial, session: { ...initial.session, workspaceRoot: undefined } },
      run: async () => { commands++; throw new Error("Unexpected corral command"); }, git: git(),
      fleet: () => fleet, save() {}, changed() {} });
    const unsubscribe = control.subscribe(() => {});
    expect(control.state().phase).toBe("none");
    control.open();
    expect(control.state().phase).toBe("pending");
    await control.summary();
    const options = control.options();
    for (const choice of ["land", "ship"]) {
      expect(options.find(option => option.choice === choice)?.unavailable).toBe("This session is outside a corral workspace.");
    }
    expect(options.filter(option => !option.unavailable).map(option => option.choice)).toEqual(["keep", "iterate"]);
    expect((await control.choose("ship")).phase).toBe("failed");
    expect((await control.choose("iterate")).phase).toBe("none");
    expect((await control.choose("keep")).phase).toBe("kept");
    unsubscribe();
    control.close();
    control.close();
    expect(control.state().phase).toBe("kept");
    expect(control.options().map(option => option.choice)).toEqual(["keep", "land", "ship", "iterate"]);
    expect(commands).toBe(0);
  });

  test("policy, empty branches and dirty worktrees disable the appropriate exits", async () => {
    for (const [policy, dirty, commits, expected] of [
      ["review", false, 1, ["land"]], ["none", false, 1, ["land", "ship"]],
      ["gate", true, 1, ["land", "ship"]], ["gate", false, 0, ["land", "ship"]],
    ] as const) {
      const control = createExit({ context: context(policy), run: async () => { throw new Error("Unexpected corral command"); },
        git: git(dirty, commits), fleet: () => fleet, save() {}, changed() {} });
      try {
        await control.summary();
        expect(control.options().filter(option => option.unavailable).map(option => option.choice)).toEqual([...expected]);
        expect(control.options().map(option => option.choice)).toEqual(["keep", "land", "ship", "iterate"]);
      } finally { control.close(); }
    }
  });

  test("here sessions cannot ship detached HEAD or the configured or implicit base", async () => {
    for (const [branch, integration, blocked] of [[null, "main", true], ["main", "main", true], ["feature", undefined, true], ["feature", "main", false]] as const) {
      const initial = context();
      const control = createExit({ context: { ...initial, session: { ...initial.session, here: true, branch } }, integrationBranch: async () => integration,
        run: async () => { throw new Error("Unexpected corral command"); }, git: git(), fleet: () => fleet, save() {}, changed() {} });
      try { await control.summary(); expect(Boolean(control.options().find(option => option.choice === "ship")?.unavailable)).toBe(blocked); }
      finally { control.close(); }
    }
  });

  test("here sessions read the selected projects table from the owner CLI", async () => {
    const initial = context();
    const control = createExit({ context: { ...initial, session: { ...initial.session, here: true, branch: "main" } },
      run: async (argv, options) => {
        expect(argv).toEqual(["--root", "/workspace", "config", "get", "projects"]);
        expect(options.cwd).toBe("/workspace");
        return { code: 0, stdout: '[app]\nbase = "main"\n', stderr: "" };
      }, git: git(), fleet: () => fleet, save() {}, changed() {} });
    try { await control.summary(); expect(control.options().find(option => option.choice === "ship")?.unavailable).toContain("base branch"); }
    finally { control.close(); }
  });

  test("land fails visibly when the gate fails or Review ends without a landing", async () => {
    for (const readback of [{ status: "review", runs: [{ gate: { result: "fail", failed_cmd: "test parser" } }] }, { status: "needs_me", runs: [] }]) {
      const control = createExit({ context: context(), run: scripted([...submission(),
        { argv: ["waive-review", "T-7", "--reason", "build session exit: project policy requires the gate only"] },
        { argv: ["sync"] }, { argv: ["show", "T-7", "--json"], value: readback },
      ], []), git: git(), fleet: () => fleet, save() {}, changed() {}, pollMs: 1 });
      try { await control.choose("land"); await settled(control); expect(control.state().phase).toBe("failed"); expect(control.state().detail).toContain("T-7"); }
      finally { control.close(); }
    }
  });

  test("summary includes working-tree numstat and fleet ownership deviations", async () => {
    const calls: string[][] = [];
    const child: FleetSnapshot["children"][number] = {
      id: "child-1", call: "call-1", title: "Parser", brief: "Build parser", owns: ["src/parser.ts"], after: [],
      profile: "default", model: "provider/model", state: "done", createdAt: 1000, startedAt: 1000, endedAt: 4000,
      edits: [{ path: "src/parser.ts", outside: false, at: 2000 }, { path: "docs/usage.md", outside: true, at: 3000 }],
      usage: { input: 10, output: 20, cost: 0.25, turns: 1 }, steers: [], pulse: [], history: "d", messages: [],
    };
    const summary = await summarize(git(true, 2, calls), context().session, { ...fleet, children: [child] });
    expect(summary).toMatchObject({ head, base, commits: 2, files: 2, added: 4, removed: 2, dirty: true });
    expect(calls.find(argv => argv[0] === "diff")).toEqual(["diff", "--numstat", base, "--"]);
    expect(summary.slices).toEqual([{ id: "child-1", state: "done", seconds: 3, cost: 0.25 }]);
    expect(summary.outside).toEqual([{ path: "docs/usage.md", by: ["child-1"] }]);
  });
});

test("ownerRun removes every Rust owner-guard agent marker from the child, not the parent", async () => {
  const keys = ["AI_AGENT", "PI_CODING_AGENT", "CORRAL_ASSISTANT", "CORRAL_PM"];
  const old = keys.map(key => process.env[key]);
  try {
    for (const key of keys) process.env[key] = "1";
    const result = await ownerRun(process.execPath)(["-e", `console.log(JSON.stringify(${JSON.stringify(keys)}.filter(key => process.env[key] !== undefined)))`], { cwd: process.cwd() });
    expect(result.code).toBe(0); expect(JSON.parse(result.stdout)).toEqual([]);
    expect(keys.every(key => process.env[key] === "1")).toBe(true);
  } finally { keys.forEach((key, index) => { if (old[index] === undefined) delete process.env[key]; else process.env[key] = old[index]; }); }
});

test("JSON parse failures include corral's stderr", async () => {
  await expect(json(async () => ({ code: 0, stdout: "not JSON", stderr: "command diagnostic" }), ["show", "T-1", "--json"], "/workspace")).rejects.toThrow("command diagnostic");
});
