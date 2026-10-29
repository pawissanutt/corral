// Exercise production startup and HostSession with only embedded code/assets at runtime.
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
  host = await HostSession.create({
    sdk, identity, cwd, agentDir: join(root, "agent"), model: "gpt-6.1-sol",
    capabilities: { catalog: 4, digest: "a".repeat(64), preset: "standard", enabled: ["codemode"] },
    modules: { role: workerRole(root), auth: fixtureAuth, delivery }, network, events: new EventLog(1),
    client: new CorralClient({ corral: "/no-corral-needed", root, identity, hostCap: "h".repeat(64), sessionId: () => "fixture" }),
    sessionManager: sdk.SessionManager.inMemory(cwd),
  });
  await host.session.bindExtensions({});
  for (const stage of ["startup", "reload"]) {
    if (stage === "reload") await host.session.reload();
    fixtureTransport().script([
      { toolCalls: [{ callId: stage, name: "codemode", arguments: {
        code: 'const s = await tools.read({path:"notes.txt"}); return s.split("\\n").filter(s => s === "keep");',
      } }] },
      { text: "Finished" },
    ]);
    await host.session.prompt(stage);
    const result = host.session.messages.find(m => m.role === "toolResult" && m.toolCallId.startsWith(`${stage}|`));
    if (result?.role !== "toolResult" || result.isError || !JSON.stringify(result.content).includes("keep")) {
      throw new Error(`compiled codemode failed: ${JSON.stringify(result)}`);
    }
    console.log(`COMPILED_CODEMODE ${stage} passed`);
  }
} finally {
  await host?.close();
  rmSync(root, { recursive: true, force: true });
}
