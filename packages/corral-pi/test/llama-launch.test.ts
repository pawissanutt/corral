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

for (const role of ["worker", "reviewer", "pm"] as const) {
  for (const provider of ["openai-codex", "anthropic", "amazon-bedrock"] as const) {
    test(`${role}/${provider} accepts the role's local permission without changing its cloud selection`, () => {
      // Given a cloud profile and the same role's independently enabled local capability.
      const cloud = {
        ...message, role, provider, model: "cloud-model",
        ...(provider === "amazon-bedrock" ? { aws: { profile: "fixture", region: "us-west-2" } } : {}),
      };
      const baseline = parseLaunchMessage(JSON.stringify({ ...cloud, capabilities: undefined }));
      // When the normal launcher forwards the local-capable role snapshot.
      const configured = parseLaunchMessage(JSON.stringify(cloud));
      // Then the selected provider, model and AWS binding retain their cloud values.
      expect(configured.provider).toBe(baseline.provider);
      expect(configured.model).toBe(baseline.model);
      expect(configured.aws).toEqual(baseline.aws);
      expect(configured.capabilities?.llama_cpp).toEqual(message.capabilities.llama_cpp);
    });
  }
}
