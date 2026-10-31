// Offline manual smoke: the production rich pane and tools, disposable files, no herdr server.
// Run: bun test/fixtures/todo-ui.ts; click Todo, try /todo and /todo down, then Ctrl+D to exit.
import { cpSync, mkdirSync, mkdtempSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";

import { EventLog } from "../../src/bridge/events.ts";
import { loadModules } from "../../src/host/modules.ts";
import { guardedFetch, restrictEnvironment } from "../../src/host/restrict.ts";
import { HostSession, authContext } from "../../src/host/session.ts";
import { CorralClient, sha256Hex } from "../../src/tools/corral.ts";
import { fixtureTransport } from "./auth/index.ts";
import { buildFixtureInstall, PACKAGE_ROOT } from "../support/fixture-install.ts";
import { writeFakeCorral } from "../support/fake-corral.ts";

const assets = buildFixtureInstall();
const work = mkdtempSync(join(tmpdir(), "corral-todo-ui-"));
let host: HostSession | undefined;
try {
  for (const tree of ["src/roles/worker", "src/roles/common", "assets"]) {
    cpSync(join(PACKAGE_ROOT, tree), join(assets.dir, tree), { recursive: true });
  }
  const cwd = join(work, "worktree");
  mkdirSync(cwd);
  restrictEnvironment(process.env);
  const identity = { store: "st-demo", project: "demo", role: "worker",
    task: "T-DEMO", run: "R1", generation: 1 } as const;
  const modules = await loadModules(assets.dir, "worker");
  const transport = modules.auth.transport?.(authContext());
  if (!transport) throw new Error("fixture transport missing");
  const network = guardedFetch(transport);
  globalThis.fetch = network.fetch;
  const sdk = await import("@earendil-works/pi-coding-agent");
  const corral = writeFakeCorral(work);
  corral.snapshot({ runs: [{ id: "R1", progress: {
    revision: 3, spec_revision: "demo",
    phases: [
      { id: "investigate", label: "Investigation", steps: [
        { id: "read", label: "Read existing UI", status: { state: "done" } },
        { id: "mouse", label: "Check mouse support", status: { state: "done" } },
      ] },
      { id: "implement", label: "Implementation", steps: [
        { id: "widget", label: "Build expandable widget", status: { state: "in_progress" }, criteria: ["A1", "A2"] },
        { id: "restore", label: "Restore progress on resume", status: { state: "pending" } },
      ] },
      { id: "verify", label: "Verification", steps: [
        { id: "test", label: "Test mouse and keyboard interaction", status: { state: "pending" } },
        { id: "extra", label: "Redundant full-suite run", status: {
          state: "dropped", reason: "Covered by the targeted UI checks",
        } },
      ] },
    ],
  } }] });
  host = await HostSession.create({
    sdk, identity, cwd, agentDir: join(work, "agent"), model: "gpt-6.1-sol", todo: true,
    modules, network, events: new EventLog(1),
    client: new CorralClient({ corral: corral.path, root: work, identity,
      hostCap: "h".repeat(64), sessionId: () => host?.sessionId() ?? "" }),
    sessionManager: sdk.SessionManager.create(cwd, join(work, "sessions")),
  });
  await host.hello("n".repeat(64), "fixture-pane", sha256Hex("c".repeat(64)));
  fixtureTransport().script([
    { toolCalls: [{ callId: "todo-done", name: "corral_todo", arguments: { op: "done", step: "widget" } }] },
    { text: "The widget step is complete. The remaining steps stay open." },
  ]);
  const terminal = new sdk.InteractiveMode(host.runtime, { verbose: false, tuiMode: "fullscreen" });
  await terminal.run();
} finally {
  await host?.close();
  assets.cleanup();
  rmSync(work, { recursive: true, force: true });
}
