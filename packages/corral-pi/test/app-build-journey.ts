// The real daemon, engine and host; only the model provider is scripted.
import assert from "node:assert/strict";
import { spawn, type ChildProcess } from "node:child_process";
import { closeSync, existsSync, mkdirSync, mkdtempSync, openSync, readFileSync, realpathSync, rmSync, statSync, watch, writeFileSync } from "node:fs";
import { createConnection, type Socket } from "node:net";
import { basename, dirname, join, resolve } from "node:path";
import { text } from "node:stream/consumers";
import type { SessionCard } from "../src/host/remote/cards.ts";
import type { FakeStep } from "./fixtures/auth/fake-transport.ts";
import { scriptedFixtureInstall } from "./support/fixture-install.ts";
import { within } from "./support/wait.ts";

const deadline = 20_000;
const steps: FakeStep[] = [
  { toolCalls: [{ callId: "owner-choice", name: "ask_user", arguments: { questions: [
    { question: "Which line should the build use?", options: ["revised", "alternate"], freeText: false },
  ] } }] },
  { text: "Waiting for the owner's choice." },
  { match: "[ask_user answer", toolCalls: [{ callId: "change-line", name: "edit", arguments: {
    path: "line.txt", edits: [{ oldText: "original", newText: "revised" }],
  } }] },
  { match: "[ask_user answer", toolCalls: [{ callId: "commit-line", name: "bash", arguments: {
    command: "git add line.txt && git commit -m 'Update the fixture line'",
  } }] },
  { match: "[ask_user answer", text: "Committed the chosen line." },
];

interface Frame { id?: number; method?: string; params?: unknown; error?: unknown; result?: unknown }
interface LivePage {
  topic: string; scope: string; cursor: { rev: number; after: string | null };
  events: { kind: string; turn?: number; state?: string; tool?: string; failed?: boolean; aborted?: boolean; error?: string }[];
  state?: { cards: { id: string; kind: string }[]; exit?: { phase: string; landed?: string; base_branch?: string } };
}
interface BuildRow { id: string; build: { worktree: string; branch: string } }
interface Removed { worktree: boolean; branch: boolean; record: boolean; kept?: string; lines: string[] }

async function* frames(socket: Socket): AsyncGenerator<Frame> {
  let pending = Buffer.alloc(0);
  for await (const chunk of socket) {
    pending = Buffer.concat([pending, chunk]);
    while (pending.length >= 4) {
      const size = pending.readUInt32LE();
      assert(size >= 5 && size <= 60 * 1024 + 5);
      if (pending.length < 4 + size) break;
      assert.deepEqual(pending.subarray(4, 9), Buffer.alloc(5));
      yield JSON.parse(pending.toString("utf8", 9, 4 + size)) as Frame;
      pending = pending.subarray(4 + size);
    }
  }
}

class DaemonClient {
  private readonly frames: AsyncGenerator<Frame>;
  private readonly notifications: Frame[] = [];
  private id = 0;
  private readonly socket: Socket;
  constructor(path: string) { this.socket = createConnection(path); this.frames = frames(this.socket); }

  private async receive(): Promise<Frame> {
    const frame = await this.frames.next();
    assert(!frame.done, "daemon closed the owner connection");
    return frame.value;
  }

  private async request<T>(method: string, params: object): Promise<T> {
    const id = ++this.id;
    const data = Buffer.from(JSON.stringify({ jsonrpc: "2.0", id, method, params }));
    const head = Buffer.alloc(9);
    head.writeUInt32LE(data.length + 5);
    this.socket.write(Buffer.concat([head, data]));
    for (;;) {
      const frame = await this.receive();
      if (frame.id === undefined) { this.notifications.push(frame); continue; }
      assert.equal(frame.id, id);
      assert.equal(frame.error, undefined, `${method}: ${JSON.stringify(frame.error)}`);
      return frame.result as T;
    }
  }

  rpc<T = unknown>(method: string, params: object): Promise<T> { return within(this.request<T>(method, params), deadline, method); }

  async through(scope: string, match: (page: LivePage) => boolean, what: string): Promise<void> {
    await within((async () => {
      for (;;) {
        const notification = this.notifications.shift() ?? await this.receive();
        if (notification.method === "sub.reset") continue;
        assert.equal(notification.method, "sub.delta");
        const page = notification.params as LivePage;
        assert.equal(page.topic, "session");
        assert.equal(page.scope, scope);
        await this.rpc("sub.ack", { topic: "session", scope, cursor: page.cursor });
        if (match(page)) return;
      }
    })(), deadline, what);
  }

  hello() {
    return this.rpc<{ caps: string[] }>("hello", { proto: { major: 1, minor: 1 }, client: { kind: "cli", version: "app-build-journey" },
      caps: ["session.v1", "session.state.v1", "assistant.v1"] });
  }

  close(): void { this.socket.destroy(); }
}

function processExit(child: ChildProcess): Promise<number | null> {
  const { promise, resolve, reject } = Promise.withResolvers<number | null>();
  child.once("close", resolve);
  child.once("error", reject);
  return promise;
}

async function waitSocket(path: string, exit: Promise<number | null>): Promise<void> {
  const ready = Promise.withResolvers<void>();
  const check = () => {
    if (existsSync(path) && (statSync(path).mode & 0o777) === 0o600) ready.resolve();
  };
  const watcher = watch(dirname(path), (_event, name) => { if (name === basename(path)) check(); });
  try {
    check();
    await within(Promise.race([ready.promise, exit.then(code => { throw new Error(`daemon exited before hello (${code})`); })]), deadline, "daemon socket");
  } finally { watcher.close(); }
}

async function run(engine: string, daemonBin: string): Promise<void> {
  const root = realpathSync(mkdtempSync("/tmp/app-build-")), repo = join(root, "repo"), state = join(root, ".corral-next");
  const bin = join(root, "bin"), runtime = join(root, "r"), data = join(root, "data");
  const env = {
    PATH: `${bin}:${process.env.PATH ?? ""}`, HOME: root, XDG_CONFIG_HOME: join(root, "config"),
    XDG_DATA_HOME: data, XDG_STATE_HOME: join(root, "state"), XDG_CACHE_HOME: join(root, "cache"),
    XDG_RUNTIME_DIR: runtime, TMPDIR: root, GIT_CONFIG_GLOBAL: "/dev/null", GIT_CONFIG_NOSYSTEM: "1",
    GIT_CEILING_DIRECTORIES: root, GIT_AUTHOR_NAME: "Journey", GIT_AUTHOR_EMAIL: "journey@example.invalid",
    GIT_COMMITTER_NAME: "Journey", GIT_COMMITTER_EMAIL: "journey@example.invalid", CORRAL_NEXT_BIN: engine,
  };
  const command = async (program: string, args: string[], cwd: string, want = 0) => {
    const child = spawn(program, args, { cwd, env, stdio: ["ignore", "pipe", "pipe"] });
    try {
      const [code, out, err] = await within(Promise.all([processExit(child), text(child.stdout), text(child.stderr)]), deadline, [program, ...args].join(" "));
      assert.equal(code, want, `${program} ${args.join(" ")}\n${out}${err}`);
      return out;
    } finally { if (child.exitCode === null && child.signalCode === null) child.kill("SIGKILL"); }
  };
  const git = (...args: string[]) => command("git", args, repo);
  const daemonPath = join(state, "daemon.sock"), daemonLog = join(root, "daemon.log");
  let id: string | undefined, daemon: ChildProcess | undefined, exited: Promise<number | null> | undefined, owner: DaemonClient | undefined;
  let stopped = false;
  const recordDir = (id: string) => join(data, "corral", "corral-pi", "build", id);
  try {
    mkdirSync(bin);
    mkdirSync(repo);
    mkdirSync(runtime, { mode: 0o700 });
    const assets = scriptedFixtureInstall(root, steps);
    const quote = (value: string) => `'${value.replaceAll("'", "'\\''")}'`;
    writeFileSync(join(bin, "corral-pi"), `#!/bin/sh\nexec ${quote(process.execPath)} ${quote(join(assets, "src/cli.ts"))} "$@"\n`, { mode: 0o700 });
    await git("init", "-b", "trunk");
    writeFileSync(join(repo, ".gitignore"), "/.worktrees/\n");
    writeFileSync(join(repo, "line.txt"), "original\n");
    await git("add", ".");
    await git("commit", "-m", "base");
    await command(engine, ["--root", root, "init", "--first-id", "1"], root);
    writeFileSync(join(state, "corral.toml"), `[runner]\nmode = "process"\n[defaults]\nbase = "trunk"\nauto_dispatch = false\nauto_land = false\n[projects.demo]\npath = "repo"\ngate = ['test "$(cat line.txt)" = revised']\n`);
    const log = openSync(daemonLog, "w");
    try {
      daemon = spawn(daemonBin, ["--root", root, "run"], { cwd: root, env, stdio: ["ignore", log, log] });
      exited = processExit(daemon);
    } finally { closeSync(log); }
    await waitSocket(daemonPath, exited);
    owner = new DaemonClient(daemonPath);
    assert((await owner.hello()).caps.includes("session.state.v1"));
    const row = await owner.rpc<BuildRow>("build.start", { project: "demo", name: "walk", here: false });
    id = row.id;
    assert.match(id, /^B-/);
    assert.equal(row.build.branch, "build/walk");
    await owner.rpc("sub.open", { topic: "session", scope: id });
    const send = await owner.rpc<{ session: string; entry: string; turn: number }>("session.send", {
      id, cmd_id: "build-journey-input", text: "Ask which line to commit", policy: "steer",
    });
    assert.equal(send.session, id);
    assert.equal(send.entry, `${id}/input/build-journey-input`);
    const cardList = async () => {
      const reply = await owner!.rpc<{ body: { kind: string; body: { cards?: SessionCard[] } } }>("detail.get", { kind: "session_cards", id });
      assert.equal(reply.body.kind, "session_cards");
      return reply.body.body.cards ?? [];
    };
    let asked = false, turnEnded = false, questionId: string | undefined;
    await owner.through(id, page => {
      asked ||= page.events.some(event => event.kind === "live_tool" && event.tool === "ask_user" && event.state === "ended" && event.failed === false);
      turnEnded ||= page.events.some(event => event.kind === "live_turn" && event.state === "ended" && event.turn === send.turn && !event.aborted && !event.error);
      questionId ??= page.state?.cards.find(card => card.kind === "question")?.id;
      return asked && turnEnded && questionId !== undefined;
    }, "live tool call and deferred question");
    const question = (await cardList()).find(card => card.id === questionId);
    assert(question);
    const option = question.questions[0]?.options.find(option => option.label === "revised");
    assert(option);
    const answered = await owner.rpc<{ status: string }>("session.card.answer", { id, card: question.id, answers: [{ options: [option.id] }] });
    assert(["answered", "delivered"].includes(answered.status));
    let committed = false;
    await owner.through(id, page => {
      committed ||= page.events.some(event => event.kind === "live_tool" && event.tool === "bash" && event.state === "ended" && event.failed === false);
      return committed && page.events.some(event => event.kind === "live_turn" && event.state === "ended" && event.turn !== send.turn && !event.aborted && !event.error);
    }, "commit after the owner's answer");
    const head = (await git("rev-parse", "build/walk")).trim();
    assert.notEqual(head, (await git("rev-parse", "trunk")).trim());
    assert.equal(await git("show", "build/walk:line.txt"), "revised\n");
    assert(!(await cardList()).some(card => card.id === questionId));

    await owner.rpc("session.action", { id, action: "exit_open" });
    await owner.through(id, page => page.state?.cards.some(card => card.kind === "exit") === true, "exit card arrives");
    // Detail invalidations, rather than a fixed sleep, tell the app when the exit summary is ready.
    let exit: SessionCard | undefined, land: SessionCard["questions"][number]["options"][number] | undefined;
    for (;;) {
      exit = (await cardList()).find(card => card.kind === "exit");
      land = exit?.questions[0]?.options.find(option => option.id === "land");
      if (exit && land && !land.unavailable) break;
      await owner.through(id, page => page.events.some(event => event.kind === "live_part"), "exit card availability");
    }
    assert.equal((await owner.rpc<{ status: string }>("session.card.answer", { id, card: exit.id,
      answers: [{ options: [land.id], text: "This scripted checkout has been inspected." }] })).status, "started");
    await owner.through(id, page => {
      const exit = page.state?.exit;
      assert.notEqual(exit?.phase, "failed", "Land must complete successfully");
      if (exit?.phase !== "landed") return false;
      assert.equal(exit.base_branch, "trunk");
      assert.equal(exit.landed, head);
      return true;
    }, "Land updates the session state");
    assert.equal(await git("show", "trunk:line.txt"), "revised\n");
    await owner.rpc("sub.close", { topic: "session", scope: id });
    await owner.rpc("session.terminate", { id });
    stopped = true;
    const dry = await owner.rpc<Removed>("build.remove", { id, dry_run: true });
    assert.equal(dry.record, true);
    assert.equal(dry.kept, undefined);
    assert(existsSync(join(recordDir(id), "session.json")));
    const removed = await owner.rpc<Removed>("build.remove", { id, dry_run: false });
    assert.equal(removed.record, true);
    assert.equal(removed.kept, undefined);
    assert(!existsSync(recordDir(id)));
    assert(!existsSync(row.build.worktree));
    await command("git", ["show-ref", "--verify", "--quiet", `refs/heads/${row.build.branch}`], repo, 1);
    await owner.rpc("daemon.stop", { drain: true });
    owner.close();
    assert.equal(await within(exited, deadline, "daemon shutdown"), 0);
  } catch (error) {
    if (id && existsSync(join(recordDir(id), "session.log"))) console.error(readFileSync(join(recordDir(id), "session.log"), "utf8").slice(-8000));
    if (existsSync(daemonLog)) console.error(readFileSync(daemonLog, "utf8").slice(-8000));
    throw error;
  } finally {
    owner?.close();
    if (id && !stopped && daemon?.exitCode === null && existsSync(daemonPath)) {
      const cleanup = new DaemonClient(daemonPath);
      try { await cleanup.hello(); await cleanup.rpc("session.terminate", { id }); }
      catch { /* Preserve the original assertion and let daemon teardown stop its processes. */ }
      finally { cleanup.close(); }
    }
    if (daemon?.exitCode === null && daemon.signalCode === null) { daemon.kill(); await exited; }
    rmSync(root, { recursive: true, force: true });
  }
}

const [engine, daemon] = process.argv.slice(2);
assert(engine && daemon, "pass the built corral-next and corrald-next executables (see app-build-journey.sh)");
await run(resolve(engine), resolve(daemon));
console.log("app build journey passed");
