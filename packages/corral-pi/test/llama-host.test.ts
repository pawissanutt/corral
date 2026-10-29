import { expect, test } from "bun:test";
import { mkdtempSync, readFileSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";

import auth from "../src/auth/index.ts";
import { localInferenceRules } from "../src/auth/local.ts";
import { EventLog } from "../src/bridge/events.ts";
import type { HostCapabilities } from "../src/host/config.ts";
import { loadModules } from "../src/host/modules.ts";
import { guardedFetch, restrictEnvironment } from "../src/host/restrict.ts";
import { HostSession } from "../src/host/session.ts";
import { CorralClient, sha256Hex } from "../src/tools/corral.ts";
import { buildFixtureInstall } from "./support/fixture-install.ts";
import { writeFakeCorral } from "./support/fake-corral.ts";
import { nextEvent } from "./support/wait.ts";

test("a local host binds, answers, reloads and persists token usage without cloud credentials", async () => {
  // Given a normal host and bridge, with only an explicitly configured local inference transport.
  const work = mkdtempSync(join(tmpdir(), "corral-pi-local-host-"));
  const assets = buildFixtureInstall();
  const originalFetch = globalThis.fetch;
  const local = {
    endpoint: "http://127.0.0.1:8080", discover: true, context_window: 32768, max_tokens: 1024, timeout_ms: 15000,
  };
  const capabilities: HostCapabilities = {
    catalog: 3, digest: "a".repeat(64), preset: "standard", enabled: ["llama_cpp.enabled"], llama_cpp: local,
  };
  const identity = { store: "st-demo", project: "demo", role: "worker" as const, generation: 1, task: "T-578", run: "R1" };
  const requests: string[] = [];
  const network = guardedFetch(async (url) => {
    requests.push(String(url));
    if (String(url).endsWith("/v1/models")) return Response.json({ data: [{ id: "new-family/new.gguf" }] });
    return new Response([
      { choices: [{ index: 0, delta: { role: "assistant", content: "Local completion" }, finish_reason: null }] },
      { choices: [{ index: 0, delta: {}, finish_reason: "stop" }],
        usage: { prompt_tokens: 19, completion_tokens: 5, total_tokens: 24 } },
    ].map((chunk) => `data: ${JSON.stringify({
      id: `response-${requests.length}`, object: "chat.completion.chunk", model: "new-family/new.gguf", ...chunk,
    })}\n\n`).join("") + "data: [DONE]\n\n", { headers: { "content-type": "text/event-stream" } });
  }, undefined, "llama.cpp", localInferenceRules(local));
  let host: HostSession | undefined;
  try {
    restrictEnvironment(process.env, "llama.cpp");
    globalThis.fetch = network.fetch;
    const sdk = await import("@earendil-works/pi-coding-agent");
    const modules = await loadModules(assets.dir, "worker");
    const events = new EventLog(1);
    const corral = writeFakeCorral(work);
    const client = new CorralClient({
      corral: corral.path, root: work, identity, hostCap: "h".repeat(64), sessionId: () => host?.sessionId() ?? "",
    });
    host = await HostSession.create({
      sdk, identity, cwd: work, agentDir: join(work, "agent"), provider: "llama.cpp",
      model: "new-family/new.gguf", capabilities, client, events, network,
      modules: { ...modules, auth: {
        ...auth,
        credentials() { throw new Error("local inference must not request a cloud credential store"); },
      } },
      sessionManager: sdk.SessionManager.create(work, join(work, "sessions")),
    });
    await host.session.bindExtensions({});
    await host.hello("n".repeat(64), "pane-1", sha256Hex("c".repeat(64)));
    // When an ordinary turn is submitted before and after reload.
    for (const stage of ["initial", "reloaded"]) {
      if (stage === "reloaded") await host.session.reload();
      const ended = nextEvent(events, (event) => event.kind === "turn_ended");
      expect((await host.submit({
        text: "Answer locally", busyPolicy: "prompt", idempotencyKey: stage, origin: "test",
      }, stage)).disposition).toBe("started");
      await ended;
      await host.drained();
    }
    // Then state, session responses and persisted collector input retain local identity and tokens.
    expect(host.state().model).toBe("llama.cpp/new-family/new.gguf");
    expect(host.state().capabilities).toEqual(capabilities);
    const answers = host.session.messages.filter((message) => message.role === "assistant");
    expect(answers).toHaveLength(2);
    expect(answers.map((answer) => answer.role === "assistant" && [answer.usage.input, answer.usage.output, answer.usage.cost.total]))
      .toEqual([[19, 5, 0], [19, 5, 0]]);
    const file = host.session.sessionFile;
    if (!file) throw new Error("bound local host needs a durable transcript");
    const records: unknown[] = readFileSync(file, "utf8").trim().split("\n").map((line) => JSON.parse(line));
    const assistants = records.filter((record) => typeof record === "object" && record !== null &&
      Reflect.get(record, "type") === "message" && Reflect.get(Reflect.get(record, "message"), "role") === "assistant");
    expect(assistants).toHaveLength(2);
    expect(requests).toEqual([`${local.endpoint}/v1/models`,
      `${local.endpoint}/v1/chat/completions`, `${local.endpoint}/v1/chat/completions`]);
    expect(network.ledger().refused).toBe(0);
  } finally {
    host?.session.dispose();
    globalThis.fetch = originalFetch;
    assets.cleanup();
    rmSync(work, { recursive: true, force: true });
  }
}, 60_000);
