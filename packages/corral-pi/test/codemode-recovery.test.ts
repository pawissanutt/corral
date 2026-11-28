// The real flag-launched worker is killed while a Code Mode child has a FIFO open. The same
// transcript reopens without replaying either the cell or the already completed engine call.
import { expect, test } from "bun:test";
import { type ChildProcess, spawn, spawnSync } from "node:child_process";
import { copyFileSync, cpSync, existsSync, mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { connect, type TestClient } from "./support/bridge-client.ts";
import { writeFakeEngine } from "./support/fake-engine.ts";
import { buildFixtureInstall, PACKAGE_ROOT } from "./support/fixture-install.ts";
import { launch, type Launch } from "./support/pi-process.ts";
import { within } from "./support/wait.ts";

test("a crashed Code Mode cell retains the interrupted child in its transcript without being replayed", async () => {
  const dir = mkdtempSync(join(tmpdir(), "pi-code-recovery-"));
  const work = join(dir, "work"), run = join(dir, "R1"), socket = join(dir, "s.sock");
  const assets = buildFixtureInstall({ parent: dir, omit: ["role"] });
  const edit = join(work, "edit.fifo"), steps = join(dir, "steps.json");
  const children: Launch[] = [], clients: TestClient[] = [];
  let writer: ChildProcess | undefined;
  try {
    mkdirSync(work);
    cpSync(join(PACKAGE_ROOT, "assets"), join(assets.dir, "assets"), { recursive: true });
    const auth = join(assets.dir, "src/auth");
    copyFileSync(join(auth, "index.ts"), join(auth, "fixture-auth.ts"));
    writeFileSync(join(auth, "index.ts"), `import { readFileSync } from "node:fs";
import auth, { fixtureTransport } from "./fixture-auth.ts";
export default {
  ...auth,
  transport() {
    const transport = fixtureTransport();
    transport.script(JSON.parse(readFileSync(${JSON.stringify(steps)}, "utf8")));
    return transport.send;
  },
};\n`);
    const engine = writeFakeEngine(dir);
    writeFileSync(join(work, "prompt.md"), "Run the fixture cell.\n");
    writeFileSync(join(work, "evidence.txt"), "fixture artifact\n");
    expect(spawnSync("mkfifo", [edit]).status).toBe(0);
    writeFileSync(steps, JSON.stringify([{ toolCalls: [{ callId: "interrupted", name: "eval_js", arguments: { code: `
      await tools.corral_artifact({path: "evidence.txt", kind: "evidence"});
      await tools.edit({path: "edit.fifo", edits: [{oldText: "a", newText: "b"}]});
      await tools.write({path: "after.txt", content: "would mean the lost cell ran again"});
    ` } }] }]));
    const env = { HOME: work, PATH: process.env.PATH, TMPDIR: dir, XDG_DATA_HOME: join(dir, "data"),
      XDG_CONFIG_HOME: join(dir, "config"), GIT_CEILING_DIRECTORIES: dir,
      CORRAL_NEXT_ROOT: dir, CORRAL_TASK: "T-683", CORRAL_ACTOR: "agent" };
    const start = () => {
      const child = launch(assets.dir, ["run", "--dir", run, "--socket", socket, "--prompt-file", "prompt.md",
        "--corral", engine.path], work, env);
      children.push(child);
      return child;
    };
    let requestId = 0;
    const call = async (client: TestClient, command: string, args: Record<string, unknown> = {}) => {
      const reply = await client.request({ version: 2, id: `req-${++requestId}`, command, args });
      if (!reply.ok || !reply.result) throw new Error(`${command}: ${JSON.stringify(reply.error)}`);
      return reply.result;
    };
    const ready = Promise.withResolvers<void>();
    writer = spawn("bash", ["-c", 'exec 3>"$1"; echo opened; read -r _', "fifo-writer", edit],
      { stdio: ["pipe", "pipe", "ignore"] });
    writer.stdout?.once("data", () => ready.resolve());
    writer.once("error", ready.reject);
    const first = start();
    await first.output(/session socket ready/);
    const client = await connect(socket);
    clients.push(client);
    const { session_file: transcript } = await call(client, "hello");
    if (typeof transcript !== "string") throw new Error("fixture worker has no transcript");
    await within(ready.promise, 10_000, "the Code Mode child to open its FIFO");
    expect(first.child.kill("SIGKILL")).toBeTrue();
    await within(first.exit, 10_000, "the interrupted host to exit");
    writer.stdin?.end();
    client.close();

    const journal = () => readFileSync(transcript, "utf8").trimEnd().split("\n").map(line => JSON.parse(line))
      .filter(entry => entry.type === "custom" && entry.customType === "corral_code_call")
      .map(entry => [entry.data.phase, entry.data.tool, entry.data.parent.call]);
    const interrupted = [["started", "corral_artifact", "cell-1.1"], ["settled", "corral_artifact", "cell-1.1"],
      ["started", "edit", "cell-1.2"]];
    expect(journal()).toEqual(interrupted);
    writeFileSync(steps, JSON.stringify([{ text: "Recovered conversation" }]));
    const resumed = start();
    await resumed.output(/session socket ready/);
    const reopened = await connect(socket);
    clients.push(reopened);
    expect((await call(reopened, "hello")).session_file).toBe(transcript);
    expect(await call(reopened, "submit_input", { text: "Continue after the crash", origin: "owner" }))
      .toMatchObject({ disposition: "started" });
    await resumed.output(/turn \d+ ended/);
    expect(await call(reopened, "shutdown")).toEqual({ shutting_down: true });
    expect(await within(resumed.exit, 10_000, "the reopened host to stop")).toBe(0);
    expect(readFileSync(transcript, "utf8")).toContain("Recovered conversation");
    expect(journal()).toEqual(interrupted);
    expect(engine.calls().map(argv => argv.slice(3)).filter(argv => argv[0] === "artifact"))
      .toEqual([["artifact", "add", "T-683", "evidence.txt", "--kind", "evidence"]]);
    expect(existsSync(join(work, "after.txt"))).toBeFalse();
  } finally {
    writer?.kill("SIGKILL");
    for (const client of clients) client.close();
    await Promise.all(children.map(child => child.stop()));
    rmSync(dir, { recursive: true, force: true });
  }
}, 60_000);
