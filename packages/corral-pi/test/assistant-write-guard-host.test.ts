// The assistant's edit/write path guard in the real host (T-544 scratch-worktrees): edits aimed
// at the main checkout or a task worktree are refused before they run, top-level and nested,
// while the session's recorded scratch and its artifact directory stay writable.
import { expect, test } from "bun:test";
import { existsSync, mkdirSync, mkdtempSync, readFileSync, realpathSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";

import type { ToolDefinition } from "@earendil-works/pi-coding-agent";
import { Type } from "typebox";

import { EventLog } from "../src/bridge/events.ts";
import { loadModules } from "../src/host/modules.ts";
import { guardedFetch, restrictEnvironment } from "../src/host/restrict.ts";
import { HostSession, authContext } from "../src/host/session.ts";
import { assistantRole } from "../src/roles/assistant/index.ts";
import type { CapabilitySet, HostIdentity, RoleContext } from "../src/roles/types.ts";
import { CorralClient, sha256Hex } from "../src/tools/corral.ts";
import { fixtureTransport } from "./fixtures/auth/index.ts";
import { buildFixtureInstall, PACKAGE_ROOT } from "./support/fixture-install.ts";
import { writeFakeCorral } from "./support/fake-corral.ts";
import { nextEvent } from "./support/wait.ts";

test("assistant edit and write reach the session's scratch, never the main checkout or a task worktree", async () => {
  // Given a workspace whose A-1 record lists scratch spike, beside a task worktree T-9.
  const assets = buildFixtureInstall();
  const work = realpathSync(mkdtempSync(join(tmpdir(), "corral-pi-assistant-write-")));
  const cwd = join(work, "ws");
  const spike = join(cwd, ".worktrees", "A-1-spike");
  for (const dir of [spike, join(cwd, ".worktrees", "T-9"), join(cwd, ".corral", "assistants"), join(work, "tmp")]) {
    mkdirSync(dir, { recursive: true });
  }
  writeFileSync(join(cwd, "README.md"), "main\n");
  writeFileSync(join(cwd, ".corral", "assistants", "A-1.md"),
    `---\nversion: 1\nid: A-1\nscratch:\n- slug: spike\n  project: corral\n  path: ${spike}\n---\n`);
  const originalFetch = globalThis.fetch;
  const originalTemp = process.env.TMPDIR;
  let host: HostSession | undefined;
  try {
    restrictEnvironment(process.env);
    // The workspace itself sits in the system temp directory, so the temp root moves aside.
    process.env.TMPDIR = join(work, "tmp");
    const sdk = await import("@earendil-works/pi-coding-agent");
    const modules = await loadModules(assets.dir, "assistant");
    const transport = modules.auth.transport?.(authContext());
    if (!transport) throw new Error("fixture transport missing");
    const network = guardedFetch(transport);
    globalThis.fetch = network.fetch;
    const identity: HostIdentity = { store: "st-demo", project: "", role: "assistant", assistant: "A-1", generation: 1 };
    const events = new EventLog(1);
    const corral = writeFakeCorral(work);
    const client = new CorralClient({
      corral: corral.path, root: work, identity, hostCap: "h".repeat(64), sessionId: () => host?.sessionId() ?? "",
    });
    const nest: ToolDefinition = {
      name: "fixture_nest", label: "Nest", description: "Runs a nested write",
      parameters: Type.Object({}),
      async execute(_id, _params, signal, _update, ctx) {
        const write = await ctx.executeTool("write", { path: "README.md", content: "nested\n" }, { signal });
        return { content: [{ type: "text", text: JSON.stringify(write.isError) }], details: undefined };
      },
    };
    const capabilities: CapabilitySet = { catalog: 2, digest: "b".repeat(64), preset: "standard", enabled: ["todo"] };
    const role = assistantRole(PACKAGE_ROOT);
    host = await HostSession.create({
      sdk, identity, cwd, agentDir: join(work, "agent"), model: "gpt-6.1-sol", capabilities,
      guidance: { projectText: false, settings: {} as never }, client, events, network,
      modules: { ...modules, role: {
        ...role, localToolNames: [nest.name], localTools: (_context: RoleContext) => [nest],
      } },
      sessionManager: sdk.SessionManager.create(cwd, join(work, "sessions")),
    });
    await host.session.bindExtensions({});
    await host.hello("n".repeat(64), "pane-1", sha256Hex("c".repeat(64)));
    const live = host;
    const result = (callId: string) => {
      const message = live.session.messages.find((m) => m.role === "toolResult" && m.toolCallId.split("|")[0] === callId);
      if (message?.role !== "toolResult") throw new Error(`no result for ${callId}`);
      return { isError: message.isError, text: message.content.map((part) => (part.type === "text" ? part.text : "")).join("") };
    };

    // When one turn edits and writes in the main checkout, a task worktree, the scratch and the artifacts.
    fixtureTransport().script([
      { toolCalls: [{ callId: "main", name: "write", arguments: { path: "README.md", content: "hacked\n" } }] },
      { toolCalls: [{ callId: "edit-main", name: "edit", arguments: {
        path: "README.md", edits: [{ oldText: "main", newText: "hacked" }] } }] },
      { toolCalls: [{ callId: "task", name: "write", arguments: { path: ".worktrees/T-9/src/lib.rs", content: "x\n" } }] },
      { toolCalls: [{ callId: "nest", name: "fixture_nest", arguments: {} }] },
      { toolCalls: [{ callId: "scratch", name: "write", arguments: { path: ".worktrees/A-1-spike/cache.rs", content: "fn a() {}\n" } }] },
      { toolCalls: [{ callId: "edit-scratch", name: "edit", arguments: {
        path: ".worktrees/A-1-spike/cache.rs", edits: [{ oldText: "fn a", newText: "fn cache" }] } }] },
      { toolCalls: [{ callId: "draft", name: "write", arguments: {
        path: ".corral/artifacts/assistant/A-1/draft.md", content: "# Draft\n" } }] },
      { text: "Finished" },
    ]);
    const ended = nextEvent(events, (event) => event.kind === "turn_ended");
    expect((await live.submit({ text: "try", busyPolicy: "prompt", idempotencyKey: "t1", origin: "test" }, "rq-t1")).disposition)
      .toBe("started");
    await ended;
    await live.drained();

    // Then the main checkout and the task worktree are refused and unchanged, nested calls too.
    for (const id of ["main", "edit-main", "task"]) {
      expect(result(id).isError).toBeTrue();
      expect(result(id).text.startsWith("assistant rule: edit and write reach only this session's scratch")).toBeTrue();
    }
    expect(result("nest").text).toBe("true");
    expect(readFileSync(join(cwd, "README.md"), "utf8")).toBe("main\n");
    expect(existsSync(join(cwd, ".worktrees", "T-9", "src"))).toBeFalse();
    // And the scratch and the session artifact directory were written.
    for (const id of ["scratch", "edit-scratch", "draft"]) expect(result(id).isError).toBeFalse();
    expect(readFileSync(join(spike, "cache.rs"), "utf8")).toBe("fn cache() {}\n");
    expect(readFileSync(join(cwd, ".corral", "artifacts", "assistant", "A-1", "draft.md"), "utf8")).toBe("# Draft\n");
  } finally {
    host?.session.dispose();
    globalThis.fetch = originalFetch;
    if (originalTemp === undefined) delete process.env.TMPDIR;
    else process.env.TMPDIR = originalTemp;
    assets.cleanup();
    rmSync(work, { recursive: true, force: true });
  }
}, 120_000);
