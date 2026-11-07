// Real SDK/CLI boundary, synthetic transport only. The parent test isolates the network first.
import assert from "node:assert/strict";
import { spawnSync } from "node:child_process";
import { createServer, type IncomingMessage, type ServerResponse } from "node:http";
import { mkdtempSync, mkdirSync, readlinkSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { createHash } from "node:crypto";

import * as sdk from "@anthropic-ai/claude-agent-sdk";
import { normalizeContext } from "@earendil-works/pi-ai";
import { getModel } from "@earendil-works/pi-ai/compat";
import { Type } from "typebox";

import { overrideSdk, resolveClaudeCode } from "../../src/claude/sdk.ts";
import { CLAUDE_SDK_API, streamClaudeSdk } from "../../src/claude/stream.ts";

// Refuse accidental direct execution before starting a provider process. The wrapper supplies
// its original network namespace identity; only loopback is brought up in the new namespace.
const originalNamespace = process.argv[2];
assert(originalNamespace && readlinkSync("/proc/self/ns/net") !== originalNamespace,
  "Claude wire fixture requires the test's isolated network namespace");
const root = mkdtempSync(join(tmpdir(), "corral-claude-wire-"));
const cwd = join(root, "repo"), home = join(root, "home");
mkdirSync(cwd); mkdirSync(home);
const env = {
  PATH: process.env.PATH, HOME: home, SHELL: "/bin/bash", TZ: "UTC",
  XDG_CONFIG_HOME: join(home, "config"), XDG_DATA_HOME: join(home, "data"),
  XDG_CACHE_HOME: join(home, "cache"), XDG_STATE_HOME: join(home, "state"),
  GIT_CONFIG_NOSYSTEM: "1", GIT_AUTHOR_NAME: "Fixture", GIT_AUTHOR_EMAIL: "fixture@example.invalid",
  GIT_COMMITTER_NAME: "Fixture", GIT_COMMITTER_EMAIL: "fixture@example.invalid",
};
function git(...args: string[]): void {
  const result = spawnSync("git", args, { cwd, env, encoding: "utf8", timeout: 5000 });
  assert.equal(result.status, 0, result.stderr || String(result.error));
}
function record(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}
function markers(value: unknown): number {
  if (Array.isArray(value)) return value.reduce((sum: number, item: unknown) => sum + markers(item), 0);
  if (!record(value)) return 0;
  return Number("cache_control" in value) + Object.values(value).reduce<number>((sum, item) => sum + markers(item), 0);
}
const hash = (value: unknown): string => createHash("sha256").update(JSON.stringify(value)).digest("hex");
const captures: Record<string, unknown>[] = [];
let transportError: unknown;
const server = createServer((request, response) => {
  void respond(request, response).catch((error: unknown) => {
    transportError = error;
    response.writeHead(500).end("fixture transport failed");
  });
});
async function respond(request: IncomingMessage, response: ServerResponse): Promise<void> {
  assert.equal(new URL(request.url ?? "", "http://127.0.0.1").pathname, "/v1/messages");
  let body = "";
  for await (const chunk of request) {
    body += String(chunk);
    assert(body.length <= 256 * 1024, "bounded synthetic request");
  }
  const payload: unknown = JSON.parse(body);
  assert(record(payload));
  captures.push(payload);
  const events = [
    { type: "message_start", message: { id: "msg_fixture", type: "message", role: "assistant",
      model: "claude-opus-5-5", content: [], stop_reason: null, stop_sequence: null,
      usage: { input_tokens: 10, output_tokens: 0 } } },
    { type: "content_block_start", index: 0, content_block: { type: "text", text: "" } },
    { type: "content_block_delta", index: 0, delta: { type: "text_delta", text: "fixture reply" } },
    { type: "content_block_stop", index: 0 },
    { type: "message_delta", delta: { stop_reason: "end_turn", stop_sequence: null }, usage: { output_tokens: 2 } },
    { type: "message_stop" },
  ];
  response.writeHead(200, { "content-type": "text/event-stream" });
  response.end(events.map((event) => `event: ${event.type}\ndata: ${JSON.stringify(event)}\n\n`).join(""));
}

try {
  await new Promise<void>((resolve, reject) => server.once("error", reject).listen(0, "127.0.0.1", resolve));
  const address = server.address();
  assert(address && typeof address !== "string");
  // Keep production request construction, credential scrub and deny hooks. Override only the
  // destination at the test SDK seam, after the production scrub (which must reject real overrides).
  overrideSdk({ ...sdk, query: ({ prompt, options }) => sdk.query({ prompt, options: {
    ...options, env: { ...options.env, ANTHROPIC_BASE_URL: `http://127.0.0.1:${address.port}` },
  } }) });
  const catalogModel = getModel("anthropic", "claude-opus-5-5");
  assert(catalogModel);
  const { compat: _compat, ...metadata } = catalogModel;
  const model = { ...metadata, api: CLAUDE_SDK_API };
  const executable = process.argv[3] ?? resolveClaudeCode();
  const version = spawnSync(executable, ["--version"], { env, encoding: "utf8", timeout: 5000 });
  assert.equal(version.status, 0, version.stderr || String(version.error));
  git("init", "-q");
  writeFileSync(join(cwd, "fixture.txt"), "one\n");
  git("add", "fixture.txt"); git("commit", "-qm", "initial fixture");
  const context = normalizeContext({ systemPrompt: "Synthetic offline cache fixture. Use host tools for current Git state.",
    tools: [{ name: "read", description: "Read a file", parameters: Type.Object({ path: Type.String() }) }],
    messages: [
      { role: "user", content: "Inspect the fixture", timestamp: 0 },
      { role: "toolResult", toolName: "read", toolCallId: "read-1", isError: false, timestamp: 0,
        content: [{ type: "text", text: "fixture contents" }] },
    ],
  });
  let baseline: Record<string, string> | undefined;
  for (const state of ["clean", "edited", "staged", "committed"]) {
    if (state === "edited") writeFileSync(join(cwd, "fixture.txt"), "two\n");
    if (state === "staged") git("add", "fixture.txt");
    if (state === "committed") git("commit", "-qm", "edited fixture");
    const before = captures.length;
    const stream = streamClaudeSdk({ configDir: join(home, "claude"), cwd, env, executable: () => executable },
      model, context, { apiKey: "sk-ant-oat01-SYNTHETIC-FIXTURE", sessionId: "wire-cache", signal: AbortSignal.timeout(10_000) });
    const response = await stream.result();
    if (transportError) throw transportError;
    assert.equal(response.stopReason, "stop", response.errorMessage);
    assert.equal(captures.length, before + 1, "exactly one synthetic inference per state");
    const payload = captures.at(-1);
    assert(payload && Array.isArray(payload.system) && Array.isArray(payload.tools) && Array.isArray(payload.messages));
    const fingerprints = Object.fromEntries(["system", "tools", "messages"].map((key) => [key, hash(payload[key])]));
    baseline ??= fingerprints;
    // Hashes keep even synthetic prompt bodies out of gate logs; this checks the final SDK wire
    // body, not the already-stable host replay. Request metadata isn't part of the cached prefix.
    assert.deepEqual(fingerprints, baseline, `${state}: effective Claude prefix changed with repository state`);
    assert(markers(payload) > 0 && markers(payload) <= 4, "SDK and replay must share the four-breakpoint budget");
  }
  console.log(JSON.stringify({ cli: version.stdout.toString().trim(), states: 4, inferenceRequests: captures.length,
    network: "isolated-loopback", prefixStable: true }));
} finally {
  overrideSdk(undefined);
  server.closeAllConnections();
  await new Promise<void>((resolve) => server.close(() => resolve()));
  rmSync(root, { recursive: true, force: true });
}
