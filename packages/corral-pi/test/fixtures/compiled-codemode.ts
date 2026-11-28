// Exercise production startup and HostSession with only embedded code/assets at runtime: the
// codemode tool runs cells on the embedded Corral kernel worker, nested reads pass the host's
// checks, variables persist across cells and a reload starts an empty kernel.
import { spawnSync } from "node:child_process";
import { writeFile } from "node:fs/promises";
import { existsSync, mkdirSync, mkdtempSync, readFileSync, rmSync, watch, writeFileSync, type FSWatcher } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import fixtureAuth, { fixtureTransport } from "./auth/index.ts";
import { EventLog } from "../../src/bridge/events.ts";
import { compiledBundle } from "../../src/host/bundle.ts";
import { prepareCompiled } from "../../src/host/compiled.ts";
import { guardedFetch } from "../../src/host/restrict.ts";
import { HostSession } from "../../src/host/session.ts";
import { workerRole } from "../../src/roles/worker/index.ts";
import { CorralClient } from "../../src/tools/corral.ts";
import { nextEvent } from "../support/wait.ts";

const root = mkdtempSync(join(tmpdir(), "corral-compiled-codemode-"));
let host: HostSession | undefined;
let outputWatcher: FSWatcher | undefined;
try {
  await prepareCompiled("run");
  const writeAssets = compiledBundle()?.writeAssets;
  if (!writeAssets) throw new Error("compiled host did not install embedded assets");
  writeAssets(root);
  const cwd = join(root, "cwd");
  mkdirSync(cwd);
  writeFileSync(join(cwd, "notes.txt"), "discard\nkeep\n");
  const network = guardedFetch(fixtureTransport().send);
  globalThis.fetch = network.fetch;
  const sdk = await import("@earendil-works/pi-coding-agent");
  const identity = { store: "st-fixture", project: "fixture", role: "worker" as const, task: "T-566", run: "R1" };
  const events = new EventLog();
  const modules: string[] = [];
  events.on((event) => { if (event.kind === "code_kernel_started") modules.push(String(event.module)); });
  host = await HostSession.create({
    sdk, identity, cwd, agentDir: join(root, "agent"), runtimeDir: join(root, "runtime"), model: "gpt-6.1-sol",
    capabilities: { catalog: 4, digest: "a".repeat(64), preset: "standard", enabled: ["codemode", "background_shell"] },
    guidance: { settings: { background_shell: { max_jobs: 2, max_output_bytes: 4096, max_runtime_secs: 60 } }, projectText: false },
    modules: { role: workerRole(root), auth: fixtureAuth }, network, events,
    client: new CorralClient({ corral: "", root, cwd, identity }),
    sessionManager: sdk.SessionManager.create(cwd, join(root, "sessions")),
  });
  await host.session.bindExtensions({});
  await host.startLocal();
  const text = (id: string) => {
    const result = host?.session.messages.find(m => m.role === "toolResult" && m.toolCallId.startsWith(`${id}|`));
    if (result?.role !== "toolResult" || result.isError) throw new Error(`compiled codemode failed: ${JSON.stringify(result)}`);
    return JSON.stringify(result.content);
  };
  for (const stage of ["startup", "reload"]) {
    if (stage === "reload") await host.session.reload();
    fixtureTransport().script([
      { toolCalls: [{ callId: `${stage}-before`, name: "eval_js", arguments: { code: "return typeof kept;" } }] },
      { toolCalls: [{ callId: `${stage}-read`, name: "eval_js", arguments: {
        code: 'const s = await tools.read({path:"notes.txt"}); let kept = s.split("\\n").filter(s => s === "keep"); return kept;',
      } }] },
      { toolCalls: [{ callId: `${stage}-kept`, name: "eval_js", arguments: { code: "return kept.length;" } }] },
      { text: "Finished" },
    ]);
    await host.session.prompt(stage);
    if (!text(`${stage}-before`).includes("Return value: undefined") || !text(`${stage}-read`).includes("keep")
      || !text(`${stage}-kept`).includes("Return value: 1")) {
      throw new Error(`compiled codemode ${stage} lost or leaked state`);
    }
    console.log(`COMPILED_CODEMODE ${stage} passed`);
  }
  // One compiled cross-layer journey: managed Bash yields, logs are peeked immediately,
  // then an explicit wait gets final bytes without an automatic duplicate turn.
  const gate = join(cwd, "command-gate");
  if (spawnSync("mkfifo", [gate]).status !== 0) throw new Error("mkfifo failed");
  // Shell readiness is not host capture readiness: its FIFO can be read before the pipe's
  // data callback runs. Watch the host-written spool before launch instead. capture() writes
  // this file and updates the peek buffer synchronously, before our watch callback can run.
  const logs = join(root, "runtime", "jobs");
  mkdirSync(logs, { recursive: true });
  const captured = Promise.withResolvers<string>();
  outputWatcher = watch(logs, (_event, name) => {
    if (!name?.endsWith(".log")) return;
    const log = join(logs, name);
    if (existsSync(log) && readFileSync(log, "utf8").includes("BEFORE\n")) captured.resolve(name.slice(0, -4));
  });
  outputWatcher.on("error", captured.reject);
  fixtureTransport().script([
    { toolCalls: [{ callId: "managed-start", name: "bash", arguments: {
      command: `echo BEFORE; cat ${gate} >/dev/null; echo AFTER`, wait_ms: 0,
    } }] }, { text: "Backgrounded" },
  ]);
  const ended = nextEvent(events, event => event.kind === "turn_ended");
  const started = await host.submitLocal({ text: "start managed", busyPolicy: "prompt", origin: "owner" });
  if (started.disposition !== "started") throw new Error(`managed Bash input was ${started.disposition}`);
  await ended;
  const job = await captured.promise;
  outputWatcher?.close();
  if (!text("managed-start").includes("background")) throw new Error("managed Bash did not yield");
  let waiting!: () => void;
  const waitStarted = new Promise<void>(resolve => { waiting = resolve; });
  const unsubscribe = host.session.subscribe(event => {
    if (event.type === "tool_execution_start" && event.toolName === "job_wait") waiting();
  });
  fixtureTransport().script([
    { toolCalls: [{ callId: "managed-peek", name: "job_output", arguments: { job, tail_lines: 1 } }] },
    { toolCalls: [{ callId: "managed-wait", name: "job_wait", arguments: { job } }] }, { text: "Completed" },
  ]);
  const final = nextEvent(events, event => event.kind === "turn_ended");
  const observed = await host.submitLocal({ text: "peek and wait", busyPolicy: "prompt", origin: "owner" });
  if (observed.disposition !== "started") throw new Error(`peek and wait input was ${observed.disposition}`);
  await waitStarted;
  await writeFile(gate, "go");
  await final;
  unsubscribe();
  const peek = text("managed-peek"), waited = text("managed-wait");
  if (!peek.includes("BEFORE")) throw new Error(`managed peek missing BEFORE: ${peek}; wait: ${waited}`);
  if (peek.includes("AFTER")) throw new Error(`managed peek waited for completion: ${peek}`);
  if (!waited.includes("BEFORE")) throw new Error(`managed wait missing buffered BEFORE: ${waited}`);
  if (!waited.includes("AFTER")) throw new Error(`managed wait missing AFTER: ${waited}`);
  console.log("COMPILED_COMMAND launch/yield/peek/wait passed");
  if (modules.length < 2 || !modules.every((module) => /\$bunfs|~BUN|%7EBUN/.test(module))) {
    throw new Error(`the kernel worker did not load from the executable: ${JSON.stringify(modules)}`);
  }
  console.log(`COMPILED_CODEMODE kernel ${modules[0]}`);
} finally {
  outputWatcher?.close();
  await host?.close();
  rmSync(root, { recursive: true, force: true });
}
