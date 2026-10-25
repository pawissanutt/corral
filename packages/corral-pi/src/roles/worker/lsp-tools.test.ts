// The LSP tools as worker-local tools in a real host session: the loader refuses colliding
// local tool names, a model call reaches the fake server by argv, and a missing server comes
// back to the model as an error tool result while the turn and the host carry on.
import { afterAll, beforeAll, expect, test } from "bun:test";
import { mkdirSync, mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { fileURLToPath } from "node:url";

import { EventLog } from "../../bridge/events.ts";
import { ModuleInvalid, loadModules } from "../../host/modules.ts";
import { guardedFetch, restrictEnvironment } from "../../host/restrict.ts";
import { HostSession, authContext } from "../../host/session.ts";
import { CorralClient, sha256Hex } from "../../tools/corral.ts";
import { fixtureTransport } from "../../../test/fixtures/auth/index.ts";
import { writeFakeCorral } from "../../../test/support/fake-corral.ts";
import { type FixtureInstall, buildFixtureInstall } from "../../../test/support/fixture-install.ts";
import { nextEvent } from "../../../test/support/wait.ts";

import { workerRole } from "./index.ts";

const identity = { store: "st-0123456789abcdef", project: "demo", role: "worker" as const, task: "T-009", run: "R1", generation: 1 };
const FAKE = fileURLToPath(new URL("../../extensions/lsp/testing/fake-server.ts", import.meta.url));
const originalFetch = globalThis.fetch;
const installs: FixtureInstall[] = [];
let work: string;

beforeAll(() => {
  work = mkdtempSync(join(tmpdir(), "corral-pi-lsp-host-"));
  restrictEnvironment(process.env);
});

afterAll(() => {
  globalThis.fetch = originalFetch;
  installs.forEach((built) => built.cleanup());
  rmSync(work, { recursive: true, force: true });
});

const ROLE = (local: string) => `export default {
  role: "worker", upstreamTools: ["read"], corralOps: ["hello", "event", "receipt_lookup"],
  tools: () => [], contract: () => "", ${local}
};\n`;

test("the loader refuses local tool names that collide with upstream, corral or other local tools", async () => {
  const cases: [string, RegExp][] = [
    [`localToolNames: ["read"], localTools: () => [],`, /local tool read collides/],
    [`localToolNames: ["corral_done"], localTools: () => [],`, /local tool corral_done collides/],
    [`localToolNames: ["lsp_x", "lsp_x"], localTools: () => [],`, /local tool lsp_x collides/],
    [`localToolNames: ["Bad-Name"], localTools: () => [],`, /local tool name Bad-Name must match/],
    [`localToolNames: ["lsp_x"],`, /localTools\(\) needs localToolNames, and localToolNames needs localTools\(\)/],
  ];
  for (const [local, message] of cases) {
    const built = buildFixtureInstall();
    installs.push(built);
    writeFileSync(join(built.dir, "src/roles/worker/index.ts"), ROLE(local));
    let error: unknown;
    try {
      await loadModules(built.dir, "worker");
    } catch (caught) {
      error = caught;
    }
    expect(error instanceof ModuleInvalid).toBeTrue();
    expect((error as Error).message).toMatch(message);
  }
  const ok = buildFixtureInstall();
  installs.push(ok);
  writeFileSync(join(ok.dir, "src/roles/worker/index.ts"), ROLE(`localToolNames: ["lsp_x"], localTools: () => [],`));
  expect((await loadModules(ok.dir, "worker")).role.localToolNames).toEqual(["lsp_x"]);
});

test("a worker turn calls lsp_definition on the configured server and gets lsp_server_unavailable as a tool error", async () => {
  const assets = buildFixtureInstall();
  installs.push(assets);
  const cwd = join(work, "worktree");
  mkdirSync(cwd, { recursive: true });
  writeFileSync(join(cwd, "lib.fk"), "def gamma\nuse gamma\n");
  writeFileSync(join(cwd, "main.gh"), "ghost\n");
  const fixture = await loadModules(assets.dir, "worker");
  const role = workerRole(fileURLToPath(new URL("../../..", import.meta.url)), {
    servers: [
      { id: "fake", command: [process.execPath, FAKE], languages: { ".fk": "fake" } },
      { id: "ghost", command: ["corral-no-such-language-server"], languages: { ".gh": "ghost" } },
    ],
    limits: { quietMs: 0 },
  });
  const modules = { ...fixture, role };
  const events = new EventLog(identity.generation);
  const transport = modules.auth.transport?.(authContext());
  if (!transport) throw new Error("the fixture auth module supplies a transport");
  const network = guardedFetch(transport);
  globalThis.fetch = network.fetch as typeof globalThis.fetch;
  const sdk = await import("@earendil-works/pi-coding-agent");
  const corral = writeFakeCorral(work);
  let host: HostSession | undefined;
  const client = new CorralClient({ corral: corral.path, root: work, identity, hostCap: "h".repeat(64), sessionId: () => host?.sessionId() ?? "" });
  host = await HostSession.create({
    sdk, identity, cwd, agentDir: join(work, "pi-agent"), model: "gpt-6.1-sol", modules, client, events, network,
    sessionManager: sdk.SessionManager.create(cwd, join(work, "sessions")),
  });
  await host.session.bindExtensions({});
  await host.hello("n".repeat(64), "pane-1", sha256Hex("c".repeat(64)));
  expect(host.session.getActiveToolNames()).toContain("lsp_definition");

  fixtureTransport().script([
    { toolCalls: [{ callId: "call-def", name: "lsp_definition", arguments: { path: "lib.fk", line: 2, column: 5 } }] },
    { toolCalls: [{ callId: "call-ghost", name: "lsp_diagnostics", arguments: { path: "main.gh" } }] },
    { text: "DONE" },
  ]);
  const ended = nextEvent(events, (event) => event.kind === "turn_ended" && event.turn === 1);
  const submitted = await host.submit({ text: "Look it up.", busyPolicy: "prompt", idempotencyKey: "k-lsp", origin: "test" }, "rq-lsp");
  expect(submitted).toEqual({ disposition: "started", turn: 1 });
  await ended;
  await host.drained();

  const results = host.session.messages.filter((message) => message.role === "toolResult") as unknown as
    { toolName: string; isError: boolean; content: { type: string; text?: string }[] }[];
  expect(results.map((result) => [result.toolName, result.isError])).toEqual([["lsp_definition", false], ["lsp_diagnostics", true]]);
  expect(JSON.parse(results[0]?.content[0]?.text ?? "null").locations).toEqual([
    { path: "lib.fk", line: 1, column: 1, end_line: 1, end_column: 5, preview: "def gamma" },
  ]);
  expect(results[1]?.content[0]?.text).toContain("lsp_server_unavailable: ghost: corral-no-such-language-server is not on PATH");
  expect(host.state().denied_tools).toBe(0);
  await host.session.dispose?.();
});
