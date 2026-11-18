// Run via engine-daemon-journey.sh. The only substitute is Pi's model provider.
import assert from "node:assert/strict";
import { spawn, type ChildProcess } from "node:child_process";
import { closeSync, existsSync, mkdirSync, mkdtempSync, openSync, readFileSync, readdirSync, realpathSync, rmSync, statSync, watch, writeFileSync } from "node:fs";
import { createConnection, type Socket } from "node:net";
import { basename, dirname, join, resolve } from "node:path";
import { text } from "node:stream/consumers";
import type { FakeStep } from "./fixtures/auth/fake-transport.ts";
import { scriptedFixtureInstall } from "./support/fixture-install.ts";
import { within } from "./support/wait.ts";

const deadline = 20_000;
const first = "T-001", second = "T-002";
const workerInput = "Show the worker's live reply.";
const workerText = "Live worker reply.";
const reviewNotes = "Use the final wording instead of the draft.";
const approvalNotes = "The final wording is correct.";
const chatInput = "Give me the fixture greeting.";
const chatText = "Hello from the chat session.";
const resumedText = "Received the held gate notes.";
const edit = (callId: string, oldText: string, newText: string) => ({
  callId, name: "edit", arguments: { path: "line.txt", edits: [{ oldText, newText }] },
});
const done = (callId: string, summary: string) => ({ callId, name: "corral_done", arguments: { summary } });
const verdict = (callId: string, verdict: string, notes: string) => ({ callId, name: "corral_verdict", arguments: { verdict, notes } });
const steps: FakeStep[] = [
  { match: `${first}/R1/prompt.md`, toolCalls: [edit("draft", "original", "draft")] },
  { match: `${first}/R1/prompt.md`, toolCalls: [done("first-done", "Drafted one line")] },
  { match: `${first}/R2/prompt.md`, toolCalls: [verdict("changes", "changes", reviewNotes)] },
  { match: reviewNotes, toolCalls: [edit("final", "draft", "final")] },
  { match: reviewNotes, toolCalls: [done("second-done", "Revised one line")] },
  { match: `Review round 1 for ${first}.`, toolCalls: [verdict("approve", "approve", approvalNotes)] },
  { match: `${second}/R1/prompt.md`, toolCalls: [edit("failing", "final", "paused candidate")] },
  { match: workerInput, text: workerText },
  { match: "Gate failed:", text: resumedText },
  { match: chatInput, text: chatText },
];

function processExit(child: ChildProcess): Promise<number | null> {
  const { promise, resolve, reject } = Promise.withResolvers<number | null>();
  child.once("error", reject);
  child.once("exit", resolve);
  return promise;
}

function quote(text: string): string { return `'${text.replaceAll("'", "'\\''")}'`; }

function waitFile<T>(file: string, read: () => T | undefined): Promise<T> {
  const { promise, resolve, reject } = Promise.withResolvers<T>();
  const check = () => {
    try {
      const value = read();
      if (value !== undefined) resolve(value);
    } catch (error) {
      if ((error as NodeJS.ErrnoException).code !== "ENOENT") reject(error);
    }
  };
  const watcher = watch(dirname(file), check);
  watcher.once("error", reject);
  check();
  return within(promise, deadline, file).finally(() => watcher.close());
}

interface RpcFrame { id?: number; method?: string; params?: unknown; error?: unknown; result?: unknown }
interface LivePage {
  scope: string;
  topic: string;
  cursor: { rev: number; after: string | null };
  events: { kind: string; turn?: number; state?: string; text?: string }[];
}

async function* controlFrames(socket: Socket): AsyncGenerator<RpcFrame> {
  let pending = Buffer.alloc(0);
  for await (const chunk of socket) {
    pending = Buffer.concat([pending, chunk]);
    while (pending.length >= 4) {
      const size = pending.readUInt32LE();
      assert(size >= 5 && size <= 60 * 1024 + 5, `invalid control frame length ${size}`);
      if (pending.length < 4 + size) break;
      assert.deepEqual(pending.subarray(4, 9), Buffer.alloc(5), "non-control frame on owner socket");
      yield JSON.parse(pending.toString("utf8", 9, 4 + size)) as RpcFrame;
      pending = pending.subarray(4 + size);
    }
  }
}

class DaemonClient {
  private readonly frames: AsyncGenerator<RpcFrame>;
  private readonly notifications: RpcFrame[] = [];
  private id = 0;

  constructor(private readonly socket: Socket) { this.frames = controlFrames(socket); }

  private async receive(): Promise<RpcFrame> {
    const frame = await this.frames.next();
    assert(!frame.done, "daemon closed the owner connection");
    return frame.value;
  }

  private async request<T>(method: string, params: object): Promise<T> {
    const id = ++this.id;
    const data = Buffer.from(JSON.stringify({ jsonrpc: "2.0", id, method, params }));
    const header = Buffer.alloc(9);
    header.writeUInt32LE(data.length + 5);
    this.socket.write(Buffer.concat([header, data]));
    while (true) {
      const frame = await this.receive();
      if (frame.id === undefined) {
        this.notifications.push(frame);
        continue;
      }
      assert.equal(frame.id, id);
      assert.equal(frame.error, undefined, `${method}: ${JSON.stringify(frame.error)}`);
      return frame.result as T;
    }
  }

  rpc<T = unknown>(method: string, params: object): Promise<T> {
    return within(this.request<T>(method, params), deadline, method);
  }

  async liveThrough(scope: string, text: string): Promise<void> {
    let turn: number | undefined;
    const untilEnd = async () => {
      while (true) {
        const frame = this.notifications.shift() ?? await this.receive();
        assert.equal(frame.method, "sub.delta");
        const page = frame.params as LivePage;
        assert.equal(page.topic, "session");
        assert.equal(page.scope, scope);
        const matching = page.events.find(event => event.kind === "live_text" && event.text === text);
        if (matching) turn = matching.turn;
        await this.rpc("sub.ack", { topic: "session", scope, cursor: page.cursor });
        if (turn !== undefined && page.events.some(event =>
          event.kind === "live_turn" && event.state === "ended" && event.turn === turn)) return;
      }
    };
    await within(untilEnd(), deadline, `live text and turn end for ${scope}: ${text}`);
  }

  close(): void { this.socket.destroy(); }
}

interface EngineEvent {
  seq: number;
  kind: string;
  run: string;
  actor: string;
  detail: { notes?: string; round?: number; approve?: boolean; key?: string; result?: string };
}
interface TaskView {
  task: { status: string };
  runs: { id: string; role: string; worktree: string; branch: string; outcome?: string; session?: { pi: { dir: string } } }[];
}
interface HostRecord { id: string; pid: number; sessionFile: string; paused: boolean; held_input: { text: string }[] }
interface Entry {
  type: string;
  message?: { role: string; content: string | { type: string; name?: string; text?: string; arguments?: { verdict: string } }[] };
}

function json<T>(text: string): T { return JSON.parse(text) as T; }
function entries(file: string): Entry[] {
  return readFileSync(file, "utf8").trimEnd().split("\n").map(line => json<Entry>(line));
}
function userText(file: string): string[] {
  return entries(file).flatMap(({ message }) => message?.role === "user"
    ? [typeof message.content === "string" ? message.content : message.content.map(item => item.text ?? "").join("")] : []);
}

async function run(engine: string, daemonBin: string): Promise<void> {
  // A short path leaves room for Unix sockets. Nothing inherits the owner's config,
  // provider keys, git hooks, corral selectors, or herdr environment.
  const root = realpathSync(mkdtempSync("/tmp/ce2e-"));
  const repo = join(root, "repo"), state = join(root, ".corral-next");
  const bin = join(root, "bin"), runtime = join(root, "r");
  const env = {
    PATH: `${bin}:${process.env.PATH}`, HOME: root, XDG_CONFIG_HOME: join(root, "config"),
    XDG_DATA_HOME: join(root, "data"), XDG_STATE_HOME: join(root, "state"), XDG_CACHE_HOME: join(root, "cache"),
    XDG_RUNTIME_DIR: runtime, TMPDIR: root, GIT_CONFIG_GLOBAL: "/dev/null", GIT_CONFIG_NOSYSTEM: "1",
    GIT_CEILING_DIRECTORIES: root, GIT_AUTHOR_NAME: "Journey", GIT_AUTHOR_EMAIL: "journey@example.invalid",
    GIT_COMMITTER_NAME: "Journey", GIT_COMMITTER_EMAIL: "journey@example.invalid", CORRAL_NEXT_BIN: engine,
  };
  const tasks = new Set<string>();
  const command = async (argv: string[], cwd = root, allowFailure = false) => {
    const child = spawn(argv[0]!, argv.slice(1), { cwd, env, stdio: ["ignore", "pipe", "pipe"] });
    try {
      const [code, out, err] = await within(Promise.all([
        processExit(child), text(child.stdout), text(child.stderr),
      ]), deadline + 2_000, argv.join(" "));
      if (!allowFailure) assert.equal(code, 0, `${argv.join(" ")}\n${out}${err}`);
      return { code, out };
    } finally {
      if (child.exitCode === null && child.signalCode === null) child.kill("SIGKILL");
    }
  };
  const git = async (...args: string[]) => (await command(["git", ...args], repo)).out;
  const cli = async <T>(...argv: string[]) => json<T>((await command([engine, "--root", root, "--json", ...argv])).out);
  const newTask = async (expected: string) => {
    const { task } = await cli<{ task: { id: string } }>("new", "Exercise the Pi journey", "-p", "demo");
    tasks.add(task.id);
    assert.equal(task.id, expected);
  };
  const wait = async (task: string, kind: string, after: number) => {
    const event = await cli<EngineEvent>("wait", "--task", task, "--for", `${kind},error,blocked`,
      "--after", String(after), "--timeout", "15s");
    assert.equal(event.kind, kind, JSON.stringify(event));
    return event;
  };
  const record = (dir: string) => json<HostRecord>(readFileSync(join(dir, "session.json"), "utf8"));
  let owner: DaemonClient | undefined;
  let daemon: ChildProcess | undefined;
  let daemonExited: Promise<number | null> | undefined;
  try {
    mkdirSync(bin);
    mkdirSync(repo);
    mkdirSync(runtime, { mode: 0o700 });
    const assets = scriptedFixtureInstall(root, steps);
    // Put the real source host on PATH for both the Pi harness and daemon-launched chat.
    writeFileSync(join(bin, "corral-pi"), `#!/bin/sh\nexec ${quote(process.execPath)} ${quote(join(assets, "src/cli.ts"))} "$@"\n`, { mode: 0o700 });
    await git("init", "-b", "trunk");
    writeFileSync(join(repo, ".gitignore"), "/.worktrees/\n");
    writeFileSync(join(repo, "line.txt"), "original\n");
    await git("add", ".");
    await git("commit", "-m", "base");
    const base = (await git("rev-parse", "trunk")).trim();
    await command([engine, "--root", root, "init", "--first-id", "1"]);
    writeFileSync(join(state, "corral.toml"), `
[runner]
mode = "process"
[defaults]
profile = "worker"
auto_dispatch = false
auto_land = false
[harnesses.pi]
command = "corral-pi run {model_args} --prompt-file {prompt_file} --corral {corral} --headless"
[profiles.worker]
harness = "pi"
[profiles.reviewer]
harness = "pi"
[projects.demo]
path = "repo"
reviewer = "reviewer"
gate = ["test -s line.txt && ! grep -qx 'paused candidate' line.txt"]
`);

    await newTask(first);
    const dispatched = await cli<{ seq: number }>("dispatch", first);
    const initial = await wait(first, "review_started", dispatched.seq);
    const reviewDir = join(state, "runs", first, "reviewer");
    const originalReviewer = record(reviewDir);
    assert.equal(originalReviewer.id, `${first}/reviewer`);
    const ready = await wait(first, "ready", initial.seq);
    const reviewed = await cli<TaskView>("show", first);
    assert.equal(reviewed.task.status, "review");
    const reviewers = reviewed.runs.filter(run => run.role === "reviewer");
    assert.deepEqual(reviewers.map(run => run.id), [initial.run]);
    assert.deepEqual(record(reviewDir), originalReviewer, "the reviewer process and transcript must survive changes");
    const transcript = originalReviewer.sessionFile;
    assert.deepEqual(readdirSync(dirname(transcript)).filter(name => name.endsWith(".jsonl")), [basename(transcript)]);
    const rereview = `Review round 1 for ${first}.`;
    assert(userText(transcript).some(text => text.startsWith(rereview)));
    assert.deepEqual(entries(transcript).flatMap(({ message }) => Array.isArray(message?.content)
      ? message.content.filter(item => item.type === "toolCall" && item.name === "corral_verdict").map(item => item.arguments?.verdict)
      : []), ["changes", "approve"]);
    const worker = reviewed.runs.find(run => run.role === "worker");
    assert(worker?.session?.pi);
    assert(userText(record(worker.session.pi.dir).sessionFile).includes(reviewNotes));
    const events = await cli<EngineEvent[]>("log", first);
    const progression = events.filter(event => ["done", "gate", "review_started", "verdict", "sent_back", "ready"].includes(event.kind));
    assert.deepEqual(progression.map(event => event.kind),
      ["done", "gate", "review_started", "verdict", "sent_back", "done", "gate", "review_started", "verdict", "ready"]);
    assert.deepEqual(progression.filter(event => event.kind === "done").map(event => [event.run, event.actor]),
      [[worker.id, "agent"], [worker.id, "agent"]]);
    const gates = progression.filter(event => event.kind === "gate");
    assert.deepEqual(gates.map(event => event.detail.result), ["pass", "pass"]);
    assert.notEqual(gates[0]?.detail.key, gates[1]?.detail.key, "changed commit needs a new gate");
    assert.deepEqual(progression.filter(event => event.kind === "verdict").map(event =>
      [event.run, event.actor, event.detail.round, event.detail.approve, event.detail.notes]),
    [[initial.run, "reviewer", 0, false, reviewNotes], [initial.run, "reviewer", 1, true, approvalNotes]]);
    assert.equal(progression.at(-1)?.seq, ready.seq);

    await cli("approve", first);
    const landed = await cli<TaskView>("show", first);
    assert.equal(landed.task.status, "done");
    assert.equal(landed.runs.find(run => run.id === worker.id)?.outcome, "landed");
    assert.equal(await git("show", "trunk:line.txt"), "final\n");
    assert.equal(await git("diff", "--name-only", base, "trunk"), "line.txt\n");
    assert(!existsSync(worker.worktree));
    assert.equal((await command(["git", "show-ref", "--verify", "--quiet", `refs/heads/${worker.branch}`], repo, true)).code, 1);
    const usage = await cli<{ total_runs: number; covered_runs: number; groups: { key: string; total: { requests: number; input: number } }[] }>("usage", "--by", "profile");
    assert.equal(usage.total_runs, 2);
    assert.equal(usage.covered_runs, 2, JSON.stringify(usage));
    assert.deepEqual(usage.groups.map(group => group.key), ["reviewer", "worker"]);
    assert.deepEqual(usage.groups.map(group => group.total.requests), [2, 4]);
    assert(usage.groups.every(group => group.total.input > 0));
    tasks.delete(first);

    const daemonPath = join(state, "daemon.sock");
    const daemonLog = openSync(join(root, "daemon.log"), "w");
    try {
      daemon = spawn(daemonBin, ["--root", root, "run"], { cwd: root, env, stdio: ["ignore", daemonLog, daemonLog] });
      daemonExited = processExit(daemon);
    } finally {
      closeSync(daemonLog);
    }
    await Promise.race([
      waitFile(daemonPath, () => existsSync(daemonPath) && (statSync(daemonPath).mode & 0o777) === 0o600 ? true : undefined),
      daemonExited.then(code => { throw new Error(`daemon exited before the socket was ready (${code})`); }),
    ]);
    owner = new DaemonClient(createConnection(daemonPath));
    const welcome = await owner.rpc<{ caps: string[] }>("hello", {
      proto: { major: 1, minor: 1 }, client: { kind: "cli", version: "journey" }, caps: ["tasks.v1", "session.v1", "assistant.v1"],
    });
    assert(welcome.caps.includes("session.v1") && welcome.caps.includes("assistant.v1"));
    await newTask(second);
    await owner.rpc("op.submit", { op: { dispatch: { id: second } } });
    const scope = `${second}_R1`;
    await owner.rpc("sub.open", { topic: "session", scope });
    await owner.rpc("op.submit", { op: { reply: { id: second, text: workerInput } } });
    await owner.liveThrough(scope, workerText);
    assert.equal((await owner.rpc<{ paused: boolean }>("session.pause", { id: scope })).paused, true);

    const result = await owner.rpc<{ seq: number }>("op.submit", { op: { done: { id: second, summary: "Exercise a red gate while paused" } } });
    const sentBack = await wait(second, "sent_back", result.seq - 1);
    const pausedRun = join(state, "runs", second, "R1");
    const paused = record(pausedRun);
    assert(paused.paused);
    assert.equal(paused.held_input.length, 1);
    const held = paused.held_input[0]?.text;
    assert(held && held.startsWith("Gate failed:") && held === sentBack.detail.notes);
    assert(!userText(paused.sessionFile).includes(held));
    assert.equal((await owner.rpc<{ paused: boolean }>("session.resume", { id: scope })).paused, false);
    await owner.liveThrough(scope, resumedText);
    assert.equal(userText(paused.sessionFile).filter(text => text === held).length, 1);
    assert.deepEqual(record(pausedRun).held_input, []);
    await owner.rpc("sub.close", { topic: "session", scope });

    const chat = await owner.rpc<{ entry: string; state: { state: string } }>("assistant.send", {
      cmd_id: "fixture-chat", thread: "quick", target: "assistant", text: chatInput,
      typed_at: new Date().toISOString(), context: [], attachments: [],
    });
    assert.equal(chat.state.state, "sent");
    assert(chat.entry.startsWith("A-1/input/"));
    await owner.rpc("sub.open", { topic: "session", scope: "A-1" });
    await owner.liveThrough("A-1", chatText);
    const chatRecord = record(join(state, "chat", "A-1"));
    assert.equal(chatRecord.id, "A-1");
    assert(userText(chatRecord.sessionFile).includes(chatInput));
    assert(readFileSync(chatRecord.sessionFile, "utf8").includes(chatText));
    await owner.rpc("session.terminate", { id: "A-1" });
    await owner.rpc("session.terminate", { id: scope });
    tasks.delete(second);
    await owner.rpc("daemon.stop", { drain: true });
    owner.close();
    assert.equal(await within(daemonExited, deadline, "daemon shutdown"), 0);
  } catch (error) {
    for (const id of tasks) {
      const out = await command([engine, "--root", root, "log", id], root, true);
      console.error(out.out);
      const runDir = join(state, "runs", id);
      if (!existsSync(runDir)) continue;
      for (const name of readdirSync(runDir)) {
        const path = join(runDir, name, "session.log");
        if (existsSync(path)) console.error(`${path}:\n${readFileSync(path, "utf8")}`);
        if (existsSync(join(runDir, name, "session.json"))) {
          const { sessionFile } = record(join(runDir, name));
          if (existsSync(sessionFile)) console.error(`${sessionFile}:\n${readFileSync(sessionFile, "utf8").slice(-8_000)}`);
        }
      }
    }
    const log = join(root, "daemon.log");
    if (existsSync(log)) console.error(readFileSync(log, "utf8"));
    throw error;
  } finally {
    if (owner && daemon?.exitCode === null && existsSync(join(state, "chat", "A-1", "chat.json"))) {
      // On a failed assertion in the live watch, that client can still be reading. Use a new owner connection for cleanup.
      owner.close();
      const cleanup = new DaemonClient(createConnection(join(state, "daemon.sock")));
      try {
        await cleanup.rpc("hello", { proto: { major: 1, minor: 1 }, client: { kind: "cli", version: "journey" }, caps: ["assistant.v1"] });
        await cleanup.rpc("session.terminate", { id: "A-1" });
      } catch { /* preserve the test failure */ }
      finally { cleanup.close(); }
    }
    owner?.close();
    if (daemon?.exitCode === null && daemon.signalCode === null) { daemon.kill(); await daemonExited; }
    try {
      for (const id of tasks) await command([engine, "--root", root, "drop", id], root, true);
    } finally {
      rmSync(root, { recursive: true, force: true });
    }
  }
}

const [engine, daemon] = process.argv.slice(2);
assert(engine && daemon, "pass the built corral-next and corrald-next executables (see engine-daemon-journey.sh)");
const started = performance.now();
await run(resolve(engine), resolve(daemon));
console.log(`engine/daemon/Pi journey passed in ${((performance.now() - started) / 1000).toFixed(2)} s`);
