import { expect, test } from "bun:test";
import { mkdtempSync, mkdirSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { join } from "node:path";
import { tmpdir } from "node:os";
import * as sdk from "@earendil-works/pi-coding-agent";
import type { AuthModule } from "../src/auth/types.ts";
import { normalizeContext } from "@earendil-works/pi-ai";
import { imageContext } from "../src/extensions/image/context.ts";
import { ImageGeneration } from "../src/extensions/image/tool.ts";
import { EventLog } from "../src/bridge/events.ts";
import { workspaceSettings } from "../src/host/config.ts";
import { guardedFetch } from "../src/host/restrict.ts";
import { HostSession } from "../src/host/session.ts";
import { assistantRole } from "../src/roles/assistant/index.ts";
import { reviewerRole } from "../src/roles/reviewer/index.ts";
import { buildRole } from "../src/roles/build/index.ts";
import { workerRole } from "../src/roles/worker/index.ts";
import { CorralClient } from "../src/tools/corral.ts";
import fixtureAuth from "./fixtures/auth/index.ts";
import { requestBody } from "./fixtures/auth/fake-transport.ts";
import { PACKAGE_ROOT } from "./support/fixture-install.ts";

const dot = "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR4nGP4z8DwHwAFAAH/iZk9HQAAAABJRU5ErkJggg==";

test("preview-only bindings reject writes, and planning rejects even previews before dispatch", async () => {
  let calls = 0, planning = true;
  const images = new ImageGeneration({ provider: "openai", model: "gpt-image-1" }, {
    auth: { provider: "openai-codex", providers: ["openai"], credentials: () => ({ read: async () => ({ type: "api_key", key: "sk-SYNTHETIC-image" }),
      list: async () => [], modify: async () => { throw new Error(); }, delete: async () => {} }) },
    fetch: async () => { calls++; return Response.json({ data: [{ b64_json: dot }] }); },
  }, () => {});
  const tool = images.tool({ refusal: () => planning ? "Plan mode" : undefined });
  const run = (path?: string) => tool.execute("draw", { prompt: "a red dot", path }, undefined, undefined, {} as never);
  await expect(run()).rejects.toThrow("Plan mode");
  planning = false;
  await expect(run("result.png")).rejects.toThrow("read-only");
  expect(calls).toBe(0);
  const result = await run();
  expect(result.content[0]?.type === "text" && JSON.parse(result.content[0].text).path).toBeNull();
  expect(result.content[1]).toMatchObject({ type: "image", data: dot });
  const recorded = normalizeContext({ messages: [{ role: "toolResult", toolName: "image_gen", toolCallId: "draw", content: result.content, isError: false, timestamp: 0 }] });
  expect(imageContext(recorded, { input: ["text"] }).messages[0]?.content).toEqual([result.content[0]!]);
  expect(imageContext(recorded, { input: ["text", "image"] })).toBe(recorded);
  expect(recorded.messages[0]?.content).toHaveLength(2);
  const mac = images.tool({ root: "/workspace", refusal: () => undefined }, "darwin");
  await expect(mac.execute("save", { prompt: "dot", path: "sample.png" }, undefined, undefined, {} as never)).rejects.toThrow("read-only");
  expect(calls).toBe(1);
  const preview = await mac.execute("preview", { prompt: "dot" }, undefined, undefined, {} as never);
  expect(preview.content[1]).toMatchObject({ type: "image", data: dot });
  expect(calls).toBe(2);
  images.close();
});

test("configured hosted tools register once for Assistant, Build and reviewer without checking provider access", async () => {
  for (const role of [assistantRole, buildRole, reviewerRole].map(create => create(PACKAGE_ROOT))) {
    const cwd = mkdtempSync(join(tmpdir(), "hosted-role-"));
    const identity = { store: "local", project: "demo", role: role.role, task: "T-1", run: "R1" };
    let host: HostSession | undefined;
    try {
      host = await HostSession.create({ sdk, identity, cwd, agentDir: join(cwd, "agent"), model: "gpt-6.1-sol", ui: "plain",
        modules: { role, auth: fixtureAuth }, capabilities: { catalog: 1, enabled: [], hosted_tools: {
          web_search: { provider: "openai-codex", model: "gpt-6.1-sol" }, image_gen: { provider: "openai", model: "gpt-image-1" } } },
        client: new CorralClient({ corral: "", root: cwd, cwd, identity }), events: new EventLog(),
        network: guardedFetch(async () => { throw new Error("unexpected provider call"); }), sessionManager: sdk.SessionManager.inMemory(cwd) });
      for (const name of ["web_search", "image_gen"]) expect(host.session.getActiveToolNames().filter(tool => tool === name)).toHaveLength(1);
      if (role.role === "reviewer") await expect(host.session.getToolDefinition("image_gen")!.execute("draw", { prompt: "dot", path: "sample.png" }, undefined, undefined, {} as never)).rejects.toThrow("read-only");
    } finally { await host?.close(); rmSync(cwd, { recursive: true, force: true }); }
  }
});

test("workspace tools run with the selected account, not the worker's Claude chat route", async () => {
  const cwd = mkdtempSync(join(tmpdir(), "pi-hosted-tools-"));
  let host: HostSession | undefined;
  try {
    mkdirSync(join(cwd, ".corral-next"));
    writeFileSync(join(cwd, ".corral-next/pi.toml"), '[hosted_tools]\nweb_search = "openai-codex/gpt-6.1-sol"\nimage_gen = "openai/gpt-image-1"\n');
    const { capabilities } = workspaceSettings(cwd, "worker", "R1", undefined, true);
    let sends = 0, imageSends = 0, status = 200;
    const png = dot;
    const auth: AuthModule = { provider: "openai-codex", providers: ["anthropic", "openai-codex", "openai"],
      credentials(ctx) { return ctx.provider === "openai-codex" ? fixtureAuth.credentials(ctx) : ctx.provider === "openai" ? {
        read: async () => ({ type: "api_key", key: "sk-SYNTHETIC-image" }), list: async () => [{ providerId: "openai", type: "api_key" }],
        modify: async () => { throw new Error(); }, delete: async () => {},
      } : {
        read: async () => ({ type: "oauth", access: "sk-ant-oat-SYNTHETIC", refresh: "r", expires: Date.now() + 3600000 }),
        list: async () => [{ providerId: "anthropic", type: "oauth" }], modify: async () => { throw new Error(); }, delete: async () => {},
      }; },
      transport: ({ provider }) => async (url, init) => {
        if (provider === "openai") {
          expect(String(url)).toBe("https://api.openai.com/v1/images/generations");
          expect(new Headers(init?.headers).get("authorization")).toBe("Bearer sk-SYNTHETIC-image");
          imageSends++;
          return Response.json({ data: [{ b64_json: png }] });
        }
        expect(provider).toBe("openai-codex");
        expect(String(url)).toBe("https://chatgpt.com/backend-api/codex/responses");
        expect(requestBody(init)).toMatchObject({ model: "gpt-6.1-sol", tool_choice: { type: "web_search" } });
        sends++;
        if (status !== 200) return new Response("private failure body", { status });
        const item = { type: "web_search_call", id: "w1", status: "completed", action: { type: "search", sources: [{ url: "https://example.org/", title: "Example" }] } };
        return new Response([{ type: "response.output_item.done", item }, { type: "response.completed", response: { status: "completed", output: [item] } }]
          .map(v => `data: ${JSON.stringify(v)}\n\n`).join(""), { headers: { "Content-Type": "text/event-stream" } });
      } };
    const identity = { store: "local", project: "demo", role: "worker" as const, task: "T-1", run: "R1" };
    host = await HostSession.create({ sdk, identity, cwd, agentDir: join(cwd, "agent"), model: "claude-sonnet-5-5", provider: "anthropic", ui: "plain",
      modules: { role: workerRole(PACKAGE_ROOT), auth }, capabilities: { ...capabilities, enabled: [] },
      client: new CorralClient({ corral: "", root: cwd, cwd, identity }), events: new EventLog(), network: guardedFetch(async () => { throw new Error("unexpected chat transport"); }),
      sessionManager: sdk.SessionManager.create(cwd, join(cwd, "sessions")) });
    await host.startLocal();
    expect(host.loadout).not.toContain("web_fetch");
    const search = host.session.getToolDefinition("web_search")!;
    const run = (id: string) => search.execute(id, { query: "example" }, undefined, undefined, {} as never);
    const response = await run("one");
    expect(response.content[0]?.type === "text" && JSON.parse(response.content[0].text)).toMatchObject({ provider: "openai-codex", model: "gpt-6.1-sol", results: [{ url: "https://example.org/" }] });
    status = 401;
    const failure = await run("two").then(r => ({ failed: r.isError === true, text: JSON.stringify(r) }), e => ({ failed: true, text: String(e) }));
    expect(failure.failed).toBeTrue();
    expect(failure.text).not.toContain("private failure body");
    expect(sends).toBe(2);
    const image = host.session.getToolDefinition("image_gen")!;
    const draw = (path?: string) => image.execute("img", { prompt: "draw a dot", path }, undefined, undefined, {} as never);
    const result = await draw(process.platform === "linux" ? "sample.png" : undefined);
    expect(result.content[1]).toEqual({ type: "image", data: png, mimeType: "image/png" });
    if (process.platform === "linux") expect(readFileSync(join(cwd, "sample.png"), "base64")).toBe(png);
    await expect(draw("sample.png")).rejects.toThrow();
    expect(imageSends).toBe(1);
    expect(host.session.model?.provider).toBe("anthropic");
    expect(host.runtime.services.modelRuntime.getModel("openai-codex", "gpt-6.1-sol")).toBeUndefined();
  } finally { await host?.close(); rmSync(cwd, { recursive: true, force: true }); }
});
