import { spawn, type ChildProcess } from "node:child_process";
import { accessSync, constants, existsSync, mkdirSync, mkdtempSync, readFileSync, readdirSync, realpathSync, rmSync, writeFileSync } from "node:fs";
import { join, resolve } from "node:path";
import { setTimeout as sleep } from "node:timers/promises";
import { parseArgs } from "node:util";
import { BRIDGE_VERSION } from "../../src/bridge/protocol.ts";
import type { FakeStep } from "../fixtures/auth/fake-transport.ts";
import { connect, type TestClient } from "./bridge-client.ts";
import { FAKE_HOST_SCRIPT_ENV, shellQuote, writeHostLauncher } from "./fake-host.ts";
import { PACKAGE_ROOT, scriptedFixtureInstall } from "./fixture-install.ts";
import { within } from "./wait.ts";
import { ok, REPO, run } from "./workspace.ts";

interface StageOptions {
  readonly listen: string;
  readonly origin: string;
  readonly daemon: string;
  readonly engine: string;
  readonly script?: string;
  readonly providerScript: string;
}

function options(args: string[]): StageOptions {
  const { values } = parseArgs({ args, options: {
    listen: { type: "string", default: "127.0.0.1:7843" }, origin: { type: "string", default: "http://127.0.0.1:8893" },
    daemon: { type: "string" }, engine: { type: "string" }, script: { type: "string" }, "provider-script": { type: "string" },
  } });
  if (values.listen.endsWith(":0")) throw new Error("--listen needs a fixed port to print and pair against");
  return {
    listen: values.listen, origin: values.origin,
    daemon: resolve(values.daemon ?? process.env.CORRALD_NEXT_BIN ?? join(REPO, "target/debug/corrald-next")),
    engine: resolve(values.engine ?? process.env.CORRAL_NEXT_BIN ?? join(REPO, "target/debug/corral-next")),
    ...(values.script ? { script: resolve(values.script) } : {}),
    providerScript: resolve(values["provider-script"] ?? join(PACKAGE_ROOT, "test/fixtures/sessions/real-provider.json")),
  };
}

interface StageProcess { child: ChildProcess; exited: Promise<number | null> }
function start(command: string, args: string[], cwd: string, env: NodeJS.ProcessEnv): StageProcess {
  // Ctrl-C must reach the staging driver, not kill the daemon before it can shut down.
  const child = spawn(command, args, { cwd, env, detached: true, stdio: ["ignore", "ignore", "inherit"] });
  const { promise: exited, resolve, reject } = Promise.withResolvers<number | null>();
  child.once("error", reject);
  child.once("exit", resolve);
  return { child, exited };
}

function beforeExit<T>(proc: StageProcess, result: Promise<T>, name: string): Promise<T> {
  return Promise.race([result, proc.exited.then(code => { throw new Error(`${name} exited before it was ready (${code})`); })]);
}

async function untilReady<T>(probe: () => Promise<T | undefined>, signal: AbortSignal, name: string): Promise<T> {
  const deadline = Date.now() + 30_000;
  while (Date.now() < deadline) {
    signal.throwIfAborted();
    const value = await probe();
    if (value !== undefined) return value;
    await sleep(50, undefined, { signal });
  }
  throw new Error(`Timed out waiting for ${name}`);
}

async function withHost<T>(socket: string, use: (client: TestClient) => Promise<T>): Promise<T | undefined> {
  let client: TestClient;
  try { client = await connect(socket); }
  catch (error) {
    const { code } = error as NodeJS.ErrnoException;
    if (code === "ENOENT" || code === "ECONNREFUSED") return undefined;
    throw error;
  }
  try { return await use(client); }
  finally { client.close(); }
}

async function stopHosts(directory: string): Promise<void> {
  if (!existsSync(directory)) return;
  const sockets = readdirSync(directory, { withFileTypes: true }).filter(entry => entry.isSocket());
  await Promise.all(sockets.map(async ({ name }) => {
    try {
      await withHost(join(directory, name), client => within((async () => {
        const reply = await client.request({ version: BRIDGE_VERSION, id: "stage-stop", command: "shutdown", args: {} });
        if (!reply.ok) throw new Error(reply.error?.reason ?? "host refused shutdown");
        // The real host closes the bridge only after draining its work and stopping jobs.
        await client.closed();
      })(), 10_000, `shutdown of ${name}`));
    } catch (error) { console.error(error); }
  }));
}

async function stage(settings: StageOptions): Promise<void> {
  for (const binary of [settings.engine, settings.daemon]) accessSync(binary, constants.X_OK);
  const root = realpathSync(mkdtempSync("/tmp/corral-parity-"));
  const state = join(root, ".corral-next"), repo = join(root, "repo"), bin = join(root, "bin");
  const runtime = join(root, "r"), data = join(root, "data"), sockets = join(runtime, "corral-next");
  const buildData = join(data, "corral", "corral-pi"), socket = join(state, "daemon.sock");
  const env = {
    PATH: `${bin}:${process.env.PATH ?? "/usr/bin:/bin"}`, HOME: root, TMPDIR: root,
    XDG_CONFIG_HOME: join(root, "config"), XDG_DATA_HOME: data, XDG_STATE_HOME: join(root, "state"),
    XDG_CACHE_HOME: join(root, "cache"), XDG_RUNTIME_DIR: runtime,
    GIT_CONFIG_GLOBAL: "/dev/null", GIT_CONFIG_NOSYSTEM: "1", GIT_CEILING_DIRECTORIES: root,
    GIT_AUTHOR_NAME: "Parity fixture", GIT_AUTHOR_EMAIL: "parity@example.invalid",
    GIT_COMMITTER_NAME: "Parity fixture", GIT_COMMITTER_EMAIL: "parity@example.invalid",
    CORRAL_NEXT_BIN: settings.engine, CORRAL_NEXT_ROOT: root,
    ...(settings.script ? { [FAKE_HOST_SCRIPT_ENV]: settings.script } : {}),
  };
  const stop = new AbortController(), interrupted = Promise.withResolvers<void>();
  const interrupt = () => { stop.abort(); interrupted.resolve(); };
  process.on("SIGINT", interrupt);
  process.on("SIGTERM", interrupt);
  let daemon: StageProcess | undefined, seed: StageProcess | undefined;
  try {
    for (const dir of [repo, bin, runtime]) mkdirSync(dir, { mode: 0o700 });
    const launcher = writeHostLauncher(bin, settings.script ? undefined : join(scriptedFixtureInstall(root,
      JSON.parse(readFileSync(settings.providerScript, "utf8")) as FakeStep[]), "src/cli.ts"));
    const git = (...args: string[]) => ok(["git", ...args], { cwd: repo, env });
    git("init", "-b", "trunk");
    writeFileSync(join(repo, ".gitignore"), "/.worktrees/\n");
    writeFileSync(join(repo, "line.txt"), "original\n");
    git("add", "."); git("commit", "-m", "Initial parity fixture");
    ok([settings.engine, "--root", root, "init", "--first-id", "1"], { cwd: root, env });
    writeFileSync(join(state, "corral.toml"), `
[runner]
mode = "process"
[defaults]
profile = "parity"
[profiles.parity]
harness = "pi"
model = "openai-codex/gpt-6.1-sol"
[projects.demo]
path = "repo"
gate = ["test -s line.txt"]
[daemon]
listen = ${JSON.stringify(settings.listen)}
web_origin = [${JSON.stringify(settings.origin)}]
`);
    daemon = start(settings.daemon, ["--root", root, "run"], root, env);
    await beforeExit(daemon, untilReady(async () => {
      if (!existsSync(socket)) return undefined;
      const status = run([settings.daemon, "--root", root, "status"], { cwd: root, env, timeout: 3_000 });
      return status.status === 0 && status.stdout.trim() !== "daemon down" ? true : undefined;
    }, stop.signal, "corrald-next"), "corrald-next");

    // Start a build with the same launcher and data root that the daemon uses. The real host can
    // allocate its own ID independently of the daemon's build.start implementation.
    const seedSocket = join(sockets, "seed.sock");
    seed = start(launcher, ["build", "--headless", "--project", "demo", "--name", "parity-ready", "--corral", settings.engine,
      "--model", "openai-codex/gpt-6.1-sol", "--data-dir", buildData, "--socket", seedSocket,
      ...(settings.script ? ["--id", "B-parity"] : [])], repo, env);
    const hello = await beforeExit(seed, untilReady(() => withHost(seedSocket, async client => {
      const reply = await within(client.request({ version: BRIDGE_VERSION, id: "stage-hello", command: "hello", args: {} }), 2_000, "seed hello");
      if (reply.ok) return reply.result;
      if (reply.error?.code !== "starting") throw new Error(reply.error?.reason ?? "seed did not answer hello");
      return undefined;
    }), stop.signal, "seed host"), "seed host");
    console.log(`Workspace: ${root}\nDaemon socket: ${socket}\nListen: ${settings.listen}\nWeb origin: ${settings.origin}`);
    console.log(`Seed build: ${hello.session_id}\nSeed transcript: ${hello.session_file}`);
    console.log(`Pair: ${shellQuote(settings.daemon)} --root ${shellQuote(root)} pair`);
    console.log("Stop: press Ctrl-C here (or send SIGTERM to the staging process).");
    await Promise.race([interrupted.promise, daemon.exited.then(code => {
      if (code !== 0) throw new Error(`corrald-next exited (${code})`);
    })]);
  } catch (error) {
    if (!stop.signal.aborted) throw error;
  } finally {
    interrupt();
    try {
      if (daemon && daemon.child.exitCode === null && daemon.child.signalCode === null) {
        run([settings.daemon, "--root", root, "stop"], { cwd: root, env, timeout: 3_000 });
        try { await within(daemon.exited, 10_000, "corrald-next shutdown"); }
        catch { daemon.child.kill("SIGKILL"); await daemon.exited.catch(() => undefined); }
      }
      await stopHosts(sockets);
    } finally {
      if (seed && seed.child.exitCode === null && seed.child.signalCode === null) {
        try { await within(seed.exited, 2_000, "seed exit"); }
        catch { seed.child.kill("SIGKILL"); await seed.exited.catch(() => undefined); }
      }
      rmSync(root, { recursive: true, force: true });
      process.off("SIGINT", interrupt);
      process.off("SIGTERM", interrupt);
    }
  }
}

if (import.meta.main) await stage(options(process.argv.slice(2)));
