import { expect, test } from "bun:test";
import { mkdirSync, mkdtempSync, readFileSync, rmSync, statSync, watch, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { buildFixtureInstall } from "./support/fixture-install.ts";
import { connect, type TestClient } from "./support/bridge-client.ts";
import { launch, type Launch } from "./support/pi-process.ts";
import { within } from "./support/wait.ts";

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

test("one flag launch runs the fixture to idle; v2 answers, pauses, restarts and delivers held input before owner text", async () => {
  const dir = mkdtempSync(join(tmpdir(), "pi-cli-"));
  const assets = buildFixtureInstall({ parent: dir });
  const cwd = join(dir, "worktree");
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

test("chat opens a socket and takes owner input without the legacy planning store", async () => {
  const dir = mkdtempSync(join(tmpdir(), "pi-chat-"));
  const assets = buildFixtureInstall({ parent: dir });
  const socket = join(dir, "s.sock");
  const chat = launch(assets.dir, ["chat", "--dir", "A-1", "--socket", socket], dir,
    { HOME: dir, PATH: process.env.PATH, XDG_DATA_HOME: join(dir, "data"), TMPDIR: dir,
      GIT_CEILING_DIRECTORIES: dir, CORRAL_NEXT_ROOT: dir, CORRAL_ACTOR: "assistant" });
  let client: TestClient | undefined;
  try {
    await chat.output(/session socket ready/);
    client = await connect(socket);
    const hello = await client.request({ version: 2, id: "hello", command: "hello" });
    expect(hello.result).toMatchObject({ protocol: 2, role: "assistant", session_id: "A-1", pid: chat.child.pid });
    const sent = await client.request({ version: 2, id: "message", command: "submit_input", args: { origin: "owner", text: "Hello chat" } });
    expect(sent.result?.disposition).toBe("started");
    await chat.output(/turn 1 ended/);
    expect(readFileSync(String(hello.result?.session_file), "utf8")).toContain("Hello chat");
    await client.request({ version: 2, id: "shutdown", command: "shutdown" });
    expect(await within(chat.exit, 10_000, "chat shutdown")).toBe(0);
  } finally {
    client?.close();
    await chat.stop();
    rmSync(dir, { recursive: true, force: true });
  }
}, 20_000);
