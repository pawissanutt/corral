import { expect, test } from "bun:test";
import type { ToolDefinition } from "@earendil-works/pi-coding-agent";

import type { WebAccess } from "./client.ts";
import { hostedSearchTool, webTools } from "./index.ts";

function record<Args extends unknown[], Result>(fn: (...args: Args) => Result) {
  const calls: Args[] = [];
  return Object.assign((...args: Args) => {
    calls.push(args);
    return fn(...args);
  }, { calls });
}

function fixture() {
  const access = {
    fetch: record(async (url: string) => ({ url, status: 200, content_type: "text/plain", body: "fixture", bytes: 7, truncated: false })),
    search: record(async () => ({ configured: false as const, setup: "configure SearXNG", results: [] })),
  } satisfies WebAccess;
  return access;
}

function tool(tools: readonly ToolDefinition[], name: string) {
  const match = tools.find((item) => item.name === name);
  if (!match) throw new Error(`Missing ${name}`);
  return match;
}

async function invoke(definition: ToolDefinition, params: Record<string, unknown>, signal?: AbortSignal) {
  const result = await definition.execute("fixture-id", params, signal, undefined, undefined as never);
  const part = result.content[0];
  if (part?.type !== "text") throw new Error("Expected text result");
  return part.text;
}

test("disabled search preserves public fetch and local snapshot reads", async () => {
  const access = fixture();
  const tools = webTools(access, false);
  expect(tools.map(({ name }) => name)).toEqual(["web_fetch", "web_read"]);
  const page = JSON.parse(await invoke(tool(tools, "web_fetch"), { url: "https://example.org" }));
  const snapshot = JSON.parse(await invoke(tool(tools, "web_read"), { content_id: page.content_id }));
  expect(snapshot).toEqual(page);
  expect(access.fetch.calls).toHaveLength(1);
  expect(access.search.calls).toHaveLength(0);
});

test("standalone hosted search uses the explicit label and forwards call identity, count and signal", async () => {
  const search = record(async (..._args: unknown[]) => "hosted result");
  const definition = hostedSearchTool(search, "Claude fixture (metered API)");
  expect(definition.name).toBe("web_search");
  expect(definition.label).toBe("Claude fixture (metered API)");
  const { signal } = new AbortController();
  expect(await invoke(definition, { query: "docs" }, signal)).toBe("hosted result");
  expect(search.calls[0]).toEqual(["fixture-id", "docs", 5, signal]);
  await invoke(definition, { query: "docs", num_results: 20 }, signal);
  expect(search.calls[1]).toEqual(["fixture-id", "docs", 20, signal]);
});

test("blank queries never dispatch on any route; legacy selection still works", async () => {
  const access = fixture();
  const subscription = record(async () => "legacy subscription result");
  const hosted = record(async () => "hosted result");
  const searxng = tool(webTools(access), "web_search");
  const legacy = tool(webTools(access, subscription), "web_search");
  for (const definition of [searxng, legacy, hostedSearchTool(hosted, "hosted fixture")]) {
    await expect(invoke(definition, { query: " \n\t\u00a0" })).rejects.toThrow("Search query must not be blank.");
  }
  expect(access.search.calls).toHaveLength(0);
  expect(subscription.calls).toHaveLength(0);
  expect(hosted.calls).toHaveLength(0);
  expect(JSON.parse(await invoke(searxng, { query: "docs" }))).toEqual({ configured: false, setup: "configure SearXNG", results: [] });
  expect(await invoke(legacy, { query: "docs" })).toBe("legacy subscription result");
  expect(access.search.calls).toHaveLength(1);
});
