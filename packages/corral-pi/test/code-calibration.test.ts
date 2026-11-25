import { expect, test } from "bun:test";
import { mkdtempSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import * as sdk from "@earendil-works/pi-coding-agent";
import { saveSession, sessionDir } from "../src/build/session-store.ts";
import { EventLog } from "../src/bridge/events.ts";
import { guardedFetch } from "../src/host/restrict.ts";
import { HostSession } from "../src/host/session.ts";
import { buildRole } from "../src/roles/build/index.ts";
import { baseCalibration } from "../src/roles/common/family.ts";
import { CorralClient } from "../src/tools/corral.ts";
import auth from "./fixtures/auth/index.ts";
import { fakeCodexTransport, requestBody } from "./fixtures/auth/fake-transport.ts";
import { PACKAGE_ROOT } from "./support/fixture-install.ts";

test("trusted Code sends the shared GPT section once without the Build overlay", async () => {
  const cwd = mkdtempSync(join(tmpdir(), "code-calibration-"));
  const identity = { store: "local", project: "", role: "build" as const };
  const transport = fakeCodexTransport();
  let body: unknown, host: HostSession | undefined;
  const network = guardedFetch((input, init) => { body = requestBody(init); return transport.send(input, init); });
  const role = buildRole(PACKAGE_ROOT), model = "gpt-6.1-sol", at = new Date().toISOString();
  try {
    const session = { version: 1, id: "B-code", name: "code", created: at, updated: at, worktree: cwd, repoRoot: cwd,
      branch: null, base: null, here: true, bare: true, repository: false, trustedCode: true } as const;
    saveSession(cwd, session);
    host = await HostSession.create({ sdk, identity, cwd, agentDir: join(cwd, "agent"), model, ui: "plain", network,
      capabilities: { catalog: 1, enabled: [], hosted_tools: { image_gen: { provider: "openai-codex", model }, web_search: { provider: "openai-codex", model } } },
      build: { dir: sessionDir(cwd, session.id), corral: "", settings: { maxParallel: 1, exitPolicy: "none", profiles: [] }, session },
      modules: { role, auth, assetRoot: PACKAGE_ROOT }, client: new CorralClient({ corral: "", root: cwd, cwd, identity }),
      events: new EventLog(), sessionManager: sdk.SessionManager.create(cwd, join(cwd, "sessions")) });
    await host.startLocal();
    for (const name of ["web_search", "image_gen"]) expect(host.session.getActiveToolNames().filter(tool => tool === name)).toHaveLength(1);
    await host.session.prompt("Reply with ready");
    const payload = JSON.stringify(body);
    expect(payload.split(baseCalibration(PACKAGE_ROOT, "openai-codex", model)!)).toHaveLength(2);
    expect(payload).not.toContain(role.calibration!("openai-codex", model)!.split("\n")[0]!);
    await host.planMode!.enter();
    const image = host.session.getToolDefinition("image_gen")!;
    await expect(image.execute("preview", { prompt: "dot" }, undefined, undefined, {} as never)).rejects.toThrow("blocked in this session phase");
  } finally { await host?.close(); rmSync(cwd, { recursive: true, force: true }); }
});
