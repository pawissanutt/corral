// Exercise production startup and HostSession with only embedded code/assets at runtime: the
// codemode tool runs cells on the embedded Corral kernel worker, nested reads pass the host's
// checks, variables persist across cells and a reload starts an empty kernel.
import { mkdirSync, mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import fixtureAuth, { fixtureTransport } from "./auth/index.ts";
import delivery from "./delivery/index.ts";
import { EventLog } from "../../src/bridge/events.ts";
import { guardedFetch } from "../../src/host/restrict.ts";
import { HostSession } from "../../src/host/session.ts";
import { workerRole } from "../../src/roles/worker/index.ts";
import { CorralClient } from "../../src/tools/corral.ts";

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
  host = await HostSession.create({
    sdk, identity, cwd, agentDir: join(root, "agent"), model: "gpt-6.1-sol",
    capabilities: { catalog: 4, digest: "a".repeat(64), preset: "standard", enabled: ["codemode"] },
    modules: { role: workerRole(root), auth: fixtureAuth, delivery }, network, events,
    client: new CorralClient({ corral: "/no-corral-needed", root, identity, hostCap: "h".repeat(64), sessionId: () => "fixture" }),
    sessionManager: sdk.SessionManager.inMemory(cwd),
  });
  await host.session.bindExtensions({});
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
  if (modules.length < 2 || !modules.every((module) => /\$bunfs|~BUN|%7EBUN/.test(module))) {
    throw new Error(`the kernel worker did not load from the executable: ${JSON.stringify(modules)}`);
  }
  console.log(`COMPILED_CODEMODE kernel ${modules[0]}`);
} finally {
  await host?.close();
  rmSync(root, { recursive: true, force: true });
}
