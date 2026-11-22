import { expect, test } from "bun:test";
import { execFileSync } from "node:child_process";
import { mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { BridgeRefusal, SESSION_PART_MAX } from "../src/bridge/protocol.ts";
import { validatePlan } from "../src/build/board.ts";
import { gitRun } from "../src/build/corral-cli.ts";
import { createJournal } from "../src/build/record.ts";
import type { BuildRecord } from "../src/build/record-types.ts";
import type { BoardPlan, BuildContext } from "../src/build/types.ts";
import { boardPart, recordPart, registerBoard } from "../src/host/remote/board.ts";
import { RemoteRegistry } from "../src/host/remote/registry.ts";
import { capabilities } from "../src/host/remote/state.ts";

const at = "2026-01-01T00:00:00Z";

test("a clipped waiting board plan keeps every task key, tag, and original body length", () => {
  const body = "大\0\n".repeat(7_000);
  const tags = ['"'.repeat(190), "bug"];
  const plan = validatePlan({ title: "A plan", project: "project", why: body, tasks: Array.from({ length: 20 }, (_, i) => ({
    key: `task-${i}`, title: `Task ${i}`, depends: i === 0 ? [] : [`task-${i - 1}`], body, tags,
  })) });
  const part = boardPart({ proposed: () => plan, tasks: () => [{ id: "T-123", title: "Filed", status: "blocked", phase: "blocked",
    depends: ["T-122"], detail: "blocked by T-122", updatedAt: Date.parse(at) }] });
  expect(part.proposed?.tasks.map(({ key, tags }) => ({ key, tags }))).toEqual(plan.tasks.map(({ key, tags }) => ({ key, tags })));
  expect(part.proposed?.tasks.every(task => task.body_bytes === Buffer.byteLength(body) && Buffer.byteLength(task.body) <= 2048)).toBeTrue();
  expect(part.tasks[0]).toMatchObject({ id: "T-123", phase: "blocked", depends: ["T-122"], updated_at: new Date(at).toISOString() });
  expect(Buffer.byteLength(JSON.stringify(part))).toBeLessThanOrEqual(SESSION_PART_MAX);
});

function proposedBoard(initial: BoardPlan) {
  const registry = new RemoteRegistry();
  let proposed: BoardPlan | undefined = initial;
  registerBoard(registry, {
    board: { proposed: () => proposed, tasks: () => [], refresh: async () => {} },
    record: async () => { throw new Error("unexpected record read"); }, ready: () => true,
  });
  return { registry, replace(plan?: BoardPlan) { proposed = plan; } };
}

interface BriefPage { key: string; body: string; bytes: number; offset: number; next?: string }

test("a proposed task's brief longer than one page is served whole across pages, in order, on character boundaries", async () => {
  const body = "x".repeat(40 * 1024 - 1) + "😀" + "\0\"漢🙂\n".repeat(9000) + "last";
  const longKey = "task".repeat(60);
  const plan = { title: "Follow-up", why: "", tasks: [
    { key: longKey, title: "Implement", depends: [], body },
    { key: "@1", title: "Review", depends: [longKey], body: "Read the whole brief" },
  ] };
  const { registry } = proposedBoard(plan);
  const board = await registry.get("board")!({}) as { proposed: { tasks: { key: string; depends: string[]; body_bytes: number }[] } };
  const [first, second] = board.proposed.tasks;
  const key = first!.key, bytes = Buffer.byteLength(body);
  expect(first!.body_bytes).toBe(bytes);
  expect(key).not.toBe(second!.key);
  expect(second!.depends).toEqual([key]);
  expect(capabilities(registry, { open: false, closing: false, rewind: false }).some(capability => capability.id === "board_plan_task")).toBe(true);
  const parts: string[] = [];
  let after: string | undefined, offset = 0;
  do {
    const page = await registry.get("board_plan_task")!({ key, after }) as BriefPage;
    const size = Buffer.byteLength(page.body);
    expect(page).toMatchObject({ key, bytes, offset });
    expect(size).toBeGreaterThan(0);
    expect(size).toBeLessThanOrEqual(40 * 1024);
    expect(Buffer.byteLength(JSON.stringify(page))).toBeLessThanOrEqual(SESSION_PART_MAX);
    expect(Buffer.byteLength(page.next ?? "")).toBeLessThanOrEqual(128);
    expect(page.body).not.toContain("\uFFFD");
    parts.push(page.body);
    offset += size;
    expect(offset).toBeLessThanOrEqual(bytes);
    after = page.next;
  } while (after !== undefined);
  expect(parts.length).toBeGreaterThan(1);
  expect(parts.join("")).toBe(body);
});

test("a missing proposed task key or dismissed plan refuses a fresh brief read", () => {
  const { registry, replace } = proposedBoard({ title: "Follow-up", why: "", tasks: [{ key: "found", title: "Read", depends: [], body: "" }] });
  const read = registry.get("board_plan_task")!;
  expect(() => read({ key: "missing" })).toThrow(BridgeRefusal);
  for (const key of ["missing", "found"]) {
    if (key === "found") replace();
    try { read({ key }); }
    catch (error) {
      expect((error as BridgeRefusal).code).toBe("board_plan_task_missing");
      continue;
    }
    throw new Error("the absent task was served");
  }
});

test("a brief cursor cannot continue after the board proposal is replaced or dismissed", async () => {
  const plan = { title: "Follow-up", why: "", tasks: [{ key: "task", title: "Read", depends: [], body: "界".repeat(20_000) }] };
  const { registry, replace } = proposedBoard(plan);
  const read = registry.get("board_plan_task")!;
  const page = await read({ key: "task" }) as BriefPage;
  expect(page.next).toBeDefined();
  for (const replacement of [{ ...plan, title: "New proposal" }, undefined]) {
    replace(replacement);
    try { read({ key: "task", after: page.next }); }
    catch (error) {
      expect((error as BridgeRefusal).code).toBe("stale_page");
      continue;
    }
    throw new Error("the stale cursor was accepted");
  }
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

test("an open build record shows a commit made after the journal's initial zero-diff snapshot", async () => {
  const root = mkdtempSync(join(tmpdir(), "corral-live-record-"));
  const repo = join(root, "repo");
  const git = (...args: string[]) => execFileSync("git", ["-c", "user.name=Fixture", "-c", "user.email=fixture@example.com",
    "-c", "commit.gpgsign=false", ...args], { cwd: repo, encoding: "utf8" }).trim();
  try {
    execFileSync("git", ["init", "-q", repo]);
    const file = join(repo, "label");
    writeFileSync(file, "old\n");
    git("add", "label");
    git("commit", "-qm", "Base");
    const context: BuildContext = { dir: join(root, "build"), corral: "", settings: { maxParallel: 1, profiles: [], exitPolicy: "none" },
      session: { version: 1, id: "B-fixture", name: "fixture", created: at, updated: at, repoRoot: repo, worktree: repo,
        branch: git("symbolic-ref", "--short", "HEAD"), base: git("rev-parse", "HEAD"), here: true } };
    const journal = createJournal({ context, git: gitRun(), fleet: () => ({ children: [], mail: [], maxParallel: 1 }) });
    await journal.refresh();
    expect(journal.record().diff?.commits).toBe(0);

    writeFileSync(file, "new\n");
    git("commit", "-qam", "Change label");
    const registry = new RemoteRegistry();
    registerBoard(registry, { record: journal.liveRecord, ready: () => true });
    expect(await registry.get("record")!({})).toMatchObject({
      outcome: { state: "open" }, diff: { files: 1, added: 1, removed: 1, commits: 1 }, commits_total: 1,
      commits: [{ sha: git("rev-parse", "HEAD"), subject: "Change label" }],
    });
    expect(journal.record().diff?.commits).toBe(0);
  } finally { rmSync(root, { recursive: true, force: true }); }
});
