import { expect, test } from "bun:test";
import { appendFileSync, cpSync, mkdirSync, mkdtempSync, readFileSync, realpathSync, rmSync, statSync, watch, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { buildFixtureInstall, PACKAGE_ROOT } from "./support/fixture-install.ts";
import { connect, type TestClient } from "./support/bridge-client.ts";
import { launch, type Launch } from "./support/pi-process.ts";
import { within } from "./support/wait.ts";
import { writeFakeEngine } from "./support/fake-engine.ts";
import type { FakeStep } from "./fixtures/auth/fake-transport.ts";

async function transcriptThrough(path: string, text: string): Promise<string[]> {
  const { promise, resolve } = Promise.withResolvers<string[]>();
  const check = () => {
    const entries = readFileSync(path, "utf8").trimEnd().split("\n").flatMap(line => {
      try { return [JSON.parse(line) as { message?: { role: string; content: unknown } }]; }
      catch { return []; } // The last append may not be visible yet.
    });
    const users = entries.flatMap(entry => {
      if (entry.message?.role !== "user") return [];
      const content = entry.message.content;
      return [typeof content === "string" ? content : (content as { text?: string }[]).map(part => part.text).join("")];
    });
    if (users.includes(text)) resolve(users);
  };
  const watcher = watch(path, check);
  try { check(); return await within(promise, 10_000, `the transcript to contain ${text}`); }
  finally { watcher.close(); }
}

/** Run the shipped role at the real CLI boundary, with only provider responses scripted. */
function scriptedInstall(dir: string, steps: readonly FakeStep[]): string {
  const assets = buildFixtureInstall({ parent: dir, omit: ["role"] });
  cpSync(join(PACKAGE_ROOT, "assets"), join(assets.dir, "assets"), { recursive: true });
  appendFileSync(join(assets.dir, "src/auth/index.ts"), `\nfixtureTransport().script(${JSON.stringify(steps)});\n`);
  return assets.dir;
}

test("one flag launch runs the fixture to idle; v2 answers, pauses, restarts and delivers held input before owner text", async () => {
  const dir = mkdtempSync(join(tmpdir(), "pi-cli-"));
  const assets = buildFixtureInstall({ parent: dir });
  const cwd = join(realpathSync(dir), "worktree");
  const run = join(dir, "run");
  const socket = join(dir, "s.sock");
  mkdirSync(cwd);
  writeFileSync(join(cwd, "p.md"), "Reply with a fixture response.\n");
  const env = { HOME: cwd, XDG_DATA_HOME: join(dir, "data"), XDG_CONFIG_HOME: join(dir, "config"),
    PATH: process.env.PATH, TMPDIR: dir, GIT_CEILING_DIRECTORIES: dir,
    CORRAL_NEXT_ROOT: dir, CORRAL_TASK: "T-1", CORRAL_RUN: "R1", CORRAL_ACTOR: "agent" };
  const children: Launch[] = [];
  const clients: TestClient[] = [];
  function launchRun(): Launch {
    const handle = launch(assets.dir, ["run", "--dir", run, "--socket", socket, "--prompt-file", "p.md"], cwd, env);
    children.push(handle);
    return handle;
  }
  let requestId = 0;
  const call = async (client: TestClient, command: string, args: Record<string, unknown> = {}) => {
    const reply = await client.request({ version: 2, id: `id-${++requestId}`, command, args });
    if (!reply.ok || !reply.result) throw new Error(`${command}: ${JSON.stringify(reply.error)}`);
    return reply.result;
  };
  const input = (client: TestClient, text: string, origin = "owner") => call(client, "submit_input", { text, origin });
  const record = () => JSON.parse(readFileSync(join(run, "session.json"), "utf8"));
  try {
    const first = launchRun();
    await first.output(/session socket ready/);
    await first.output(/turn 1 ended/);
    const client = await connect(socket);
    clients.push(client);
    const hello = await call(client, "hello");
    const transcript = String(hello.session_file);
    expect(hello).toEqual({ protocol: 2, role: "worker", session_id: "T-1/R1", session_file: transcript, pid: first.child.pid });
    expect(record()).toMatchObject({ id: "T-1/R1", role: "worker", worktree: cwd, sessionFile: transcript, pid: first.child.pid, paused: false });
    expect(statSync(join(run, "session.json")).mode & 0o777).toBe(0o600);
    expect((await call(client, "get_state")).turn).toEqual({ current: 1, open: false });
    expect(readFileSync(transcript, "utf8")).toContain(`Read ${join(cwd, "p.md")}.`);

    const duplicate = launchRun();
    expect(await within(duplicate.exit, 10_000, "the second host to exit")).toBe(2);
    expect(duplicate.stderr()).toContain("session already live; corral stop T-1/R1");
    expect((await call(client, "hello")).pid).toBe(first.child.pid);

    expect((await input(client, "/fixture-ask")).disposition).toBe("handled");
    expect((await input(client, "Ask me about the direction.")).disposition).toBe("started");
    await first.output(/turn 2 ended/);
    const state = await call(client, "get_state");
    const [question] = state.owner_questions as { id: string }[];
    expect(typeof question?.id).toBe("string");
    expect(state.owner_questions).toEqual([{ id: question?.id, status: "pending", question: "Which direction?",
      options: ["Left", "Right"], free_text: true }]);
    const answer = await call(client, "answer", { question: question?.id, option: "Right" });
    expect(answer.status).toMatch(/^(answered|delivered)$/);
    await first.output(/turn 3 ended/);
    expect((await call(client, "get_state")).owner_questions).toEqual([]);
    expect(readFileSync(transcript, "utf8")).toContain("Which direction?: Right");

    await input(client, "/fixture-hold");
    await input(client, "Turn that pause should stop.");
    await first.output(/turn 4 started/);
    expect(await call(client, "pause")).toMatchObject({ paused: true, cancelled: true, turn: 4 });
    expect((await input(client, "First automated note", "auto")).disposition).toBe("queued");
    expect((await input(client, "Second automated note", "auto")).disposition).toBe("queued");
    expect(record()).toMatchObject({ paused: true, held_input: [{ text: "First automated note" }, { text: "Second automated note" }] });
    expect(readFileSync(transcript, "utf8")).not.toContain("automated note");

    client.close();
    first.child.kill("SIGKILL");
    await within(first.exit, 10_000, "the fixture host to stop");
    const resumed = launchRun();
    await resumed.output(/session socket ready/);
    const reopened = await connect(socket);
    clients.push(reopened);
    expect((await call(reopened, "hello")).session_file).toBe(transcript);
    expect((await call(reopened, "get_state")).paused).toBeTrue();
    expect((await input(reopened, "Owner message")).disposition).toMatch(/started|queued/);
    expect((await transcriptThrough(transcript, "Owner message")).slice(-3))
      .toEqual(["First automated note", "Second automated note", "Owner message"]);
    expect(record()).toMatchObject({ paused: false, held_input: [], pid: resumed.child.pid });
    expect(await call(reopened, "shutdown")).toEqual({ shutting_down: true });
    expect(await within(resumed.exit, 10_000, "the session socket to shut down")).toBe(0);
  } finally {
    for (const client of clients) client.close();
    await Promise.all(children.map(processHandle => processHandle.stop()));
    rmSync(dir, { recursive: true, force: true });
  }
}, 35_000);

test("chat runs the engine as the assistant and edits plain workspace memory", async () => {
  const dir = mkdtempSync(join(tmpdir(), "pi-chat-"));
  let chat: Launch | undefined;
  let client: TestClient | undefined;
  try {
    const memory = join(dir, ".corral-next", "memory", "notes.md");
    mkdirSync(join(dir, ".corral-next", "memory"), { recursive: true });
    writeFileSync(memory, "# Notes\nOriginal text\n");
    const engine = writeFakeEngine(dir, { list: { result: [{ id: "T-1" }] } });
    const assets = scriptedInstall(dir, [
      { toolCalls: [{ callId: "list", name: "bash", arguments: { command: 'test "$CORRAL_ACTOR" = assistant && "$CORRAL" list --json' } }] },
      { toolCalls: [{ callId: "memory", name: "edit", arguments: { path: memory, edits: [{ oldText: "Original text", newText: "Updated note" }] } }] },
      { text: "Recorded the note." },
    ]);
    const socket = join(dir, "s.sock");
    chat = launch(assets, ["chat", "--dir", "A-1", "--socket", socket, "--corral", engine.path], dir,
      { HOME: dir, PATH: process.env.PATH, XDG_DATA_HOME: join(dir, "data"), TMPDIR: dir,
        GIT_CEILING_DIRECTORIES: dir, CORRAL_NEXT_ROOT: dir });
    await chat.output(/session socket ready/);
    client = await connect(socket);
    const hello = await client.request({ version: 2, id: "hello", command: "hello" });
    expect(hello.result).toMatchObject({ protocol: 2, role: "assistant", session_id: "A-1", pid: chat.child.pid });
    const sent = await client.request({ version: 2, id: "message", command: "submit_input",
      args: { origin: "owner", text: "List tasks and update the notes." } });
    expect(sent.result?.disposition).toBe("started");
    await chat.output(/turn 1 ended/);
    expect(engine.calls().filter(argv => argv[0] === "list")).toEqual([["list", "--json"]]);
    expect(readFileSync(memory, "utf8")).toBe("# Notes\nUpdated note\n");
    expect(readFileSync(String(hello.result?.session_file), "utf8")).toContain("Recorded the note.");
    await client.request({ version: 2, id: "shutdown", command: "shutdown" });
    expect(await within(chat.exit, 10_000, "chat shutdown")).toBe(0);
  } finally {
    client?.close();
    await chat?.stop();
    rmSync(dir, { recursive: true, force: true });
  }
}, 20_000);

test("a warmed chat relaunches after shutdown before the first turn", async () => {
  const dir = mkdtempSync(join(tmpdir(), "pi-warm-"));
  const processes: Launch[] = [];
  const clients: TestClient[] = [];
  try {
    const assets = scriptedInstall(dir, [{ text: "The relaunched chat is ready." }]);
    const socket = join(dir, "s.sock");
    const start = () => {
      const chat = launch(assets, ["chat", "--dir", "A-1", "--socket", socket, "--headless"], dir,
        { HOME: dir, PATH: process.env.PATH, XDG_DATA_HOME: join(dir, "data"), TMPDIR: dir,
          GIT_CEILING_DIRECTORIES: dir, CORRAL_NEXT_ROOT: dir, CORRAL_ACTOR: "assistant" });
      processes.push(chat);
      return chat;
    };
    const first = start();
    await first.output(/session socket ready/);
    const warm = await connect(socket);
    clients.push(warm);
    const request = (client: TestClient, command: string, args: Record<string, unknown> = {}) =>
      client.request({ version: 2, id: command, command, args });
    expect((await request(warm, "get_state")).result?.turn).toEqual({ current: 0, open: false });
    expect((await request(warm, "shutdown")).ok).toBeTrue();
    expect(await within(first.exit, 10_000, "warmed chat shutdown")).toBe(0);

    const relaunched = start();
    await relaunched.output(/session socket ready/);
    const chat = await connect(socket);
    clients.push(chat);
    const hello = (await request(chat, "hello")).result;
    expect(hello?.session_id).toBe("A-1");
    expect((await request(chat, "submit_input", { origin: "owner", text: "Continue." })).result?.disposition).toBe("started");
    await relaunched.output(/turn 1 ended/);
    expect(readFileSync(String(hello?.session_file), "utf8")).toContain("The relaunched chat is ready.");
    expect((await request(chat, "shutdown")).ok).toBeTrue();
    expect(await within(relaunched.exit, 10_000, "relaunched chat shutdown")).toBe(0);
  } finally {
    for (const client of clients) client.close();
    await Promise.all(processes.map(chat => chat.stop()));
    rmSync(dir, { recursive: true, force: true });
  }
}, 20_000);

test("reviewer answers an automated re-review and reopens the same transcript after a kill", async () => {
  const dir = mkdtempSync(join(tmpdir(), "pi-review-"));
  const processes: Launch[] = [];
  const clients: TestClient[] = [];
  try {
    const promptFile = join(dir, "prompt.md");
    writeFileSync(promptFile, "# T-1 Review the candidate\n\n## Review round 0\n");
    const engine = writeFakeEngine(dir);
    const rereview = "Review round 1 for T-1.\n\nVerify the required fix.";
    const assets = scriptedInstall(dir, [
      { toolCalls: [{ callId: "first-verdict", name: "corral_verdict",
        arguments: { verdict: "changes", notes: "Add regression coverage." } }] },
      { match: rereview, toolCalls: [{ callId: "second-verdict", name: "corral_verdict",
        arguments: { verdict: "approve", notes: "Regression coverage verified." } }] },
    ]);
    const socket = join(dir, "r.sock");
    const run = join(dir, ".corral-next", "runs", "T-1", "reviewer");
    const env = { HOME: dir, PATH: process.env.PATH, TMPDIR: dir, XDG_DATA_HOME: join(dir, "data"),
      GIT_CEILING_DIRECTORIES: dir, CORRAL_NEXT_ROOT: dir, CORRAL_TASK: "T-1", CORRAL_RUN: "R2", CORRAL_ACTOR: "reviewer" };
    let seq = 0;
    const call = async (client: TestClient, command: string, args?: Record<string, unknown>) => {
      const reply = await client.request({ version: 2, id: `review-${++seq}`, command, args });
      if (!reply.ok || !reply.result) throw new Error(`${command}: ${JSON.stringify(reply.error)}`);
      return reply.result;
    };
    const start = async () => {
      const host = launch(assets, ["run", "--dir", run, "--socket", socket, "--corral", engine.path, "--prompt-file", promptFile], dir, env);
      processes.push(host);
      await host.output(/session socket ready/);
      const client = await connect(socket);
      clients.push(client);
      const hello = await call(client, "hello");
      expect(hello).toMatchObject({ role: "reviewer", session_id: "T-1/reviewer" });
      return { host, client, transcript: String(hello.session_file) };
    };
    const initial = await start();
    await initial.host.output(/turn 1 ended/);
    expect((await call(initial.client, "submit_input", { text: rereview, origin: "auto", busy_policy: "steer" })).disposition).toBe("started");
    await initial.host.output(/turn 2 ended/);
    const before = readFileSync(initial.transcript, "utf8");
    expect(before).toContain("Add regression coverage.");
    expect(before).toContain("Review round 1 for T-1.");
    expect(before).toContain("Regression coverage verified.");
    const verdicts = () => engine.calls().filter(argv => argv[3] === "verdict");
    expect(verdicts()).toEqual([
      ["--root", dir, "--json", "verdict", "T-1", "changes", "--notes", "Add regression coverage."],
      ["--root", dir, "--json", "verdict", "T-1", "approve", "--notes", "Regression coverage verified."],
    ]);
    initial.host.child.kill("SIGKILL");
    await within(initial.host.exit, 10_000, "the reviewer to stop");
    const reopened = await start();
    expect(reopened.transcript).toBe(initial.transcript);
    expect((await call(reopened.client, "get_state")).turn).toEqual({ current: 2, open: false });
    expect(readFileSync(reopened.transcript, "utf8")).toBe(before);
    expect(verdicts()).toHaveLength(2);
    await call(reopened.client, "shutdown");
    expect(await within(reopened.host.exit, 10_000, "reviewer shutdown")).toBe(0);
  } finally {
    for (const client of clients) client.close();
    await Promise.all(processes.map(host => host.stop()));
    rmSync(dir, { recursive: true, force: true });
  }
}, 30_000);


test("worker relaunch repairs an interrupted transcript, then a retry inherits it unless fresh", async () => {
  const dir = mkdtempSync(join(tmpdir(), "pi-retry-"));
  const assets = buildFixtureInstall({ parent: dir });
  const socket = join(dir, "s.sock");
  const env = { HOME: dir, XDG_DATA_HOME: join(dir, "data"), PATH: process.env.PATH, TMPDIR: dir,
    GIT_CEILING_DIRECTORIES: dir, CORRAL_NEXT_ROOT: dir, CORRAL_TASK: "T-1", CORRAL_ACTOR: "agent" };
  const handles: Launch[] = [], clients: TestClient[] = [];
  let seq = 0;
  const call = async (client: TestClient, command: string, args?: Record<string, unknown>) => {
    const reply = await client.request({ version: 2, id: `retry-${++seq}`, command, args });
    if (!reply.ok || !reply.result) throw new Error(`${command}: ${JSON.stringify(reply.error)}`);
    return reply.result;
  };
  const input = (client: TestClient, text: string) => call(client, "submit_input", { origin: "owner", text });
  const start = async (run: string, fresh = false) => {
    const runDir = join(dir, ".corral-next", "runs", "T-1", run);
    const process = launch(assets.dir, ["run", "--dir", runDir, "--socket", socket], dir,
      { ...env, CORRAL_RUN: run, CORRAL_FRESH: fresh ? "1" : "0" });
    handles.push(process);
    await process.output(/session socket ready/);
    const client = await connect(socket);
    clients.push(client);
    const hello = await call(client, "hello");
    return { client, process, transcript: String(hello.session_file) };
  };
  const stop = async (session: Awaited<ReturnType<typeof start>>) => {
    await call(session.client, "shutdown");
    expect(await within(session.process.exit, 10_000, "worker shutdown")).toBe(0);
  };
  try {
    const first = await start("R1");
    await input(first.client, "Reply only BEFORE_KILL.");
    await first.process.output(/turn 1 ended/);
    await input(first.client, "/fixture-hold");
    await input(first.client, "Continue this interrupted conversation.");
    await first.process.output(/turn 2 started/);
    expect((await call(first.client, "get_state")).turn).toEqual({ current: 2, open: true });
    first.process.child.kill("SIGKILL");
    await within(first.process.exit, 10_000, "killed fixture host");
    appendFileSync(first.transcript, '{"type":"cut-off');

    const reopened = await start("R1");
    expect(reopened.transcript).toBe(first.transcript);
    expect((await call(reopened.client, "get_state")).turn).toEqual({ current: 2, open: false });
    await input(reopened.client, "Reply only REOPENED.");
    await reopened.process.output(/turn 3 ended/);
    await stop(reopened);
    const archived = readFileSync(first.transcript, "utf8");
    expect(archived).not.toContain("cut-off");
    expect(archived).toContain("BEFORE_KILL");
    expect(archived).toContain("REOPENED");

    const retry = await start("R2");
    expect(retry.transcript).not.toBe(first.transcript);
    expect(readFileSync(retry.transcript, "utf8")).toBe(archived);
    await input(retry.client, "Reply only RETRIED.");
    await retry.process.output(/turn 4 ended/);
    await stop(retry);
    expect(readFileSync(first.transcript, "utf8")).toBe(archived);

    const fresh = await start("R3", true);
    expect((await call(fresh.client, "get_state")).turn).toEqual({ current: 0, open: false });
    expect(readFileSync(fresh.transcript, "utf8")).not.toMatch(/BEFORE_KILL|RETRIED/);
    await stop(fresh);
  } finally {
    for (const client of clients) client.close();
    await Promise.all(handles.map(process => process.stop()));
    rmSync(dir, { recursive: true, force: true });
  }
}, 45_000);
