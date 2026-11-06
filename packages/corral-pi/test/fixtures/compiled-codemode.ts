// Exercise production startup and HostSession with only embedded code/assets at runtime: the
// codemode tool runs cells on the embedded Corral kernel worker, nested reads pass the host's
// checks, variables persist across cells and a reload starts an empty kernel.
import { spawnSync } from "node:child_process";
import { readFile, writeFile } from "node:fs/promises";
import { mkdirSync, mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import fixtureAuth, { fixtureTransport } from "./auth/index.ts";
import delivery from "./delivery/index.ts";
import { EventLog } from "../../src/bridge/events.ts";
import { guardedFetch } from "../../src/host/restrict.ts";
import { HostSession } from "../../src/host/session.ts";
import { workerRole } from "../../src/roles/worker/index.ts";
import { CorralClient, sha256Hex } from "../../src/tools/corral.ts";
import { writeFakeCorral } from "../support/fake-corral.ts";
import { nextEvent } from "../support/wait.ts";

const root = mkdtempSync(join(tmpdir(), "corral-compiled-codemode-"));
let host: HostSession | undefined;
try {
  process.argv = [process.execPath, process.execPath, "--write-install", root];
  await import("../../src/host/compiled.ts");
  const cwd = join(root, "cwd");
  mkdirSync(cwd);
  writeFileSync(join(cwd, "notes.txt"), "discard\nkeep\n");
  const network = guardedFetch(fixtureTransport().send);
  globalThis.fetch = network.fetch;
  const sdk = await import("@earendil-works/pi-coding-agent");
  const identity = { store: "st-fixture", project: "fixture", role: "worker" as const, generation: 1, task: "T-566", run: "R1" };
  const events = new EventLog(1);
  const modules: string[] = [];
  events.on((event) => { if (event.kind === "code_kernel_started") modules.push(String(event.module)); });
  const corral = writeFakeCorral(root);
  host = await HostSession.create({
    sdk, identity, cwd, agentDir: join(root, "agent"), runtimeDir: join(root, "runtime"), model: "gpt-6.1-sol",
    capabilities: { catalog: 4, digest: "a".repeat(64), preset: "standard", enabled: ["codemode", "background_shell"] },
    guidance: { settings: { background_shell: { max_jobs: 2, max_output_bytes: 4096, max_runtime_secs: 60 } }, projectText: false },
    modules: { role: workerRole(root), auth: fixtureAuth, delivery }, network, events,
    client: new CorralClient({ corral: corral.path, root, identity, hostCap: "h".repeat(64), sessionId: () => host?.sessionId() ?? "" }),
    sessionManager: sdk.SessionManager.create(cwd, join(root, "sessions")),
  });
  await host.session.bindExtensions({});
  await host.hello("n".repeat(64), "fixture-pane", sha256Hex("c".repeat(64)));
  const text = (id: string) => {
    const result = host?.session.messages.find(m => m.role === "toolResult" && m.toolCallId.startsWith(`${id}|`));
    if (result?.role !== "toolResult" || result.isError) throw new Error(`compiled codemode failed: ${JSON.stringify(result)}`);
    return JSON.stringify(result.content);
  };
  for (const stage of ["startup", "reload"]) {
    if (stage === "reload") await host.session.reload();
    fixtureTransport().script([
      { toolCalls: [{ callId: `${stage}-before`, name: "codemode", arguments: { code: "return typeof kept;" } }] },
      { toolCalls: [{ callId: `${stage}-read`, name: "codemode", arguments: {
        code: 'const s = await tools.read({path:"notes.txt"}); let kept = s.split("\\n").filter(s => s === "keep"); return kept;',
      } }] },
      { toolCalls: [{ callId: `${stage}-kept`, name: "codemode", arguments: { code: "return kept.length;" } }] },
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
  const gate = join(cwd, "command-gate"), ready = join(cwd, "command-ready");
  for (const fifo of [gate, ready]) if (spawnSync("mkfifo", [fifo]).status !== 0) throw new Error("mkfifo failed");
  fixtureTransport().script([
    { toolCalls: [{ callId: "managed-start", name: "bash", arguments: {
      command: `echo BEFORE; echo ready > ${ready}; cat ${gate} >/dev/null; echo AFTER`, wait_ms: 0,
    } }] }, { text: "Backgrounded" },
  ]);
  const ended = nextEvent(events, event => event.kind === "turn_ended");
  await host.submit({ text: "start managed", busyPolicy: "prompt", origin: "fixture", idempotencyKey: "managed-start" }, "managed-start");
  await ended;
  await readFile(ready, "utf8");
  if (!text("managed-start").includes("background")) throw new Error("managed Bash did not yield");
  let waiting!: () => void;
  const waitStarted = new Promise<void>(resolve => { waiting = resolve; });
  const unsubscribe = host.session.subscribe(event => {
    if (event.type === "tool_execution_start" && event.toolName === "job_wait") waiting();
  });
  fixtureTransport().script([
    { toolCalls: [{ callId: "managed-peek", name: "job_output", arguments: { job: "j1-1", tail_lines: 1 } }] },
    { toolCalls: [{ callId: "managed-wait", name: "job_wait", arguments: { job: "j1-1" } }] }, { text: "Completed" },
  ]);
  const final = nextEvent(events, event => event.kind === "turn_ended");
  await host.submit({ text: "peek and wait", busyPolicy: "prompt", origin: "fixture", idempotencyKey: "managed-wait" }, "managed-wait");
  await waitStarted;
  await writeFile(gate, "go");
  await final;
  unsubscribe();
  if (!text("managed-peek").includes("BEFORE") || text("managed-peek").includes("AFTER") || !text("managed-wait").includes("AFTER")) {
    throw new Error("managed command lost output or peek waited for completion");
  }
  console.log("COMPILED_COMMAND launch/yield/peek/wait passed");
  if (modules.length < 2 || !modules.every((module) => /\$bunfs|~BUN|%7EBUN/.test(module))) {
    throw new Error(`the kernel worker did not load from the executable: ${JSON.stringify(modules)}`);
  }
  console.log(`COMPILED_CODEMODE kernel ${modules[0]}`);
} finally {
  await host?.close();
  rmSync(root, { recursive: true, force: true });
}
