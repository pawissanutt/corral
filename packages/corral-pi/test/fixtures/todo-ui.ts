// Offline manual smoke: the production rich pane and tools, disposable files, no herdr server.
// Run: bun test/fixtures/todo-ui.ts [worker|pm|assistant]; click Todo, try /todo and /todo down,
// then Ctrl+D to exit. The pm and assistant variants show a session request plan (T-679).
import { cpSync, mkdirSync, mkdtempSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";

import { EventLog } from "../../src/bridge/events.ts";
import { loadModules } from "../../src/host/modules.ts";
import { guardedFetch, restrictEnvironment } from "../../src/host/restrict.ts";
import { HostSession, authContext } from "../../src/host/session.ts";
import type { HostIdentity } from "../../src/roles/types.ts";
import { CorralClient, sha256Hex } from "../../src/tools/corral.ts";
import { fixtureTransport } from "./auth/index.ts";
import { buildFixtureInstall, PACKAGE_ROOT } from "../support/fixture-install.ts";
import { writeFakeCorral } from "../support/fake-corral.ts";

const role = (process.argv[2] ?? "worker") as "worker" | "pm" | "assistant";
if (!["worker", "pm", "assistant"].includes(role)) throw new Error(`unknown role ${role}`);
const session = role !== "worker";
const assets = buildFixtureInstall();
const work = mkdtempSync(join(tmpdir(), "corral-todo-ui-"));
let host: HostSession | undefined;
try {
  for (const tree of [`src/roles/${role}`, "src/roles/common", "assets"]) {
    cpSync(join(PACKAGE_ROOT, tree), join(assets.dir, tree), { recursive: true });
  }
  const cwd = join(work, "worktree");
  mkdirSync(cwd);
  restrictEnvironment(process.env);
  const identity: HostIdentity = role === "worker"
    ? { store: "st-demo", project: "demo", role, task: "T-DEMO", run: "R1", generation: 1 }
    : role === "pm" ? { store: "st-demo", project: "demo", role, generation: 1 }
      : { store: "st-demo", project: "", role, assistant: "A-1", generation: 1 };
  const modules = await loadModules(assets.dir, role);
  const transport = modules.auth.transport?.(authContext());
  if (!transport) throw new Error("fixture transport missing");
  const network = guardedFetch(transport);
  globalThis.fetch = network.fetch;
  const sdk = await import("@earendil-works/pi-coding-agent");
  const corral = writeFakeCorral(work);
  const progress = session ? {
    revision: 7, spec_revision: role === "pm" ? "pm:" : "assistant:A-1", plan: "P2",
    phases: [
      { id: "triage", label: "Triage the owner's request", steps: [
        { id: "read", label: "Read the board and the open questions", status: { state: "done" }, criteria: [] },
        { id: "route", label: "Route T-680 to a worker", status: { state: "in_progress" }, criteria: [] },
      ] },
      { id: "follow", label: "Follow through", steps: [
        { id: "land", label: "Confirm T-612 landed", status: { state: "pending" }, criteria: [] },
        { id: "ask", label: "Ask the owner about the budget", status: { state: "pending" }, criteria: [] },
        { id: "old", label: "Re-run the stale report", status: { state: "dropped", reason: "Superseded by T-677" }, criteria: [] },
      ] },
    ],
    waits: [
      { step: "land", on: { kind: "event", event: "T-612 lands on main" } },
      { step: "ask", on: { kind: "owner", question: "Raise the paid-call budget for T-680?" } },
    ],
  } : {
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
  };
  corral.snapshot(role === "worker" ? { runs: [{ id: "R1", progress }] }
    : role === "pm" ? { container: { todo: progress, todo_requests: null } }
      : { container: { state: { todo: progress, todo_requests: null } } });
  host = await HostSession.create({
    sdk, identity, cwd, agentDir: join(work, "agent"), model: "gpt-6.1-sol", todo: true,
    modules, network, events: new EventLog(1),
    client: new CorralClient({ corral: corral.path, root: work, identity,
      hostCap: "h".repeat(64), sessionId: () => host?.sessionId() ?? "" }),
    sessionManager: sdk.SessionManager.create(cwd, join(work, "sessions")),
  });
  await host.hello("n".repeat(64), "fixture-pane", sha256Hex("c".repeat(64)));
  fixtureTransport().script([
    { toolCalls: [{ callId: "todo-done", name: "corral_todo",
      arguments: { op: "done", step: session ? "route" : "widget" } }] },
    { text: "That step is complete. The remaining steps stay open." },
  ]);
  const terminal = new sdk.InteractiveMode(host.runtime, { verbose: false, tuiMode: "fullscreen" });
  await terminal.run();
} finally {
  await host?.close();
  assets.cleanup();
  rmSync(work, { recursive: true, force: true });
}
