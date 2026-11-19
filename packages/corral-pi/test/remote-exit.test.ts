import { expect, test } from "bun:test";
import { execFileSync } from "node:child_process";
import { existsSync, mkdirSync, mkdtempSync, readFileSync, realpathSync, rmSync, watch, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { setImmediate as nextPoll } from "node:timers/promises";
import { join } from "node:path";
import { loadSession } from "../src/build/session-store.ts";
import type { BuildSessionRecord } from "../src/build/types.ts";
import type { SessionCard } from "../src/host/remote/cards.ts";
import { connect, type Reply } from "./support/bridge-client.ts";
import { scriptedFixtureInstall } from "./support/fixture-install.ts";
import { launch } from "./support/pi-process.ts";
import { within } from "./support/wait.ts";

const ID = "B-exit";
interface Fixture {
  root: string; repo: string; record: BuildSessionRecord;
  git(cwd: string, ...args: string[]): string;
  request(command: string, args?: Record<string, unknown>): Promise<Reply>;
}

async function withBuild(run: (fixture: Fixture) => Promise<void>) {
  const root = realpathSync(mkdtempSync(join(tmpdir(), "pi-exit-"))), repo = join(root, "repo"), data = join(root, "data");
  try {
    const env = { PATH: process.env.PATH, HOME: root, TMPDIR: root, XDG_RUNTIME_DIR: join(root, "runtime"), CORRAL_NEXT_ROOT: root,
      GIT_CONFIG_GLOBAL: "/dev/null", GIT_CONFIG_NOSYSTEM: "1", GIT_AUTHOR_NAME: "Exit Test", GIT_AUTHOR_EMAIL: "exit@example.test",
      GIT_COMMITTER_NAME: "Exit Test", GIT_COMMITTER_EMAIL: "exit@example.test" };
    const git = (cwd: string, ...args: string[]) => execFileSync("git", ["-C", cwd, ...args], { env, encoding: "utf8", stdio: ["ignore", "pipe", "pipe"] }).trim();
    mkdirSync(repo);
    git(repo, "init", "-b", "main");
    writeFileSync(join(repo, ".gitignore"), "/.worktrees/\n");
    git(repo, "add", ".gitignore");
    git(repo, "commit", "-m", "base");
    mkdirSync(join(root, ".corral-next"));
    writeFileSync(join(root, ".corral-next", "corral.toml"), '[defaults]\nbase = "main"\n[projects.demo]\npath = "repo"\ngate = ["touch gate-ran"]\n');
    const socket = join(root, "build.sock");
    const assets = scriptedFixtureInstall(root, []);
    const child = launch(assets, ["build", "--headless", "--name", "fixture", "--id", ID, "--project", "demo", "--data-dir", data,
      "--socket", socket, "--corral", "/unused/corral"], repo, env);
    try {
      await child.output(/session socket ready/);
      const client = await connect(socket);
      const request = (command: string, args: Record<string, unknown> = {}) => client.request({ version: 2, id: command, command, args });
      try {
        await run({ root, repo, record: loadSession(data, ID), git, request });
        expect((await request("shutdown")).result).toEqual({ shutting_down: true });
        expect(await within(child.exit, 10_000, "build shutdown")).toBe(0);
      } finally { client.close(); }
    } finally { await child.stop(); }
  } finally { rmSync(root, { recursive: true, force: true }); }
}

async function exitCard(fixture: Fixture, landAvailable = false): Promise<SessionCard> {
  expect((await fixture.request("exit_open")).result).toEqual({ phase: "pending" });
  const deadline = performance.now() + 3000;
  while (performance.now() < deadline) {
    const reply = await fixture.request("cards");
    expect(reply.ok).toBeTrue();
    const card = (reply.result?.cards as SessionCard[]).find(card => card.kind === "exit");
    const land = card?.questions[0]?.options.find(option => option.id === "land");
    if (card && (!landAvailable || land && land.unavailable === undefined)) return card;
    await nextPoll();
  }
  throw new Error("build exit did not become available");
}

test("Land without the required reason is refused before anything runs", async () => {
  await withBuild(async fixture => {
    const { record, git, repo, root } = fixture;
    const base = git(repo, "rev-parse", "main");
    git(record.worktree, "commit", "--allow-empty", "-m", "candidate");
    const card = await exitCard(fixture, true);
    expect(card.questions[0]?.options.find(option => option.id === "land")?.needs_text).toBeDefined();
    const answer = await fixture.request("card_answer", { card: card.id, answers: [{ options: ["land"], text: " " }] });
    expect(answer.error?.code).toBe("exit_reason_required");
    expect(git(repo, "rev-parse", "main")).toBe(base);
    expect(loadSession(join(root, "data"), ID).exit).toBeUndefined();
    expect(existsSync(join(record.worktree, "gate-ran"))).toBeFalse();
    const shown = (await fixture.request("cards")).result?.cards as SessionCard[];
    expect(shown.map(card => card.id)).toContain(card.id);
  });
}, 20_000);

test("the exit outcome reaches the parent conversation in a headless session", async () => {
  await withBuild(async fixture => {
    expect((await fixture.request("get_state")).result?.mode).toBe("headless");
    const card = await exitCard(fixture);
    expect((await fixture.request("card_answer", { card: card.id, answers: [{ options: ["keep"] }] })).result).toEqual({ status: "started" });
    expect((await fixture.request("card_answer", { card: card.id, answers: [{ options: ["keep"] }] })).result).toEqual({ status: "missing" });
    const file = fixture.record.sessionFile!;
    const outcomeRecorded = Promise.withResolvers<void>();
    const readOutcomes = () => readFileSync(file, "utf8").split("\n").slice(0, -1)
      .map(line => JSON.parse(line) as { type: string; customType?: string; details?: unknown })
      .filter(entry => entry.type === "custom_message" && entry.customType === "corral_build_exit");
    const watcher = watch(file, () => { if (readOutcomes().length) outcomeRecorded.resolve(); });
    try {
      if (readOutcomes().length) outcomeRecorded.resolve();
      await within(outcomeRecorded.promise, 3000, "exit entry in the parent transcript");
    } finally { watcher.close(); }
    const outcomes = readOutcomes();
    expect(outcomes).toHaveLength(1);
    expect(outcomes[0]?.details).toMatchObject({ choice: "keep", phase: "kept" });
  });
}, 20_000);
