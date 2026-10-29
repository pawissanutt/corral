import { expect, test } from "bun:test";
import { readFileSync } from "node:fs";
import { fileURLToPath } from "node:url";

import { HOST_CAPABILITIES, LaunchMessageInvalid, parseLaunchMessage } from "./config.ts";

const message = {
  version: 1, store: "st-demo", project: "demo", role: "reviewer",
  task: "T-001", run: "R1", review: "R1-review2", callback: "g1:c2", generation: 1,
  secrets: { host_cap: "h".repeat(64), ctl_cap: "c".repeat(64), bind_nonce: "n".repeat(64) },
  pane: "p2", cwd: "/work/review", root: "/work", corral: "/bin/corral",
  assets: "/assets", runtime_dir: "/runtime", session_dir: "/sessions", model: "gpt-6.1-sol",
};

test("todo is enabled only by a boolean launch snapshot and defaults off for legacy launches", () => {
  expect(parseLaunchMessage(JSON.stringify(message)).todo).toBeFalse();
  for (const todo of [true, false]) {
    expect(parseLaunchMessage(JSON.stringify({ ...message, todo })).todo).toBe(todo);
  }
  for (const todo of [null, "true", 1, {}, []]) {
    expect(() => parseLaunchMessage(JSON.stringify({ ...message, todo }))).toThrow(LaunchMessageInvalid);
  }
});

test("a capability snapshot decides todo and is kept whole; keys this host lacks are refused", () => {
  // Given the snapshot corral recorded for the generation, with a legacy todo flag that disagrees:
  const capabilities = { catalog: 1, digest: "d".repeat(64), preset: "standard", enabled: ["todo"] };
  // When the host parses it, then the snapshot wins and is exposed unchanged.
  const parsed = parseLaunchMessage(JSON.stringify({ ...message, todo: false, capabilities }));
  expect(parsed.todo).toBeTrue();
  expect(parsed.capabilities).toEqual(capabilities);
  expect(parseLaunchMessage(JSON.stringify({ ...message, todo: true, capabilities: { ...capabilities, enabled: [] } })).todo)
    .toBeFalse();
  expect(parseLaunchMessage(JSON.stringify(message)).capabilities).toBeUndefined();
  for (const invalid of [
    { ...capabilities, enabled: ["teleport"] }, { ...capabilities, digest: "short" },
    { ...capabilities, catalog: 0 }, { ...capabilities, enabled: "todo" }, [],
  ]) {
    expect(() => parseLaunchMessage(JSON.stringify({ ...message, capabilities: invalid }))).toThrow(LaunchMessageInvalid);
  }
});

test("snapshot settings and project-text trust reach the host; a corral without them sends none", () => {
  // Given a snapshot with compound settings and a trust decision, and one from an older corral:
  const capabilities = { catalog: 1, digest: "d".repeat(64), preset: "standard", enabled: ["skills", "compaction"] };
  const settings = { skills: { user_dirs: ["~/.agents/skills"] }, compaction: { reserve_tokens: 1 } };
  const parsed = parseLaunchMessage(JSON.stringify({ ...message, capabilities: { ...capabilities, settings, project_text: true } }));
  // Then the host keeps both, an older snapshot carries no guidance, and malformed values refuse.
  expect(parsed.guidance).toEqual({ settings, projectText: true });
  expect(parsed.capabilities).toEqual(capabilities);
  expect(parseLaunchMessage(JSON.stringify({ ...message, capabilities })).guidance).toBeUndefined();
  for (const invalid of [
    { settings: [], project_text: true }, { settings: { skills: [] }, project_text: true }, { settings: {}, project_text: "yes" },
  ]) {
    expect(() => parseLaunchMessage(JSON.stringify({ ...message, capabilities: { ...capabilities, ...invalid } })))
      .toThrow(LaunchMessageInvalid);
  }
});

test("compat.json advertises exactly the capabilities this host implements", () => {
  const compat = JSON.parse(readFileSync(fileURLToPath(new URL("../../compat.json", import.meta.url)), "utf8"));
  expect(compat.capabilities).toEqual([...HOST_CAPABILITIES]);
});

test("web settings come from the generation snapshot and require an eligible enabled capability", () => {
  // Given resolved values deliberately different from Rust's defaults.
  const web = { hosts: ["docs.example.org"], max_bytes: 700, timeout_ms: 2200, max_redirects: 1,
    searxng_url: "http://127.0.0.1:8888/" };
  const capabilities = { catalog: 2, digest: "e".repeat(64), preset: "standard", enabled: ["web.enabled"], web };
  // When parsed, then every setting is retained without host-side default resolution.
  expect(parseLaunchMessage(JSON.stringify({ ...message, capabilities })).capabilities?.web).toEqual(web);
  // And missing settings, malformed settings, disabled mismatches and the PM ceiling refuse launch.
  for (const invalid of [
    { ...capabilities, web: undefined }, { ...capabilities, enabled: [] },
    { ...capabilities, web: { ...web, max_bytes: 0 } },
    { ...capabilities, web: { ...web, max_redirects: 11 } },
    { ...capabilities, web: { ...web, searxng_url: "https://user:password@example.org" } },
  ]) {
    expect(() => parseLaunchMessage(JSON.stringify({ ...message, capabilities: invalid }))).toThrow(LaunchMessageInvalid);
  }
  expect(() => parseLaunchMessage(JSON.stringify({ ...message, role: "pm", capabilities }))).toThrow(LaunchMessageInvalid);
});

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

test("the pane ui is optional (rich in the host), plain is accepted and anything else is refused", () => {
  expect(parseLaunchMessage(JSON.stringify(message)).ui).toBeUndefined();
  expect(parseLaunchMessage(JSON.stringify({ ...message, ui: "plain" })).ui).toBe("plain");
  expect(parseLaunchMessage(JSON.stringify({ ...message, ui: "rich" })).ui).toBe("rich");
  expect(() => parseLaunchMessage(JSON.stringify({ ...message, ui: "fancy" }))).toThrow(LaunchMessageInvalid);
});
