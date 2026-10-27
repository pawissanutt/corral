import { expect, test } from "bun:test";

import { LaunchMessageInvalid, parseLaunchMessage } from "./config.ts";

const message = {
  version: 1, store: "st-demo", project: "demo", role: "reviewer",
  task: "T-001", run: "R1", review: "R1-review2", callback: "g1:c2", generation: 1,
  secrets: { host_cap: "h".repeat(64), ctl_cap: "c".repeat(64), bind_nonce: "n".repeat(64) },
  pane: "p2", cwd: "/work/review", root: "/work", corral: "/bin/corral",
  assets: "/assets", runtime_dir: "/runtime", session_dir: "/sessions", model: "gpt-6.1-sol",
};

test("the optional exact resume path survives launch parsing; older launch messages remain valid", () => {
  expect(parseLaunchMessage(JSON.stringify(message)).resumeSession).toBeUndefined();
  expect(parseLaunchMessage(JSON.stringify({ ...message, resume_session: null })).resumeSession).toBeUndefined();
  const resumed = parseLaunchMessage(JSON.stringify({ ...message, resume_session: "/sessions/exact.jsonl" }));
  expect(resumed.resumeSession).toBe("/sessions/exact.jsonl");
  expect(resumed.sessionDir).toBe("/sessions");
  expect(resumed.identity.generation).toBe(1);
  expect(resumed.freshSession).toBeFalse();
  expect(parseLaunchMessage(JSON.stringify({ ...message, fresh_session: true })).freshSession).toBeTrue();
  expect(() => parseLaunchMessage(JSON.stringify({ ...message, fresh_session: "true" }))).toThrow(LaunchMessageInvalid);
  expect(() => parseLaunchMessage(JSON.stringify({ ...message, resume_session: "relative.jsonl" }))).toThrow(LaunchMessageInvalid);
});

test("the launch provider is optional, defaults to Codex in the host, and must be a known provider", () => {
  expect(parseLaunchMessage(JSON.stringify(message)).provider).toBeUndefined();
  expect(parseLaunchMessage(JSON.stringify({ ...message, provider: null })).provider).toBeUndefined();
  expect(parseLaunchMessage(JSON.stringify({ ...message, provider: "anthropic", model: "claude-opus-5-5" })).provider)
    .toBe("anthropic");
  expect(() => parseLaunchMessage(JSON.stringify({ ...message, provider: "mistral" }))).toThrow(LaunchMessageInvalid);
});
