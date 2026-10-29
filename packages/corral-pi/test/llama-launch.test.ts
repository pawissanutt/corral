import { expect, test } from "bun:test";

import { parseLaunchMessage } from "../src/host/config.ts";

const message = {
  version: 1, store: "st-demo", project: "demo", role: "worker", task: "T-578", run: "R1", generation: 1,
  secrets: { host_cap: "h".repeat(64), ctl_cap: "c".repeat(64), bind_nonce: "n".repeat(64) },
  pane: "pane-1", cwd: "/work", root: "/root", corral: "/bin/corral", assets: "/assets",
  runtime_dir: "/runtime", session_dir: "/sessions", model: "new-family/new-model.gguf", provider: "llama.cpp",
  capabilities: {
    catalog: 3, digest: "a".repeat(64), preset: "standard", enabled: ["llama_cpp.enabled"],
    llama_cpp: {
      endpoint: "http://127.0.0.1:8080", discover: true, context_window: 32768, max_tokens: 8192, timeout_ms: 15000,
    },
  },
};

test("a configured local provider accepts a newly named model without a catalog patch", () => {
  // Given a generation-bound origin and an arbitrary endpoint-owned model ID.
  // When the normal host launch boundary parses the message.
  const config = parseLaunchMessage(JSON.stringify(message));
  // Then local inference has its own provider identity, without impersonating a cloud route.
  expect(config.provider).toBe("llama.cpp");
  expect(config.model).toBe("new-family/new-model.gguf");
});

test("a local route requires its enabled capability and resolved settings", () => {
  // Given a local provider without the explicit local inference opt-in.
  // When launch parses it, then no implicit local origin is invented.
  expect(() => parseLaunchMessage(JSON.stringify({ ...message, capabilities: undefined }))).toThrow();
  expect(() => parseLaunchMessage(JSON.stringify({
    ...message, capabilities: { ...message.capabilities, enabled: [], llama_cpp: undefined },
  }))).toThrow();
});

test("local configuration does not redirect an existing cloud provider", () => {
  // Given a cloud route carrying an enabled local inference capability.
  // When launch parses it, then contradictory routing is refused.
  expect(() => parseLaunchMessage(JSON.stringify({ ...message, provider: "openai-codex", model: "gpt-6.1-sol" }))).toThrow();
});
